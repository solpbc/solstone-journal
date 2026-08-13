// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use axum::{body::Bytes, response::Response};
use serde_json::{Map, Value, json};

use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::{
    http::{invalid_config_value, json_response, settings_operation_failed},
    request_body::{JsonBody, json_body},
};

pub const DEFAULT_THINKING_SURFACES: &str = "on_tap";

pub async fn get(journal_root: std::path::PathBuf) -> Response {
    json_response(Value::Object(load_chat_config(&journal_root)))
}

pub async fn update(
    journal_root: std::path::PathBuf,
    lock_options: LockOptions,
    body: Bytes,
) -> Response {
    let JsonBody::Value(Value::Object(updates)) = json_body(body) else {
        return invalid_config_value("chat update must be an object");
    };
    if updates
        .get("thinking_surfaces")
        .is_some_and(|value| !matches!(value.as_str(), Some("always" | "on_tap" | "never")))
    {
        return invalid_config_value("invalid thinking_surfaces");
    }
    let mut config = load_chat_config(&journal_root);
    for (key, value) in updates {
        config.insert(key, value);
    }
    let path = journal_root.join("config/chat.json");
    let _lock = match hold_lock(&path, lock_options) {
        Ok(lock) => lock,
        Err(_) => return settings_operation_failed(),
    };
    match solstone_core_journal_io::write_json(
        &path,
        &Value::Object(config.clone()),
        solstone_core_journal_io::JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    ) {
        Ok(()) => json_response(Value::Object(config)),
        Err(_) => settings_operation_failed(),
    }
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
