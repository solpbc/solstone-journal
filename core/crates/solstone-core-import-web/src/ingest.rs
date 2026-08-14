// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    AppState,
    callosum::emit_best_effort,
    http::{error, json as json_response},
    journal_sources::JournalSourceIdentity,
    multipart,
};
use axum::{
    extract::{Json, Multipart, Path as AxumPath, State},
    http::StatusCode,
    response::Response,
};
use chrono::Utc;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteOptions, append_jsonl, atomic_replace, find_available_segment,
};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

const MAX_ATTEMPTS: usize = 32;
fn state(root: &Path, prefix: &str, area: &str) -> std::path::PathBuf {
    root.join("imports").join(prefix).join(area)
}
fn read_json(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}))
}
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    atomic_replace(
        path,
        serde_json::to_vec_pretty(value)
            .map_err(|e| e.to_string())?
            .as_slice(),
        AtomicWriteOptions::default(),
    )
    .map_err(|e| e.to_string())
}
fn decision(path: &Path, value: Value) {
    let _ = append_jsonl(path, &value);
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn clean_file(name: &str) -> Option<String> {
    Path::new(name)
        .file_name()
        .and_then(|x| x.to_str())
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
}

pub(crate) async fn segments(
    State(app): State<AppState>,
    AxumPath(_): AxumPath<String>,
    identity: JournalSourceIdentity,
    multipart_body: Multipart,
) -> Response {
    segments_with_attempts(app, identity.prefix(), multipart_body, MAX_ATTEMPTS).await
}

async fn segments_with_attempts(
    app: AppState,
    prefix: &str,
    multipart_body: Multipart,
    max_attempts: usize,
) -> Response {
    let parts = match multipart::collect(multipart_body).await {
        Ok(p) => p,
        Err(e) => {
            return error(
                StatusCode::BAD_REQUEST,
                "I couldn't read that JSON request.",
                "invalid_json_request",
                e.to_owned(),
            );
        }
    };
    let mut fields: HashMap<String, Vec<_>> = HashMap::new();
    for p in parts {
        fields.entry(p.name.clone()).or_default().push(p);
    }
    let metadata = fields
        .get("metadata")
        .and_then(|v| v.first())
        .and_then(|p| serde_json::from_slice::<Value>(&p.bytes).ok());
    let Some(Value::Object(meta)) = metadata else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't find a required field.",
            "missing_required_field",
            "Missing metadata".to_owned(),
        );
    };
    let Some(items) = meta.get("segments").and_then(Value::as_array) else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't find a required field.",
            "missing_required_field",
            "Missing segments array".to_owned(),
        );
    };
    let base = state(&app.root, prefix, "segments");
    let log = base.join("log.jsonl");
    let mut new = Map::new();
    let (mut copied, mut skipped, mut deconflicted) = (0, 0, 0);
    let mut errors = Vec::new();
    for (idx, item) in items.iter().enumerate() {
        let result = (|| -> Result<(), String> {
            let obj = item
                .as_object()
                .ok_or("Segment metadata must be an object")?;
            let day = obj
                .get("day")
                .and_then(Value::as_str)
                .filter(|x| x.len() == 8 && x.bytes().all(|b| b.is_ascii_digit()))
                .ok_or("Invalid day format")?;
            let stream = obj
                .get("stream")
                .and_then(Value::as_str)
                .filter(|x| !x.is_empty())
                .ok_or("Invalid stream format")?;
            let key = obj
                .get("segment_key")
                .and_then(Value::as_str)
                .filter(|x| x.contains('_'))
                .ok_or("Invalid segment_key format")?;
            let names: Vec<String> = obj
                .get("files")
                .and_then(Value::as_array)
                .ok_or("Segment must list at least one file")?
                .iter()
                .map(|v| {
                    clean_file(v.as_str().unwrap_or(""))
                        .ok_or("Invalid filename in metadata".to_owned())
                })
                .collect::<Result<_, _>>()?;
            if names.is_empty() || names.iter().collect::<HashSet<_>>().len() != names.len() {
                return Err("Duplicate filenames in metadata".into());
            }
            let uploads = fields.get(&format!("files_{idx}"));
            let mut payload = HashMap::new();
            for p in uploads.into_iter().flatten() {
                let n =
                    clean_file(p.filename.as_deref().unwrap_or("")).ok_or("Invalid filename")?;
                payload.insert(n, p.bytes.clone());
            }
            if payload.len() != names.len() || names.iter().any(|n| !payload.contains_key(n)) {
                return Err("Missing uploaded files".into());
            }
            let stream_dir = app.root.join("chronicle").join(day).join(stream);
            let mut target = stream_dir.join(key);
            let original = key.to_owned();
            let mut action = "copied";
            let mut reason = "new segment";
            if target.exists() {
                let exact = names.iter().all(|n| {
                    fs::read(target.join(n))
                        .ok()
                        .is_some_and(|b| hash(&b) == hash(&payload[n]))
                });
                if exact {
                    action = "skipped";
                    reason = "exact match"
                } else {
                    let Some(next) = find_available_segment(&stream_dir, key, max_attempts)
                        .map_err(|e| e.to_string())?
                    else {
                        return Err("No available segment slot".into());
                    };
                    target = stream_dir.join(&next);
                    action = "deconflicted";
                    reason = "segment key conflict";
                }
            }
            let final_key = target
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap()
                .to_owned();
            if action != "skipped" {
                fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                for n in &names {
                    atomic_replace(
                        target.join(n),
                        &payload[n],
                        AtomicWriteOptions { mode: Some(0o600) },
                    )
                    .map_err(|e| e.to_string())?;
                }
            }
            let rec = json!({"files":names.iter().map(|n|json!({"name":n,"sha256":hash(&payload[n]),"size":payload[n].len()})).collect::<Vec<_>>(),"imported_via":"peer_link","link_id":null});
            let day_state = new
                .entry(day.to_owned())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .unwrap();
            day_state.insert(format!("{stream}/{final_key}"), rec.clone());
            if action == "deconflicted" {
                day_state.insert(format!("{stream}/{original}"), rec);
            }
            let mut entry = json!({"ts":Utc::now().to_rfc3339(),"action":action,"item_type":"segment","item_id":format!("{day}/{stream}/{final_key}"),"reason":reason,"files":names,"imported_via":"peer_link","link_id":null});
            if action == "deconflicted" {
                entry["original_key"] = json!(original)
            }
            decision(&log, entry);
            match action {
                "copied" => copied += 1,
                "skipped" => skipped += 1,
                _ => deconflicted += 1,
            };
            Ok(())
        })();
        if let Err(e) = result {
            errors.push(json!({"segment_key":item.get("segment_key").and_then(Value::as_str).unwrap_or(""),"day":item.get("day").and_then(Value::as_str).unwrap_or(""),"error":e}));
        }
    }
    if !new.is_empty() {
        let path = base.join("state.json");
        let mut existing = read_json(&path);
        let obj = existing.as_object_mut().unwrap();
        for (k, v) in new {
            let d = obj.entry(k).or_insert_with(|| json!({}));
            d.as_object_mut()
                .unwrap()
                .extend(v.as_object().unwrap().clone());
        }
        let _ = write_json(&path, &existing);
    }
    let written = copied + deconflicted;
    if written > 0 {
        emit_best_effort(
            &app.root,
            json!({"tract":"supervisor","event":"request","cmd":["journal","indexer","--rescan"]}),
        );
    }
    json_response(
        StatusCode::OK,
        json!({"segments_received":written,"segments_skipped":skipped,"segments_deconflicted":deconflicted,"errors":errors}),
    )
}

