// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use axum::{body::Bytes, response::Response};
use serde_json::{Value, json};

use crate::{
    config::truthy,
    http::{invalid_config_value, json_response, missing_request_body, settings_operation_failed},
    request_body::{JsonBody, json_body},
};

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

pub async fn update(journal_root: PathBuf, body: Bytes) -> Response {
    let JsonBody::Value(Value::Object(request)) = json_body(body) else {
        return missing_request_body();
    };
    let mut schedules = fs::read(journal_root.join("config/schedules.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    for (service, entry_name) in [("plaud", "sync:plaud"), ("obsidian", "sync:obsidian")] {
        if let Some(value) = request.get(service) {
            let Some(value) = value.as_object() else {
                return invalid_config_value(format!("{service} must be an object"));
            };
            if let Some(enabled) = value.get("enabled") {
                let Some(enabled) = enabled.as_bool() else {
                    return invalid_config_value(format!("{service}.enabled must be a boolean"));
                };
                let entry = schedules.entry(entry_name.to_owned()).or_insert_with(|| json!({"cmd":["journal","importer","--sync",service,"--save"],"every":"hourly"}));
                let target = entry.as_object_mut().expect("created object");
                target.insert("enabled".to_owned(), Value::Bool(enabled));
            }
        }
    }
    let path = journal_root.join("config/schedules.json");
    match solstone_core_journal_io::write_json(
        &path,
        &Value::Object(schedules),
        solstone_core_journal_io::JsonWriteOptions {
            mode: Some(0o600),
            ..Default::default()
        },
    ) {
        Ok(()) => get(journal_root).await,
        Err(_) => settings_operation_failed(),
    }
}
