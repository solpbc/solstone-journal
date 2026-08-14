// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::HashMap, fs, path::Path};

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Map, Value, json};

use crate::{
    AppState,
    http::{error, json as json_response},
};

fn records(root: &Path) -> Vec<Map<String, Value>> {
    let directory = root.join("apps/import/journal_sources");
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let text = fs::read_to_string(entry.path()).ok()?;
            serde_json::from_str::<Value>(&text)
                .ok()?
                .as_object()
                .cloned()
        })
        .collect()
}

fn source(root: &Path, name: &str) -> Option<Map<String, Value>> {
    records(root)
        .into_iter()
        .find(|record| record.get("name").and_then(Value::as_str) == Some(name))
}

fn prefix(record: &Map<String, Value>) -> Option<String> {
    record
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| key.len() >= 8)
        .map(|key| key[..8].to_owned())
}

fn problem_missing(name: &str) -> Response {
    error(
        StatusCode::NOT_FOUND,
        "I couldn't use that journal source.",
        "journal_source_problem",
        format!("Journal source '{name}' not found"),
    )
}

pub(crate) async fn list(State(state): State<AppState>) -> Response {
    let items: Vec<Value> = records(&state.root).into_iter().filter(|record| record.get("pair_mode").and_then(Value::as_str) != Some("pl")).filter_map(|record| Some(json!({
        "name": record.get("name")?, "prefix": prefix(&record)?,
        "status": if record.get("revoked") == Some(&Value::Bool(true)) { "revoked" } else { "active" },
        "created_at": record.get("created_at")?,
    }))).collect();
    json_response(
        StatusCode::OK,
        json!({"items": items, "total": items.len()}),
    )
}

pub(crate) async fn status(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let Some(record) = source(&state.root, &name) else {
        return problem_missing(&name);
    };
    let Some(prefix) = prefix(&record) else {
        return problem_missing(&name);
    };
    json_response(
        StatusCode::OK,
        json!({
            "name": record.get("name").cloned().unwrap_or_else(|| json!("")), "prefix": prefix,
            "status": if record.get("revoked") == Some(&Value::Bool(true)) { "revoked" } else { "active" },
            "created_at": record.get("created_at").cloned().unwrap_or(Value::Null),
            "revoked": record.get("revoked").cloned().unwrap_or(Value::Bool(false)),
            "revoked_at": record.get("revoked_at").cloned().unwrap_or(Value::Null),
            "stats": record.get("stats").cloned().unwrap_or_else(|| json!({})),
        }),
    )
}

pub(crate) async fn staged(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(record) = source(&state.root, &name) else {
        return problem_missing(&name);
    };
    let Some(prefix) = prefix(&record) else {
        return problem_missing(&name);
    };
    let area = query.get("area").map(String::as_str);
    if area.is_some_and(|area| !matches!(area, "entities" | "facets" | "config")) {
        return error(
            StatusCode::BAD_REQUEST,
            "I couldn't use one of those values.",
            "invalid_request_value",
            "Area must be one of: entities, facets, config".to_owned(),
        );
    }
    let root = state.root.join("imports").join(prefix);
    let mut items = Vec::new();
    let entities = root.join("entities/staged");
    if area.is_none_or(|area| area == "entities")
        && let Ok(entries) = fs::read_dir(entities)
    {
        for entry in entries.flatten() {
            if let Ok(text) = fs::read_to_string(entry.path())
                && let Ok(value) = serde_json::from_str::<Value>(&text)
                && let Some(payload) = value.as_object()
            {
                items.push(json!({"area":"entities", "source_id": entry.path().file_stem().and_then(|x| x.to_str()).unwrap_or(""), "reason": payload.get("reason"), "source_entity": payload.get("source_entity"), "match_candidates": payload.get("match_candidates"), "staged_at": payload.get("staged_at")}));
            }
        }
    }
    let facets = root.join("facets/staged");
    if area.is_none_or(|area| area == "facets") && facets.exists() {
        let mut staged = Vec::new();
        collect_staged_facets(&facets, &facets, &mut staged);
        items.extend(staged);
    }
    if area.is_none_or(|area| area == "config")
        && let Ok(text) = fs::read_to_string(root.join("config/diff.json"))
        && let Ok(diff) = serde_json::from_str::<Value>(&text)
        && diff.is_object()
    {
        items.push(json!({"area":"config", "diff": diff}));
    }
    json_response(
        StatusCode::OK,
        json!({"items": items, "total": items.len()}),
    )
}

fn collect_staged_facets(root: &Path, directory: &Path, items: &mut Vec<Value>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_staged_facets(root, &path, items);
            continue;
        }
        if !path.to_string_lossy().ends_with(".staged.json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(Value::Object(payload)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let parts: Vec<_> = relative.components().collect();
        if parts.len() < 3 {
            continue;
        }
        let facet = parts[0].as_os_str().to_string_lossy();
        let file_type = parts[1].as_os_str().to_string_lossy();
        let mut item = Map::new();
        item.insert("area".into(), json!("facets"));
        item.insert("staged_file".into(), json!(relative.to_string_lossy()));
        item.insert("facet".into(), json!(facet));
        item.insert("file_type".into(), json!(file_type));
        item.extend(payload);
        items.push(Value::Object(item));
    }
}
