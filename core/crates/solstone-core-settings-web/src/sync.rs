// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use axum::response::Response;
use serde_json::{Value, json};

use crate::{config::truthy, http::json_response};

pub async fn get(journal_root: PathBuf) -> Response {
    let schedules = fs::read(journal_root.join("config/schedules.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let configured_token = config
        .get("env")
        .and_then(Value::as_object)
        .and_then(|values| values.get("PLAUD_ACCESS_TOKEN"))
        .is_some_and(truthy);
    json_response(json!({
        "plaud": status(schedules.get("sync:plaud"), configured_token || std::env::var_os("PLAUD_ACCESS_TOKEN").is_some_and(|value| !value.is_empty())),
        "obsidian": status(schedules.get("sync:obsidian"), true),
    }))
}

fn status(entry: Option<&Value>, available: bool) -> Value {
    let values = entry.and_then(Value::as_object);
    json!({"available": available, "enabled": values.and_then(|values| values.get("enabled")).and_then(Value::as_bool).unwrap_or(entry.is_some()), "configured": entry.is_some()})
}
