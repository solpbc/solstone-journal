// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use caseless::default_case_fold_str;
use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_entity_matching::{entity_slug, normalize_resolution_query};
use solstone_core_journal_io::{AtomicWriteOptions, path_lexists, read_text, write_text};

use crate::hold_facet_trust_lock;

use super::error::{
    FacetEntityWriteError, FacetStoreError, FacetWriteError, ObservationLookupError,
    ObservationWriteError,
};
use super::facet_entities::list_scoped_facet_entities;
use super::map::list_facet_entity_directories;
use super::paths::facet_entity_observations_path;

const OBSERVATION_RETRY_ATTEMPTS: usize = 3;

/// Result of resolving a name-or-id observation query to a relationship directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservationEntityResolution {
    Resolved { entity_dir: String },
    NoSuchEntity,
}

/// Result of looking up observations through a name-or-id query.
#[derive(Debug, Clone, PartialEq)]
pub enum ObservationLookup {
    Unresolvable,
    Resolved {
        entity_dir: String,
        observations: Vec<Value>,
    },
}

/// Counts returned after applying observation operations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObservationOperationCounts {
    pub update: usize,
    pub add: usize,
    pub drop: usize,
    pub keep: usize,
    pub skipped: usize,
}

/// Resolve an observation query through stored facet identity before using slug compatibility.
pub fn resolve_observation_entity_dir(
    journal_root: &Path,
    facet_dir: &str,
    query: &str,
) -> Result<ObservationEntityResolution, FacetEntityWriteError> {
    let entities = list_scoped_facet_entities(journal_root, facet_dir, true, true)?;
    if let Some(entity) = entities.iter().find(|entity| entity.entity_id == query) {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: entity.relationship_dir.clone(),
        });
    }
    if let Some(entity) = entities.iter().find(|entity| entity.entity_dir == query) {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: entity.relationship_dir.clone(),
        });
    }
    if let Some(entity) = entities
        .iter()
        .find(|entity| entity.relationship_dir == query)
    {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: entity.relationship_dir.clone(),
        });
    }

    let wanted = normalize_resolution_query(query);
    if let Some(entity) = entities.iter().find(|entity| {
        let name = entity
            .identity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        normalize_resolution_query(name) == wanted
    }) {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: entity.relationship_dir.clone(),
        });
    }

    let derived = entity_slug(query);
    if !derived.is_empty()
        && list_facet_entity_directories(journal_root, facet_dir)?
            .iter()
            .any(|entity_dir| entity_dir == &derived)
    {
        return Ok(ObservationEntityResolution::Resolved {
            entity_dir: derived,
        });
    }

    Ok(ObservationEntityResolution::NoSuchEntity)
}

