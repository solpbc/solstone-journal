// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use axum::{body::Bytes, response::Response};
use serde_json::{Value, json};
use solstone_core_journal_io::{LockOptions, MalformedPolicy, hold_lock, read_json};

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
    if request.is_empty() {
        return missing_request_body();
    }
    let path = journal_root.join("config/schedules.json");
    let _lock = match hold_lock(&path, LockOptions::default()) {
        Ok(lock) => lock,
        Err(_) => return settings_operation_failed(),
    };
    let Ok(Value::Object(mut schedules)) = read_json(&path, json!({}), MalformedPolicy::Raise)
    else {
        return settings_operation_failed();
    };
    let mut changed_fields = serde_json::Map::new();
    for (service, entry_name) in [("plaud", "sync:plaud"), ("obsidian", "sync:obsidian")] {
        if let Some(value) = request.get(service) {
            let Some(value) = value.as_object() else {
                return invalid_config_value(format!("{service} must be an object"));
            };
            if let Some(enabled) = value.get("enabled") {
                let Some(enabled) = enabled.as_bool() else {
                    return invalid_config_value(format!("{service}.enabled must be a boolean"));
                };
                let old_entry = schedules.get(entry_name).and_then(Value::as_object);
                let old_enabled = old_entry
                    .and_then(|entry| entry.get("enabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(old_entry.is_some());
                if enabled != old_enabled {
                    let entry = schedules.entry(entry_name.to_owned()).or_insert_with(|| json!({"cmd":["journal","importer","--sync",service,"--save"],"every":"hourly"}));
                    let target = entry.as_object_mut().expect("created object");
                    target.insert("enabled".to_owned(), Value::Bool(enabled));
                    changed_fields.insert(format!("{service}.enabled"), Value::Bool(enabled));
                }
            }
        }
    }
    if !changed_fields.is_empty()
        && solstone_core_journal_io::write_json(
            &path,
            &Value::Object(schedules),
            solstone_core_journal_io::JsonWriteOptions {
                mode: Some(0o600),
                ..Default::default()
            },
        )
        .is_err()
    {
        return settings_operation_failed();
    }
    drop(_lock);
    if !changed_fields.is_empty()
        && solstone_core_facets::append_action_log(
            &journal_root,
            None,
            "app",
            "settings",
            "sync_update",
            json!({"changed_fields": changed_fields}),
        )
        .is_err()
    {
        return settings_operation_failed();
    }
    get(journal_root).await
}
