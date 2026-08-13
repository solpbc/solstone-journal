// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{body::Bytes, response::Response};
use serde_json::{Map, Value, json};
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockError, LockOptions, mutate_journal_config,
};

use crate::{
    http::{
        config_busy, invalid_config_value, json_response, missing_request_body,
        settings_operation_failed,
    },
    request_body::{JsonBody, json_body},
};

pub async fn get(journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let tmux = config
        .get("observe")
        .and_then(|value| value.get("tmux"))
        .and_then(Value::as_object);
    json_response(json!({
        "tmux": {
            "enabled": tmux.and_then(|values| values.get("enabled")).cloned().unwrap_or(Value::Bool(true)),
            "capture_interval": tmux.and_then(|values| values.get("capture_interval")).cloned().unwrap_or(json!(5)),
        },
        "defaults": {"tmux": {"enabled": true, "capture_interval": 5, "capture_interval_min": 1, "capture_interval_max": 60}},
    }))
}

pub async fn update(journal_root: PathBuf, lock_options: LockOptions, body: Bytes) -> Response {
    let JsonBody::Value(Value::Object(request)) = json_body(body) else {
        return missing_request_body();
    };
    if request.is_empty() {
        return missing_request_body();
    }
    if let Some(tmux) = request.get("tmux") {
        let Some(tmux) = tmux.as_object() else {
            return invalid_config_value("tmux must be an object");
        };
        if tmux.get("enabled").is_some_and(|value| !value.is_boolean()) {
            return invalid_config_value("tmux.enabled must be a boolean");
        }
        if tmux.get("capture_interval").is_some_and(|value| {
            !value.is_i64()
                || value
                    .as_i64()
                    .is_none_or(|value| !(1..=60).contains(&value))
        }) {
            return invalid_config_value(
                "tmux.capture_interval must be an integer between 1 and 60",
            );
        }
    }
    match mutate_journal_config(&journal_root, lock_options, |config| {
        let observe = config
            .entry("observe".to_owned())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut();
        let Some(observe) = observe else {
            return JournalConfigMutation {
                changed: false,
                value: Map::new(),
            };
        };
        let mut changed = false;
        let mut changed_fields = Map::new();
        if let Some(tmux) = request.get("tmux").and_then(Value::as_object) {
            let target = observe
                .entry("tmux".to_owned())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("object");
            for key in ["enabled", "capture_interval"] {
                if let Some(value) = tmux.get(key) {
                    if target.get(key) != Some(value) {
                        changed_fields.insert(format!("tmux.{key}"), value.clone());
                        changed = true;
                    }
                    target.insert(key.to_owned(), value.clone());
                }
            }
        }
        JournalConfigMutation {
            changed,
            value: changed_fields,
        }
    }) {
        Ok(transaction) => {
            if !transaction.value.is_empty()
                && solstone_core_facets::append_action_log(
                    &journal_root,
                    None,
                    "app",
                    "settings",
                    "observe_update",
                    json!({"changed_fields": transaction.value}),
                )
                .is_err()
            {
                return settings_operation_failed();
            }
            get(journal_root).await
        }
        Err(solstone_core_journal_config_write::ConfigMutationError::Lock(LockError::Timeout(
            _,
        ))) => config_busy(),
        Err(_) => settings_operation_failed(),
    }
}
