// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{collections::BTreeMap, path::PathBuf};

use axum::{body::Bytes, response::Response};
use chrono::{Days, Local, Utc};
use serde_json::{Map, Value, json};
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockError, LockOptions, mutate_journal_config,
};
use solstone_core_retention::policy::{policy_from_retention, policy_would_release};

use crate::{
    http::{
        config_busy, invalid_config_value, invalid_request_value, json_response,
        missing_request_body, settings_operation_failed, settings_operation_failed_with_detail,
    },
    request_body::{JsonBody, json_body},
    retention_executor::{self, ExecutorError},
};

struct LogRetentionConfig {
    enabled: bool,
    days: i64,
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn python_int(value: &Value) -> Option<i64> {
    if value.is_boolean() {
        return None;
    }
    value
        .as_i64()
        .or_else(|| {
            value.as_f64().and_then(|value| {
                (value.is_finite() && value >= i64::MIN as f64 && value <= i64::MAX as f64)
                    .then(|| value.trunc() as i64)
            })
        })
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

fn log_retention_config(config: &Map<String, Value>) -> Result<LogRetentionConfig, ()> {
    let retention = config
        .get("retention")
        .filter(|value| python_truthy(value))
        .map(|value| value.as_object().ok_or(()))
        .transpose()?;
    let journal_logs = retention
        .and_then(|retention| retention.get("journal_logs"))
        .filter(|value| python_truthy(value))
        .map(|value| value.as_object().ok_or(()))
        .transpose()?;
    let enabled = journal_logs
        .and_then(|journal_logs| journal_logs.get("enabled"))
        .map_or(Ok(true), |value| value.as_bool().ok_or(()))?;
    let days = journal_logs
        .and_then(|journal_logs| journal_logs.get("days"))
        .map_or(Ok(30), |value| python_int(value).ok_or(()))?;
    (days >= 1)
        .then_some(LogRetentionConfig { enabled, days })
        .ok_or(())
}

fn cutoff_day(days: i64) -> Option<String> {
    Local::now()
        .date_naive()
        .checked_sub_days(Days::new(days.try_into().ok()?))
        .map(|day| day.format("%Y%m%d").to_string())
}

fn count(value: Option<&Value>) -> i64 {
    value.and_then(Value::as_i64).unwrap_or(0)
}

fn reason(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
        .to_owned()
}

fn human_bytes(bytes: i64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value.abs() < 1024.0 {
            return if unit == "B" {
                format!("{} B", value as i64)
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1024.0;
    }
    format!("{value:.1} PB")
}

fn compaction_result(stats: Option<&Map<String, Value>>, dry_run: bool) -> Value {
    let stats = stats.cloned().unwrap_or_default();
    let bytes_freed = if !dry_run && !python_truthy(stats.get("rewritten").unwrap_or(&Value::Null))
    {
        0
    } else {
        count(stats.get("bytes_before")) - count(stats.get("bytes_after"))
    };
    let errors = stats
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|error| reason(error.get("reason")))
        .collect::<Vec<_>>();
    json!({
        "exists": python_truthy(stats.get("exists").unwrap_or(&Value::Null)),
        "lines_total": count(stats.get("lines_total")),
        "lines_kept": count(stats.get("lines_kept")),
        "lines_removed": count(stats.get("lines_dropped")),
        "unparseable_lines_kept": count(stats.get("undateable_kept")),
        "bytes_freed": bytes_freed,
        "rewritten": python_truthy(stats.get("rewritten").unwrap_or(&Value::Null)),
        "errors": errors,
    })
}

fn empty_prune_result(enabled: bool, dry_run: Value, days: i64) -> Option<Value> {
    Some(json!({
        "enabled": enabled,
        "dry_run": dry_run,
        "days": days,
        "cutoff_day": cutoff_day(days)?,
        "files_deleted": 0,
        "dirs_deleted": 0,
        "bytes_freed": 0,
        "bytes_freed_human": "0 B",
        "by_class": {},
        "by_day": {},
        "retention_log": {},
        "errors": [],
        "audit_written": false,
        "partial_error": false,
    }))
}

fn prune_result_from_receipt(receipt: &Value, dry_run: Value, days: i64) -> Option<Value> {
    let plan = receipt
        .get("detail")
        .and_then(Value::as_object)
        .map_or_else(|| receipt.get("plan"), |detail| detail.get("plan"))?
        .as_object()?;
    let dry_run_truthy = python_truthy(&dry_run);
    let prefix = if dry_run_truthy { "planned" } else { "removed" };
    let mut by_class = Map::new();
    for (name, stats) in plan
        .get("by_class")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let Some(stats) = stats.as_object() else {
            continue;
        };
        let errors = stats
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|error| Value::String(reason(error.get("reason"))))
            .collect::<Vec<_>>();
        by_class.insert(
            name.clone(),
            json!({
                "files_deleted": count(stats.get(&format!("{prefix}_files"))),
                "bytes_freed": count(stats.get(&format!("{prefix}_bytes"))),
                "dirs_deleted": count(stats.get(&format!("{prefix}_dirs"))),
                "skipped": count(stats.get("skipped")),
                "errors": errors,
            }),
        );
    }
    let mut by_day = Map::new();
    for (day, stats) in plan
        .get("by_day")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let Some(stats) = stats.as_object() else {
            continue;
        };
        by_day.insert(
            day.clone(),
            json!({
                "files_deleted": count(stats.get(&format!("{prefix}_files"))),
                "bytes_freed": count(stats.get(&format!("{prefix}_bytes"))),
                "dirs_deleted": count(stats.get(&format!("{prefix}_dirs"))),
            }),
        );
    }
    let mut errors = plan
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|error| {
            let reason = reason(error.get("reason"));
            json!({
                "class": error.get("class").cloned().unwrap_or(Value::Null),
                "path": error.get("path").cloned().unwrap_or(Value::Null),
                "day": error.get("day").cloned().unwrap_or(Value::Null),
                "reason": reason,
                "message": reason,
                "hint": error.get("hint").cloned().unwrap_or(Value::Null),
            })
        })
        .collect::<Vec<_>>();
    if let Some(oplogs) = plan.get("oplogs").and_then(Value::as_object) {
        let mut files_deleted = 0i64;
        let mut bytes_freed = 0i64;
        let mut oplog_errors = Vec::new();
        for target in oplogs
            .get("prunable")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let bytes = count(target.get("bytes"));
            let removed =
                dry_run_truthy || python_truthy(target.get("removed").unwrap_or(&Value::Null));
            if removed {
                files_deleted = files_deleted.saturating_add(1);
                bytes_freed = bytes_freed.saturating_add(bytes);
            }
            let Some(reason) = target.get("error").and_then(Value::as_str) else {
                continue;
            };
            let path = target.get("path").cloned().unwrap_or(Value::Null);
            let day = target.get("day").cloned().unwrap_or(Value::Null);
            let error = json!({
                "class": "oplog_retention",
                "path": path,
                "day": day,
                "reason": reason,
                "message": reason,
                "hint": Value::Null,
            });
            oplog_errors.push(Value::String(reason.to_owned()));
            errors.push(error);
        }
        by_class.insert(
            "oplog_retention".to_owned(),
            json!({
                "files_deleted": files_deleted,
                "bytes_freed": bytes_freed,
                "dirs_deleted": 0,
                "skipped": 0,
                "errors": oplog_errors,
            }),
        );
    }
    let partial_error = !errors.is_empty();
    let compactions = plan.get("compactions").and_then(Value::as_object);
    let retention_log = compaction_result(
        compactions
            .and_then(|compactions| compactions.get("retention_log"))
            .and_then(Value::as_object),
        dry_run_truthy,
    );
    let files_deleted = by_class
        .values()
        .map(|stats| count(stats.get("files_deleted")))
        .sum::<i64>();
    let dirs_deleted = by_class
        .values()
        .map(|stats| count(stats.get("dirs_deleted")))
        .sum::<i64>();
    let bytes_freed = by_class
        .values()
        .map(|stats| count(stats.get("bytes_freed")))
        .sum::<i64>()
        + count(retention_log.get("bytes_freed"));
    Some(json!({
        "enabled": true,
        "dry_run": dry_run,
        "days": days,
        "cutoff_day": cutoff_day(days)?,
        "files_deleted": files_deleted,
        "dirs_deleted": dirs_deleted,
        "bytes_freed": bytes_freed,
        "bytes_freed_human": human_bytes(bytes_freed),
        "by_class": by_class,
        "by_day": by_day,
        "retention_log": retention_log,
        "errors": errors,
        "audit_written": false,
        "partial_error": partial_error,
    }))
}

