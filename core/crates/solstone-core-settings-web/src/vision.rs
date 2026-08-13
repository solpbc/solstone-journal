// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::http::json_response;

pub async fn get(journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let describe = config.get("describe").and_then(Value::as_object);
    let registry = solstone_core_describe_categories::category_registry();
    let mut defaults = Map::new();
    for (name, metadata) in registry.as_object().expect("category registry") {
        defaults.insert(name.clone(), json!({
            "label": metadata.get("label").cloned().unwrap_or_else(|| json!(name)),
            "group": metadata.get("group").cloned().unwrap_or_else(|| json!("Screen Analysis")),
            "extraction": metadata.get("extraction").cloned().unwrap_or_else(|| json!("")),
            "importance": metadata.get("importance").cloned().unwrap_or_else(|| json!("normal")),
        }));
    }
    json_response(json!({
        "max_extractions": describe.and_then(|value| value.get("max_extractions")).cloned().unwrap_or(json!(20)),
        "redact": describe.and_then(|value| value.get("redact")).cloned().unwrap_or(json!([])),
        "categories": describe.and_then(|value| value.get("categories")).cloned().unwrap_or(json!({})),
        "category_defaults": defaults,
    }))
}
