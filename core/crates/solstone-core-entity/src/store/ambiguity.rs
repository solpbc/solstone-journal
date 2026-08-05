// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_io::AtomicWriteError;
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::LockError;
use solstone_core_journal_io::LockOptions;
use solstone_core_journal_io::MalformedPolicy;
use solstone_core_journal_io::hold_lock;
use solstone_core_journal_io::read_text;
use solstone_core_journal_io::write_text;

use crate::{EntityTrustLockError, hold_entity_trust_lock};

use super::error::EntityStoreError;
use super::paths::ambiguities_path;
use super::write::{EntityWriteError, mutate_ambiguities, origin_key, serialize_ambiguity_rows};

const AMBIGUITY_SCHEMA_VERSION: u64 = 1;

/// The ambiguity identifiers rewritten by a facet-directory rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityAmbiguityRescopeReport {
    pub rewritten_ambiguity_ids: Vec<String>,
}

/// Durable ambiguity rows changed or removed while deleting one entity's references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityAmbiguityRemovalReport {
    pub rewritten_ambiguity_ids: Vec<String>,
    pub removed_ambiguity_ids: Vec<String>,
}

/// Failure while rescoping facet references in durable ambiguity rows.
#[derive(Debug)]
pub enum EntityAmbiguityRescopeError {
    TrustLock(EntityTrustLockError),
    Read(EntityStoreError),
    Lock(LockError),
    Write(AtomicWriteError),
    InvalidRow {
        ambiguity_id: Option<String>,
        detail: String,
    },
}

impl fmt::Display for EntityAmbiguityRescopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::InvalidRow {
                ambiguity_id,
                detail,
            } => write!(
                formatter,
                "invalid ambiguity row {}: {detail}",
                ambiguity_id.as_deref().unwrap_or("<missing id>")
            ),
        }
    }
}

impl Error for EntityAmbiguityRescopeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::InvalidRow { .. } => None,
        }
    }
}

