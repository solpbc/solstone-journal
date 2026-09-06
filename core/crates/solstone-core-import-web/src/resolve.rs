// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    fs,
    path::{Component, Path},
};

use axum::{
    extract::{Json, Path as AxumPath, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Map, Value, json};
use solstone_core_entity_matching::entity_slug;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};
use solstone_core_journal_io::{AtomicWriteOptions, append_jsonl, atomic_replace, contained_path};

use crate::{
    AppState,
    http::{error, json as json_response},
};

fn source_state(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    if !safe_component(name) {
        return None;
    }
    let source: Value = serde_json::from_slice(
        &fs::read(contained(root, &format!("apps/import/journal_sources/{name}.json")).ok()?)
            .ok()?,
    )
    .ok()?;
    let key = source.get("key")?.as_str()?.get(..8)?;
    contained(root, &format!("imports/{}", &key[..8])).ok()
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '\\'])
        && matches!(
            Path::new(value).components().collect::<Vec<_>>().as_slice(),
            [Component::Normal(_)]
        )
}

fn safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !Path::new(value).is_absolute()
        && !value.contains('\\')
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn contained(root: &Path, relative: &str) -> Result<std::path::PathBuf, String> {
    contained_path(root, relative).map_err(|error| error.to_string())
}
fn read(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({}))
}
fn write(path: &Path, value: &Value) -> Result<(), String> {
    fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    atomic_replace(
        path,
        serde_json::to_vec_pretty(value)
            .map_err(|error| error.to_string())?
            .as_slice(),
        AtomicWriteOptions::default(),
    )
    .map_err(|error| error.to_string())
}
fn append_resolution_log(state: &Path, area: &str, value: Value) -> Result<(), String> {
    let path = contained(state, &format!("{area}/log.jsonl"))?;
    append_jsonl(path, &value).map_err(|error| error.to_string())
}

fn remove_derived(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) async fn entity(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    let Some(state) = source_state(&app.root, &name) else {
        return error(
            StatusCode::NOT_FOUND,
            "that journal source couldn't be used.",
            "journal_source_problem",
            format!("Journal source '{name}' not found"),
        );
    };
    let Some(source_id) = data
        .get("source_id")
        .and_then(Value::as_str)
        .filter(|id| safe_component(id))
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            "source_id is required".into(),
        );
    };
    let staged = match contained(&state, &format!("entities/staged/{source_id}.json")) {
        Ok(path) => path,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Invalid staged entity path".into(),
            );
        }
    };
    let payload = read(&staged);
    if !staged.exists() {
        return error(
            StatusCode::NOT_FOUND,
            "that import isn't in your journal.",
            "import_not_found",
            format!("Staged entity '{source_id}' not found."),
        );
    }
    let action = data.get("action").and_then(Value::as_str).unwrap_or("");
    if !matches!(action, "merge" | "create" | "skip") {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            "Action must be 'merge', 'create', or 'skip'.".into(),
        );
    }
    let source = payload
        .get("source_entity")
        .and_then(Value::as_object)
        .cloned()
        .ok_or("source entity");
    let source = match source {
        Ok(source) => source,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Staged entity is missing source_entity.".into(),
            );
        }
    };
    let reason = payload.get("reason").and_then(Value::as_str).unwrap_or("");
    let target_id = match action {
        "merge" => {
            let Some(target_id) = data
                .get("target")
                .and_then(Value::as_str)
                .filter(|id| safe_component(id))
            else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "one of those values couldn't be used.",
                    "invalid_request_value",
                    "--target is required for merge.".into(),
                );
            };
            let target_path =
                match contained(&app.root, &format!("entities/{target_id}/entity.json")) {
                    Ok(path) => path,
                    Err(_) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            "one of those values couldn't be used.",
                            "invalid_request_value",
                            "Invalid target entity path".into(),
                        );
                    }
                };
            if !target_path.exists() {
                return error(
                    StatusCode::NOT_FOUND,
                    "that import isn't in your journal.",
                    "import_not_found",
                    format!("Target entity '{target_id}' not found."),
                );
            }
            let target = read(&target_path).as_object().cloned().unwrap_or_default();
            let merged = merge_entity_fields(&target, &source);
            if let Err(detail) = write(&target_path, &Value::Object(merged)) {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "that import couldn't be saved.",
                    "import_metadata_failed",
                    detail,
                );
            }
            target_id.to_owned()
        }
        "create" => {
            let requested_id = match source.get("id") {
                Some(Value::String(id)) if safe_component(id) => id.to_owned(),
                Some(Value::String(_)) | Some(_) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "one of those values couldn't be used.",
                        "invalid_request_value",
                        "Staged entity has an invalid id.".into(),
                    );
                }
                None => source_id.to_owned(),
            };
            let id = if reason == "id_collision"
                || contained(&app.root, &format!("entities/{requested_id}"))
                    .is_ok_and(|path| path.exists())
            {
                let name = source.get("name").and_then(Value::as_str).unwrap_or("");
                let base = entity_slug(name);
                let Some(id) = allocate_entity_id(&app.root, &base) else {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "one of those values couldn't be used.",
                        "invalid_request_value",
                        format!("Unable to allocate a slug for '{name}'."),
                    );
                };
                id
            } else {
                requested_id
            };
            let mut created = source.clone();
            created.insert("id".into(), json!(id));
            if reason == "principal_conflict" {
                created.insert("is_principal".into(), json!(false));
            }
            let target = match contained(&app.root, &format!("entities/{id}/entity.json")) {
                Ok(path) => path,
                Err(_) => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "one of those values couldn't be used.",
                        "invalid_request_value",
                        "Invalid target entity path".into(),
                    );
                }
            };
            if let Err(detail) = write(&target, &Value::Object(created)) {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "that import couldn't be saved.",
                    "import_metadata_failed",
                    detail,
                );
            }
            id
        }
        _ => String::new(),
    };
    if action != "skip" {
        let state_path = match contained(&state, "entities/state.json") {
            Ok(path) => path,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "one of those values couldn't be used.",
                    "invalid_request_value",
                    "Invalid entity state path".into(),
                );
            }
        };
        let mut entity_state = read(&state_path);
        if !entity_state.is_object() {
            entity_state = json!({});
        }
        entity_state
            .as_object_mut()
            .unwrap()
            .entry("id_map")
            .or_insert_with(|| json!({}))[source_id] = json!(target_id);
        if let Err(detail) = write(&state_path, &entity_state) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "that import couldn't be saved.",
                "import_metadata_failed",
                detail,
            );
        }
    }
    if let Err(detail) = remove_derived(&staged) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "that import couldn't be saved.",
            "import_metadata_failed",
            detail,
        );
    }
    if let Err(detail) = append_resolution_log(
        &state,
        "entities",
        json!({"action":format!("resolved_{action}"),"item_type":"entity","item_id":source_id,"reason":reason,"source":source,"target_id":if target_id.is_empty(){Value::Null}else{json!(target_id)}}),
    ) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "that import couldn't be saved.",
            "import_metadata_failed",
            detail,
        );
    }
    json_response(
        StatusCode::OK,
        json!({"target_id":if target_id.is_empty(){Value::Null}else{json!(target_id)}}),
    )
}