fn valid_facet_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}
fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|part| matches!(part, "" | "." | ".."))
}

pub(crate) async fn facets(
    State(app): State<AppState>,
    AxumPath(_): AxumPath<String>,
    identity: JournalSourceIdentity,
    multipart_body: Multipart,
) -> Response {
    let parts = match multipart::collect(multipart_body).await {
        Ok(parts) => parts,
        Err(detail) => {
            return error(
                StatusCode::BAD_REQUEST,
                "I couldn't read that JSON request.",
                "invalid_json_request",
                detail.to_owned(),
            );
        }
    };
    let metadata = parts
        .iter()
        .find(|part| part.name == "metadata")
        .and_then(|part| serde_json::from_slice::<Value>(&part.bytes).ok());
    let Some(facets) = metadata
        .as_ref()
        .and_then(|value| value.get("facets"))
        .and_then(Value::as_array)
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't find a required field.",
            "missing_required_field",
            "Missing facets array".into(),
        );
    };
    let base = state(&app.root, identity.prefix(), "facets");
    let entity_id_map =
        read_json(&state(&app.root, identity.prefix(), "entities").join("state.json"))
            .get("id_map")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
    let state_path = base.join("state.json");
    let mut facet_state = read_json(&state_path);
    let received = facet_state
        .as_object_mut()
        .and_then(|state| {
            state
                .entry("received")
                .or_insert_with(|| json!({}))
                .as_object_mut()
        })
        .expect("facet state object");
    let (mut created, mut merged, mut skipped, mut staged) = (0, 0, 0, 0);
    let mut errors = Vec::new();
    for (facet_index, facet) in facets.iter().enumerate() {
        let outcome = (|| -> Result<(), String> {
            let facet = facet
                .as_object()
                .ok_or("Facet metadata must be an object")?;
            let name = facet
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| valid_facet_name(name))
                .ok_or("Invalid facet name")?;
            let files = facet
                .get("files")
                .and_then(Value::as_array)
                .ok_or("Facet files must be an array")?;
            let mut items = Vec::new();
            for (file_index, descriptor) in files.iter().enumerate() {
                let descriptor = descriptor
                    .as_object()
                    .ok_or("Facet file metadata must be an object")?;
                let relative = descriptor
                    .get("path")
                    .and_then(Value::as_str)
                    .filter(|path| safe_relative(path))
                    .ok_or("Invalid path")?;
                let kind = descriptor
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or("Facet file metadata must include path and type")?;
                let bytes = &parts
                    .iter()
                    .find(|part| part.name == format!("files_{facet_index}_{file_index}"))
                    .ok_or("Missing uploaded file")?
                    .bytes;
                items.push(crate::facet_ingest::FacetItem {
                    path: relative,
                    kind,
                    bytes,
                });
            }
            let outcome = crate::facet_ingest::process_facet(
                &app.root,
                name,
                &items,
                &base.join("staged"),
                &entity_id_map,
                received,
            )?;
            created += outcome.created;
            merged += outcome.merged;
            skipped += outcome.skipped;
            staged += outcome.staged;
            for entry in outcome.decisions {
                decision(&base.join("log.jsonl"), entry);
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            errors.push(json!({"facet":facet.get("name").and_then(Value::as_str).unwrap_or(""),"error":error}));
        }
    }
    let _ = write_json(&state_path, &facet_state);
    json_response(
        StatusCode::OK,
        json!({"created":created,"merged":merged,"skipped":skipped,"staged":staged,"errors":errors}),
    )
}

