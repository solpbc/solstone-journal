// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    AppState,
    callosum::emit_best_effort,
    http::{error, json as json_response},
    journal_sources::{JournalSourceIdentity, provenance_for_prefix, record_received},
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
    AtomicWriteOptions, append_jsonl, atomic_replace, bump_stream_marker, contained_path,
    find_available_segment,
};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::{Component, Path},
};

const MAX_ATTEMPTS: usize = 32;
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
fn decision(path: &Path, mut value: Value) -> Result<(), String> {
    // Every door audit entry carries the paired peer identity.  The state root is
    // structurally `root/imports/<prefix>/<area>/log.jsonl`.
    if let Some(root) = path.ancestors().nth(4)
        && let Some(prefix) = path
            .ancestors()
            .nth(2)
            .and_then(Path::file_name)
            .and_then(|p| p.to_str())
        && let Some(provenance) = provenance_for_prefix(root, prefix)
        && let (Some(entry), Some(provenance)) = (value.as_object_mut(), provenance.as_object())
    {
        entry.extend(provenance.clone());
    }
    append_jsonl(path, &value).map_err(|error| error.to_string())
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

fn safe_component(value: &str) -> bool {
    matches!(
        Path::new(value).components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    )
}

fn contained(root: &Path, relative: &str) -> Result<std::path::PathBuf, String> {
    contained_path(root, relative).map_err(|error| error.to_string())
}

fn import_file(
    root: &Path,
    prefix: &str,
    area: &str,
    file: &str,
) -> Result<std::path::PathBuf, String> {
    contained(root, &format!("imports/{prefix}/{area}/{file}"))
}

fn invalid_owned_path(detail: String) -> Response {
    error(
        StatusCode::BAD_REQUEST,
        "I couldn't use one of those values.",
        "invalid_request_value",
        detail,
    )
}

fn valid_stream(value: &str) -> bool {
    value == "_default"
        || (value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            }))
}

fn valid_segment_key(value: &str) -> bool {
    let Some((time, duration)) = value.split_once('_') else {
        return false;
    };
    time.len() == 6
        && time.bytes().all(|byte| byte.is_ascii_digit())
        && !duration.is_empty()
        && duration.bytes().all(|byte| byte.is_ascii_digit())
}

