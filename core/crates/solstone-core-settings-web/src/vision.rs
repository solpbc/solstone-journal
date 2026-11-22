// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::{body::Bytes, response::Response};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::window_catalog::{strings, text};
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

pub async fn get(locale: &str, journal_root: PathBuf) -> Response {
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let describe = config.get("describe").and_then(Value::as_object);
    let registry = solstone_core_describe_categories::category_registry();
    let mut defaults = Map::new();
    for (name, metadata) in registry.as_object().expect("category registry") {
        defaults.insert(name.clone(), json!({
            "label": category_label(locale, name, metadata),
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

fn category_label(locale: &str, name: &str, metadata: &Value) -> Value {
    solstone_core_convey_http::window_catalog::screen_category_label(locale, name)
        .map(Value::String)
        .or_else(|| metadata.get("label").cloned())
        .unwrap_or_else(|| json!(name))
}

pub async fn update(
    locale: &str,
    journal_root: PathBuf,
    lock_options: LockOptions,
    body: Bytes,
) -> Response {
    let JsonBody::Value(Value::Object(request)) = json_body(body) else {
        return missing_request_body(locale);
    };
    if request.is_empty() {
        return missing_request_body(locale);
    }
    if let Some(value) = request.get("max_extractions")
        && (!value.is_i64()
            || value
                .as_i64()
                .is_none_or(|value| !(5..=100).contains(&value)))
    {
        return invalid_config_value(
            locale,
            text(
                locale,
                "journal.window.settings.vision.detail.max.extractions.must.be.an.integer",
            ),
        );
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
            return invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.vision.detail.redact.may.contain.at.most.rules",
                ),
            );
        }
        Some(Value::Array(_)) => {
            return invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.vision.detail.each.redact.rule.must.be.characters",
                ),
            );
        }
        Some(_) => {
            return invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.vision.detail.redact.must.be.a.list.of",
                ),
            );
        }
        None => None,
    };
    let registry = solstone_core_describe_categories::category_registry();
    if let Some(categories) = request.get("categories") {
        let Some(categories) = categories.as_object() else {
            return invalid_config_value(
                locale,
                text(
                    locale,
                    "journal.window.settings.vision.detail.categories.must.be.an.object",
                ),
            );
        };
        for (name, override_value) in categories {
            if registry.get(name).is_none() {
                return invalid_config_value(
                    locale,
                    strings(
                        locale,
                        "journal.window.settings.vision.detail.unknown.category",
                        [("name", name)],
                    ),
                );
            };
            if !override_value.is_null() {
                let Some(value) = override_value.as_object() else {
                    return invalid_config_value(
                        locale,
                        text(
                            locale,
                            "journal.window.settings.vision.detail.category.config.must.be.an.object",
                        ),
                    );
                };
                if let Some(importance) = value.get("importance")
                    && !matches!(
                        importance.as_str(),
                        Some("high" | "normal" | "low" | "ignore")
                    )
                {
                    return invalid_config_value(
                        locale,
                        strings(
                            locale,
                            "journal.window.settings.vision.detail.importance",
                            [
                                ("name", name),
                                ("importance", importance.as_str().unwrap_or_default()),
                            ],
                        ),
                    );
                }
                if value
                    .get("extraction")
                    .is_some_and(|value| !value.is_string())
                {
                    return invalid_config_value(
                        locale,
                        strings(
                            locale,
                            "journal.window.settings.vision.detail.extraction",
                            [("name", name)],
                        ),
                    );
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
                value: Map::new(),
            };
        };
        let mut changed = false;
        let mut changed_fields = Map::new();
        if let Some(value) = request.get("max_extractions") {
            if describe.get("max_extractions") != Some(value) {
                changed_fields.insert(
                    "max_extractions".to_owned(),
                    json!({"old": describe.get("max_extractions"), "new": value}),
                );
                changed = true;
            }
            describe.insert("max_extractions".to_owned(), value.clone());
        }
        if let Some(redact) = &redact {
            let value = Value::Array(redact.clone());
            if describe.get("redact") != Some(&value) {
                changed_fields.insert(
                    "redact".to_owned(),
                    json!({"old": describe.get("redact"), "new": value}),
                );
                changed = true;
            }
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
                        if let Some(old) = categories_target.remove(name) {
                            changed_fields.insert(
                                format!("categories.{name}"),
                                json!({"old": old, "new": Value::Null}),
                            );
                            changed = true;
                        }
                    }
                    Value::Object(values) if !values.is_empty() => {
                        if categories_target.get(name) != Some(value) {
                            changed_fields.insert(
                                format!("categories.{name}"),
                                json!({"old": categories_target.get(name), "new": value}),
                            );
                            changed = true;
                        }
                        categories_target.insert(name.clone(), value.clone());
                    }
                    _ => {}
                }
            }
        }
        JournalConfigMutation {
            changed,
            value: changed_fields,
        }
    });
    match result {
        Ok(transaction) => {
            if !transaction.value.is_empty()
                && solstone_core_facets::append_action_log(
                    &journal_root,
                    None,
                    "app",
                    "settings",
                    "vision_update",
                    json!({"changed_fields": transaction.value}),
                )
                .is_err()
            {
                return settings_operation_failed(locale);
            }
            get(locale, journal_root).await
        }
        Err(solstone_core_journal_config_write::ConfigMutationError::Lock(LockError::Timeout(
            _,
        ))) => config_busy(locale),
        Err(_) => settings_operation_failed(locale),
    }
}
