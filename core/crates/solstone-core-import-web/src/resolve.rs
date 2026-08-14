// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use axum::{
    extract::{Json, Path as AxumPath, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Map, Value, json};
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, mutate_journal_config,
};
use solstone_core_journal_io::{AtomicWriteOptions, append_jsonl, atomic_replace};

use crate::{
    AppState,
    http::{error, json as json_response},
};

fn source_state(root: &Path, name: &str) -> Option<std::path::PathBuf> {
    let source: Value = serde_json::from_slice(
        &fs::read(
            root.join("apps/import/journal_sources")
                .join(format!("{name}.json")),
        )
        .ok()?,
    )
    .ok()?;
    let key = source.get("key")?.as_str()?;
    Some(root.join("imports").join(&key[..8]))
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
fn log(state: &Path, area: &str, value: Value) {
    let _ = append_jsonl(state.join(area).join("log.jsonl"), &value);
}

pub(crate) async fn entity(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    let Some(state) = source_state(&app.root, &name) else {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't use that journal source.",
            "journal_source_problem",
            format!("Journal source '{name}' not found"),
        );
    };
    let Some(source_id) = data.get("source_id").and_then(Value::as_str) else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "source_id is required".into(),
        );
    };
    let staged = state
        .join("entities/staged")
        .join(format!("{source_id}.json"));
    let payload = read(&staged);
    if !staged.exists() {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't find that import.",
            "import_not_found",
            format!("Staged entity '{source_id}' not found."),
        );
    }
    let action = data.get("action").and_then(Value::as_str).unwrap_or("");
    if action == "merge" || action == "create" {
        let entity = payload.get("source_entity").cloned().unwrap_or(Value::Null);
        let id = data
            .get("target")
            .and_then(Value::as_str)
            .or_else(|| entity.get("id").and_then(Value::as_str))
            .unwrap_or(source_id);
        if action == "create" {
            let _ = write(
                &app.root.join("entities").join(id).join("entity.json"),
                &entity,
            );
        }
    }
    let _ = fs::remove_file(&staged);
    log(
        &state,
        "entities",
        json!({"action":format!("resolved_{action}"),"item_type":"entity","item_id":source_id}),
    );
    json_response(StatusCode::OK, json!({"target_id":data.get("target")}))
}

pub(crate) async fn facet(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    let Some(state) = source_state(&app.root, &name) else {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't use that journal source.",
            "journal_source_problem",
            format!("Journal source '{name}' not found"),
        );
    };
    let Some(file) = data
        .get("staged_file")
        .and_then(Value::as_str)
        .filter(|file| !file.contains(".."))
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "Invalid staged file".into(),
        );
    };
    let staged = state.join("facets/staged").join(file);
    if !staged.exists() {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't find that import.",
            "import_not_found",
            "Staged facet file not found.".into(),
        );
    }
    let payload = read(&staged);
    if data.get("mode").and_then(Value::as_str) == Some("apply")
        && payload.get("reason").and_then(Value::as_str) == Some("facet_json_conflict")
    {
        if let Some(facet) = file.split('/').next() {
            let _ = write(
                &app.root.join("facets").join(facet).join("facet.json"),
                payload.get("source_content").unwrap_or(&Value::Null),
            );
        }
    }
    let _ = fs::remove_file(&staged);
    log(
        &state,
        "facets",
        json!({"action":"resolved_apply","item_type":"facet","item_id":file}),
    );
    json_response(StatusCode::OK, json!({}))
}

pub(crate) async fn config(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    resolve_config(&app.root, source_state(&app.root, &name), data).await
}
pub(crate) async fn config_all(
    State(app): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(data): Json<Value>,
) -> Response {
    let Some(state) = source_state(&app.root, &name) else {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't use that journal source.",
            "journal_source_problem",
            format!("Journal source '{name}' not found"),
        );
    };
    let category = data.get("category").and_then(Value::as_str).unwrap_or("");
    let diff = read(&state.join("config/diff.json"));
    let fields = diff
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, entry)| entry.get("category").and_then(Value::as_str) == Some(category))
        .map(|(field, _)| field.clone())
        .collect::<Vec<_>>();
    for field in &fields {
        let response = resolve_config(
            &app.root,
            Some(state.clone()),
            json!({"field":field,"action":"apply"}),
        )
        .await;
        if response.status() != StatusCode::OK {
            return response;
        }
    }
    json_response(StatusCode::OK, json!({"count":fields.len()}))
}
async fn resolve_config(root: &Path, state: Option<std::path::PathBuf>, data: Value) -> Response {
    let Some(state) = state else {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't use that journal source.",
            "journal_source_problem",
            "Journal source not found".into(),
        );
    };
    let field = data.get("field").and_then(Value::as_str).unwrap_or("");
    let action = data.get("action").and_then(Value::as_str).unwrap_or("");
    let path = state.join("config/diff.json");
    if !path.exists() {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't find that import.",
            "import_not_found",
            "No staged config diff found.".into(),
        );
    }
    let mut diff = read(&path);
    let Some(entry) = diff.get(field).cloned() else {
        return error(
            StatusCode::NOT_FOUND,
            "I couldn't find that import.",
            "import_not_found",
            format!("Config field '{field}' is not staged."),
        );
    };
    if action == "apply" {
        let value = entry.get("source").cloned().unwrap_or(Value::Null);
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
                "I couldn't save that import.",
                "import_metadata_failed",
                config_error.to_string(),
            );
        }
    }
    diff.as_object_mut().unwrap().remove(field);
    if diff.as_object().unwrap().is_empty() {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(state.join("config/source_config.json"));
    } else {
        let _ = write(&path, &diff);
    }
    log(
        &state,
        "config",
        json!({"action":if action == "apply" {"config_field_applied"} else {"config_field_kept"},"item_type":"config","item_id":field}),
    );
    json_response(StatusCode::OK, json!({}))
}
fn set(config: &mut Map<String, Value>, field: &str, value: Value) {
    let mut parts = field.split('.').peekable();
    let mut current = config;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.into(), value);
            return;
        }
        current = current
            .entry(part.to_owned())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("object config path");
    }
}
