// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_io::{MalformedPolicy, read_text};

use super::error::EntityStoreError;
use super::paths::ambiguities_path;

const AMBIGUITY_SCHEMA_VERSION: u64 = 1;

/// Read durable ambiguity rows with Python-compatible strictness behavior.
pub fn read_ambiguities(
    journal_root: &Path,
    policy: MalformedPolicy,
) -> Result<Vec<Value>, EntityStoreError> {
    let path = ambiguities_path(journal_root)?;
    let contents = read_text(&path, String::new())?;
    let mut rows = Vec::new();

    for (index, bytes) in contents.as_bytes().split(|byte| *byte == b'\n').enumerate() {
        let line = index + 1;
        let raw = std::str::from_utf8(bytes).expect("split valid UTF-8 text on ASCII newline");
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let value = match serde_json::from_str::<Value>(raw) {
            Ok(value) => value,
            Err(error) => {
                handle_malformed(&path, line, policy, &error)?;
                continue;
            }
        };
        let Some(object) = value.as_object() else {
            handle_non_object(
                &path,
                line,
                format!("expected object, got {}", python_type_name(&value)),
                policy,
            )?;
            continue;
        };
        if policy == MalformedPolicy::Raise {
            validate_row(object).map_err(|detail| invalid_row(&path, line, detail))?;
        }
        rows.push(value);
    }

    Ok(rows)
}