fn allocate_entity_id(root: &Path, base: &str) -> Option<String> {
    if base.is_empty() {
        return None;
    }
    (1..=101)
        .map(|attempt| {
            if attempt == 1 {
                base.to_owned()
            } else {
                format!("{base}_{attempt}")
            }
        })
        .find(|candidate| !root.join("entities").join(candidate).exists())
}

fn merge_entity_fields(
    target: &Map<String, Value>,
    source: &Map<String, Value>,
) -> Map<String, Value> {
    let mut merged = target.clone();
    for field in ["aka", "emails"] {
        let mut items = target
            .get(field)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for item in source
            .get(field)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !items.iter().any(|old| {
                old.as_str()
                    .zip(item.as_str())
                    .is_some_and(|(old, new)| old.eq_ignore_ascii_case(new))
            }) {
                items.push(item.clone());
            }
        }
        if !items.is_empty() {
            merged.insert(field.into(), Value::Array(items));
        }
    }
    if let Some(source_created) = source.get("created_at") {
        match target.get("created_at") {
            Some(target_created) if created_at_not_after(target_created, source_created) => {}
            _ => {
                merged.insert("created_at".into(), source_created.clone());
            }
        }
    }
    merged
}

fn created_at_not_after(target: &Value, source: &Value) -> bool {
    match (target.as_f64(), source.as_f64()) {
        (Some(target), Some(source)) => target <= source,
        _ => {
            serde_json::to_string(target).unwrap_or_default()
                <= serde_json::to_string(source).unwrap_or_default()
        }
    }
}

pub(crate) async fn facet(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    let Some(state) = source_state(&app.root, &name) else {
        return error(
            StatusCode::NOT_FOUND,
            "that journal source couldn't be used.",
            "journal_source_problem",
            format!("Journal source '{name}' not found"),
        );
    };
    let Some(file) = data
        .get("staged_file")
        .and_then(Value::as_str)
        .filter(|file| safe_relative(file))
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            "Invalid staged file".into(),
        );
    };
    let staged = match contained(&state, &format!("facets/staged/{file}")) {
        Ok(path) => path,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Invalid staged file".into(),
            );
        }
    };
    if !staged.exists() {
        return error(
            StatusCode::NOT_FOUND,
            "that import isn't in your journal.",
            "import_not_found",
            "Staged facet file not found.".into(),
        );
    }
    let payload = read(&staged);
    let parts = file.split('/').collect::<Vec<_>>();
    if parts.len() < 3 {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            format!("Staged facet file '{file}' has an invalid path."),
        );
    }
    let facet_name = parts[0];
    let file_type = parts[1];
    let reason = payload.get("reason").and_then(Value::as_str).unwrap_or("");
    let mode = data.get("mode").and_then(Value::as_str).unwrap_or("");
    if !matches!(mode, "apply" | "skip") {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            "Mode must be 'apply' or 'skip'.".into(),
        );
    }
    let item_id = if reason == "facet_json_conflict" {
        format!("{facet_name}/facet.json")
    } else {
        format!(
            "{facet_name}/{}",
            payload
                .get("source_path")
                .and_then(Value::as_str)
                .unwrap_or(file)
        )
    };
    let mut extra = json!({"facet":facet_name,"staged_path":staged});
    let action = if mode == "skip" {
        "resolved_skip"
    } else if reason == "unmapped_entity" {
        let entities_state = match contained(&state, "entities/state.json") {
            Ok(path) => read(&path),
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "one of those values couldn't be used.",
                    "invalid_request_value",
                    "Invalid entity state path".into(),
                );
            }
        };
        let Some(id_map) = entities_state.get("id_map").and_then(Value::as_object) else {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Entity has no mapping yet. Run entity review first.".into(),
            );
        };
        let source_entity_id = payload
            .get("source_entity_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(target_entity_id) = id_map.get(source_entity_id).and_then(Value::as_str) else {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                format!("Entity {source_entity_id} has no mapping yet. Run entity review first."),
            );
        };
        let source_path = payload
            .get("source_path")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !safe_relative(source_path) {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Staged facet source path is invalid".into(),
            );
        }
        let target_relative = source_path.replacen(source_entity_id, target_entity_id, 1);
        let target = match contained(&app.root, &format!("facets/{facet_name}/{target_relative}")) {
            Ok(path) => path,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "one of those values couldn't be used.",
                    "invalid_request_value",
                    "Invalid facet target path".into(),
                );
            }
        };
        let source_data = payload
            .get("source_data")
            .and_then(Value::as_str)
            .unwrap_or("");
        match file_type {
            "entity_relationship" => {
                let mut source: Map<String, Value> = match serde_json::from_str(source_data) {
                    Ok(value) => value,
                    Err(parse_error) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            "one of those values couldn't be used.",
                            "invalid_request_value",
                            parse_error.to_string(),
                        );
                    }
                };
                source.insert("entity_id".into(), json!(target_entity_id));
                if let Some(owner) = read(&target).as_object() {
                    source.extend(owner.clone());
                }
                if let Err(detail) = write(&target, &Value::Object(source)) {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "that import couldn't be saved.",
                        "import_metadata_failed",
                        detail,
                    );
                }
            }
            "entity_observations" | "detected_entities" | "activity_records" => {
                let source = parse_jsonl(source_data).map_err(|detail| {
                    error(
                        StatusCode::BAD_REQUEST,
                        "one of those values couldn't be used.",
                        "invalid_request_value",
                        detail,
                    )
                });
                let source = match source {
                    Ok(source) => source,
                    Err(response) => return response,
                };
                let mut owner = parse_jsonl(&fs::read_to_string(&target).unwrap_or_default())
                    .unwrap_or_default();
                for mut item in source {
                    remap_item_ids(&mut item, source_entity_id, target_entity_id);
                    let duplicate = if file_type == "entity_observations" {
                        owner.iter().any(|old| {
                            old.get("content") == item.get("content")
                                && old.get("observed_at") == item.get("observed_at")
                        })
                    } else {
                        owner.iter().any(|old| old.get("id") == item.get("id"))
                    };
                    if !duplicate {
                        owner.push(item);
                    }
                }
                if let Err(detail) = write_jsonl(&target, &owner) {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "that import couldn't be saved.",
                        "import_metadata_failed",
                        detail,
                    );
                }
            }
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "one of those values couldn't be used.",
                    "invalid_request_value",
                    format!("Unsupported staged facet file type '{file_type}'."),
                );
            }
        }
        extra["target_path"] = json!(target);
        "resolved_apply"
    } else if reason == "facet_json_conflict" {
        let target = match contained(&app.root, &format!("facets/{facet_name}/facet.json")) {
            Ok(path) => path,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "one of those values couldn't be used.",
                    "invalid_request_value",
                    "Invalid facet target path".into(),
                );
            }
        };
        if let Err(detail) = write(
            &target,
            payload.get("source_content").unwrap_or(&Value::Null),
        ) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "that import couldn't be saved.",
                "import_metadata_failed",
                detail,
            );
        }
        extra["target_path"] = json!(target);
        "resolved_apply"
    } else {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            format!("Unsupported staged facet reason '{reason}'."),
        );
    };
    if let Err(detail) = remove_derived(&staged) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "that import couldn't be saved.",
            "import_metadata_failed",
            detail,
        );
    }
    let mut entry =
        json!({"action":action,"item_type":file_type,"item_id":item_id,"reason":reason});
    entry
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    if let Err(detail) = append_resolution_log(&state, "facets", entry) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "that import couldn't be saved.",
            "import_metadata_failed",
            detail,
        );
    }
    json_response(StatusCode::OK, json!({}))
}