/// Read facet-scoped entity observations without interpreting JSONL records.
pub fn read_facet_entity_observations(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<Option<String>, FacetStoreError> {
    let path = facet_entity_observations_path(journal_root, facet_dir, entity_dir)?;
    if !path_lexists(&path)? {
        return Ok(None);
    }
    read_text(&path, String::new())
        .map(Some)
        .map_err(Into::into)
}

/// Atomically replace facet-scoped entity observations without parsing JSONL.
pub fn write_facet_entity_observations(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    content: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = facet_entity_observations_path(journal_root, facet_dir, entity_dir)?;
    write_text(&path, content, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(FacetWriteError::ContentWrite)
}

/// Load parsed facet-scoped observations, tolerating malformed JSONL rows.
pub fn load_observations(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<Vec<Value>, FacetStoreError> {
    let Some(content) = read_facet_entity_observations(journal_root, facet_dir, entity_dir)? else {
        return Ok(Vec::new());
    };
    Ok(content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            (!line.is_empty())
                .then(|| serde_json::from_str::<Value>(line).ok())
                .flatten()
        })
        .collect())
}

/// Atomically replace parsed facet-scoped observations as compact JSONL.
pub fn save_observations(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    observations: &[Value],
) -> Result<(), FacetWriteError> {
    let mut content = String::new();
    for observation in observations {
        content.push_str(&serde_json::to_string(observation).expect("Value serializes"));
        content.push('\n');
    }
    write_facet_entity_observations(journal_root, facet_dir, entity_dir, &content)
}

/// Count the same parsed rows that [`load_observations`] returns.
pub fn count_observations(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<usize, FacetStoreError> {
    Ok(load_observations(journal_root, facet_dir, entity_dir)?.len())
}

/// Count valid YYYYMMDD source days from the same parsed rows as [`load_observations`].
pub fn observation_day_counts(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
) -> Result<BTreeMap<String, usize>, FacetStoreError> {
    let mut counts = BTreeMap::new();
    for observation in load_observations(journal_root, facet_dir, entity_dir)? {
        let Some(day) = observation.get("source_day").and_then(Value::as_str) else {
            continue;
        };
        if is_day_key(day) {
            *counts.entry(day.to_owned()).or_default() += 1;
        }
    }
    Ok(counts)
}

/// Add one observation under the facet trust lock, retrying only I/O failures.
pub fn add_observation(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    content: &str,
    source_day: Option<&str>,
    relation: Option<&Value>,
) -> Result<(Vec<Value>, usize), ObservationWriteError> {
    let content = content.trim();
    if content.is_empty() {
        return Err(ObservationWriteError::EmptyContent);
    }
    retry_add_operation(|| {
        let _trust = hold_facet_trust_lock(journal_root)?;
        let mut observations = load_observations(journal_root, facet_dir, entity_dir)?;
        observations.push(new_observation(
            content,
            source_day.filter(|day| !day.is_empty()),
            relation,
        ));
        save_observations(journal_root, facet_dir, entity_dir, &observations)?;
        let count = observations.len();
        Ok((observations, count))
    })
}

/// Apply add, update, drop, and keep operations under the facet trust lock.
pub fn record_observation_ops(
    journal_root: &Path,
    facet_dir: &str,
    entity_dir: &str,
    operations: &[Value],
    source_day: Option<&str>,
) -> Result<ObservationOperationCounts, ObservationWriteError> {
    let resolved = match resolve_observation_entity_dir(journal_root, facet_dir, entity_dir) {
        Ok(ObservationEntityResolution::Resolved { entity_dir }) => entity_dir,
        Ok(ObservationEntityResolution::NoSuchEntity) => entity_dir.to_owned(),
        Err(error) => return Err(ObservationWriteError::Resolve(error)),
    };
    retry_record_operation(|| {
        let _trust = hold_facet_trust_lock(journal_root)?;
        let snapshot = load_observations(journal_root, facet_dir, &resolved)?;
        let (observations, counts, changed) =
            apply_observation_ops(&snapshot, operations, source_day);
        if changed {
            save_observations(journal_root, facet_dir, &resolved, &observations)?;
        }
        Ok(counts)
    })
}

/// Load observations through a name-or-id query without conflating empty and unreadable data.
pub fn load_observations_for_query(
    journal_root: &Path,
    facet_dir: &str,
    query: &str,
) -> Result<ObservationLookup, ObservationLookupError> {
    match resolve_observation_entity_dir(journal_root, facet_dir, query)
        .map_err(ObservationLookupError::Resolve)?
    {
        ObservationEntityResolution::NoSuchEntity => Ok(ObservationLookup::Unresolvable),
        ObservationEntityResolution::Resolved { entity_dir } => {
            let observations =
                load_observations(journal_root, facet_dir, &entity_dir).map_err(|source| {
                    ObservationLookupError::Read {
                        entity_dir: entity_dir.clone(),
                        source,
                    }
                })?;
            Ok(ObservationLookup::Resolved {
                entity_dir,
                observations,
            })
        }
    }
}

fn operation_counts() -> ObservationOperationCounts {
    ObservationOperationCounts::default()
}

fn target_index_in_snapshot(index: Option<&Value>, snapshot: &[Value]) -> Option<usize> {
    index
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < snapshot.len())
}

fn target_quote_matches(observation: &Value, target_quote: Option<&Value>) -> bool {
    let Some(target_quote) = target_quote
        .and_then(Value::as_str)
        .filter(|quote| !quote.trim().is_empty())
    else {
        return false;
    };
    let Some(content) = observation.get("content").and_then(Value::as_str) else {
        return false;
    };
    default_case_fold_str(content).contains(&default_case_fold_str(target_quote.trim()))
}

fn new_observation(content: &str, source_day: Option<&str>, relation: Option<&Value>) -> Value {
    let mut observation = Map::new();
    observation.insert("content".to_owned(), Value::String(content.to_owned()));
    observation.insert(
        "observed_at".to_owned(),
        Value::Number(Utc::now().timestamp_millis().into()),
    );
    if let Some(source_day) = source_day {
        observation.insert(
            "source_day".to_owned(),
            Value::String(source_day.to_owned()),
        );
    }
    if let Some(relation) = relation {
        observation.insert("relation".to_owned(), relation.clone());
    }
    Value::Object(observation)
}

fn apply_observation_ops(
    snapshot: &[Value],
    operations: &[Value],
    source_day: Option<&str>,
) -> (Vec<Value>, ObservationOperationCounts, bool) {
    let mut counts = operation_counts();
    let mut updates = BTreeMap::new();
    let mut drops = std::collections::BTreeSet::new();
    let mut additions = Vec::new();
    let mut changed = false;

    for operation in operations {
        let Some(operation) = operation.as_object() else {
            counts.skipped += 1;
            continue;
        };
        let action = operation.get("op").and_then(Value::as_str);
        if action == Some("add") {
            let Some(content) = operation
                .get("content")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|content| !content.is_empty())
            else {
                counts.skipped += 1;
                continue;
            };
            additions.push(new_observation(
                content,
                source_day,
                operation.get("relation"),
            ));
            counts.add += 1;
            changed = true;
            continue;
        }

        if !matches!(action, Some("update" | "drop" | "keep")) {
            counts.skipped += 1;
            continue;
        }
        let Some(target_index) = target_index_in_snapshot(operation.get("target_index"), snapshot)
        else {
            counts.skipped += 1;
            continue;
        };
        if !target_quote_matches(&snapshot[target_index], operation.get("target_quote")) {
            counts.skipped += 1;
            continue;
        }

        match action.expect("matched action") {
            "keep" => counts.keep += 1,
            "drop" => {
                drops.insert(target_index);
                updates.remove(&target_index);
                counts.drop += 1;
                changed = true;
            }
            "update" => {
                let Some(content) = operation
                    .get("content")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|content| !content.is_empty())
                else {
                    counts.skipped += 1;
                    continue;
                };
                updates.insert(
                    target_index,
                    new_observation(content, source_day, operation.get("relation")),
                );
                drops.remove(&target_index);
                counts.update += 1;
                changed = true;
            }
            _ => unreachable!("matched operation action"),
        }
    }

    if !changed {
        return (snapshot.to_vec(), counts, false);
    }
    let mut observations = Vec::new();
    for (index, observation) in snapshot.iter().enumerate() {
        if drops.contains(&index) {
            continue;
        }
        observations.push(
            updates
                .remove(&index)
                .unwrap_or_else(|| observation.clone()),
        );
    }
    observations.extend(additions);
    (observations, counts, true)
}