pub(crate) async fn entities(
    State(app): State<AppState>,
    AxumPath(_): AxumPath<String>,
    identity: JournalSourceIdentity,
    Json(payload): Json<Value>,
) -> Response {
    let Some(items) = payload.get("entities").and_then(Value::as_array) else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't find a required field.",
            "missing_required_field",
            "Missing entities array".into(),
        );
    };
    let base = state(&app.root, identity.prefix(), "entities");
    let state_path = base.join("state.json");
    let mut entity_state = read_json(&state_path);
    if !entity_state.is_object() {
        entity_state = json!({});
    }
    let mut received = entity_state
        .get("received")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut id_map = entity_state
        .get("id_map")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (mut auto_merged, mut created, mut staged, mut skipped) = (0, 0, 0, 0);
    let mut errors = Vec::new();
    let mut dirty = false;
    for incoming in items {
        let result = (|| -> Result<(), String> {
            let mut source = incoming
                .as_object()
                .cloned()
                .ok_or("Entity data must be an object")?;
            let name = source
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or("Entity name is required")?
                .to_owned();
            let id = source
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| entity_slug(&name));
            if id.is_empty() {
                return Err("Entity id is required".into());
            }
            source.insert("id".into(), json!(id));
            let source_value = Value::Object(source.clone());
            let content_hash = hash(
                serde_json::to_string(&source_value)
                    .map_err(|error| error.to_string())?
                    .as_bytes(),
            );
            if received.get(&id).and_then(Value::as_str) == Some(&content_hash) {
                skipped += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"skipped","item_type":"entity","item_id":id,"match_tier":null,"reason":"idempotent","source":source_value,"target":null,"fields_changed":[]}),
                );
                return Ok(());
            }
            let targets = journal_entities(&app.root);
            let exact = targets
                .iter()
                .filter(|(_, target)| {
                    target
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|target_name| target_name.eq_ignore_ascii_case(&name))
                })
                .collect::<Vec<_>>();
            let staged_path = base.join("staged").join(format!("{id}.json"));
            if exact.len() == 1 {
                let (target_id, target) = exact[0];
                let (merged, fields_changed) = merge_entity_fields(target, &source);
                let path = app
                    .root
                    .join("entities")
                    .join(target_id)
                    .join("entity.json");
                write_json(&path, &Value::Object(merged.clone()))?;
                let _ = fs::remove_file(&staged_path);
                id_map.insert(id.clone(), json!(target_id));
                auto_merged += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"auto_merged","item_type":"entity","item_id":id,"match_tier":1,"reason":"high_confidence_match","source":source_value,"target":merged,"fields_changed":fields_changed}),
                );
            } else if exact.len() > 1 {
                stage_entity(&base, &id, &source_value, "low_confidence_match", exact.iter().map(|(target_id, target)| json!({"id":target_id,"name":target.get("name"),"tier":1})).collect())?;
                staged += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"entity","item_id":id,"match_tier":1,"reason":"low_confidence_match","source":source_value,"target":null,"fields_changed":[]}),
                );
            } else if targets.iter().any(|(target_id, _)| target_id == &id) {
                let target = &targets
                    .iter()
                    .find(|(target_id, _)| target_id == &id)
                    .unwrap()
                    .1;
                stage_entity(
                    &base,
                    &id,
                    &source_value,
                    "id_collision",
                    vec![json!({"id":id,"name":target.get("name"),"tier":null})],
                )?;
                staged += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"entity","item_id":id,"match_tier":null,"reason":"id_collision","source":source_value,"target":null,"fields_changed":[]}),
                );
            } else if source.get("is_principal") == Some(&Value::Bool(true))
                && targets
                    .iter()
                    .any(|(_, target)| target.get("is_principal") == Some(&Value::Bool(true)))
            {
                stage_entity(&base, &id, &source_value, "principal_conflict", vec![])?;
                staged += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"entity","item_id":id,"match_tier":null,"reason":"principal_conflict","source":source_value,"target":null,"fields_changed":[]}),
                );
            } else {
                let path = app.root.join("entities").join(&id).join("entity.json");
                fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
                write_json(&path, &source_value)?;
                id_map.insert(id.clone(), json!(id));
                created += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"created","item_type":"entity","item_id":id,"match_tier":null,"reason":"no_match","source":source_value,"target":null,"fields_changed":[]}),
                );
            }
            received.insert(id, json!(content_hash));
            dirty = true;
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(json!({"entity_id":incoming.get("id").and_then(Value::as_str).unwrap_or(""),"error":error}));
        }
    }
    if dirty {
        entity_state = json!({"id_map":id_map,"received":received});
        let _ = write_json(&state_path, &entity_state);
    }
    json_response(
        StatusCode::OK,
        json!({"auto_merged":auto_merged,"created":created,"staged":staged,"skipped":skipped,"errors":errors}),
    )
}