fn parse_jsonl(text: &str) -> Result<Vec<Value>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect()
}

fn write_jsonl(path: &Path, values: &[Value]) -> Result<(), String> {
    let content = values
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .join("\n");
    write_bytes(path, content.as_bytes())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::create_dir_all(path.parent().expect("write parent")).map_err(|error| error.to_string())?;
    atomic_replace(path, bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
}

fn remap_item_ids(item: &mut Value, source_id: &str, target_id: &str) {
    if item.get("id").and_then(Value::as_str) == Some(source_id) {
        item["id"] = json!(target_id);
    }
    if let Some(items) = item
        .get_mut("active_entities")
        .and_then(Value::as_array_mut)
    {
        for entity_id in items {
            if entity_id.as_str() == Some(source_id) {
                *entity_id = json!(target_id);
            }
        }
    }
}

pub(crate) async fn config(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    apply_config_field(&app.root, source_state(&app.root, &name), data).await
}
pub(crate) async fn config_all(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    let Some(state) = source_state(&app.root, &name) else {
        return error(
            StatusCode::NOT_FOUND,
            "that journal source couldn't be used.",
            "journal_source_problem",
            format!("Journal source '{name}' not found"),
        );
    };
    let category = data.get("category").and_then(Value::as_str).unwrap_or("");
    if !matches!(category, "transferable" | "preference") {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            "Category must be 'transferable' or 'preference'.".into(),
        );
    }
    let diff_path = match contained(&state, "config/diff.json") {
        Ok(path) => path,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Invalid config diff path".into(),
            );
        }
    };
    let diff = read(&diff_path);
    let fields = diff
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, entry)| entry.get("category").and_then(Value::as_str) == Some(category))
        .map(|(field, _)| field.clone())
        .collect::<Vec<_>>();
    let mut count = 0;
    for field in &fields {
        let response = apply_config_field(
            &app.root,
            Some(state.clone()),
            json!({"field":field,"action":"apply"}),
        )
        .await;
        if response.status() != StatusCode::OK {
            return response;
        }
        count += 1;
    }
    json_response(StatusCode::OK, json!({"count":count}))
}
async fn apply_config_field(
    root: &Path,
    state: Option<std::path::PathBuf>,
    data: Value,
) -> Response {
    let Some(state) = state else {
        return error(
            StatusCode::NOT_FOUND,
            "that journal source couldn't be used.",
            "journal_source_problem",
            "Journal source not found".into(),
        );
    };
    let field = data.get("field").and_then(Value::as_str).unwrap_or("");
    let action = data.get("action").and_then(Value::as_str).unwrap_or("");
    let path = match contained(&state, "config/diff.json") {
        Ok(path) => path,
        Err(_) => {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Invalid config diff path".into(),
            );
        }
    };
    if !path.exists() {
        return error(
            StatusCode::NOT_FOUND,
            "that import isn't in your journal.",
            "import_not_found",
            "No staged config diff found.".into(),
        );
    }
    let mut diff = read(&path);
    let Some(entry) = diff.get(field).cloned() else {
        return error(
            StatusCode::NOT_FOUND,
            "that import isn't in your journal.",
            "import_not_found",
            format!("Config field '{field}' is not staged."),
        );
    };
    if !valid_config_field(field) {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            format!("Config field '{field}' is invalid."),
        );
    }
    if !matches!(action, "apply" | "keep") {
        return error(
            StatusCode::BAD_REQUEST,
            "one of those values couldn't be used.",
            "invalid_request_value",
            "Action must be 'apply' or 'keep'.".into(),
        );
    }
    if action == "apply" {
        let value = entry.get("source").cloned().unwrap_or(Value::Null);
        if contained(root, "config/journal.json").is_err() {
            return error(
                StatusCode::BAD_REQUEST,
                "one of those values couldn't be used.",
                "invalid_request_value",
                "Invalid journal config path".into(),
            );
        }
        let result = mutate_journal_config(root, LockOptions::default(), |config| {
            set(config, field, value);
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        });
        if let Err(config_error) = result {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "that import couldn't be saved.",
                "import_metadata_failed",
                config_error.to_string(),
            );
        }
    }
    diff.as_object_mut().unwrap().remove(field);
    if diff.as_object().unwrap().is_empty() {
        if let Err(detail) = remove_derived(&path) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "that import couldn't be saved.",
                "import_metadata_failed",
                detail,
            );
        }
        let source_path = match contained(&state, "config/source_config.json") {
            Ok(path) => path,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "one of those values couldn't be used.",
                    "invalid_request_value",
                    "Invalid staged config path".into(),
                );
            }
        };
        if let Err(detail) = remove_derived(&source_path) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "that import couldn't be saved.",
                "import_metadata_failed",
                detail,
            );
        }
    } else {
        if let Err(detail) = write(&path, &diff) {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "that import couldn't be saved.",
                "import_metadata_failed",
                detail,
            );
        }
    }
    if let Err(detail) = append_resolution_log(
        &state,
        "config",
        json!({
            "action":if action == "apply" {"config_field_applied"} else {"config_field_kept"},
            "item_type":"config",
            "item_id":field,
            "reason":if action == "apply" {"review_apply"} else {"review_keep"},
            "category":entry.get("category").cloned().unwrap_or(Value::Null),
            "source":entry.get("source").cloned().unwrap_or(Value::Null),
            "target_previous":entry.get("target").cloned().unwrap_or(Value::Null),
        }),
    ) {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "that import couldn't be saved.",
            "import_metadata_failed",
            detail,
        );
    }
    json_response(StatusCode::OK, json!({}))
}

