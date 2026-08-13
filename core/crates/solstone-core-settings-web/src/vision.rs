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

pub async fn update(journal_root: PathBuf, lock_options: LockOptions, body: Bytes) -> Response {
    let JsonBody::Value(Value::Object(request)) = json_body(body) else {
        return missing_request_body();
    };
    if let Some(value) = request.get("max_extractions")
        && (!value.is_i64()
            || value
                .as_i64()
                .is_none_or(|value| !(5..=100).contains(&value)))
    {
        return invalid_config_value("max_extractions must be an integer between 5 and 100");
    }
    let redact = match request.get("redact") {
        Some(Value::Array(values))
            if values.len() <= 50
                && values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| value.len() <= 200)) =>
        {
            Some(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| Value::String(value.to_owned()))
                    .collect::<Vec<_>>(),
            )
        }
        Some(Value::Array(values)) if values.len() > 50 => {
            return invalid_config_value("redact may contain at most 50 rules");
        }
        Some(Value::Array(_)) => {
            return invalid_config_value("each redact rule must be 200 characters or fewer");
        }
        Some(_) => return invalid_config_value("redact must be a list of strings"),
        None => None,
    };
    let registry = solstone_core_describe_categories::category_registry();
    if let Some(categories) = request.get("categories") {
        let Some(categories) = categories.as_object() else {
            return invalid_config_value("categories must be an object");
        };
        for (name, override_value) in categories {
            if registry.get(name).is_none() {
                return invalid_config_value(format!("Unknown category: {name}"));
            };
            if !override_value.is_null() {
                let Some(value) = override_value.as_object() else {
                    return invalid_config_value("category config must be an object");
                };
                if let Some(importance) = value.get("importance")
                    && !matches!(
                        importance.as_str(),
                        Some("high" | "normal" | "low" | "ignore")
                    )
                {
                    return invalid_config_value(format!(
                        "Invalid importance for {name}: {}. Must be one of: high, ignore, low, normal",
                        importance.as_str().unwrap_or_default()
                    ));
                }
                if value
                    .get("extraction")
                    .is_some_and(|value| !value.is_string())
                {
                    return invalid_config_value(format!("extraction for {name} must be a string"));
                }
            }
        }
    }
    let result = mutate_journal_config(&journal_root, lock_options, |config| {
        let describe = config
            .entry("describe".to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(describe) = describe.as_object_mut() else {
            return JournalConfigMutation {
                changed: false,
                value: false,
            };
        };
        let mut changed = false;
        if let Some(value) = request.get("max_extractions") {
            changed |= describe.get("max_extractions") != Some(value);
            describe.insert("max_extractions".to_owned(), value.clone());
        }
        if let Some(redact) = &redact {
            let value = Value::Array(redact.clone());
            changed |= describe.get("redact") != Some(&value);
            describe.insert("redact".to_owned(), value);
        }
        if let Some(categories) = request.get("categories").and_then(Value::as_object) {
            let categories_target = describe
                .entry("categories".to_owned())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("object");
            for (name, value) in categories {
                match value {
                    Value::Null => {
                        changed |= categories_target.remove(name).is_some();
                    }
                    Value::Object(values) if !values.is_empty() => {
                        changed |= categories_target.get(name) != Some(value);
                        categories_target.insert(name.clone(), value.clone());
                    }
                    _ => {}
                }
            }
        }
        JournalConfigMutation {
            changed,
            value: true,
        }
    });
    match result {
        Ok(_) => get(journal_root).await,
        Err(solstone_core_journal_config_write::ConfigMutationError::Lock(LockError::Timeout(
            _,
        ))) => config_busy(),
        Err(_) => settings_operation_failed(),
    }
}