fn dirty_segment_days(root: &Path, days: &BTreeSet<String>) -> Result<(), String> {
    let mut failures = Vec::new();
    for day in days {
        if let Err(error) = bump_stream_marker(root, day) {
            failures.push(format!("{day}: {error}"));
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "stream marker update failed after journal-source segment content was published: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

pub(crate) async fn segments(
    State(app): State<AppState>,
    AxumPath(_): AxumPath<String>,
    identity: JournalSourceIdentity,
    multipart_body: Multipart,
) -> Response {
    segments_with_attempts(
        app,
        identity.prefix(),
        multipart_body,
        MAX_ATTEMPTS,
        Some(&identity),
    )
    .await
}

async fn segments_with_attempts(
    app: AppState,
    prefix: &str,
    multipart_body: Multipart,
    max_attempts: usize,
    identity: Option<&JournalSourceIdentity>,
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
    let log = match import_file(&app.root, prefix, "segments", "log.jsonl") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let mut new = Map::new();
    let (mut copied, mut skipped, mut deconflicted) = (0, 0, 0);
    let mut mutated_days = BTreeSet::new();
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
                .filter(|stream| valid_stream(stream))
                .ok_or("Invalid stream format")?;
            let key = obj
                .get("segment_key")
                .and_then(Value::as_str)
                .filter(|key| valid_segment_key(key))
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
            // The transfer record spelling `_default` denotes direct layout.
            let stream_relative = if stream == solstone_core_journal_io::DEFAULT_STREAM {
                format!("chronicle/{day}")
            } else {
                format!("chronicle/{day}/{stream}")
            };
            let stream_dir = contained(&app.root, &stream_relative)?;
            let mut target = contained(&app.root, &format!("{stream_relative}/{key}"))?;
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
                    target = contained(&app.root, &format!("{stream_relative}/{next}"))?;
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
                    let file = contained(&app.root, &format!("{stream_relative}/{final_key}/{n}"))?;
                    atomic_replace(file, &payload[n], AtomicWriteOptions { mode: Some(0o600) })
                        .map_err(|e| e.to_string())?;
                    mutated_days.insert(day.to_owned());
                }
            }
            let mut rec = json!({"files":names.iter().map(|n|json!({"name":n,"sha256":hash(&payload[n]),"size":payload[n].len()})).collect::<Vec<_>>()});
            if let Some(identity) = identity {
                rec.as_object_mut().expect("segment record object").extend(
                    identity
                        .provenance()
                        .as_object()
                        .expect("provenance object")
                        .clone(),
                );
            }
            let day_state = new
                .entry(day.to_owned())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .unwrap();
            day_state.insert(format!("{stream}/{final_key}"), rec.clone());
            if action == "deconflicted" {
                day_state.insert(format!("{stream}/{original}"), rec);
            }
            let mut entry = json!({"ts":Utc::now().to_rfc3339(),"action":action,"item_type":"segment","item_id":format!("{day}/{stream}/{final_key}"),"reason":reason,"files":names});
            if let Some(identity) = identity {
                entry.as_object_mut().expect("segment log object").extend(
                    identity
                        .provenance()
                        .as_object()
                        .expect("provenance object")
                        .clone(),
                );
            }
            if action == "deconflicted" {
                entry["original_key"] = json!(original)
            }
            decision(&log, entry)?;
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
        let path = match import_file(&app.root, prefix, "segments", "state.json") {
            Ok(path) => path,
            Err(detail) => return invalid_owned_path(detail),
        };
        let mut existing = read_json(&path);
        let obj = existing.as_object_mut().unwrap();
        for (k, v) in new {
            let d = obj.entry(k).or_insert_with(|| json!({}));
            d.as_object_mut()
                .unwrap()
                .extend(v.as_object().unwrap().clone());
        }
        if let Err(detail) = write_json(&path, &existing) {
            let detail = dirty_segment_days(&app.root, &mutated_days)
                .err()
                .map_or(detail.clone(), |marker| format!("{detail}; {marker}"));
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't save that import.",
                "import_metadata_failed",
                detail,
            );
        }
    }
    let written = copied + deconflicted;
    if let Some(identity) = identity
        && let Err(detail) = record_received(&app.root, identity, "segments_received", written)
    {
        let detail = dirty_segment_days(&app.root, &mutated_days)
            .err()
            .map_or(detail.clone(), |marker| format!("{detail}; {marker}"));
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
    if let Err(detail) = dirty_segment_days(&app.root, &mutated_days) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "stream_marker_failed",
            detail,
        );
    }
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

