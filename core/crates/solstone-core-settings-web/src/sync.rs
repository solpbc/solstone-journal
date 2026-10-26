// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{body::Bytes, response::Response};
use serde_json::{Value, json};
use solstone_core_journal_io::durability::{ArtifactId, DurableRead, read_json_durable};
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::{
    config::truthy,
    http::{invalid_config_value, json_response, missing_request_body, settings_operation_failed},
    request_body::{JsonBody, json_body},
};

pub async fn get(journal_root: PathBuf) -> Response {
    let schedules_path = journal_root.join("config/schedules.json");
    let schedules = match read_json_durable::<Value>(ArtifactId::SchedulesConfig, &schedules_path) {
        Ok(DurableRead::Present(Value::Object(map))) => map,
        _ => serde_json::Map::new(),
    };
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    json_response(json!({
        "plaud": status(schedules.get("sync:plaud"), source_sync_plaud_available(&config)),
        "obsidian": status(schedules.get("sync:obsidian"), true),
    }))
}

fn status(entry: Option<&Value>, available: bool) -> Value {
    let values = entry.and_then(Value::as_object);
    json!({"available": available, "enabled": values.and_then(|values| values.get("enabled")).and_then(Value::as_bool).unwrap_or(entry.is_some()), "configured": entry.is_some()})
}

pub async fn update(journal_root: PathBuf, body: Bytes) -> Response {
    let changes = match parse_sync_update(body) {
        Ok(changes) => changes,
        Err(error) => return *error,
    };
    let path = journal_root.join("config/schedules.json");
    let _lock = match hold_lock(&path, LockOptions::default()) {
        Ok(lock) => lock,
        Err(_) => return settings_operation_failed(),
    };
    let mut schedules = match read_json_durable::<Value>(ArtifactId::SchedulesConfig, &path) {
        Ok(DurableRead::Present(Value::Object(map))) => map,
        Ok(DurableRead::Absent) => serde_json::Map::new(),
        _ => return settings_operation_failed(),
    };
    let mut changed_fields = serde_json::Map::new();
    for (service, enabled) in changes {
        let entry_name = format!("sync:{service}");
        let old_entry = schedules.get(&entry_name).and_then(Value::as_object);
        let old_enabled = old_entry
            .and_then(|entry| entry.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(old_entry.is_some());
        if enabled != old_enabled {
            let entry = schedules.entry(entry_name).or_insert_with(|| json!({"cmd":["solstone","journal","importer","--sync",service,"--save"],"every":"hourly"}));
            entry
                .as_object_mut()
                .expect("created object")
                .insert("enabled".into(), Value::Bool(enabled));
            changed_fields.insert(format!("{service}.enabled"), Value::Bool(enabled));
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

pub fn source_sync_plaud_available(config: &serde_json::Map<String, Value>) -> bool {
    config
        .get("env")
        .and_then(Value::as_object)
        .and_then(|values| values.get("PLAUD_ACCESS_TOKEN"))
        .is_some_and(truthy)
        || std::env::var_os("PLAUD_ACCESS_TOKEN").is_some_and(|value| !value.is_empty())
}

pub fn parse_sync_update(body: Bytes) -> Result<Vec<(&'static str, bool)>, Box<Response>> {
    let JsonBody::Value(Value::Object(request)) = json_body(body) else {
        return Err(Box::new(missing_request_body()));
    };
    if request.is_empty() {
        return Err(Box::new(missing_request_body()));
    }
    let mut changes = Vec::new();
    for service in ["plaud", "obsidian"] {
        if let Some(value) = request.get(service) {
            let Some(value) = value.as_object() else {
                return Err(Box::new(invalid_config_value(format!(
                    "{service} must be an object"
                ))));
            };
            if let Some(enabled) = value.get("enabled") {
                let Some(enabled) = enabled.as_bool() else {
                    return Err(Box::new(invalid_config_value(format!(
                        "{service}.enabled must be a boolean"
                    ))));
                };
                changes.push((service, enabled));
            }
        }
    }
    Ok(changes)
}

pub(crate) async fn get_routed(
    root: PathBuf,
    delegate: Option<axum::Extension<crate::SourceSyncDelegate>>,
) -> Response {
    match delegate {
        Some(axum::Extension(delegate)) => delegate.get().await,
        None => get(root).await,
    }
}

pub(crate) async fn update_routed(
    root: PathBuf,
    delegate: Option<axum::Extension<crate::SourceSyncDelegate>>,
    body: Bytes,
) -> Response {
    match delegate {
        Some(axum::Extension(delegate)) => delegate.update(body).await,
        None => update(root, body).await,
    }
}