fn valid_config_field(field: &str) -> bool {
    !field.is_empty() && field.split('.').all(|part| !part.is_empty())
}
fn set(config: &mut Map<String, Value>, field: &str, value: Value) {
    let mut parts = field.split('.').peekable();
    let mut current = config;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.into(), value);
            return;
        }
        let child = current.entry(part.to_owned()).or_insert_with(|| json!({}));
        if !child.is_object() {
            *child = json!({});
        }
        current = child
            .as_object_mut()
            .expect("an object was installed immediately above");
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::{
        body::to_bytes,
        extract::{Json, Path as AxumPath, State},
        http::StatusCode,
    };
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use solstone_core_journal_config_write::{
        JournalConfigMutation, LockOptions, mutate_journal_config,
    };

    use super::{config, config_all, entity, facet};
    use crate::AppState;

    fn write_json(path: &std::path::Path, value: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn state(root: &TempDir) -> std::path::PathBuf {
        write_json(
            &root.path().join("apps/import/journal_sources/peer.json"),
            json!({"key":"prefix01-key-material"}),
        );
        root.path().join("imports/prefix01")
    }

    async fn apply_entity_resolution(root: &TempDir, data: Value) -> Value {
        let response = entity(
            State(AppState {
                root: root.path().to_owned(),
            }),
            AxumPath("peer".to_owned()),
            Json(data),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn apply_facet_resolution(root: &TempDir, data: Value) {
        let response = facet(
            State(AppState {
                root: root.path().to_owned(),
            }),
            AxumPath("peer".to_owned()),
            Json(data),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn apply_config_resolution(root: &TempDir, data: Value) {
        let response = config(
            State(AppState {
                root: root.path().to_owned(),
            }),
            AxumPath("peer".to_owned()),
            Json(data),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    async fn apply_all_config_resolutions(root: &TempDir, data: Value) -> Value {
        let response = config_all(
            State(AppState {
                root: root.path().to_owned(),
            }),
            AxumPath("peer".to_owned()),
            Json(data),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    async fn apply_all_config_resolutions_status(root: &TempDir, data: Value) -> StatusCode {
        config_all(
            State(AppState {
                root: root.path().to_owned(),
            }),
            AxumPath("peer".to_owned()),
            Json(data),
        )
        .await
        .status()
    }

    #[tokio::test]
    async fn apply_entity_merge_preserves_owner_fields_and_cleans_up_proposal() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("entities/owner/entity.json"),
            json!({"id":"owner","name":"Owner","owner_only":true,"shared":"owner","aka":["Old"],"emails":["OWNER@example.test"],"created_at":10}),
        );
        let staged = state.join("entities/staged/incoming.json");
        write_json(
            &staged,
            json!({"reason":"ambiguous","source_entity":{"id":"incoming","name":"Incoming","source_only":true,"shared":"source","aka":["old","New"],"emails":["owner@example.test","new@example.test"],"created_at":5}}),
        );

        let body = apply_entity_resolution(
            &root,
            json!({"source_id":"incoming","action":"merge","target":"owner"}),
        )
        .await;
        assert_eq!(body, json!({"target_id":"owner"}));
        assert!(!staged.exists(), "merge consumes its derived proposal");
        let merged: Value = serde_json::from_slice(
            &fs::read(root.path().join("entities/owner/entity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(merged["owner_only"], true);
        assert!(merged.get("source_only").is_none());
        assert_eq!(merged["shared"], "owner");
        assert_eq!(merged["created_at"], 5);
        assert_eq!(merged["aka"], json!(["Old", "New"]));
        assert_eq!(
            merged["emails"],
            json!(["OWNER@example.test", "new@example.test"])
        );
        let log = fs::read_to_string(state.join("entities/log.jsonl")).unwrap();
        assert!(log.contains("resolved_merge"));
    }

    #[tokio::test]
    async fn apply_entity_create_consumes_proposal_and_logs_resolution() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        let staged = state.join("entities/staged/incoming.json");
        write_json(
            &staged,
            json!({"reason":"principal_conflict","source_entity":{"id":"created","name":"Created","is_principal":true}}),
        );

        apply_entity_resolution(&root, json!({"source_id":"incoming","action":"create"})).await;
        assert!(!staged.exists(), "create consumes its derived proposal");
        let created: Value = serde_json::from_slice(
            &fs::read(root.path().join("entities/created/entity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(created["is_principal"], false);
        assert!(
            fs::read_to_string(state.join("entities/log.jsonl"))
                .unwrap()
                .contains("resolved_create")
        );
    }

    #[tokio::test]
    async fn apply_entity_create_allocates_slug_for_a_colliding_owner_id() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("entities/taken/entity.json"),
            json!({"id":"taken","name":"Owner","owner_only":true}),
        );
        write_json(
            &state.join("entities/staged/incoming.json"),
            json!({"reason":"id_collision","source_entity":{"id":"taken","name":"Alice Johnson"}}),
        );

        assert_eq!(
            apply_entity_resolution(&root, json!({"source_id":"incoming","action":"create"})).await,
            json!({"target_id":"alice_johnson"})
        );
        let owner: Value = serde_json::from_slice(
            &fs::read(root.path().join("entities/taken/entity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            owner,
            json!({"id":"taken","name":"Owner","owner_only":true})
        );
        let created: Value = serde_json::from_slice(
            &fs::read(root.path().join("entities/alice_johnson/entity.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(created["id"], "alice_johnson");
    }

    #[tokio::test]
    async fn apply_entity_skip_consumes_proposal_and_logs_resolution() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        let staged = state.join("entities/staged/incoming.json");
        write_json(
            &staged,
            json!({"reason":"ambiguous","source_entity":{"id":"incoming","name":"Incoming"}}),
        );

        assert_eq!(
            apply_entity_resolution(&root, json!({"source_id":"incoming","action":"skip"})).await,
            json!({"target_id":null})
        );
        assert!(!staged.exists(), "skip consumes its derived proposal");
        assert!(
            fs::read_to_string(state.join("entities/log.jsonl"))
                .unwrap()
                .contains("resolved_skip")
        );
    }

    #[tokio::test]
    async fn apply_facet_skip_consumes_its_derived_proposal_and_logs() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        let staged = state.join("facets/staged/work/facet_json/facet.json.staged.json");
        write_json(
            &staged,
            json!({"reason":"facet_json_conflict","source_content":{"name":"source"},"target_content":{"name":"owner"}}),
        );

        apply_facet_resolution(
            &root,
            json!({"staged_file":"work/facet_json/facet.json.staged.json","mode":"skip"}),
        )
        .await;
        assert!(!staged.exists());
        let log = fs::read_to_string(state.join("facets/log.jsonl")).unwrap();
        assert!(log.contains("resolved_skip"));
        assert!(log.contains("facet_json_conflict"));
    }

    #[tokio::test]
    async fn apply_facet_unmapped_relationship_remaps_and_preserves_target_values() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &state.join("entities/state.json"),
            json!({"id_map":{"source-person":"owner-person"}}),
        );
        let staged = state.join("facets/staged/work/entity_relationship/entity.json.staged.json");
        write_json(
            &staged,
            json!({"reason":"unmapped_entity","source_entity_id":"source-person","source_path":"entities/source-person/entity.json","source_data":"{\"shared\":\"source\",\"source_only\":true}"}),
        );
        write_json(
            &root
                .path()
                .join("facets/work/entities/owner-person/entity.json"),
            json!({"shared":"owner","owner_only":true}),
        );

        apply_facet_resolution(
            &root,
            json!({"staged_file":"work/entity_relationship/entity.json.staged.json","mode":"apply"}),
        )
        .await;
        assert!(!staged.exists());
        let relationship: Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("facets/work/entities/owner-person/entity.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(relationship["entity_id"], "owner-person");
        assert_eq!(
            relationship["shared"], "owner",
            "{{**source, **target}} keeps owner value"
        );
        assert_eq!(relationship["source_only"], true);
        assert_eq!(relationship["owner_only"], true);
        assert!(
            fs::read_to_string(state.join("facets/log.jsonl"))
                .unwrap()
                .contains("resolved_apply")
        );
    }

    #[tokio::test]
    async fn apply_facet_conflict_apply_consumes_proposal_and_records_action() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        let staged = state.join("facets/staged/work/facet_json/facet.json.staged.json");
        write_json(
            &staged,
            json!({"reason":"facet_json_conflict","source_content":{"name":"source"},"target_content":{"name":"owner"}}),
        );
        write_json(
            &root.path().join("facets/work/facet.json"),
            json!({"name":"owner"}),
        );

        apply_facet_resolution(
            &root,
            json!({"staged_file":"work/facet_json/facet.json.staged.json","mode":"apply"}),
        )
        .await;
        assert!(!staged.exists());
        let facet: Value =
            serde_json::from_slice(&fs::read(root.path().join("facets/work/facet.json")).unwrap())
                .unwrap();
        assert_eq!(facet, json!({"name":"source"}));
        assert!(
            fs::read_to_string(state.join("facets/log.jsonl"))
                .unwrap()
                .contains("resolved_apply")
        );
    }

    #[tokio::test]
    async fn apply_config_rewrites_nonempty_diff_and_appends_resolution_log() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("config/journal.json"),
            json!({"appearance":{"theme":"owner"},"concurrent":{"keep":true}}),
        );
        write_json(
            &state.join("config/diff.json"),
            json!({
                "appearance.theme":{"source":"source","target":"owner","category":"preference"},
                "other.value":{"source":2,"target":1,"category":"transferable"}
            }),
        );
        write_json(
            &state.join("config/source_config.json"),
            json!({"appearance":{"theme":"source"}}),
        );

        apply_config_resolution(&root, json!({"field":"appearance.theme","action":"apply"})).await;
        let config: Value =
            serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(config["appearance"]["theme"], "source");
        assert_eq!(
            config["concurrent"]["keep"], true,
            "CAS mutation preserves concurrent material"
        );
        let diff: Value =
            serde_json::from_slice(&fs::read(state.join("config/diff.json")).unwrap()).unwrap();
        assert_eq!(diff.as_object().unwrap().len(), 1);
        assert!(diff.get("other.value").is_some());
        assert!(state.join("config/source_config.json").exists());
        let log = fs::read_to_string(state.join("config/log.jsonl")).unwrap();
        assert!(log.contains("config_field_applied"));
        assert!(log.contains("review_apply"));
    }

    #[tokio::test]
    async fn apply_config_replaces_a_scalar_intermediate_with_the_reviewed_object_path() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("config/journal.json"),
            json!({"appearance":"dark"}),
        );
        write_json(
            &state.join("config/diff.json"),
            json!({"appearance.theme":{"source":"light","target":"dark","category":"preference"}}),
        );

        apply_config_resolution(&root, json!({"field":"appearance.theme","action":"apply"})).await;

        let config: Value =
            serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(config, json!({"appearance":{"theme":"light"}}));
    }

    #[tokio::test]
    async fn resolution_log_write_failure_is_surfaced_instead_of_a_success_response() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("config/journal.json"),
            json!({"theme":"dark"}),
        );
        write_json(
            &state.join("config/diff.json"),
            json!({"theme":{"source":"light","target":"dark","category":"preference"}}),
        );
        fs::create_dir_all(state.join("config/log.jsonl")).unwrap();

        let response = config(
            State(AppState {
                root: root.path().to_owned(),
            }),
            AxumPath("peer".to_owned()),
            Json(json!({"field":"theme","action":"apply"})),
        )
        .await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn criterion_9_apply_config_and_sibling_mutation_both_survive() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("config/journal.json"),
            json!({"appearance":{"theme":"owner"},"sibling":{"version":1}}),
        );
        write_json(
            &state.join("config/diff.json"),
            json!({"appearance.theme":{"source":"source","target":"owner","category":"preference"}}),
        );
        mutate_journal_config(root.path(), LockOptions::default(), |config| {
            config.insert("sibling".into(), json!({"version":2}));
            JournalConfigMutation {
                changed: true,
                value: (),
            }
        })
        .unwrap();

        apply_config_resolution(&root, json!({"field":"appearance.theme","action":"apply"})).await;
        let config: Value =
            serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(config["appearance"]["theme"], "source");
        assert_eq!(config["sibling"], json!({"version":2}));
    }

    #[tokio::test]
    async fn apply_config_empty_diff_unlinks_both_config_proposals_and_logs() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("config/journal.json"),
            json!({"appearance":{"theme":"owner"}}),
        );
        let diff = state.join("config/diff.json");
        let source = state.join("config/source_config.json");
        write_json(
            &diff,
            json!({"appearance.theme":{"source":"source","target":"owner","category":"preference"}}),
        );
        write_json(&source, json!({"appearance":{"theme":"source"}}));

        apply_config_resolution(&root, json!({"field":"appearance.theme","action":"keep"})).await;
        assert!(
            !diff.exists(),
            "last resolved field removes config/diff.json"
        );
        assert!(
            !source.exists(),
            "last resolved field removes config/source_config.json"
        );
        let config: Value =
            serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(config["appearance"]["theme"], "owner");
        assert!(
            fs::read_to_string(state.join("config/log.jsonl"))
                .unwrap()
                .contains("config_field_kept")
        );
    }

    #[tokio::test]
    async fn apply_all_config_resolutions_applies_each_matching_field_and_consumes_final_diff() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(
            &root.path().join("config/journal.json"),
            json!({"a":0,"b":0}),
        );
        let diff = state.join("config/diff.json");
        let source = state.join("config/source_config.json");
        write_json(
            &diff,
            json!({
                "a":{"source":1,"target":0,"category":"preference"},
                "b":{"source":2,"target":0,"category":"preference"},
                "unrelated":{"source":3,"target":0,"category":"transferable"}
            }),
        );
        write_json(&source, json!({"a":1,"b":2,"unrelated":3}));

        assert_eq!(
            apply_all_config_resolutions(&root, json!({"category":"preference"})).await,
            json!({"count":2})
        );
        let config: Value =
            serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(config["a"], 1);
        assert_eq!(config["b"], 2);
        let remainder: Value = serde_json::from_slice(&fs::read(&diff).unwrap()).unwrap();
        assert_eq!(
            remainder,
            json!({"unrelated":{"source":3,"target":0,"category":"transferable"}})
        );
        assert!(source.exists());
        let log = fs::read_to_string(state.join("config/log.jsonl")).unwrap();
        assert_eq!(
            log.lines().count(),
            2,
            "one resolution entry is emitted per field transaction"
        );
    }

    #[tokio::test]
    async fn criterion_16_config_all_keeps_the_remainder_after_its_second_field_is_invalid() {
        let root = TempDir::new().unwrap();
        let state = state(&root);
        write_json(&root.path().join("config/journal.json"), json!({"a":0}));
        let diff = state.join("config/diff.json");
        write_json(
            &diff,
            json!({
                "a":{"source":1,"target":0,"category":"preference"},
                "broken..field":{"source":2,"target":0,"category":"preference"}
            }),
        );

        assert_eq!(
            apply_all_config_resolutions_status(&root, json!({"category":"preference"})).await,
            StatusCode::BAD_REQUEST
        );
        let config: Value =
            serde_json::from_slice(&fs::read(root.path().join("config/journal.json")).unwrap())
                .unwrap();
        assert_eq!(config["a"], 1, "the first CAS transaction stays applied");
        let remainder: Value = serde_json::from_slice(&fs::read(diff).unwrap()).unwrap();
        assert_eq!(
            remainder,
            json!({"broken..field":{"source":2,"target":0,"category":"preference"}})
        );
        assert_eq!(
            fs::read_to_string(state.join("config/log.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1,
            "the invalid second field has no resolution entry"
        );
    }

    /// Criterion 15's whole-tree oracle.  It snapshots both roots so that a
    /// handler cannot hide an owner-domain write outside its paired state tree.
    mod differential {
        use std::{collections::BTreeMap, fs, path::Path};

        use axum::extract::{Json, Path as AxumPath, State};
        use serde_json::{Value, json};
        use sha2::{Digest, Sha256};
        use tempfile::TempDir;

        use super::{AppState, config, config_all, entity, facet, state, write_json};

        // Declared normalizations: volatile `ts` JSON fields, vh_<uuid4hex>
        // path components, and atomic `.tmp_*` path components.  They are
        // intentionally visible here rather than hidden in assertion output.
        const NORMALIZED_TS: &str = "<ts>";
        const NORMALIZED_VH: &str = "vh_<uuid4hex>";
        const NORMALIZED_TMP: &str = ".tmp_<normalized>";

        #[derive(Default)]
        struct Snapshot(BTreeMap<String, String>);

        impl Snapshot {
            fn capture(journal_root: &Path, state_dir: &Path) -> Self {
                let mut nodes = BTreeMap::new();
                visit("journal", journal_root, journal_root, &mut nodes);
                visit("state", state_dir, state_dir, &mut nodes);
                Self(nodes)
            }

            fn delta(&self, after: &Self) -> Vec<String> {
                let mut delta = Vec::new();
                for (path, before) in &self.0 {
                    match after.0.get(path) {
                        None => delta.push(format!("- {path}")),
                        Some(current) if current != before => delta.push(format!("~ {path}")),
                        _ => {}
                    }
                }
                for path in after.0.keys().filter(|path| !self.0.contains_key(*path)) {
                    delta.push(format!("+ {path}"));
                }
                delta.sort();
                delta
            }
        }

        fn visit(prefix: &str, root: &Path, path: &Path, nodes: &mut BTreeMap<String, String>) {
            let relative = path.strip_prefix(root).expect("descendant");
            let metadata = fs::symlink_metadata(path).expect("snapshot metadata");
            if !relative.as_os_str().is_empty() {
                let key = format!("{prefix}/{}", normalize_path(relative));
                if metadata.file_type().is_symlink() {
                    nodes.insert(
                        key,
                        format!(
                            "symlink:{}",
                            fs::read_link(path).expect("symlink target").display()
                        ),
                    );
                    return;
                }
                if metadata.is_dir() {
                    nodes.insert(key, "dir".into());
                } else {
                    let bytes = fs::read(path).expect("snapshot file");
                    nodes.insert(
                        key,
                        format!("{:x}", Sha256::digest(normalize_bytes(&bytes))),
                    );
                }
            }
            if metadata.is_dir() {
                let mut children = fs::read_dir(path)
                    .expect("snapshot directory")
                    .map(|entry| entry.expect("directory entry").path())
                    .collect::<Vec<_>>();
                children.sort();
                for child in children {
                    visit(prefix, root, &child, nodes);
                }
            }
        }

        fn normalize_path(path: &Path) -> String {
            path.components()
                .map(|part| {
                    let part = part.as_os_str().to_string_lossy();
                    if part.starts_with(".tmp_") {
                        NORMALIZED_TMP.to_owned()
                    } else if part.starts_with("vh_")
                        && part.len() == 35
                        && part[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
                    {
                        NORMALIZED_VH.to_owned()
                    } else {
                        part.into_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("/")
        }

        fn normalize_bytes(bytes: &[u8]) -> Vec<u8> {
            let Ok(text) = std::str::from_utf8(bytes) else {
                return bytes.to_vec();
            };
            text.lines()
                .map(|line| {
                    serde_json::from_str::<Value>(line)
                        .map(|mut value| {
                            normalize_value(&mut value);
                            serde_json::to_string(&value).expect("normalized JSON")
                        })
                        .unwrap_or_else(|_| line.to_owned())
                })
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes()
        }

        fn normalize_value(value: &mut Value) {
            match value {
                Value::Object(object) => {
                    for (key, child) in object {
                        if key == "ts" {
                            *child = json!(NORMALIZED_TS);
                        } else {
                            normalize_value(child);
                        }
                    }
                }
                Value::Array(items) => items.iter_mut().for_each(normalize_value),
                _ => {}
            }
        }

        fn assert_delta(before: Snapshot, after: Snapshot, expected: &[&str]) {
            let mut expected = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
            expected.sort();
            assert_eq!(before.delta(&after), expected);
        }

        async fn call_entity(root: &TempDir, body: Value) {
            let response = entity(
                State(AppState {
                    root: root.path().to_owned(),
                }),
                AxumPath("peer".to_owned()),
                Json(body),
            )
            .await;
            assert!(response.status().is_success());
        }

        async fn call_facet(root: &TempDir, body: Value) {
            let response = facet(
                State(AppState {
                    root: root.path().to_owned(),
                }),
                AxumPath("peer".to_owned()),
                Json(body),
            )
            .await;
            assert!(response.status().is_success());
        }

        async fn call_config(root: &TempDir, body: Value) {
            let response = config(
                State(AppState {
                    root: root.path().to_owned(),
                }),
                AxumPath("peer".to_owned()),
                Json(body),
            )
            .await;
            assert!(response.status().is_success());
        }

        async fn call_config_all(root: &TempDir, body: Value) {
            let response = config_all(
                State(AppState {
                    root: root.path().to_owned(),
                }),
                AxumPath("peer".to_owned()),
                Json(body),
            )
            .await;
            assert!(response.status().is_success());
        }

        async fn entity_status(root: &TempDir, body: Value) -> axum::http::StatusCode {
            entity(
                State(AppState {
                    root: root.path().to_owned(),
                }),
                AxumPath("peer".to_owned()),
                Json(body),
            )
            .await
            .status()
        }

        async fn facet_status(root: &TempDir, body: Value) -> axum::http::StatusCode {
            facet(
                State(AppState {
                    root: root.path().to_owned(),
                }),
                AxumPath("peer".to_owned()),
                Json(body),
            )
            .await
            .status()
        }

        async fn config_status(root: &TempDir, body: Value) -> axum::http::StatusCode {
            config(
                State(AppState {
                    root: root.path().to_owned(),
                }),
                AxumPath("peer".to_owned()),
                Json(body),
            )
            .await
            .status()
        }

        #[cfg(unix)]
        #[tokio::test]
        async fn symlinked_resolution_paths_are_refused_without_a_whole_tree_delta() {
            use std::os::unix::fs::symlink;

            let root = TempDir::new().unwrap();
            let outside = TempDir::new().unwrap();
            let state_dir = state(&root);

            fs::create_dir_all(state_dir.join("entities")).unwrap();
            symlink(outside.path(), state_dir.join("entities/staged")).unwrap();
            let before = Snapshot::capture(root.path(), &state_dir);
            assert_eq!(
                entity_status(&root, json!({"source_id":"incoming","action":"skip"})).await,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_delta(before, Snapshot::capture(root.path(), &state_dir), &[]);

            fs::remove_file(state_dir.join("entities/staged")).unwrap();
            write_json(
                &state_dir.join("entities/staged/incoming.json"),
                json!({"reason":"ambiguous","source_entity":{"id":"incoming","name":"Incoming"}}),
            );
            fs::create_dir_all(root.path().join("entities")).unwrap();
            symlink(outside.path(), root.path().join("entities/owner")).unwrap();
            let before = Snapshot::capture(root.path(), &state_dir);
            assert_eq!(
                entity_status(
                    &root,
                    json!({"source_id":"incoming","action":"merge","target":"owner"}),
                )
                .await,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_delta(before, Snapshot::capture(root.path(), &state_dir), &[]);

            fs::create_dir_all(state_dir.join("facets")).unwrap();
            symlink(outside.path(), state_dir.join("facets/staged")).unwrap();
            let before = Snapshot::capture(root.path(), &state_dir);
            assert_eq!(
                facet_status(
                    &root,
                    json!({"staged_file":"work/facet_json/facet.json.staged.json","mode":"skip"}),
                )
                .await,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_delta(before, Snapshot::capture(root.path(), &state_dir), &[]);

            fs::remove_file(state_dir.join("facets/staged")).unwrap();
            write_json(
                &state_dir.join("facets/staged/work/facet_json/facet.json.staged.json"),
                json!({"reason":"facet_json_conflict","source_content":{"name":"inbound"},"target_content":{}}),
            );
            fs::create_dir_all(root.path().join("facets")).unwrap();
            symlink(outside.path(), root.path().join("facets/work")).unwrap();
            let before = Snapshot::capture(root.path(), &state_dir);
            assert_eq!(
                facet_status(
                    &root,
                    json!({"staged_file":"work/facet_json/facet.json.staged.json","mode":"apply"}),
                )
                .await,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_delta(before, Snapshot::capture(root.path(), &state_dir), &[]);
            assert!(fs::read_dir(outside.path()).unwrap().next().is_none());

            write_json(
                &state_dir.join("config/diff.json"),
                json!({"appearance.theme":{"source":"dark","target":"light","category":"preference"}}),
            );
            fs::create_dir_all(root.path().join("config")).unwrap();
            symlink(
                outside.path().join("journal.json"),
                root.path().join("config/journal.json"),
            )
            .unwrap();
            let before = Snapshot::capture(root.path(), &state_dir);
            assert_eq!(
                config_status(&root, json!({"field":"appearance.theme","action":"apply"})).await,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_delta(before, Snapshot::capture(root.path(), &state_dir), &[]);
            assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
        }

        #[tokio::test]
        async fn traversal_resolution_inputs_are_refused_without_a_whole_tree_delta() {
            let root = TempDir::new().unwrap();
            let state_dir = state(&root);
            write_json(
                &state_dir.join("entities/not.json"),
                json!({"reason":"ambiguous","source_entity":{"id":"not","name":"Not"}}),
            );
            write_json(
                &state_dir.join("entities/staged/incoming.json"),
                json!({"reason":"ambiguous","source_entity":{"id":"incoming","name":"Incoming"}}),
            );
            write_json(
                &root.path().join("config/entity.json"),
                json!({"id":"owner","owner_only":true}),
            );
            let absolute = root.path().join("owner-proposal.staged.json");
            write_json(
                &absolute,
                json!({"reason":"facet_json_conflict","source_content":{},"target_content":{}}),
            );
            let before = Snapshot::capture(root.path(), &state_dir);

            assert_eq!(
                entity_status(&root, json!({"source_id":"../not","action":"skip"})).await,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_eq!(
                entity_status(
                    &root,
                    json!({"source_id":"incoming","action":"merge","target":"../config"}),
                )
                .await,
                axum::http::StatusCode::BAD_REQUEST
            );
            assert_eq!(
                facet_status(
                    &root,
                    json!({"staged_file":absolute.to_string_lossy(),"mode":"skip"}),
                )
                .await,
                axum::http::StatusCode::BAD_REQUEST
            );

            assert_delta(before, Snapshot::capture(root.path(), &state_dir), &[]);
        }

        #[tokio::test]
        async fn entity_resolution_has_only_its_declared_whole_tree_delta() {
            let root = TempDir::new().unwrap();
            let state_dir = state(&root);
            write_json(
                &root.path().join("entities/owner/entity.json"),
                json!({"id":"owner","name":"Owner"}),
            );
            write_json(
                &state_dir.join("entities/staged/incoming.json"),
                json!({"reason":"ambiguous","source_entity":{"id":"incoming","name":"Incoming"}}),
            );
            let before = Snapshot::capture(root.path(), &state_dir);
            call_entity(
                &root,
                json!({"source_id":"incoming","action":"merge","target":"owner"}),
            )
            .await;
            assert_delta(
                before,
                Snapshot::capture(root.path(), &state_dir),
                &[
                    "~ journal/entities/owner/entity.json",
                    "- journal/imports/prefix01/entities/staged/incoming.json",
                    "+ journal/imports/prefix01/entities/state.json",
                    "+ journal/imports/prefix01/entities/log.jsonl",
                    "- state/entities/staged/incoming.json",
                    "+ state/entities/state.json",
                    "+ state/entities/log.jsonl",
                ],
            );
        }

        #[tokio::test]
        async fn facet_resolution_has_only_its_declared_whole_tree_delta() {
            let root = TempDir::new().unwrap();
            let state_dir = state(&root);
            write_json(
                &state_dir.join("facets/staged/work/facet_json/facet.json.staged.json"),
                json!({"reason":"facet_json_conflict","source_content":{"name":"source"},"target_content":{"name":"owner"}}),
            );
            let before = Snapshot::capture(root.path(), &state_dir);
            call_facet(
                &root,
                json!({"staged_file":"work/facet_json/facet.json.staged.json","mode":"apply"}),
            )
            .await;
            assert_delta(
                before,
                Snapshot::capture(root.path(), &state_dir),
                &[
                    "+ journal/facets",
                    "+ journal/facets/work",
                    "+ journal/facets/work/facet.json",
                    "- journal/imports/prefix01/facets/staged/work/facet_json/facet.json.staged.json",
                    "+ journal/imports/prefix01/facets/log.jsonl",
                    "- state/facets/staged/work/facet_json/facet.json.staged.json",
                    "+ state/facets/log.jsonl",
                ],
            );
        }

        #[tokio::test]
        async fn config_resolution_has_only_its_declared_whole_tree_delta() {
            let root = TempDir::new().unwrap();
            let state_dir = state(&root);
            write_json(
                &root.path().join("config/journal.json"),
                json!({"appearance":{"theme":"owner"}}),
            );
            write_json(
                &state_dir.join("config/diff.json"),
                json!({"appearance.theme":{"source":"source","target":"owner","category":"preference"}}),
            );
            write_json(
                &state_dir.join("config/source_config.json"),
                json!({"appearance":{"theme":"source"}}),
            );
            let before = Snapshot::capture(root.path(), &state_dir);
            call_config(&root, json!({"field":"appearance.theme","action":"apply"})).await;
            assert_delta(
                before,
                Snapshot::capture(root.path(), &state_dir),
                &[
                    "~ journal/config/journal.json",
                    "+ journal/config/journal.json.lock",
                    "- journal/imports/prefix01/config/diff.json",
                    "- journal/imports/prefix01/config/source_config.json",
                    "+ journal/imports/prefix01/config/log.jsonl",
                    "- state/config/diff.json",
                    "- state/config/source_config.json",
                    "+ state/config/log.jsonl",
                ],
            );
        }

        #[tokio::test]
        async fn config_all_resolution_has_only_its_declared_whole_tree_delta() {
            let root = TempDir::new().unwrap();
            let state_dir = state(&root);
            write_json(
                &root.path().join("config/journal.json"),
                json!({"a":0,"b":0}),
            );
            write_json(
                &state_dir.join("config/diff.json"),
                json!({"a":{"source":1,"target":0,"category":"preference"},"b":{"source":2,"target":0,"category":"preference"}}),
            );
            write_json(
                &state_dir.join("config/source_config.json"),
                json!({"a":1,"b":2}),
            );
            let before = Snapshot::capture(root.path(), &state_dir);
            call_config_all(&root, json!({"category":"preference"})).await;
            assert_delta(
                before,
                Snapshot::capture(root.path(), &state_dir),
                &[
                    "~ journal/config/journal.json",
                    "+ journal/config/journal.json.lock",
                    "- journal/imports/prefix01/config/diff.json",
                    "- journal/imports/prefix01/config/source_config.json",
                    "+ journal/imports/prefix01/config/log.jsonl",
                    "- state/config/diff.json",
                    "- state/config/source_config.json",
                    "+ state/config/log.jsonl",
                ],
            );
        }
    }
}