/// Strictly load one matching resolved ambiguity choice.
pub fn load_resolved_ambiguity_choice(
    journal_root: &Path,
    scope: &Value,
    normalized_query: &str,
) -> Result<Option<Value>, EntityStoreError> {
    let Some(target_key) = scope_key(scope) else {
        return Ok(None);
    };
    for row in read_ambiguities(journal_root, MalformedPolicy::Raise)? {
        if row.get("normalized_query").and_then(Value::as_str) != Some(normalized_query) {
            continue;
        }
        if row.get("scope").and_then(scope_key).as_deref() != Some(&target_key) {
            continue;
        }
        if row.get("status").and_then(Value::as_str) == Some("resolved") {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

fn handle_malformed(
    path: &Path,
    line: usize,
    policy: MalformedPolicy,
    error: &serde_json::Error,
) -> Result<(), EntityStoreError> {
    match policy {
        MalformedPolicy::Raise => Err(invalid_row(path, line, "malformed JSON")),
        MalformedPolicy::Skip => Ok(()),
        MalformedPolicy::WarnAndSkip => {
            log::warn!(
                "entity ambiguities: malformed JSONL line {} in {}: {}",
                line,
                path.display(),
                error
            );
            Ok(())
        }
    }
}

fn handle_non_object(
    path: &Path,
    line: usize,
    detail: String,
    policy: MalformedPolicy,
) -> Result<(), EntityStoreError> {
    match policy {
        MalformedPolicy::Raise => Err(invalid_row(path, line, detail)),
        MalformedPolicy::Skip => Ok(()),
        MalformedPolicy::WarnAndSkip => {
            log::warn!(
                "entity ambiguities: non-object JSONL line {} in {} ({})",
                line,
                path.display(),
                detail
            );
            Ok(())
        }
    }
}

fn invalid_row(path: &Path, line: usize, detail: impl Into<String>) -> EntityStoreError {
    EntityStoreError::AmbiguityInvalidRow {
        path: path.to_path_buf(),
        line,
        detail: detail.into(),
    }
}

pub(super) fn validate_row(row: &Map<String, Value>) -> Result<(), &'static str> {
    let schema_version = row.get("schema_version");
    if !is_integer(schema_version)
        || schema_version.and_then(Value::as_u64) != Some(AMBIGUITY_SCHEMA_VERSION)
    {
        return Err("unsupported or missing schema_version");
    }

    let ambiguity_id = non_empty_string(row.get("ambiguity_id")).ok_or("missing ambiguity_id")?;
    let scope = row
        .get("scope")
        .and_then(Value::as_object)
        .ok_or("scope is not an object")?;
    let scope_key = match scope.get("kind").and_then(Value::as_str) {
        Some("journal") => {
            if !scope.get("facet").is_none_or(Value::is_null) {
                return Err("journal scope includes a facet");
            }
            "journal".to_owned()
        }
        Some("facet") => {
            let facet = non_empty_string(scope.get("facet")).ok_or("facet scope has no facet")?;
            format!("facet:{facet}")
        }
        _ => return Err("scope has an unknown kind"),
    };

    let normalized_query =
        non_empty_string(row.get("normalized_query")).ok_or("missing normalized_query")?;
    for field in ["original_query", "latest_query", "first_seen", "last_seen"] {
        if non_empty_string(row.get(field)).is_none() {
            return Err(match field {
                "original_query" => "missing original_query",
                "latest_query" => "missing latest_query",
                "first_seen" => "missing first_seen",
                "last_seen" => "missing last_seen",
                _ => unreachable!(),
            });
        }
    }

    let expected_id = crate::ambiguity_id(&format!("{scope_key}|{normalized_query}"));
    if ambiguity_id != expected_id {
        return Err("ambiguity_id does not match scope/query");
    }

    let observed_tier = row.get("observed_tier");
    if !is_integer(observed_tier) || !matches!(observed_tier.and_then(Value::as_i64), Some(5..=8)) {
        return Err("observed_tier is not a low-confidence tier");
    }
    let observed_tier = observed_tier
        .and_then(Value::as_i64)
        .expect("checked integer tier");

    let status = row.get("status").and_then(Value::as_str);
    if !matches!(status, Some("open" | "resolved")) {
        return Err("status is not open or resolved");
    }
    let resolved_entity_id = row.get("resolved_entity_id");
    let resolved_at = row.get("resolved_at");
    if status == Some("resolved") {
        if non_empty_string(resolved_entity_id).is_none() {
            return Err("resolved row has no entity choice");
        }
        if non_empty_string(resolved_at).is_none() {
            return Err("resolved row has no timestamp");
        }
    } else if !resolved_entity_id.is_none_or(Value::is_null)
        || !resolved_at.is_none_or(Value::is_null)
    {
        return Err("open row contains a resolved choice");
    }

    let candidates = row.get("ranked_candidates").and_then(Value::as_array);
    let Some(candidates) = candidates.filter(|candidates| !candidates.is_empty()) else {
        return Err("ranked_candidates is not a populated list");
    };
    for candidate in candidates {
        let candidate = candidate
            .as_object()
            .ok_or("ranked candidate is not an object")?;
        if non_empty_string(candidate.get("id")).is_none() {
            return Err("ranked candidate has no id");
        }
        if non_empty_string(candidate.get("name")).is_none() {
            return Err("ranked candidate has no name");
        }
        if !is_integer(candidate.get("tier"))
            || candidate.get("tier").and_then(Value::as_i64) != Some(observed_tier)
        {
            return Err("ranked candidate has an invalid tier");
        }
        if !matches!(candidate.get("score"), Some(Value::Number(_))) {
            return Err("ranked candidate has an invalid score");
        }
    }

    let origins = row.get("origins").and_then(Value::as_array);
    let origin_keys = row.get("origin_keys").and_then(Value::as_array);
    let (Some(origins), Some(origin_keys)) = (origins, origin_keys) else {
        return Err("origins/origin_keys is not a list");
    };
    if origins.len() != origin_keys.len() || origins.is_empty() {
        return Err("origins and origin_keys are inconsistent");
    }
    for origin in origins {
        let origin = origin.as_object().ok_or("origin is not an object")?;
        if non_empty_string(origin.get("lane")).is_none() {
            return Err("origin has no lane");
        }
        if origin.values().any(|value| !value.is_string()) {
            return Err("origin contains a non-string value");
        }
    }
    if origin_keys
        .iter()
        .any(|key| non_empty_string(Some(key)).is_none())
    {
        return Err("origin_keys contains an invalid key");
    }

    if !is_integer(row.get("occurrence_count"))
        || row
            .get("occurrence_count")
            .and_then(Value::as_i64)
            .is_none_or(|count| count < 1)
    {
        return Err("invalid occurrence_count");
    }
    let audit = row
        .get("audit")
        .and_then(Value::as_object)
        .ok_or("invalid audit.prior_choices")?;
    let priors = audit
        .get("prior_choices")
        .and_then(Value::as_array)
        .ok_or("invalid audit.prior_choices")?;
    for prior in priors {
        let prior = prior.as_object().ok_or("prior choice is not an object")?;
        for field in ["resolved_entity_id", "resolved_at", "replaced_at"] {
            if non_empty_string(prior.get(field)).is_none() {
                return Err(match field {
                    "resolved_entity_id" => "prior choice has no resolved_entity_id",
                    "resolved_at" => "prior choice has no resolved_at",
                    "replaced_at" => "prior choice has no replaced_at",
                    _ => unreachable!(),
                });
            }
        }
        if let Some(replaced_by) = prior.get("replaced_by_origin")
            && !replaced_by.is_null()
            && (replaced_by.as_object().is_none()
                || non_empty_string(replaced_by.get("lane")).is_none())
        {
            return Err("invalid prior-choice origin");
        }
    }
    Ok(())
}

fn scope_key(scope: &Value) -> Option<String> {
    let scope = scope.as_object()?;
    match scope.get("kind").and_then(Value::as_str) {
        Some("journal") if scope.get("facet").is_none_or(Value::is_null) => {
            Some("journal".to_owned())
        }
        Some("facet") => non_empty_string(scope.get("facet")).map(|facet| format!("facet:{facet}")),
        _ => None,
    }
}

fn is_integer(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Number(number)) if number.as_i64().is_some() || number.as_u64().is_some())
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.as_i64().is_some() || number.as_u64().is_some() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}
