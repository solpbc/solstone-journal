// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_convey_http::window_catalog::text;

use std::path::{Path, PathBuf};

use axum::{body::Bytes, response::Response};
use serde_json::{Value, json};
use solstone_core_journal_io::durability::{ArtifactId, DurableRead, read_json_durable};
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::{
    http::{invalid_config_value, json_response, missing_request_body, settings_operation_failed},
    request_body::{JsonBody, json_body},
};

pub async fn get(journal_root: PathBuf) -> Response {
    let schedules_path = journal_root.join("config/schedules.json");
    let schedules = match read_json_durable::<Value>(ArtifactId::SchedulesConfig, &schedules_path) {
        Ok(DurableRead::Present(Value::Object(map))) => map,
        _ => serde_json::Map::new(),
    };
    json_response(json!({
        "plaud": status(schedules.get("sync:plaud"), source_sync_plaud_available(&journal_root)),
    }))
}

fn status(entry: Option<&Value>, available: bool) -> Value {
    let values = entry.and_then(Value::as_object);
    json!({"available": available, "enabled": values.and_then(|values| values.get("enabled")).and_then(Value::as_bool).unwrap_or(entry.is_some()), "configured": entry.is_some()})
}

pub async fn update(locale: &str, journal_root: PathBuf, body: Bytes) -> Response {
    let changes = match parse_sync_update_in(locale, body) {
        Ok(changes) => changes,
        Err(error) => return *error,
    };
    let path = journal_root.join("config/schedules.json");
    let _lock = match hold_lock(&path, LockOptions::default()) {
        Ok(lock) => lock,
        Err(_) => return settings_operation_failed(locale),
    };
    let mut schedules = match read_json_durable::<Value>(ArtifactId::SchedulesConfig, &path) {
        Ok(DurableRead::Present(Value::Object(map))) => map,
        Ok(DurableRead::Absent) => serde_json::Map::new(),
        _ => return settings_operation_failed(locale),
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
        return settings_operation_failed(locale);
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
        return settings_operation_failed(locale);
    }
    get(journal_root).await
}

pub fn source_sync_plaud_available(journal_root: &Path) -> bool {
    solstone_core_import::plaud::is_connected(journal_root)
}

pub fn parse_sync_update(body: Bytes) -> Result<Vec<(&'static str, bool)>, Box<Response>> {
    parse_sync_update_in(solstone_core_convey_http::window_catalog::ENGLISH, body)
}

fn parse_sync_update_in(
    locale: &str,
    body: Bytes,
) -> Result<Vec<(&'static str, bool)>, Box<Response>> {
    let JsonBody::Value(Value::Object(request)) = json_body(body) else {
        return Err(Box::new(missing_request_body(locale)));
    };
    if request.is_empty() {
        return Err(Box::new(missing_request_body(locale)));
    }
    let mut changes = Vec::new();
    if let Some(value) = request.get("plaud") {
        let Some(value) = value.as_object() else {
            return Err(Box::new(invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.sync.detail.plaud.must.be.an.object",
                )
                .to_owned(),
            )));
        };
        if let Some(enabled) = value.get("enabled") {
            let Some(enabled) = enabled.as_bool() else {
                return Err(Box::new(invalid_config_value(
                    locale,
                    text(
                        locale,
                        "journal.window.settings.sync.detail.plaud.enabled.must.be.a.boolean",
                    )
                    .to_owned(),
                )));
            };
            changes.push(("plaud", enabled));
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
    locale: &str,
    root: PathBuf,
    delegate: Option<axum::Extension<crate::SourceSyncDelegate>>,
    body: Bytes,
) -> Response {
    match delegate {
        Some(axum::Extension(delegate)) => match parse_sync_update_in(locale, body) {
            Ok(changes) => delegate.update(changes).await,
            Err(error) => *error,
        },
        None => update(locale, root, body).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SourceSyncAuthority, SourceSyncDelegate, SourceSyncResponse};
    use axum::http::StatusCode;
    use solstone_core_convey_http::window_catalog::ENGLISH;
    use std::sync::Arc;

    struct SourceAuthority;

    impl SourceSyncAuthority for SourceAuthority {
        fn get(&self) -> SourceSyncResponse {
            unreachable!("the test only sends updates")
        }

        fn update(&self, changes: Vec<(&'static str, bool)>) -> SourceSyncResponse {
            assert_eq!(changes, [("plaud", false)]);
            Box::pin(async { json_response(json!({"saved": true})) })
        }

        fn trigger_plaud_sync(&self) -> SourceSyncResponse {
            unreachable!("the test only sends updates")
        }
    }

    #[tokio::test]
    async fn sync_update_validates_before_dispatching_to_the_embedded_authority() {
        let delegate = SourceSyncDelegate::new(Arc::new(SourceAuthority));
        let send = |body: &'static str| {
            update_routed(
                ENGLISH,
                PathBuf::new(),
                Some(axum::Extension(delegate.clone())),
                Bytes::from_static(body.as_bytes()),
            )
        };

        let invalid = send(r#"{"plaud":{"enabled":"invalid"}}"#).await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(invalid.into_body(), usize::MAX)
            .await
            .expect("invalid update body");
        let payload: Value = serde_json::from_slice(&body).expect("invalid update JSON");
        assert_eq!(payload["reason_code"], "invalid_config_value");
        assert_eq!(payload["detail"], "plaud.enabled must be a boolean");

        let valid = send(r#"{"plaud":{"enabled":false}}"#).await;
        assert_eq!(valid.status(), StatusCode::OK);
    }
}
