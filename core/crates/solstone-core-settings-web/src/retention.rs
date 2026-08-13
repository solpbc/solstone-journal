// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::BTreeMap, path::PathBuf};

use axum::{body::Bytes, response::Response};
use chrono::{Local, Utc};
use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockError, LockOptions, mutate_journal_config,
};

use crate::{
    http::{
        config_busy, invalid_config_value, invalid_request_value, json_response,
        missing_request_body, settings_operation_failed, settings_operation_failed_with_detail,
    },
    request_body::{JsonBody, json_body},
    retention_executor::{self, ExecutorError},
};

#[derive(Debug, Serialize)]
pub(crate) struct RetentionRule {
    anchor: String,
    period: Option<i64>,
    priority: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct RetentionPolicy {
    default_rule: RetentionRule,
    per_stream: Vec<(String, RetentionRule)>,
    minimum_age: i64,
    enabled: bool,
}

fn rule(mode: Option<&str>, days: Option<&Value>) -> RetentionRule {
    match mode {
        Some("days") => {
            let period = days
                .and_then(|value| {
                    value
                        .as_i64()
                        .or_else(|| value.as_bool().map(i64::from))
                        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                })
                .unwrap_or(0);
            RetentionRule {
                anchor: "captured".to_owned(),
                period: (period > 0).then_some(period),
                priority: 0,
            }
        }
        Some("processed") => RetentionRule {
            anchor: "processed".to_owned(),
            period: Some(0),
            priority: 0,
        },
        _ => RetentionRule {
            anchor: "captured".to_owned(),
            period: None,
            priority: 0,
        },
    }
}

pub(crate) fn policy_payload(retention: &Map<String, Value>) -> RetentionPolicy {
    let per_stream = retention
        .get("per_stream")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            value.as_object().map(|stream| {
                (
                    name.clone(),
                    rule(
                        stream.get("raw_media").and_then(Value::as_str),
                        stream.get("raw_media_days"),
                    ),
                )
            })
        })
        .collect();
    let minimum_age = retention
        .get("raw_media_minimum_days")
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_bool().map(i64::from))
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or(0)
        .max(0);
    RetentionPolicy {
        default_rule: rule(
            retention.get("raw_media").and_then(Value::as_str),
            retention.get("raw_media_days"),
        ),
        per_stream,
        minimum_age,
        enabled: true,
    }
}

pub(crate) fn policy_would_release(policy: &RetentionPolicy) -> bool {
    policy.default_rule.period.is_some()
        || policy
            .per_stream
            .iter()
            .any(|(_, rule)| rule.period.is_some())
}

pub(crate) fn read_policy_marks(receipt: &Value, stream: Option<&str>) -> BTreeMap<String, Value> {
    receipt
        .get("marks")
        .and_then(|value| value.get("marks"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, mark)| {
            mark.get("class").and_then(Value::as_str) == Some("policy_raw_release")
                && stream.is_none_or(|stream| {
                    mark.get("target")
                        .and_then(|target| target.get("stream"))
                        .and_then(Value::as_str)
                        == Some(stream)
                })
        })
        .map(|(id, mark)| (id.clone(), mark.clone()))
        .collect()
}