fn entity_slug(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn journal_entities(root: &Path) -> Vec<(String, Map<String, Value>)> {
    fs::read_dir(root.join("entities"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            let value = read_json(&entry.path().join("entity.json"));
            value.as_object().cloned().map(|entity| (id, entity))
        })
        .collect()
}

fn merge_entity_fields(
    target: &Map<String, Value>,
    source: &Map<String, Value>,
) -> (Map<String, Value>, Vec<String>) {
    let mut merged = target.clone();
    for field in ["aka", "emails"] {
        let mut values = Vec::new();
        for object in [target, source] {
            for value in object
                .get(field)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let value = value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("");
                if !value.is_empty()
                    && !values
                        .iter()
                        .any(|existing: &String| existing.eq_ignore_ascii_case(value))
                {
                    values.push(value.to_owned());
                }
            }
        }
        if !values.is_empty() {
            values.sort_by_key(|value| value.to_ascii_lowercase());
            merged.insert(field.to_owned(), json!(values));
        }
    }
    match (target.get("created_at"), source.get("created_at")) {
        (Some(target), Some(source)) => {
            merged.insert(
                "created_at".into(),
                if earlier_created_at(source, target) {
                    source.clone()
                } else {
                    target.clone()
                },
            );
        }
        (None, Some(source)) => {
            merged.insert("created_at".into(), source.clone());
        }
        _ => {}
    }
    let changed = merged
        .iter()
        .filter(|(field, value)| target.get(*field) != Some(*value))
        .map(|(field, _)| field.clone())
        .collect();
    (merged, changed)
}

fn earlier_created_at(source: &Value, target: &Value) -> bool {
    match (source.as_f64(), target.as_f64()) {
        (Some(source), Some(target)) => source < target,
        _ => {
            serde_json::to_string(source).unwrap_or_default()
                < serde_json::to_string(target).unwrap_or_default()
        }
    }
}

fn stage_entity(
    base: &Path,
    id: &str,
    source: &Value,
    reason: &str,
    candidates: Vec<Value>,
) -> Result<(), String> {
    let staged = base.join("staged").join(format!("{id}.json"));
    fs::create_dir_all(staged.parent().unwrap()).map_err(|error| error.to_string())?;
    write_json(
        &staged,
        &json!({"source_entity":source,"match_candidates":candidates,"reason":reason,"staged_at":Utc::now().to_rfc3339()}),
    )
}