/// Rescope every strict ambiguity row that references `old_facet`.
pub fn rescope_facet_ambiguities(
    journal_root: &Path,
    old_facet: &str,
    new_facet: &str,
) -> Result<EntityAmbiguityRescopeReport, EntityAmbiguityRescopeError> {
    let _trust =
        hold_entity_trust_lock(journal_root).map_err(EntityAmbiguityRescopeError::TrustLock)?;
    let path = ambiguities_path(journal_root).map_err(EntityAmbiguityRescopeError::Read)?;
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(EntityAmbiguityRescopeError::Lock)?;
    let mut rows = read_ambiguities(journal_root, MalformedPolicy::Raise)
        .map_err(EntityAmbiguityRescopeError::Read)?;
    let mut rewritten_ambiguity_ids = Vec::new();

    for row in &mut rows {
        let ambiguity_id = row
            .get("ambiguity_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let object =
            row.as_object_mut()
                .ok_or_else(|| EntityAmbiguityRescopeError::InvalidRow {
                    ambiguity_id: ambiguity_id.clone(),
                    detail: "row is not an object".to_owned(),
                })?;
        if !row_references_facet(object, old_facet) {
            continue;
        }

        if let Some(scope) = object.get_mut("scope").and_then(Value::as_object_mut)
            && scope.get("facet").and_then(Value::as_str) == Some(old_facet)
        {
            scope.insert("facet".to_owned(), Value::String(new_facet.to_owned()));
        }
        if let Some(origins) = object.get_mut("origins").and_then(Value::as_array_mut) {
            for origin in origins.iter_mut() {
                rescope_origin(origin, old_facet, new_facet);
            }
            let keys = origins
                .iter()
                .map(|origin| {
                    origin_key(origin).map(Value::String).map_err(|error| {
                        EntityAmbiguityRescopeError::InvalidRow {
                            ambiguity_id: ambiguity_id.clone(),
                            detail: error.to_string(),
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            object.insert("origin_keys".to_owned(), Value::Array(keys));
        }
        if let Some(priors) = object
            .get_mut("audit")
            .and_then(Value::as_object_mut)
            .and_then(|audit| audit.get_mut("prior_choices"))
            .and_then(Value::as_array_mut)
        {
            for prior in priors {
                if let Some(origin) = prior
                    .as_object_mut()
                    .and_then(|prior| prior.get_mut("replaced_by_origin"))
                {
                    rescope_origin(origin, old_facet, new_facet);
                }
            }
        }

        let scope_key = object.get("scope").and_then(scope_key).ok_or_else(|| {
            EntityAmbiguityRescopeError::InvalidRow {
                ambiguity_id: ambiguity_id.clone(),
                detail: "invalid scope".to_owned(),
            }
        })?;
        let normalized_query =
            non_empty_string(object.get("normalized_query")).ok_or_else(|| {
                EntityAmbiguityRescopeError::InvalidRow {
                    ambiguity_id: ambiguity_id.clone(),
                    detail: "missing normalized_query".to_owned(),
                }
            })?;
        let rewritten_id = crate::ambiguity_id(&format!("{scope_key}|{normalized_query}"));
        object.insert(
            "ambiguity_id".to_owned(),
            Value::String(rewritten_id.clone()),
        );
        validate_row(object).map_err(|detail| EntityAmbiguityRescopeError::InvalidRow {
            ambiguity_id,
            detail: detail.to_owned(),
        })?;
        rewritten_ambiguity_ids.push(rewritten_id);
    }

    let contents = serialize_ambiguity_rows(&rows).map_err(EntityAmbiguityRescopeError::Write)?;
    write_text(&path, &contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(EntityAmbiguityRescopeError::Write)?;
    Ok(EntityAmbiguityRescopeReport {
        rewritten_ambiguity_ids,
    })
}

/// Remove one entity from ambiguity candidates and reopen its resolved choices.
pub fn remove_entity_ambiguity_references(
    journal_root: &Path,
    entity_id: &str,
) -> Result<EntityAmbiguityRemovalReport, EntityWriteError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    let mut report = EntityAmbiguityRemovalReport {
        rewritten_ambiguity_ids: Vec::new(),
        removed_ambiguity_ids: Vec::new(),
    };

    mutate_ambiguities(journal_root, |rows| {
        rows.retain_mut(|row| {
            let object = row
                .as_object_mut()
                .expect("strict ambiguity reader returns objects");
            let ambiguity_id = object
                .get("ambiguity_id")
                .and_then(Value::as_str)
                .expect("strict ambiguity rows have ids")
                .to_owned();
            let candidates = object
                .get_mut("ranked_candidates")
                .and_then(Value::as_array_mut)
                .expect("strict ambiguity rows have candidates");
            let candidates_before = candidates.len();
            candidates
                .retain(|candidate| candidate.get("id").and_then(Value::as_str) != Some(entity_id));
            let candidates_changed = candidates.len() != candidates_before;
            if candidates.is_empty() {
                report.removed_ambiguity_ids.push(ambiguity_id);
                return false;
            }

            let resolved_changed =
                object.get("resolved_entity_id").and_then(Value::as_str) == Some(entity_id);
            if resolved_changed {
                object.insert("status".to_owned(), Value::String("open".to_owned()));
                object.remove("resolved_entity_id");
                object.remove("resolved_at");
            }
            if candidates_changed || resolved_changed {
                report.rewritten_ambiguity_ids.push(ambiguity_id);
            }
            true
        });
        Ok(Value::Null)
    })?;

    Ok(report)
}

fn row_references_facet(row: &Map<String, Value>, facet: &str) -> bool {
    row.get("scope")
        .and_then(Value::as_object)
        .and_then(|scope| scope.get("facet"))
        .and_then(Value::as_str)
        == Some(facet)
        || row
            .get("origins")
            .and_then(Value::as_array)
            .is_some_and(|origins| origins.iter().any(|origin| origin_facet_is(origin, facet)))
        || row
            .get("audit")
            .and_then(Value::as_object)
            .and_then(|audit| audit.get("prior_choices"))
            .and_then(Value::as_array)
            .is_some_and(|priors| {
                priors.iter().any(|prior| {
                    prior
                        .as_object()
                        .and_then(|prior| prior.get("replaced_by_origin"))
                        .is_some_and(|origin| origin_facet_is(origin, facet))
                })
            })
}

fn origin_facet_is(origin: &Value, facet: &str) -> bool {
    origin
        .as_object()
        .and_then(|origin| origin.get("facet"))
        .and_then(Value::as_str)
        == Some(facet)
}

fn rescope_origin(origin: &mut Value, old_facet: &str, new_facet: &str) {
    let Some(origin) = origin.as_object_mut() else {
        return;
    };
    if origin.get("facet").and_then(Value::as_str) == Some(old_facet) {
        origin.insert("facet".to_owned(), Value::String(new_facet.to_owned()));
    }
    if let Some(path) = origin.get("path").and_then(Value::as_str) {
        let old_segment = format!("facets/{old_facet}/");
        if path.contains(&old_segment) {
            origin.insert(
                "path".to_owned(),
                Value::String(path.replace(&old_segment, &format!("facets/{new_facet}/"))),
            );
        }
    }
}

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

pub(super) fn scope_key(scope: &Value) -> Option<String> {
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