pub(crate) fn describe_mark_receipt(before: &Value, after: &Value, stream: Option<&str>) -> Value {
    let before = read_policy_marks(before, stream);
    let after_marks = read_policy_marks(after, stream);
    let mut held = Vec::new();
    let mut no_media = Vec::new();
    let mut not_eligible = Vec::new();
    for entry in after
        .get("plan")
        .and_then(|value| value.get("skipped_segments"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if stream.is_some_and(|stream| entry.get("stream").and_then(Value::as_str) != Some(stream))
        {
            continue;
        }
        match entry.get("reason").and_then(Value::as_str) {
            Some("held") => held.push(entry.clone()),
            Some("no_media") => no_media.push(entry.clone()),
            Some("policy") => not_eligible.push(entry.clone()),
            _ => {}
        }
    }
    let marked = after_marks
        .iter()
        .filter(|(id, _)| !before.contains_key(*id))
        .map(|(id, mark)| {
            let mut value = mark.as_object().cloned().unwrap_or_default();
            value.insert("id".to_owned(), Value::String(id.clone()));
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    json!({"marked":marked,"standing_total":after_marks.len(),"held":held,"no_media":no_media,"not_eligible":not_eligible,"unreadable_days":after.get("plan").and_then(|value| value.get("unreadable_days")).cloned().unwrap_or_else(|| json!([]))})
}

pub async fn update(journal_root: PathBuf, lock_options: LockOptions, body: Bytes) -> Response {
    let JsonBody::Value(Value::Object(request)) = json_body(body) else {
        return missing_request_body();
    };
    if request.is_empty() {
        return missing_request_body();
    }
    if let Some(mode) = request.get("raw_media")
        && !matches!(mode.as_str(), Some("keep" | "days" | "processed"))
    {
        return invalid_config_value(format!(
            "Invalid mode: {}",
            mode.as_str().unwrap_or_default()
        ));
    }
    if let Some(days) = request.get("raw_media_days")
        && !days.is_null()
        && (!days.is_i64() || days.as_i64().is_some_and(|days| days < 1))
        && !days.is_boolean()
    {
        return invalid_config_value("days must be a positive integer");
    }
    let mut per_stream = None;
    if let Some(value) = request.get("per_stream") {
        let Some(streams) = value.as_object() else {
            return invalid_config_value("per_stream must be an object");
        };
        let mut normalized = Map::new();
        for (name, value) in streams {
            let Some(stream) = value.as_object() else {
                continue;
            };
            if let Some(mode) = stream.get("raw_media")
                && !matches!(mode.as_str(), Some("keep" | "days" | "processed"))
            {
                return invalid_config_value(format!(
                    "Invalid mode for {name}: {}",
                    mode.as_str().unwrap_or_default()
                ));
            }
            if let Some(days) = stream.get("raw_media_days")
                && !days.is_null()
                && (!days.is_i64() || days.as_i64().is_some_and(|days| days < 1))
                && !days.is_boolean()
            {
                return invalid_config_value(format!("Invalid days for {name}"));
            }
            normalized.insert(name.clone(), value.clone());
        }
        per_stream = Some(normalized);
    }
    if let Some(value) = request.get("journal_logs") {
        let Some(logs) = value.as_object() else {
            return invalid_config_value("journal_logs must be an object");
        };
        if logs.get("enabled").is_some_and(|value| !value.is_boolean()) {
            return invalid_config_value("enabled must be a boolean");
        }
        if let Some(days) = logs.get("days")
            && (!days.is_i64() || days.is_boolean() || days.as_i64().is_some_and(|days| days < 1))
        {
            return invalid_config_value("days must be a positive integer");
        }
    }
    match mutate_journal_config(&journal_root, lock_options, |config| {
        let old = config
            .get("retention")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let retention = config
            .entry("retention".to_owned())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("retention object");
        let mut changed = Map::new();
        for field in ["raw_media", "raw_media_days"] {
            if let Some(value) = request.get(field) {
                if retention.get(field) != Some(value) {
                    changed.insert(
                        field.to_owned(),
                        json!({"old":retention.get(field),"new":value}),
                    );
                }
                retention.insert(field.to_owned(), value.clone());
            }
        }
        if let Some(value) = &per_stream {
            let value = Value::Object(value.clone());
            if old.get("per_stream") != Some(&value) {
                changed.insert(
                    "per_stream".to_owned(),
                    json!({"old":old.get("per_stream"),"new":value}),
                );
            }
            retention.insert("per_stream".to_owned(), value);
        }
        if let Some(logs) = request.get("journal_logs").and_then(Value::as_object) {
            let current = retention.get("journal_logs").and_then(Value::as_object);
            let mut next = Map::new();
            next.insert(
                "enabled".to_owned(),
                current
                    .and_then(|value| value.get("enabled"))
                    .cloned()
                    .unwrap_or_else(|| json!(true)),
            );
            next.insert(
                "days".to_owned(),
                current
                    .and_then(|value| value.get("days"))
                    .cloned()
                    .unwrap_or_else(|| json!(30)),
            );
            for (key, value) in logs {
                next.insert(key.clone(), value.clone());
            }
            let value = Value::Object(next);
            if retention.get("journal_logs") != Some(&value) {
                changed.insert(
                    "journal_logs".to_owned(),
                    json!({"old":retention.get("journal_logs"),"new":value}),
                );
            }
            retention.insert("journal_logs".to_owned(), value);
        }
        JournalConfigMutation {
            changed: !changed.is_empty(),
            value: (changed, Value::Object(retention.clone())),
        }
    }) {
        Ok(transaction) => {
            if !transaction.value.0.is_empty()
                && solstone_core_facets::append_action_log(
                    &journal_root,
                    None,
                    "app",
                    "settings",
                    "retention_update",
                    json!({"changed_fields":transaction.value.0}),
                )
                .is_err()
            {
                return settings_operation_failed();
            }
            json_response(json!({"success":true,"retention":transaction.value.1}))
        }
        Err(solstone_core_journal_config_write::ConfigMutationError::Lock(LockError::Timeout(
            _,
        ))) => config_busy(),
        Err(_) => settings_operation_failed(),
    }
}

pub async fn purge(journal_root: PathBuf, body: Bytes) -> Response {
    let request = match json_body(body) {
        JsonBody::Value(Value::Object(value)) => value,
        JsonBody::Value(_) => return invalid_request_value("request body must be an object"),
        JsonBody::Missing | JsonBody::Invalid => Map::new(),
    };
    let stream = request
        .get("stream_filter")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let policy = policy_payload(
        config
            .get("retention")
            .and_then(Value::as_object)
            .unwrap_or(&Map::new()),
    );
    let before = match retention_executor::marks(journal_root.clone()).await {
        Ok(value) => value,
        Err(error) => {
            return settings_operation_failed_with_detail(format!(
                "could not build the list: {error}"
            ));
        }
    };
    if !policy_would_release(&policy) {
        return json_response(
            json!({"marked":[],"standing_total":read_policy_marks(&before,stream).len(),"held":[],"no_media":[],"not_eligible":[],"unreadable_days":[]}),
        );
    }
    let now = Utc::now();
    let policy = serde_json::to_string(&policy).expect("policy JSON");
    match retention_executor::mark(
        journal_root.clone(),
        Local::now().format("%Y-%m-%d").to_string(),
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        policy,
    )
    .await
    {
        Ok(after) => {
            let response = describe_mark_receipt(&before, &after, stream);
            let count = response["marked"].as_array().map_or(0, Vec::len);
            if solstone_core_facets::append_action_log(
                &journal_root,
                None,
                "app",
                "settings",
                "retention_mark",
                json!({"stream_filter":stream,"new_marks":count}),
            )
            .is_err()
            {
                return settings_operation_failed();
            }
            json_response(response)
        }
        Err(error) => {
            settings_operation_failed_with_detail(format!("could not build the list: {error}"))
        }
    }
}

pub async fn prune_logs(journal_root: PathBuf, body: Bytes) -> Response {
    let request = match json_body(body) {
        JsonBody::Value(Value::Object(value)) => value,
        JsonBody::Value(_) => return invalid_config_value("request body must be an object"),
        JsonBody::Missing | JsonBody::Invalid => Map::new(),
    };
    let days = request.get("days");
    if let Some(days) = days
        && (!days.is_i64() || days.is_boolean() || days.as_i64().is_some_and(|days| days < 1))
    {
        return invalid_config_value("days must be a positive integer");
    }
    let config = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let effective = days
        .and_then(Value::as_i64)
        .or_else(|| {
            config
                .get("retention")
                .and_then(|value| value.get("journal_logs"))
                .and_then(|value| value.get("days"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(30);
    let dry_run = request
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    match retention_executor::prune_logs(
        journal_root,
        Local::now().format("%Y-%m-%d").to_string(),
        effective.to_string(),
        dry_run,
    )
    .await
    {
        Ok(receipt) => json_response(receipt),
        Err(ExecutorError::Refused(_)) => settings_operation_failed(),
        Err(ExecutorError::Unavailable(_)) => settings_operation_failed(),
    }
}