pub(crate) async fn imports(
    State(app): State<AppState>,
    AxumPath(_): AxumPath<String>,
    identity: JournalSourceIdentity,
    Json(payload): Json<Value>,
) -> Response {
    let Some(items) = payload.get("imports").and_then(Value::as_array) else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't find a required field.",
            "missing_required_field",
            "Missing imports array".into(),
        );
    };
    let base = state(&app.root, identity.prefix(), "imports");
    let state_path = base.join("state.json");
    let mut imports_state = read_json(&state_path);
    if !imports_state.is_object() {
        imports_state = json!({});
    }
    let received_hashes = imports_state
        .as_object_mut()
        .expect("object")
        .entry("received")
        .or_insert_with(|| json!({}));
    if !received_hashes.is_object() {
        *received_hashes = json!({});
    }
    let mut copied = 0;
    let mut skipped = 0;
    let mut staged = 0;
    let mut errors = Vec::new();
    for item in items {
        let result = (|| -> Result<(), String> {
            let object = item.as_object().ok_or("Import item must be an object")?;
            let id = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| {
                    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit() || byte == b'_')
                })
                .ok_or("Invalid import id")?;
            let import_json = object
                .get("import_json")
                .and_then(Value::as_object)
                .ok_or("import_json must be an object")?;
            let imported_json = object
                .get("imported_json")
                .and_then(Value::as_object)
                .ok_or("imported_json must be an object")?;
            let manifest = object
                .get("content_manifest")
                .and_then(Value::as_array)
                .ok_or("content_manifest must be an array")?;
            let content_hash = hash(serde_json::to_string(&json!({"import_json":import_json,"imported_json":imported_json,"content_manifest":manifest})).map_err(|error| error.to_string())?.as_bytes());
            if received_hashes[id].as_str() == Some(&content_hash) {
                skipped += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"skipped","item_type":"import","item_id":id,"reason":"idempotent"}),
                );
                return Ok(());
            }
            let target = app.root.join("imports").join(id);
            if target.is_dir() {
                fs::create_dir_all(base.join("staged")).map_err(|error| error.to_string())?;
                write_json(
                    &base.join("staged").join(format!("{id}.json")),
                    &json!({"import_id":id,"import_json":import_json,"imported_json":imported_json,"content_manifest":manifest,"reason":"id_collision","staged_at":Utc::now().to_rfc3339()}),
                )?;
                staged += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"import","item_id":id,"reason":"id_collision"}),
                );
            } else {
                fs::create_dir_all(&target).map_err(|error| error.to_string())?;
                write_json(
                    &target.join("import.json"),
                    &Value::Object(import_json.clone()),
                )?;
                write_json(
                    &target.join("imported.json"),
                    &Value::Object(imported_json.clone()),
                )?;
                let rows = manifest
                    .iter()
                    .map(serde_json::to_string)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?
                    .join("\n");
                let manifest_bytes = if rows.is_empty() {
                    Vec::new()
                } else {
                    format!("{rows}\n").into_bytes()
                };
                atomic_replace(
                    target.join("content_manifest.jsonl"),
                    &manifest_bytes,
                    AtomicWriteOptions { mode: Some(0o600) },
                )
                .map_err(|error| error.to_string())?;
                copied += 1;
                decision(
                    &base.join("log.jsonl"),
                    json!({"ts":Utc::now().to_rfc3339(),"action":"copied","item_type":"import","item_id":id,"reason":"new"}),
                );
            }
            received_hashes[id] = json!(content_hash);
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(json!({"import_id":item.get("id").and_then(Value::as_str).unwrap_or(""),"error":error}));
        }
    }
    let _ = write_json(&state_path, &imports_state);
    json_response(
        StatusCode::OK,
        json!({"copied":copied,"skipped":skipped,"staged":staged,"errors":errors}),
    )
}

