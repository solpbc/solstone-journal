// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use axum::response::Response;
use serde_json::{Map, Value, json};

use crate::http::json_response;

pub const DEFAULT_THINKING_SURFACES: &str = "on_tap";

pub async fn get(journal_root: std::path::PathBuf) -> Response {
    json_response(Value::Object(load_chat_config(&journal_root)))
}

pub fn load_chat_config(journal_root: &Path) -> Map<String, Value> {
    let mut config = fs::read_to_string(journal_root.join("config/chat.json"))
        .ok()
        .and_then(|source| serde_json::from_str::<Value>(&source).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if !matches!(
        config.get("thinking_surfaces").and_then(Value::as_str),
        Some("always" | "on_tap" | "never")
    ) {
        config.insert(
            "thinking_surfaces".to_owned(),
            json!(DEFAULT_THINKING_SURFACES),
        );
    }
    config
}