fn has_retired_facet_content(facets: &[Value]) -> bool {
    facets.iter().any(|facet| {
        facet
            .get("files")
            .and_then(Value::as_array)
            .is_some_and(|files| {
                files.iter().any(|descriptor| {
                    let kind = descriptor.get("type").and_then(Value::as_str);
                    let path = descriptor.get("path").and_then(Value::as_str);
                    kind == Some("todos")
                        || path.is_some_and(|path| path == "todos" || path.starts_with("todos/"))
                })
            })
    })
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
    if has_retired_facet_content(facets) {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "Retired facet content is not accepted; nothing was imported.".into(),
        );
    }
    let entity_state_path =
        match import_file(&app.root, identity.prefix(), "entities", "state.json") {
            Ok(path) => path,
            Err(detail) => return invalid_owned_path(detail),
        };
    let entity_id_map = read_json(&entity_state_path)
        .get("id_map")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let state_path = match import_file(&app.root, identity.prefix(), "facets", "state.json") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let staged_path = match import_file(&app.root, identity.prefix(), "facets", "staged") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let log_path = match import_file(&app.root, identity.prefix(), "facets", "log.jsonl") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
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
    let mut received_facets = HashSet::new();
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
                    .ok_or("Facet file metadata must include path and type")?;
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
                crate::facet_ingest::FacetRoots {
                    // Native deliberately has one journal-root authority; see `FacetRoots`.
                    direct: &app.root,
                    ambient: &app.root,
                },
                name,
                &items,
                &staged_path,
                &entity_id_map,
                received,
            )?;
            created += outcome.created;
            merged += outcome.merged;
            skipped += outcome.skipped;
            staged += outcome.staged;
            errors.extend(outcome.errors);
            if outcome.wrote_files {
                received_facets.insert(name.to_owned());
            }
            for entry in outcome.decisions {
                decision(&log_path, entry)?;
            }
            Ok(())
        })();
        if let Err(error) = outcome {
            errors.push(json!({"facet":facet.get("name").and_then(Value::as_str).unwrap_or(""),"error":error}));
        }
    }
    if let Err(detail) = write_json(&state_path, &facet_state) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
    if let Err(detail) = record_received(
        &app.root,
        &identity,
        "facets_received",
        received_facets.len(),
    ) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
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
    let state_path = match import_file(&app.root, identity.prefix(), "entities", "state.json") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let log_path = match import_file(&app.root, identity.prefix(), "entities", "log.jsonl") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
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
            let id = match source.get("id") {
                Some(Value::String(id)) if safe_component(id) => id.to_owned(),
                Some(Value::String(_)) => return Err("Invalid entity id".into()),
                Some(_) => return Err("Invalid entity id".into()),
                None => entity_slug(&name),
            };
            if !safe_component(&id) {
                return Err("Invalid entity id".into());
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
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"skipped","item_type":"entity","item_id":id,"match_tier":null,"reason":"idempotent","source":source_value,"target":null,"fields_changed":[]}),
                )?;
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
            let staged_path = contained(
                &app.root,
                &format!("imports/{}/entities/staged/{id}.json", identity.prefix()),
            )?;
            if exact.len() == 1 {
                let (target_id, target) = exact[0];
                let (merged, fields_changed) = merge_entity_fields(target, &source);
                let path = contained(&app.root, &format!("entities/{target_id}/entity.json"))?;
                write_json(&path, &Value::Object(merged.clone()))?;
                if let Err(error) = fs::remove_file(&staged_path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(error.to_string());
                }
                id_map.insert(id.clone(), json!(target_id));
                auto_merged += 1;
                decision(
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"auto_merged","item_type":"entity","item_id":id,"match_tier":1,"reason":"high_confidence_match","source":source_value,"target":merged,"fields_changed":fields_changed}),
                )?;
            } else if exact.len() > 1 {
                stage_entity(&staged_path, &source_value, "low_confidence_match", exact.iter().map(|(target_id, target)| json!({"id":target_id,"name":target.get("name"),"tier":1})).collect())?;
                staged += 1;
                decision(
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"entity","item_id":id,"match_tier":1,"reason":"low_confidence_match","source":source_value,"target":null,"fields_changed":[]}),
                )?;
            } else if targets.iter().any(|(target_id, _)| target_id == &id) {
                let target = &targets
                    .iter()
                    .find(|(target_id, _)| target_id == &id)
                    .unwrap()
                    .1;
                stage_entity(
                    &staged_path,
                    &source_value,
                    "id_collision",
                    vec![json!({"id":id,"name":target.get("name"),"tier":null})],
                )?;
                staged += 1;
                decision(
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"entity","item_id":id,"match_tier":null,"reason":"id_collision","source":source_value,"target":null,"fields_changed":[]}),
                )?;
            } else if source.get("is_principal") == Some(&Value::Bool(true))
                && targets
                    .iter()
                    .any(|(_, target)| target.get("is_principal") == Some(&Value::Bool(true)))
            {
                stage_entity(&staged_path, &source_value, "principal_conflict", vec![])?;
                staged += 1;
                decision(
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"entity","item_id":id,"match_tier":null,"reason":"principal_conflict","source":source_value,"target":null,"fields_changed":[]}),
                )?;
            } else {
                let path = contained(&app.root, &format!("entities/{id}/entity.json"))?;
                write_json(&path, &source_value)?;
                id_map.insert(id.clone(), json!(id));
                created += 1;
                decision(
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"created","item_type":"entity","item_id":id,"match_tier":null,"reason":"no_match","source":source_value,"target":null,"fields_changed":[]}),
                )?;
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
        if let Err(detail) = write_json(&state_path, &entity_state) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't save that import.",
                "import_metadata_failed",
                detail,
            );
        }
    }
    if let Err(detail) = record_received(
        &app.root,
        &identity,
        "entities_received",
        auto_merged + created,
    ) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
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
    staged: &Path,
    source: &Value,
    reason: &str,
    candidates: Vec<Value>,
) -> Result<(), String> {
    fs::create_dir_all(staged.parent().unwrap()).map_err(|error| error.to_string())?;
    write_json(
        staged,
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
    let state_path = match import_file(&app.root, identity.prefix(), "imports", "state.json") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let log_path = match import_file(&app.root, identity.prefix(), "imports", "log.jsonl") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
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
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"skipped","item_type":"import","item_id":id,"reason":"idempotent"}),
                )?;
                return Ok(());
            }
            let target = contained(&app.root, &format!("imports/{id}"))?;
            if target.is_dir() {
                let staged_path = import_file(
                    &app.root,
                    identity.prefix(),
                    "imports",
                    &format!("staged/{id}.json"),
                )?;
                write_json(
                    &staged_path,
                    &json!({"import_id":id,"import_json":import_json,"imported_json":imported_json,"content_manifest":manifest,"reason":"id_collision","staged_at":Utc::now().to_rfc3339()}),
                )?;
                staged += 1;
                decision(
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"import","item_id":id,"reason":"id_collision"}),
                )?;
            } else {
                fs::create_dir_all(&target).map_err(|error| error.to_string())?;
                write_json(
                    &contained(&app.root, &format!("imports/{id}/import.json"))?,
                    &Value::Object(import_json.clone()),
                )?;
                write_json(
                    &contained(&app.root, &format!("imports/{id}/imported.json"))?,
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
                    contained(&app.root, &format!("imports/{id}/content_manifest.jsonl"))?,
                    &manifest_bytes,
                    AtomicWriteOptions { mode: Some(0o600) },
                )
                .map_err(|error| error.to_string())?;
                copied += 1;
                decision(
                    &log_path,
                    json!({"ts":Utc::now().to_rfc3339(),"action":"copied","item_type":"import","item_id":id,"reason":"new"}),
                )?;
            }
            received_hashes[id] = json!(content_hash);
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(json!({"import_id":item.get("id").and_then(Value::as_str).unwrap_or(""),"error":error}));
        }
    }
    if let Err(detail) = write_json(&state_path, &imports_state) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
    if let Err(detail) = record_received(&app.root, &identity, "imports_received", copied) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
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
    let hash = hash(serde_json::to_string(source).unwrap().as_bytes());
    let state_path = match import_file(&app.root, identity.prefix(), "config", "state.json") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let source_config_path =
        match import_file(&app.root, identity.prefix(), "config", "source_config.json") {
            Ok(path) => path,
            Err(detail) => return invalid_owned_path(detail),
        };
    let diff_path = match import_file(&app.root, identity.prefix(), "config", "diff.json") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let log_path = match import_file(&app.root, identity.prefix(), "config", "log.jsonl") {
        Ok(path) => path,
        Err(detail) => return invalid_owned_path(detail),
    };
    let mut st = read_json(&state_path);
    if st.get("last_hash").and_then(Value::as_str) == Some(&hash) {
        if let Err(detail) = decision(
            &log_path,
            json!({"ts":Utc::now().to_rfc3339(),"action":"skipped","item_type":"config","reason":"idempotent"}),
        ) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "I couldn't save that import.",
                "import_metadata_failed",
                detail,
            );
        }
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
    if let Err(detail) = write_json(&source_config_path, source) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
    if let Err(detail) = write_json(&diff_path, &Value::Object(diff.clone())) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
    st["last_hash"] = json!(hash);
    if let Err(detail) = write_json(&state_path, &st) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
    if let Err(detail) = decision(
        &log_path,
        json!({"ts":Utc::now().to_rfc3339(),"action":"staged","item_type":"config"}),
    ) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
    if let Err(detail) = record_received(&app.root, &identity, "config_received", 1) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "I couldn't save that import.",
            "import_metadata_failed",
            detail,
        );
    }
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
    use std::{collections::BTreeMap, fs, path::Path};

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

    fn segment_body_for(stream: &str, key: &str, bytes: &[u8]) -> Body {
        let boundary = "segment-boundary";
        let metadata = json!({"segments":[{"day":"20260801","stream":stream,"segment_key":key,"files":["entry.jsonl"]}]}).to_string();
        Body::from(format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files_0\"; filename=\"entry.jsonl\"\r\nContent-Type: application/json\r\n\r\n{}\r\n--{boundary}--\r\n",
            String::from_utf8_lossy(bytes)
        ))
    }

    fn segment_body(bytes: &[u8]) -> Body {
        segment_body_for("default", "120000_60", bytes)
    }

    fn segment_batch_body(items: &[(&str, &str, &[u8])]) -> Body {
        let boundary = "segment-boundary";
        let metadata = json!({
            "segments": items
                .iter()
                .map(|(day, key, _)| json!({
                    "day": day,
                    "stream": "default",
                    "segment_key": key,
                    "files": ["entry.jsonl"],
                }))
                .collect::<Vec<_>>()
        })
        .to_string();
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n"
        );
        for (index, (_, _, bytes)) in items.iter().enumerate() {
            body.push_str(&format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files_{index}\"; filename=\"entry.jsonl\"\r\nContent-Type: application/json\r\n\r\n{}\r\n",
                String::from_utf8_lossy(bytes)
            ));
        }
        body.push_str(&format!("--{boundary}--\r\n"));
        Body::from(body)
    }

    fn whole_tree_snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn visit(root: &Path, path: &Path, snapshot: &mut BTreeMap<String, Vec<u8>>) {
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let metadata = fs::symlink_metadata(path).unwrap();
            if !relative.is_empty() {
                let bytes = if metadata.file_type().is_symlink() {
                    fs::read_link(path)
                        .unwrap()
                        .as_os_str()
                        .to_string_lossy()
                        .into_owned()
                        .into_bytes()
                } else if metadata.is_file() {
                    fs::read(path).unwrap()
                } else {
                    b"directory".to_vec()
                };
                snapshot.insert(relative, bytes);
            }
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap().flatten() {
                    visit(root, &entry.path(), snapshot);
                }
            }
        }

        let mut snapshot = BTreeMap::new();
        visit(root, root, &mut snapshot);
        snapshot
    }

    fn facet_body(metadata: Value, contents: &[&str]) -> Body {
        let boundary = "facet-boundary";
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n"
        );
        for (index, content) in contents.iter().enumerate() {
            body.push_str(&format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"files_0_{index}\"; filename=\"facet.jsonl\"\r\n\r\n{content}\r\n"
            ));
        }
        body.push_str(&format!("--{boundary}--\r\n"));
        Body::from(body)
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
        let _ = crate::callosum::take_send_attempts();
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
    async fn direct_segment_import_preserves_layout_and_conflict_behavior() {
        let root = phase_root("empty");
        let path = format!("/app/import/journal/{PREFIX}/ingest/segments");
        let content_type = "multipart/form-data; boundary=segment-boundary";
        let day = root.path().join("chronicle/20260801");
        let direct = day.join("120000_60/entry.jsonl");
        let named = day.join("_default/120000_60/entry.jsonl");
        fs::create_dir_all(named.parent().unwrap()).unwrap();
        fs::write(&named, b"unrelated named-layout data").unwrap();
        for (bytes, field) in [
            (b"one\n".as_slice(), "segments_received"),
            (b"one\n".as_slice(), "segments_skipped"),
            (b"two\n".as_slice(), "segments_deconflicted"),
        ] {
            let (status, result) = request(
                root.path(),
                &path,
                segment_body_for("_default", "120000_60", bytes),
                content_type,
                true,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(result["errors"], json!([]));
            assert_eq!(result[field], 1);
            assert_eq!(fs::read(&direct).unwrap(), b"one\n");
            assert_eq!(fs::read(&named).unwrap(), b"unrelated named-layout data");
        }
        let state: Value = serde_json::from_slice(
            &fs::read(root.path().join("imports/corpusSo/segments/state.json")).unwrap(),
        )
        .unwrap();
        let keys = state["20260801"].as_object().unwrap();
        assert_eq!(keys.len(), 2);
        let alternate = keys
            .keys()
            .find(|key| key.as_str() != "_default/120000_60")
            .unwrap();
        let segment = alternate.strip_prefix("_default/").unwrap();
        assert_eq!(
            fs::read(day.join(segment).join("entry.jsonl")).unwrap(),
            b"two\n"
        );
        assert!(!day.join("_default").join(segment).exists());
    }

    #[tokio::test]
    async fn segment_ingest_dirties_only_copied_and_deconflicted_days() {
        use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};

        let root = phase_root("empty");
        let existing = root.path().join("chronicle/20260802/default/120000_60");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("entry.jsonl"), b"same\n").unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260804/default/140000_60")).unwrap();
        fs::write(
            root.path()
                .join("chronicle/20260804/default/140000_60/entry.jsonl"),
            b"untouched\n",
        )
        .unwrap();

        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/segments"),
            segment_batch_body(&[
                ("20260801", "110000_60", b"new-one\n"),
                ("20260802", "120000_60", b"same\n"),
                ("20260803", "130000_60", b"new-three\n"),
            ]),
            "multipart/form-data; boundary=segment-boundary",
            true,
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["segments_received"], 2);
        assert_eq!(response["segments_skipped"], 1);
        for day in ["20260801", "20260803"] {
            assert!(matches!(
                read_health_marker(root.path(), day, HealthMarkerKind::Stream).unwrap(),
                HealthMarkerState::Versioned { marker, .. } if marker.generation == 1
            ));
        }
        for day in ["20260802", "20260804"] {
            assert!(matches!(
                read_health_marker(root.path(), day, HealthMarkerKind::Stream).unwrap(),
                HealthMarkerState::Absent
            ));
        }
    }

    #[tokio::test]
    async fn segment_ingest_marker_failure_is_terminal_after_content_publication() {
        let root = phase_root("empty");
        fs::create_dir_all(root.path().join("chronicle/20260801/health/stream.updated")).unwrap();

        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/segments"),
            segment_body(b"published\n"),
            "multipart/form-data; boundary=segment-boundary",
            true,
        )
        .await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{response}");
        assert_eq!(response["reason_code"], "stream_marker_failed");
        assert!(
            response["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("stream marker update failed"))
        );
        assert_eq!(
            fs::read(
                root.path()
                    .join("chronicle/20260801/default/120000_60/entry.jsonl")
            )
            .unwrap(),
            b"published\n"
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
            None,
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
    async fn traversal_segment_components_are_refused_without_touching_the_adjacent_chronicle_tree()
    {
        let root = phase_root("empty");
        let outside = root.path().join("chronicle/escape/120000_60/entry.jsonl");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, b"owner bytes").unwrap();

        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/segments"),
            segment_body_for("../escape", "120000_60", b"peer bytes"),
            "multipart/form-data; boundary=segment-boundary",
            true,
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["errors"][0]["error"], "Invalid stream format");
        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/segments"),
            segment_body_for("default", "../escape", b"peer bytes"),
            "multipart/form-data; boundary=segment-boundary",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["errors"][0]["error"], "Invalid segment_key format");
        assert_eq!(fs::read(outside).unwrap(), b"owner bytes");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_segment_components_are_refused_without_writing_outside_the_journal() {
        use std::os::unix::fs::symlink;

        let root = phase_root("empty");
        let outside = tempfile::TempDir::new().unwrap();
        let stream = root.path().join("chronicle/20260801/default");
        fs::create_dir_all(stream.parent().unwrap()).unwrap();
        symlink(outside.path(), &stream).unwrap();
        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/segments"),
            segment_body(b"peer bytes"),
            "multipart/form-data; boundary=segment-boundary",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["errors"].as_array().unwrap().len(), 1);
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());

        let root = phase_root("empty");
        let outside = tempfile::TempDir::new().unwrap();
        let segment = root.path().join("chronicle/20260801/default/120000_60");
        fs::create_dir_all(segment.parent().unwrap()).unwrap();
        symlink(outside.path(), &segment).unwrap();
        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/segments"),
            segment_body(b"peer bytes"),
            "multipart/form-data; boundary=segment-boundary",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["errors"].as_array().unwrap().len(), 1);
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[tokio::test]
    async fn criterion_13_keyed_doors_write_their_owned_destinations() {
        let root = phase_root("empty");
        let source_path = root
            .path()
            .join("apps/import/journal_sources/corpus_peer.json");
        let mut source: Value = serde_json::from_slice(&fs::read(&source_path).unwrap()).unwrap();
        source["fingerprint"] = json!("sha256:peer-fingerprint");
        source["peer_instance_id"] = json!("peer-instance");
        fs::write(&source_path, serde_json::to_vec(&source).unwrap()).unwrap();
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
        let (status, _) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/segments"),
            segment_body(b"provenance"),
            "multipart/form-data; boundary=segment-boundary",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let boundary = "facet-provenance";
        let metadata = json!({"facets":[{"name":"work","files":[{"path":"logs/20260801.jsonl","type":"logs"}]}]});
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files_0_0\"; filename=\"20260801.jsonl\"\r\n\r\n{{\"message\":\"peer log\"}}\r\n--{boundary}--\r\n"
        );
        let (status, _) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/facets"),
            Body::from(body),
            &format!("multipart/form-data; boundary={boundary}"),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let source: Value = serde_json::from_slice(&fs::read(&source_path).unwrap()).unwrap();
        for stat in [
            "segments_received",
            "entities_received",
            "facets_received",
            "imports_received",
            "config_received",
        ] {
            assert_eq!(
                source["stats"][stat], 1,
                "{stat} increments after its write"
            );
        }
        let state: Value = serde_json::from_slice(
            &fs::read(root.path().join("imports/corpusSo/segments/state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            state["20260801"]["default/120000_60"]["link_id"],
            "sha256:peer-fingerprint"
        );
        let log =
            fs::read_to_string(root.path().join("imports/corpusSo/entities/log.jsonl")).unwrap();
        assert!(
            log.contains("peer-instance"),
            "door audit records sender provenance"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_inbound_import_directory_is_refused_without_writing_outside_the_journal() {
        use std::os::unix::fs::symlink;

        let root = phase_root("empty");
        let outside = tempfile::TempDir::new().unwrap();
        let id = "20260801_120000";
        symlink(outside.path(), root.path().join("imports").join(id)).unwrap();
        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/imports"),
            Body::from(
                json!({"imports":[{"id":id,"import_json":{},"imported_json":{},"content_manifest":[]}]}).to_string(),
            ),
            "application/json",
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["errors"].as_array().unwrap().len(), 1);
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_facet_directory_is_refused_without_a_whole_tree_delta() {
        use std::os::unix::fs::symlink;

        let root = phase_root("empty");
        let outside = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("facets")).unwrap();
        symlink(outside.path(), root.path().join("facets/work")).unwrap();
        let state = root.path().join("imports/corpusSo/facets/state.json");
        fs::write(
            &state,
            serde_json::to_vec_pretty(&json!({"received":{}})).unwrap(),
        )
        .unwrap();
        let before = (
            whole_tree_snapshot(root.path()),
            whole_tree_snapshot(outside.path()),
        );
        let boundary = "facet-symlink";
        let metadata = json!({"facets":[{"name":"work","files":[{"path":"logs/20260101.jsonl","type":"logs"}]}]});
        let body = Body::from(format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"files_0_0\"; filename=\"20260101.jsonl\"\r\n\r\n{{\"message\":\"must not escape\"}}\n\r\n--{boundary}--\r\n"
        ));
        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/facets"),
            body,
            &format!("multipart/form-data; boundary={boundary}"),
            true,
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(response["errors"].as_array().unwrap().len(), 1);
        assert_eq!(
            (
                whole_tree_snapshot(root.path()),
                whole_tree_snapshot(outside.path())
            ),
            before,
        );
    }

    #[tokio::test]
    async fn supported_facet_content_is_imported() {
        let root = phase_root("empty");
        let metadata = json!({"facets":[{"name":"work","files":[{"path":"logs/20260801.jsonl","type":"logs"}]}]});
        let (status, response) = request(
            root.path(),
            &format!("/app/import/journal/{PREFIX}/ingest/facets"),
            facet_body(metadata, &[r#"{"message":"peer log"}"#]),
            "multipart/form-data; boundary=facet-boundary",
            true,
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["created"], 1);
        assert_eq!(
            fs::read(root.path().join("facets/work/logs/20260801.jsonl")).unwrap(),
            b"{\"message\":\"peer log\"}\n"
        );
    }

    #[tokio::test]
    async fn retired_facet_content_is_rejected_before_any_import_state_changes() {
        let cases = [
            (
                json!({"facets":[{"name":"work","files":[{"path":"logs/20260801.jsonl","type":"logs"},{"path":"logs/20260802.jsonl","type":"todos"}]}]}),
                vec![r#"{"message":"supported"}"#, r#"{"message":"retired"}"#],
            ),
            (
                json!({"facets":[{"name":"work","files":[{"path":"logs/20260801.jsonl","type":"logs"},{"path":"todos/20260802.jsonl","type":"logs"}]}]}),
                vec![r#"{"message":"supported"}"#, r#"{"message":"retired"}"#],
            ),
        ];
        for (metadata, contents) in cases {
            let root = phase_root("empty");
            let before = whole_tree_snapshot(root.path());
            let (status, response) = request(
                root.path(),
                &format!("/app/import/journal/{PREFIX}/ingest/facets"),
                facet_body(metadata, &contents),
                "multipart/form-data; boundary=facet-boundary",
                true,
            )
            .await;

            assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
            assert_eq!(response["reason_code"], "invalid_request_value");
            assert!(
                response["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("nothing was imported"))
            );
            assert_eq!(whole_tree_snapshot(root.path()), before);
        }
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
    async fn traversal_entity_id_is_refused_without_writing_outside_entities() {
        let root = phase_root("empty");
        let outside = root.path().join("config/entity.json");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, b"owner config").unwrap();

        let response = post_entities(root.path(), json!([{"id":"../config","name":"Peer"}])).await;

        assert_eq!(response["created"], 0);
        assert_eq!(response["errors"][0]["error"], "Invalid entity id");
        assert_eq!(fs::read(outside).unwrap(), b"owner config");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_inbound_entity_directory_is_refused_without_writing_outside_the_journal() {
        use std::os::unix::fs::symlink;

        let root = phase_root("empty");
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("entity.json"), b"{\"name\":\"Peer\"}").unwrap();
        fs::create_dir_all(root.path().join("entities")).unwrap();
        symlink(outside.path(), root.path().join("entities/peer")).unwrap();

        let response = post_entities(root.path(), json!([{"id":"peer","name":"Peer"}])).await;

        assert_eq!(response["created"], 0);
        assert_eq!(response["errors"].as_array().unwrap().len(), 1);
        assert_eq!(
            fs::read(outside.path().join("entity.json")).unwrap(),
            b"{\"name\":\"Peer\"}"
        );
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