pub(crate) async fn config(
    State(app): State<AppState>,
    AxumPath(_): AxumPath<String>,
    identity: JournalSourceIdentity,
    Json(payload): Json<Value>,
) -> Response {
    let Some(source) = payload
        .get("config")
        .or_else(|| payload.get("journal_config"))
        .filter(|value| value.is_object())
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't find a required field.",
            "missing_required_field",
            "Missing config object".into(),
        );
    };
    let base = state(&app.root, identity.prefix(), "config");
    let hash = hash(serde_json::to_string(source).unwrap().as_bytes());
    let state_path = base.join("state.json");
    let mut st = read_json(&state_path);
    if st.get("last_hash").and_then(Value::as_str) == Some(&hash) {
        decision(
            &base.join("log.jsonl"),
            json!({"ts":Utc::now().to_rfc3339(),"action":"skipped","item_type":"config","reason":"idempotent"}),
        );
        return json_response(
            StatusCode::OK,
            json!({"staged":false,"skipped":true,"reason":"idempotent"}),
        );
    }
    let target = read_json(&app.root.join("config/journal.json"));
    let source_flat = flatten_config(source, "");
    let target_flat = flatten_config(&target, "");
    let mut diff = Map::new();
    let mut fields = source_flat
        .keys()
        .chain(target_flat.keys())
        .collect::<Vec<_>>();
    fields.sort();
    fields.dedup();
    for field in fields {
        if field == "convey.password_hash" || source_flat.get(field) == target_flat.get(field) {
            continue;
        }
        diff.insert(field.to_owned(), json!({"source":source_flat.get(field),"target":target_flat.get(field),"category":if matches!(field.as_str(), "identity.name" | "identity.preferred" | "identity.bio" | "identity.pronouns" | "identity.aliases" | "identity.email_addresses" | "identity.timezone") { "transferable" } else { "preference" }}));
    }
    let _ = fs::create_dir_all(&base);
    let _ = write_json(&base.join("source_config.json"), source);
    let _ = write_json(&base.join("diff.json"), &Value::Object(diff.clone()));
    st["last_hash"] = json!(hash);
    let _ = write_json(&state_path, &st);
    decision(
        &base.join("log.jsonl"),
        json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"config"}),
    );
    json_response(
        StatusCode::OK,
        json!({"staged":true,"skipped":false,"diff_fields":diff.len()}),
    )
}