fn prune_response(journal_root: &std::path::Path, result: Value, dry_run: bool) -> Response {
    if !dry_run
        && solstone_core_facets::append_action_log(
            journal_root,
            None,
            "app",
            "settings",
            "prune_logs",
            json!({
                "days": result["days"],
                "files_deleted": result["files_deleted"],
                "dirs_deleted": result["dirs_deleted"],
            }),
        )
        .is_err()
    {
        settings_operation_failed()
    } else {
        json_response(result)
    }
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
    let policy = policy_from_retention(
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
    let Ok(log_retention) = log_retention_config(&config) else {
        return settings_operation_failed();
    };
    let effective = days.and_then(Value::as_i64).unwrap_or(log_retention.days);
    let dry_run = request
        .get("dry_run")
        .cloned()
        .unwrap_or_else(|| json!(true));
    let dry_run_truthy = python_truthy(&dry_run);
    if !log_retention.enabled {
        return empty_prune_result(false, dry_run, effective)
            .map_or_else(settings_operation_failed, json_response);
    }
    match retention_executor::prune_logs(
        journal_root.clone(),
        Local::now().format("%Y-%m-%d").to_string(),
        effective.to_string(),
        dry_run_truthy,
    )
    .await
    {
        Ok(receipt) => prune_result_from_receipt(&receipt, dry_run.clone(), effective)
            .map_or_else(settings_operation_failed, |result| {
                prune_response(&journal_root, result, dry_run_truthy)
            }),
        // Reference contract: a refusal still carries a useful prune result.
        // Do not unify this with executor unavailability below.
        Err(ExecutorError::Refused(refused)) => {
            prune_result_from_receipt(&refused.0, dry_run, effective)
                .map_or_else(settings_operation_failed, |result| {
                    prune_response(&journal_root, result, dry_run_truthy)
                })
        }
        // Reference contract: an unavailable executor is the generic envelope.
        // Do not unify this with the receipt-bearing refusal arm above.
        Err(ExecutorError::Unavailable(_)) => settings_operation_failed(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::prune_result_from_receipt;

    #[test]
    fn oplog_receipt_entries_flow_through_the_generic_class_totals() {
        let receipt = json!({
            "plan": {
                "by_class": {},
                "by_day": {},
                "errors": [],
                "compactions": {},
                "oplogs": {
                    "prunable": [{
                        "day": "20260101",
                        "leaf": "oplog--source--run--20260101T000000Z--id.log",
                        "path": "chronicle/20260101/health/oplog--source--run--20260101T000000Z--id.log",
                        "bytes": 42,
                        "removed": false,
                        "error": null,
                    }],
                    "retained": [],
                    "bytes": 42,
                },
            },
        });
        let preview = prune_result_from_receipt(&receipt, json!(true), 7).expect("preview");
        assert_eq!(preview["files_deleted"], 1);
        assert_eq!(preview["bytes_freed"], 42);
        assert_eq!(preview["by_class"]["oplog_retention"]["files_deleted"], 1);
        assert!(preview.get("root_task_log").is_none());

        let executed_receipt = json!({
            "detail": {
                "plan": {
                    "by_class": {},
                    "by_day": {},
                    "errors": [],
                    "compactions": {},
                    "oplogs": {
                        "prunable": [
                            {
                                "day": "20260101",
                                "leaf": "removed.log",
                                "path": "chronicle/20260101/health/removed.log",
                                "bytes": 42,
                                "removed": true,
                                "error": null,
                            },
                            {
                                "day": "20260102",
                                "leaf": "raced.log",
                                "path": "chronicle/20260102/health/raced.log",
                                "bytes": 24,
                                "removed": false,
                                "error": "the oplog changed before removal",
                            }
                        ],
                        "retained": [],
                        "bytes": 66,
                    },
                },
            },
        });
        let executed =
            prune_result_from_receipt(&executed_receipt, json!(false), 7).expect("execution");
        assert_eq!(executed["files_deleted"], 1);
        assert_eq!(executed["bytes_freed"], 42);
        assert_eq!(
            executed["by_class"]["oplog_retention"]["errors"],
            json!(["the oplog changed before removal"])
        );
        assert_eq!(executed["partial_error"], true);
    }
}