fn retry_add_operation<T>(
    mut operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    let mut last_error = None;
    for attempt in 0..OBSERVATION_RETRY_ATTEMPTS {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if error.is_retryable_io() && attempt + 1 < OBSERVATION_RETRY_ATTEMPTS => {
                last_error = Some(error);
                retry_backoff(attempt);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("retry loop returns or stores its final error"))
}

fn retry_record_operation<T>(
    mut operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    let mut last_error = None;
    for attempt in 0..OBSERVATION_RETRY_ATTEMPTS {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error)
                if (error.is_retryable_io() || error.is_lock_timeout())
                    && attempt + 1 < OBSERVATION_RETRY_ATTEMPTS =>
            {
                last_error = Some(error);
                retry_backoff(attempt);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("retry loop returns or stores its final error"))
}

fn retry_backoff(attempt: usize) {
    let maximum_ms = 50 * (attempt + 1) as u64;
    let entropy = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        ^ u128::from(std::process::id());
    thread::sleep(Duration::from_millis(
        1 + (entropy % u128::from(maximum_ms)) as u64,
    ));
}

fn is_day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn retry_add_for_test<T>(
    operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    retry_add_operation(operation)
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn retry_record_for_test<T>(
    operation: impl FnMut() -> Result<T, ObservationWriteError>,
) -> Result<T, ObservationWriteError> {
    retry_record_operation(operation)
}