fn flatten_config(value: &Value, prefix: &str) -> HashMap<String, Value> {
    let mut flattened = HashMap::new();
    let Some(object) = value.as_object() else {
        return flattened;
    };
    for (key, value) in object {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if value.is_object() {
            flattened.extend(flatten_config(value, &path));
        } else {
            flattened.insert(path, value.clone());
        }
    }
    flattened
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::{
        body::{Body, to_bytes},
        extract::{FromRequest, Multipart},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::test_support::phase_root;

    use super::segments_with_attempts;

    const KEY: &str = "corpusSourceKey0000000000000000000000000000";
    const PREFIX: &str = "corpusSo";

    async fn request(
        root: &std::path::Path,
        path: &str,
        body: Body,
        content_type: &str,
        auth: bool,
    ) -> (StatusCode, Value) {
        let mut builder = Request::post(path).header("content-type", content_type);
        if auth {
            builder = builder.header("authorization", format!("Bearer {KEY}"));
        }
        let response = crate::routes(root.to_path_buf())
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn segment_body(bytes: &[u8]) -> Body {
        let boundary = "segment-boundary";
        let metadata = json!({"segments":[{"day":"20260801","stream":"default","segment_key":"120000_60","files":["entry.jsonl"]}]}).to_string();
        Body::from(format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files_0\"; filename=\"entry.jsonl\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--{boundary}--\r\n",
            String::from_utf8_lossy(bytes)
        ))
    }

    async fn multipart_for_test(body: Body) -> Multipart {
        Multipart::from_request(
            Request::post("/")
                .header(
                    "content-type",
                    "multipart/form-data; boundary=segment-boundary",
                )
                .body(body)
                .unwrap(),
            &(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn criterion_11_ingest_doors_authenticate_before_session_state() {
        for phase in ["unestablished", "corrupt", "empty", "populated"] {
            let root = phase_root(phase);
            for path in [
                "/app/import/journal/corpusSo/ingest/segments",
                "/app/import/journal/corpusSo/ingest/entities",
                "/app/import/journal/corpusSo/ingest/imports",
                "/app/import/journal/corpusSo/ingest/config",
            ] {
                let (status, _) = request(
                    root.path(),
                    path,
                    Body::from("{}"),
                    "application/json",
                    false,
                )
                .await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{phase} {path}");
            }
        }
    }

    #[tokio::test]
    async fn criterion_6_segment_states_preserve_existing_bytes_and_extra_files() {
        let root = phase_root("empty");
        let path = format!("/app/import/journal/{PREFIX}/ingest/segments");
        let content_type = "multipart/form-data; boundary=segment-boundary";
        assert_eq!(crate::callosum::take_send_attempts(), 0);
        let (status, copied) = request(
            root.path(),
            &path,
            segment_body(b"one\n"),
            content_type,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(copied["segments_received"], 1);
        // The socket is absent in this fixture; the success response alone would not prove
        // best-effort delivery was attempted.
        assert_eq!(crate::callosum::take_send_attempts(), 1);
        let target = root.path().join("chronicle/20260801/default/120000_60");
        fs::write(target.join("extra-owner-file"), b"leave me").unwrap();
        let (status, skipped) = request(
            root.path(),
            &path,
            segment_body(b"one\n"),
            content_type,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (
                skipped["segments_received"].clone(),
                skipped["segments_skipped"].clone()
            ),
            (json!(0), json!(1))
        );
        assert_eq!(
            fs::read(target.join("extra-owner-file")).unwrap(),
            b"leave me"
        );
        let state: Value = serde_json::from_slice(
            &fs::read(root.path().join("imports/corpusSo/segments/state.json")).unwrap(),
        )
        .unwrap();
        assert!(
            state["20260801"]["default/120000_60"].is_object(),
            "skipped re-send still records its arc key"
        );
        let skipped_log =
            fs::read_to_string(root.path().join("imports/corpusSo/segments/log.jsonl")).unwrap();
        assert!(skipped_log.contains("\"action\":\"skipped\""));
        assert!(skipped_log.contains("\"reason\":\"exact match\""));
        let (status, deconflicted) = request(
            root.path(),
            &path,
            segment_body(b"two\n"),
            content_type,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(deconflicted["segments_deconflicted"], 1);
        assert_eq!(fs::read(target.join("entry.jsonl")).unwrap(), b"one\n");
        let log =
            fs::read_to_string(root.path().join("imports/corpusSo/segments/log.jsonl")).unwrap();
        assert!(log.contains("\"action\":\"deconflicted\""));
        assert!(log.contains("\"original_key\":\"120000_60\""));
        let state: Value = serde_json::from_slice(
            &fs::read(root.path().join("imports/corpusSo/segments/state.json")).unwrap(),
        )
        .unwrap();
        let keys = state["20260801"].as_object().unwrap();
        assert!(keys.contains_key("default/120000_60"));
        assert_eq!(
            keys.len(),
            2,
            "deconfliction records both original and alternate keys"
        );
    }

    #[tokio::test]
    async fn criterion_7_exhaustion_is_reported_before_the_handler_creates_any_artifact() {
        let root = phase_root("empty");
        let target = root.path().join("chronicle/20260801/default/120000_60");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("entry.jsonl"), b"owner bytes").unwrap();
        let before = fs::read_dir(&target)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        let response = segments_with_attempts(
            crate::AppState {
                root: root.path().to_path_buf(),
            },
            PREFIX,
            multipart_for_test(segment_body(b"inbound bytes\n")).await,
            0,
        )
        .await;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["errors"][0]["error"], "No available segment slot");
        assert_eq!(
            fs::read_dir(&target)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            before,
            "the refusal occurs before a segment directory write, including temporary artifacts"
        );
        assert!(
            !root
                .path()
                .join("imports/corpusSo/segments/log.jsonl")
                .exists()
        );
        assert!(
            !root
                .path()
                .join("imports/corpusSo/segments/state.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn criterion_13_keyed_doors_write_their_owned_destinations() {
        let root = phase_root("empty");
        let entities = format!("/app/import/journal/{PREFIX}/ingest/entities");
        let imports = format!("/app/import/journal/{PREFIX}/ingest/imports");
        let config = format!("/app/import/journal/{PREFIX}/ingest/config");
        for (path, payload) in [
            (&entities, json!({"entities":[{"id":"ada","name":"Ada"}]})),
            (
                &imports,
                json!({"imports":[{"id":"20260801_120000","import_json":{},"imported_json":{},"content_manifest":[]}]}),
            ),
            (&config, json!({"config":{"identity":{"name":"Ada"}}})),
        ] {
            let (status, _) = request(
                root.path(),
                path,
                Body::from(payload.to_string()),
                "application/json",
                true,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{path}");
        }
        assert!(root.path().join("entities/ada/entity.json").is_file());
        assert!(
            root.path()
                .join("imports/20260801_120000/content_manifest.jsonl")
                .is_file()
        );
        assert!(
            root.path()
                .join("imports/corpusSo/config/source_config.json")
                .is_file()
        );
    }

    fn write_entity(root: &std::path::Path, id: &str, value: Value) {
        let path = root.join("entities").join(id).join("entity.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    async fn post_entities(root: &std::path::Path, entities: Value) -> Value {
        let (status, response) = request(
            root,
            &format!("/app/import/journal/{PREFIX}/ingest/entities"),
            Body::from(json!({"entities":entities}).to_string()),
            "application/json",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        response
    }

    #[tokio::test]
    async fn entity_high_confidence_merge_keeps_owner_fields_and_removes_staging() {
        let root = phase_root("empty");
        write_entity(
            root.path(),
            "owner-ada",
            json!({"id":"owner-ada","name":"Ada","title":"owner title","aka":["A."],"emails":["owner@example.test"],"created_at":10}),
        );
        let staged = root
            .path()
            .join("imports/corpusSo/entities/staged/peer-ada.json");
        fs::create_dir_all(staged.parent().unwrap()).unwrap();
        fs::write(&staged, b"old proposal").unwrap();
        let response = post_entities(root.path(), json!([{"id":"peer-ada","name":"Ada","title":"peer title","aka":["Ada Lovelace"],"emails":["OWNER@example.test","ada@example.test"],"created_at":5,"source_only":true}])).await;
        assert_eq!(response["auto_merged"], 1);
        let merged: Value = serde_json::from_slice(
            &fs::read(root.path().join("entities/owner-ada/entity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(merged["title"], "owner title");
        assert!(merged.get("source_only").is_none());
        assert_eq!(merged["created_at"], 5);
        assert!(!staged.exists());
    }

    #[tokio::test]
    async fn entity_ambiguous_match_stages_instead_of_writing_a_canonical_entity() {
        let root = phase_root("empty");
        write_entity(root.path(), "ada-one", json!({"name":"Ada"}));
        write_entity(root.path(), "ada-two", json!({"name":"Ada"}));
        let response = post_entities(root.path(), json!([{"id":"peer-ada","name":"Ada"}])).await;
        assert_eq!(response["staged"], 1);
        assert!(
            root.path()
                .join("imports/corpusSo/entities/staged/peer-ada.json")
                .is_file()
        );
        assert!(!root.path().join("entities/peer-ada/entity.json").exists());
    }

    #[tokio::test]
    async fn entity_id_collision_stages_instead_of_overwriting_owner_entity() {
        let root = phase_root("empty");
        write_entity(root.path(), "shared", json!({"id":"shared","name":"Owner"}));
        let response = post_entities(root.path(), json!([{"id":"shared","name":"Peer"}])).await;
        assert_eq!(response["staged"], 1);
        let owner: Value = serde_json::from_slice(
            &fs::read(root.path().join("entities/shared/entity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(owner["name"], "Owner");
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(
                    root.path()
                        .join("imports/corpusSo/entities/staged/shared.json")
                )
                .unwrap()
            )
            .unwrap()["reason"],
            "id_collision"
        );
    }

    #[tokio::test]
    async fn entity_principal_conflict_stages_instead_of_creating_a_second_principal() {
        let root = phase_root("empty");
        write_entity(
            root.path(),
            "owner",
            json!({"id":"owner","name":"Owner","is_principal":true}),
        );
        let response = post_entities(
            root.path(),
            json!([{"id":"peer","name":"Peer","is_principal":true}]),
        )
        .await;
        assert_eq!(response["staged"], 1);
        assert!(!root.path().join("entities/peer/entity.json").exists());
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(
                    root.path()
                        .join("imports/corpusSo/entities/staged/peer.json")
                )
                .unwrap()
            )
            .unwrap()["reason"],
            "principal_conflict"
        );
    }

    #[tokio::test]
    async fn config_diff_is_allowed_field_aware_and_hash_skip_only_logs() {
        let root = phase_root("empty");
        fs::write(root.path().join("config/journal.json"), json!({"identity":{"name":"Owner","timezone":"UTC"},"convey":{"password_hash":"owner-secret"},"theme":"dark"}).to_string()).unwrap();
        let payload = json!({"config":{"identity":{"name":"Peer","timezone":"UTC"},"convey":{"password_hash":"peer-secret"},"theme":"light"}});
        let path = format!("/app/import/journal/{PREFIX}/ingest/config");
        let (status, first) = request(
            root.path(),
            &path,
            Body::from(payload.to_string()),
            "application/json",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["diff_fields"], 2);
        let diff: Value = serde_json::from_slice(
            &fs::read(root.path().join("imports/corpusSo/config/diff.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(diff["identity.name"]["category"], "transferable");
        assert_eq!(diff["theme"]["category"], "preference");
        assert!(diff.get("convey.password_hash").is_none());
        let source_before = fs::read(
            root.path()
                .join("imports/corpusSo/config/source_config.json"),
        )
        .unwrap();
        let (status, second) = request(
            root.path(),
            &path,
            Body::from(payload.to_string()),
            "application/json",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            second,
            json!({"staged":false,"skipped":true,"reason":"idempotent"})
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join("imports/corpusSo/config/source_config.json")
            )
            .unwrap(),
            source_before
        );
    }
}
