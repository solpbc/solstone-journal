// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable entity identity, history, ambiguity, and map-cache writes.

#[cfg(test)]
use std::cell::Cell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use solstone_core_journal_io::AtomicWriteError;
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::JsonWriteOptions;
use solstone_core_journal_io::LockError;
use solstone_core_journal_io::LockOptions;
use solstone_core_journal_io::MalformedPolicy;
use solstone_core_journal_io::PathError;
use solstone_core_journal_io::ReadError;
use solstone_core_journal_io::StagedDirOptions;
use solstone_core_journal_io::StagedWriteError;
use solstone_core_journal_io::hold_lock;
use solstone_core_journal_io::path_lexists;
use solstone_core_journal_io::publish_staged_dir;
use solstone_core_journal_io::read_json;
use solstone_core_journal_io::remove_dir_all;
use solstone_core_journal_io::write_json;
use solstone_core_journal_io::write_text;

use crate::{EntityTrustLockError, ambiguity_id, hold_entity_trust_lock};

use super::ambiguity::validate_row;
use super::error::EntityStoreError;
use super::history::{
    HistoryEvent, PreparedHistoryEvent, guard_visible_event_collision, read_prepared_history,
    read_visible_history,
};
use super::identity::{IdentitySnapshot, read_entity_identity};
use super::map::read_identity_map;
use super::paths::{
    ambiguities_path, events_dir, identity_map_cache_path, identity_path, prepared_dir,
};
use super::reconcile::{PreparedHistoryOutcome, classify_prepared_history, python_json_equal};

const HISTORY_SCHEMA_VERSION: u64 = 1;
const CACHE_SCHEMA_VERSION: u64 = 1;
static VERSION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
thread_local! { static FORCE_IDENTITY_WRITE_FAILURE: Cell<bool> = const { Cell::new(false) }; }

/// Explicit history metadata for a durable identity write.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityOperationContext {
    pub kind: EntityOperationKind,
    pub caller: Value,
    pub actor: Value,
    pub metadata: Value,
}

/// Supported durable history operation kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityOperationKind {
    Create,
    Update,
    Restore,
    Merge,
    MergeUndo,
}

impl EntityOperationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Restore => "restore",
            Self::Merge => "merge",
            Self::MergeUndo => "merge_undo",
        }
    }
}

/// Outcome of one identity save request.
#[derive(Debug, Clone, PartialEq)]
pub struct EntitySaveResult {
    pub entity_dir: String,
    pub changed: bool,
    pub event: Option<Value>,
}

/// Inputs for one ambiguity observation.
#[derive(Debug, Clone, PartialEq)]
pub struct AmbiguityObservation {
    pub scope: Value,
    pub query: String,
    pub normalized_query: String,
    pub observed_tier: i64,
    pub ranked_candidates: Vec<Value>,
    pub origin: Value,
}

/// Inputs for choosing a resolved ambiguity candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct AmbiguityChoiceRequest {
    pub scope: Value,
    pub query: String,
    pub entity_id: String,
    pub origin: Option<Value>,
}

/// One entity eligible for an ambiguity resolution choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguityChoiceEntity {
    pub id: String,
    pub blocked: bool,
}

/// Result from loading or rebuilding the portable identity-map cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityMapCacheLoad {
    pub resolved: HashMap<String, String>,
    pub rebuilt: bool,
}

/// Failure while changing durable entity-store state.
#[derive(Debug)]
pub enum EntityWriteError {
    TrustLock(EntityTrustLockError),
    Read(EntityStoreError),
    InvalidIdentity {
        identity_id: String,
        detail: String,
    },
    InvalidOperationContext {
        detail: String,
    },
    AmbiguityRowInvalid {
        detail: String,
    },
    AmbiguityCountOverflow {
        ambiguity_id: String,
    },
    CreateDestinationOccupied {
        identity_id: String,
        path: PathBuf,
    },
    ReconciliationRepairRequired {
        entity_dir: String,
        version_id: String,
    },
    PreparedStageCollision {
        entity_dir: String,
        version_id: String,
    },
    PreparedStage(StagedWriteError),
    IdentityWrite(AtomicWriteError),
    VisibleEventWrite(AtomicWriteError),
    PreparedEventRemoval(PathError),
    AmbiguityLock(LockError),
    AmbiguityWrite(AtomicWriteError),
    AmbiguityChoiceNotFound {
        scope_key: String,
        normalized_query: String,
    },
    AmbiguityChoiceScopeMismatch {
        ambiguity_id: String,
    },
    AmbiguityChoiceInvalid {
        entity_id: String,
        detail: String,
    },
    CacheWriteAfterCommit(AtomicWriteError),
}

impl fmt::Display for EntityWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::InvalidIdentity {
                identity_id,
                detail,
            } => {
                write!(
                    formatter,
                    "invalid entity identity {identity_id:?}: {detail}"
                )
            }
            Self::InvalidOperationContext { detail } => {
                write!(formatter, "invalid entity operation context: {detail}")
            }
            Self::AmbiguityRowInvalid { detail } => {
                write!(formatter, "invalid entity ambiguity row: {detail}")
            }
            Self::AmbiguityCountOverflow { ambiguity_id } => {
                write!(
                    formatter,
                    "ambiguity occurrence count overflow: {ambiguity_id}"
                )
            }
            Self::CreateDestinationOccupied { identity_id, path } => write!(
                formatter,
                "cannot create entity identity {identity_id:?}; destination already exists: {}",
                path.display()
            ),
            Self::ReconciliationRepairRequired {
                entity_dir,
                version_id,
            } => write!(
                formatter,
                "prepared history for {entity_dir} cannot be reconciled: {version_id}"
            ),
            Self::PreparedStageCollision {
                entity_dir,
                version_id,
            } => write!(
                formatter,
                "prepared history event already exists for {entity_dir}: {version_id}"
            ),
            Self::PreparedStage(error) => error.fmt(formatter),
            Self::IdentityWrite(error)
            | Self::VisibleEventWrite(error)
            | Self::AmbiguityWrite(error)
            | Self::CacheWriteAfterCommit(error) => error.fmt(formatter),
            Self::PreparedEventRemoval(error) => error.fmt(formatter),
            Self::AmbiguityLock(error) => error.fmt(formatter),
            Self::AmbiguityChoiceNotFound {
                scope_key,
                normalized_query,
            } => write!(
                formatter,
                "no ambiguity row for {scope_key} query {normalized_query:?}"
            ),
            Self::AmbiguityChoiceScopeMismatch { ambiguity_id } => {
                write!(formatter, "ambiguity row scope mismatch: {ambiguity_id}")
            }
            Self::AmbiguityChoiceInvalid { entity_id, detail } => {
                write!(
                    formatter,
                    "invalid ambiguity choice {entity_id:?}: {detail}"
                )
            }
        }
    }
}

impl Error for EntityWriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::PreparedStage(error) => Some(error),
            Self::IdentityWrite(error)
            | Self::VisibleEventWrite(error)
            | Self::AmbiguityWrite(error)
            | Self::CacheWriteAfterCommit(error) => Some(error),
            Self::PreparedEventRemoval(error) => Some(error),
            Self::AmbiguityLock(error) => Some(error),
            Self::InvalidIdentity { .. }
            | Self::InvalidOperationContext { .. }
            | Self::AmbiguityRowInvalid { .. }
            | Self::AmbiguityCountOverflow { .. }
            | Self::CreateDestinationOccupied { .. }
            | Self::ReconciliationRepairRequired { .. }
            | Self::PreparedStageCollision { .. }
            | Self::AmbiguityChoiceNotFound { .. }
            | Self::AmbiguityChoiceScopeMismatch { .. }
            | Self::AmbiguityChoiceInvalid { .. } => None,
        }
    }
}

impl From<EntityTrustLockError> for EntityWriteError {
    fn from(error: EntityTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

impl From<EntityStoreError> for EntityWriteError {
    fn from(error: EntityStoreError) -> Self {
        Self::Read(error)
    }
}

impl From<PathError> for EntityWriteError {
    fn from(error: PathError) -> Self {
        Self::Read(EntityStoreError::from(error))
    }
}

impl From<ReadError> for EntityWriteError {
    fn from(error: ReadError) -> Self {
        Self::Read(EntityStoreError::from(error))
    }
}

/// Save one identity addressed by its effective identity id.
pub fn save_entity_identity(
    journal_root: &Path,
    identity_id: &str,
    identity: &Value,
    operation: Option<&EntityOperationContext>,
) -> Result<EntitySaveResult, EntityWriteError> {
    save_entity_identity_with_lock_options(journal_root, identity_id, identity, operation, None)
}

#[cfg(test)]
pub(crate) fn save_entity_identity_with_timeout(
    journal_root: &Path,
    identity_id: &str,
    identity: &Value,
    operation: Option<&EntityOperationContext>,
    lock_options: LockOptions,
) -> Result<EntitySaveResult, EntityWriteError> {
    save_entity_identity_with_lock_options(
        journal_root,
        identity_id,
        identity,
        operation,
        Some(lock_options),
    )
}

fn save_entity_identity_with_lock_options(
    journal_root: &Path,
    identity_id: &str,
    identity: &Value,
    operation: Option<&EntityOperationContext>,
    lock_options: Option<LockOptions>,
) -> Result<EntitySaveResult, EntityWriteError> {
    if identity_id.is_empty() {
        return Err(EntityWriteError::InvalidIdentity {
            identity_id: identity_id.to_owned(),
            detail: "identity id is empty".to_owned(),
        });
    }
    let _trust = match lock_options {
        Some(options) => {
            crate::trust_lock::hold_entity_trust_lock_with_options(journal_root, options)?
        }
        None => hold_entity_trust_lock(journal_root)?,
    };
    let map = read_identity_map(journal_root)?;
    let (entity_dir, creating) = match map.resolved.get(identity_id) {
        Some(directory) => (directory.clone(), false),
        None => (identity_id.to_owned(), true),
    };
    let path = identity_path(journal_root, &entity_dir)?;
    if creating && path_lexists(&path)? {
        return Err(EntityWriteError::CreateDestinationOccupied {
            identity_id: identity_id.to_owned(),
            path,
        });
    }

    reconcile_prepared_history(journal_root, &entity_dir)?;
    let before = read_entity_identity(journal_root, &entity_dir)?;
    let after = normalized_identity(identity_id, identity)?;
    if before
        .as_ref()
        .is_some_and(|snapshot| python_json_equal(snapshot.value(), &after))
        && operation.is_none()
    {
        return Ok(EntitySaveResult {
            entity_dir,
            changed: false,
            event: None,
        });
    }

    validate_operation(operation)?;
    let event = build_history_event(
        &entity_dir,
        before.as_ref(),
        &after,
        operation,
        journal_root,
    )?;
    let version_id = event["version_id"]
        .as_str()
        .expect("writer created version id");
    stage_event(journal_root, &entity_dir, version_id, &event)?;
    write_identity_snapshot(&path, &after).map_err(EntityWriteError::IdentityWrite)?;
    publish_staged_event(journal_root, &entity_dir, version_id, &event)?;
    rewrite_identity_map_cache(journal_root)?;
    Ok(EntitySaveResult {
        entity_dir,
        changed: true,
        event: Some(event),
    })
}

/// Create or update an ambiguity observation under the trust and file locks.
pub fn record_ambiguity_observation(
    journal_root: &Path,
    observation: &AmbiguityObservation,
) -> Result<Value, EntityWriteError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    mutate_ambiguities(journal_root, |rows| {
        let scope_key = ambiguity_scope_key(&observation.scope)?;
        let key = format!("{scope_key}|{}", observation.normalized_query);
        let origin_key = origin_key(&observation.origin)?;
        let now = ambiguity_now_iso();
        let existing = rows.iter_mut().find(|row| {
            row.get("normalized_query").and_then(Value::as_str)
                == Some(observation.normalized_query.as_str())
                && row
                    .get("scope")
                    .is_some_and(|scope| scope_key_for_row(scope).as_deref() == Some(&scope_key))
        });
        if let Some(row) = existing {
            let object = row.as_object_mut().expect("strict rows are objects");
            object.insert(
                "latest_query".to_owned(),
                Value::String(observation.query.clone()),
            );
            object.insert(
                "observed_tier".to_owned(),
                Value::from(observation.observed_tier),
            );
            object.insert(
                "ranked_candidates".to_owned(),
                Value::Array(observation.ranked_candidates.clone()),
            );
            let origin_keys = object
                .get_mut("origin_keys")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                    detail: "ambiguity row has invalid origin_keys".to_owned(),
                })?;
            if !origin_keys
                .iter()
                .any(|item| item.as_str() == Some(&origin_key))
            {
                origin_keys.push(Value::String(origin_key));
                object
                    .get_mut("origins")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                        detail: "ambiguity row has invalid origins".to_owned(),
                    })?
                    .push(observation.origin.clone());
                let count = object
                    .get("occurrence_count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| EntityWriteError::AmbiguityCountOverflow {
                        ambiguity_id: object
                            .get("ambiguity_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    })?;
                object.insert("occurrence_count".to_owned(), Value::from(count));
                object.insert("last_seen".to_owned(), Value::String(now));
            }
            return Ok(row.clone());
        }

        let mut row = Map::new();
        row.insert("schema_version".to_owned(), Value::from(1));
        row.insert("ambiguity_id".to_owned(), Value::String(ambiguity_id(&key)));
        row.insert("scope".to_owned(), observation.scope.clone());
        row.insert(
            "normalized_query".to_owned(),
            Value::String(observation.normalized_query.clone()),
        );
        row.insert(
            "original_query".to_owned(),
            Value::String(observation.query.clone()),
        );
        row.insert(
            "latest_query".to_owned(),
            Value::String(observation.query.clone()),
        );
        row.insert(
            "observed_tier".to_owned(),
            Value::from(observation.observed_tier),
        );
        row.insert(
            "ranked_candidates".to_owned(),
            Value::Array(observation.ranked_candidates.clone()),
        );
        row.insert(
            "origins".to_owned(),
            Value::Array(vec![observation.origin.clone()]),
        );
        row.insert(
            "origin_keys".to_owned(),
            Value::Array(vec![Value::String(origin_key)]),
        );
        row.insert("first_seen".to_owned(), Value::String(now.clone()));
        row.insert("last_seen".to_owned(), Value::String(now));
        row.insert("occurrence_count".to_owned(), Value::from(1));
        row.insert("status".to_owned(), Value::String("open".to_owned()));
        row.insert("resolved_entity_id".to_owned(), Value::Null);
        row.insert("resolved_at".to_owned(), Value::Null);
        let mut audit = Map::new();
        audit.insert("prior_choices".to_owned(), Value::Array(Vec::new()));
        row.insert("audit".to_owned(), Value::Object(audit));
        let row = Value::Object(row);
        rows.push(row.clone());
        Ok(row)
    })
}

/// Record or replace a resolved ambiguity choice.
pub fn record_ambiguity_choice(
    journal_root: &Path,
    choice: &AmbiguityChoiceRequest,
    eligible_entities: &[AmbiguityChoiceEntity],
) -> Result<Value, EntityWriteError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    let selected = eligible_entities
        .iter()
        .find(|entity| entity.id == choice.entity_id)
        .ok_or_else(|| EntityWriteError::AmbiguityChoiceInvalid {
            entity_id: choice.entity_id.clone(),
            detail: "resolved entity choice is not present in the resolution scope".to_owned(),
        })?;
    if selected.blocked {
        return Err(EntityWriteError::AmbiguityChoiceInvalid {
            entity_id: choice.entity_id.clone(),
            detail: "resolved entity choice is blocked".to_owned(),
        });
    }
    let normalized_query = crate::normalize_resolution_query(&choice.query);
    mutate_ambiguities(journal_root, |rows| {
        let scope_key = ambiguity_scope_key(&choice.scope)?;
        let row = rows
            .iter_mut()
            .find(|row| {
                row.get("normalized_query").and_then(Value::as_str)
                    == Some(normalized_query.as_str())
                    && row.get("scope").is_some_and(|scope| {
                        scope_key_for_row(scope).as_deref() == Some(&scope_key)
                    })
            })
            .ok_or_else(|| EntityWriteError::AmbiguityChoiceNotFound {
                scope_key: scope_key.clone(),
                normalized_query: normalized_query.clone(),
            })?;
        if row.get("scope") != Some(&choice.scope) {
            return Err(EntityWriteError::AmbiguityChoiceScopeMismatch {
                ambiguity_id: row
                    .get("ambiguity_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        let object = row.as_object_mut().expect("strict rows are objects");
        let now = ambiguity_now_iso();
        let previous_id = object
            .get("resolved_entity_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if object.get("status").and_then(Value::as_str) == Some("resolved")
            && previous_id
                .as_deref()
                .is_some_and(|id| id != choice.entity_id)
        {
            let previous_at = object
                .get("resolved_at")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let audit = object
                .get_mut("audit")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                    detail: "ambiguity row has invalid audit".to_owned(),
                })?;
            let priors = audit
                .get_mut("prior_choices")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
                    detail: "ambiguity row has invalid audit.prior_choices".to_owned(),
                })?;
            let mut prior = Map::new();
            prior.insert(
                "resolved_entity_id".to_owned(),
                Value::String(previous_id.expect("checked")),
            );
            prior.insert("resolved_at".to_owned(), Value::String(previous_at));
            prior.insert("replaced_at".to_owned(), Value::String(now.clone()));
            if let Some(origin) = &choice.origin {
                prior.insert("replaced_by_origin".to_owned(), origin.clone());
            }
            priors.push(Value::Object(prior));
        }
        object.insert("status".to_owned(), Value::String("resolved".to_owned()));
        object.insert(
            "resolved_entity_id".to_owned(),
            Value::String(choice.entity_id.clone()),
        );
        object.insert("resolved_at".to_owned(), Value::String(now));
        Ok(row.clone())
    })
}

/// Return a valid cache, rebuilding it from a fresh scan when unreadable.
pub fn refresh_identity_map_cache(
    journal_root: &Path,
) -> Result<IdentityMapCacheLoad, EntityWriteError> {
    if let Ok(resolved) = read_identity_map_cache(journal_root) {
        return Ok(IdentityMapCacheLoad {
            resolved,
            rebuilt: false,
        });
    }
    let _trust = hold_entity_trust_lock(journal_root)?;
    if let Ok(resolved) = read_identity_map_cache(journal_root) {
        return Ok(IdentityMapCacheLoad {
            resolved,
            rebuilt: false,
        });
    }
    let map = read_identity_map(journal_root)?;
    write_identity_map_cache(journal_root, &map.resolved)?;
    Ok(IdentityMapCacheLoad {
        resolved: map.resolved,
        rebuilt: true,
    })
}

fn reconcile_prepared_history(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<(), EntityWriteError> {
    for PreparedHistoryEvent { staging_id, event } in
        read_prepared_history(journal_root, entity_dir)?
    {
        let current = read_entity_identity(journal_root, entity_dir)?;
        match classify_prepared_history(entity_dir, &event, current.as_ref())? {
            PreparedHistoryOutcome::Publish => {
                publish_staged_event(journal_root, entity_dir, &staging_id, event.value())?
            }
            PreparedHistoryOutcome::Discard => {
                discard_staged_event(journal_root, entity_dir, &staging_id)?
            }
            PreparedHistoryOutcome::RepairRequired => {
                return Err(EntityWriteError::ReconciliationRepairRequired {
                    entity_dir: entity_dir.to_owned(),
                    version_id: staging_id,
                });
            }
        }
    }
    Ok(())
}

fn normalized_identity(identity_id: &str, identity: &Value) -> Result<Value, EntityWriteError> {
    let mut value = identity.clone();
    let Some(object) = value.as_object_mut() else {
        return Err(EntityWriteError::InvalidIdentity {
            identity_id: identity_id.to_owned(),
            detail: "identity is not an object".to_owned(),
        });
    };
    // The durable identity is addressed by `identity_id`; callers cannot use a
    // stale or omitted payload field to move it to another effective id.
    object.insert("id".to_owned(), Value::String(identity_id.to_owned()));
    Ok(value)
}

fn validate_operation(operation: Option<&EntityOperationContext>) -> Result<(), EntityWriteError> {
    if let Some(operation) = operation
        && !operation.metadata.is_object()
    {
        return Err(EntityWriteError::InvalidOperationContext {
            detail: "metadata must be an object".to_owned(),
        });
    }
    Ok(())
}

fn build_history_event(
    entity_dir: &str,
    before: Option<&IdentitySnapshot>,
    after: &Value,
    operation: Option<&EntityOperationContext>,
    journal_root: &Path,
) -> Result<Value, EntityWriteError> {
    let kind = operation
        .map(|context| context.kind.as_str())
        .unwrap_or(if before.is_some() { "update" } else { "create" });
    let mut maximum = 0_i128;
    for event in read_visible_history(journal_root, entity_dir)? {
        maximum = maximum.max(event.sequence()?);
    }
    let sequence = maximum
        .checked_add(1)
        .ok_or_else(|| EntityWriteError::InvalidIdentity {
            identity_id: entity_dir.to_owned(),
            detail: "history sequence overflow".to_owned(),
        })?;
    let sequence = i64::try_from(sequence).map_err(|_| EntityWriteError::InvalidIdentity {
        identity_id: entity_dir.to_owned(),
        detail: "history sequence exceeds durable JSON integer range".to_owned(),
    })?;
    let mut event = Map::new();
    event.insert(
        "schema_version".to_owned(),
        Value::from(HISTORY_SCHEMA_VERSION),
    );
    event.insert("version_id".to_owned(), Value::String(next_version_id()));
    event.insert("seq".to_owned(), Value::from(sequence));
    event.insert("ts".to_owned(), Value::String(history_now_iso()));
    event.insert("entity_id".to_owned(), Value::String(entity_dir.to_owned()));
    event.insert("kind".to_owned(), Value::String(kind.to_owned()));
    event.insert(
        "caller".to_owned(),
        operation
            .map(|context| context.caller.clone())
            .unwrap_or(Value::Null),
    );
    event.insert(
        "actor".to_owned(),
        operation
            .map(|context| context.actor.clone())
            .unwrap_or(Value::Null),
    );
    event.insert(
        "identity_before".to_owned(),
        before
            .map(|snapshot| snapshot.value().clone())
            .unwrap_or(Value::Null),
    );
    event.insert("identity_after".to_owned(), after.clone());
    event.insert(
        "operation".to_owned(),
        operation
            .map(|context| context.metadata.clone())
            .unwrap_or_else(|| Value::Object(Map::new())),
    );
    Ok(Value::Object(event))
}

fn stage_event(
    journal_root: &Path,
    entity_dir: &str,
    version_id: &str,
    event: &Value,
) -> Result<(), EntityWriteError> {
    let path = prepared_dir(journal_root, entity_dir)?.join(version_id);
    if path_lexists(&path)? {
        return Err(EntityWriteError::PreparedStageCollision {
            entity_dir: entity_dir.to_owned(),
            version_id: version_id.to_owned(),
        });
    }
    publish_staged_dir(
        &path,
        StagedDirOptions {
            directory_mode: Some(0o700),
        },
        |staging| {
            write_json(staging.join("event.json"), event, history_json_options())
                .map_err(io::Error::other)
        },
    )
    .map_err(EntityWriteError::PreparedStage)
}

fn publish_staged_event(
    journal_root: &Path,
    entity_dir: &str,
    version_id: &str,
    event: &Value,
) -> Result<(), EntityWriteError> {
    let event_object = HistoryEvent::from_value(
        event.clone(),
        &prepared_dir(journal_root, entity_dir)?
            .join(version_id)
            .join("event.json"),
    )?;
    let sequence = event_object.sequence()?;
    let event_version_id = event_object.version_id()?;
    let visible_path = events_dir(journal_root, entity_dir)?
        .join(format!("{sequence:020}-{event_version_id}.json"));
    let existing = if path_lexists(&visible_path)? {
        Some(HistoryEvent::from_value(
            read_json(&visible_path, Value::Null, MalformedPolicy::Raise)?,
            &visible_path,
        )?)
    } else {
        None
    };
    guard_visible_event_collision(entity_dir, &event_object, existing.as_ref())?;
    if existing.is_none() {
        write_json(&visible_path, event, history_json_options())
            .map_err(EntityWriteError::VisibleEventWrite)?;
    }
    discard_staged_event(journal_root, entity_dir, version_id)
}

fn discard_staged_event(
    journal_root: &Path,
    entity_dir: &str,
    version_id: &str,
) -> Result<(), EntityWriteError> {
    remove_dir_all(
        journal_root,
        &format!("entities/{entity_dir}/history/prepared/{version_id}"),
    )
    .map_err(EntityWriteError::PreparedEventRemoval)
}

fn mutate_ambiguities<F>(journal_root: &Path, mutate: F) -> Result<Value, EntityWriteError>
where
    F: FnOnce(&mut Vec<Value>) -> Result<Value, EntityWriteError>,
{
    let path = ambiguities_path(journal_root)?;
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(EntityWriteError::AmbiguityLock)?;
    let mut rows = super::ambiguity::read_ambiguities(journal_root, MalformedPolicy::Raise)?;
    let result = mutate(&mut rows)?;
    for row in &rows {
        let object = row.as_object().expect("strict reader returns objects");
        validate_row(object).map_err(|detail| EntityWriteError::AmbiguityRowInvalid {
            detail: format!("invalid ambiguity row: {detail}"),
        })?;
    }
    let contents = serialize_ambiguity_rows(&rows).map_err(EntityWriteError::AmbiguityWrite)?;
    write_text(&path, &contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(EntityWriteError::AmbiguityWrite)?;
    Ok(result)
}

fn rewrite_identity_map_cache(journal_root: &Path) -> Result<(), EntityWriteError> {
    let map = read_identity_map(journal_root)?;
    write_identity_map_cache(journal_root, &map.resolved)
}

fn read_identity_map_cache(
    journal_root: &Path,
) -> Result<HashMap<String, String>, EntityStoreError> {
    let path = identity_map_cache_path(journal_root)?;
    let value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)?;
    let object = value
        .as_object()
        .ok_or_else(|| EntityStoreError::IdentityNotObject { path: path.clone() })?;
    if object.get("schema_version").and_then(Value::as_u64) != Some(CACHE_SCHEMA_VERSION) {
        return Err(EntityStoreError::IdentityNotObject { path });
    }
    let resolved = object
        .get("resolved")
        .and_then(Value::as_object)
        .ok_or_else(|| EntityStoreError::IdentityNotObject { path: path.clone() })?;
    let mut result = HashMap::with_capacity(resolved.len());
    for (identity_id, directory) in resolved {
        let Some(directory) = directory.as_str() else {
            return Err(EntityStoreError::IdentityNotObject { path });
        };
        result.insert(identity_id.clone(), directory.to_owned());
    }
    Ok(result)
}

fn write_identity_map_cache(
    journal_root: &Path,
    resolved: &HashMap<String, String>,
) -> Result<(), EntityWriteError> {
    let path = identity_map_cache_path(journal_root)?;
    let mut entries = Map::new();
    for (identity_id, directory) in resolved {
        entries.insert(identity_id.clone(), Value::String(directory.clone()));
    }
    let mut value = Map::new();
    value.insert(
        "schema_version".to_owned(),
        Value::from(CACHE_SCHEMA_VERSION),
    );
    value.insert("resolved".to_owned(), Value::Object(entries));
    write_json(
        &path,
        &Value::Object(value),
        JsonWriteOptions {
            mode: Some(0o600),
            indent: Some(2),
            sort_keys: true,
        },
    )
    .map_err(EntityWriteError::CacheWriteAfterCommit)
}

fn identity_json_options() -> JsonWriteOptions {
    JsonWriteOptions {
        mode: Some(0o600),
        indent: Some(2),
        sort_keys: false,
    }
}

pub(super) fn write_identity_snapshot(
    path: &Path,
    identity: &Value,
) -> Result<(), AtomicWriteError> {
    #[cfg(test)]
    if FORCE_IDENTITY_WRITE_FAILURE.with(Cell::get) {
        return Err(AtomicWriteError::Io {
            path: path.to_path_buf(),
            source: io::Error::other("forced identity write failure"),
        });
    }
    write_json(path, identity, identity_json_options())
}

#[cfg(test)]
pub(crate) fn set_forced_identity_write_failure(enabled: bool) {
    FORCE_IDENTITY_WRITE_FAILURE.with(|value| value.set(enabled));
}

fn history_json_options() -> JsonWriteOptions {
    JsonWriteOptions {
        mode: Some(0o600),
        indent: Some(2),
        sort_keys: true,
    }
}

#[cfg(test)]
pub(crate) fn write_history_event_json_for_test(
    path: &Path,
    event: &Value,
) -> Result<(), AtomicWriteError> {
    write_json(path, event, history_json_options())
}

fn history_now_iso() -> String {
    format_history_ts(Utc::now())
}

fn format_history_ts(now: DateTime<Utc>) -> String {
    let micros = now.timestamp_subsec_micros();
    if micros == 0 {
        now.to_rfc3339_opts(SecondsFormat::Secs, true)
    } else {
        format!("{}.{micros:06}Z", now.format("%Y-%m-%dT%H:%M:%S"))
    }
}

fn ambiguity_now_iso() -> String {
    format_ambiguity_ts(Utc::now())
}

fn format_ambiguity_ts(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn next_version_id() -> String {
    let sequence = VERSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "vh_{:x}",
        md5::compute(format!("{}:{nanos}:{sequence}", std::process::id()))
    )
}

fn ambiguity_scope_key(scope: &Value) -> Result<String, EntityWriteError> {
    scope_key_for_row(scope).ok_or_else(|| EntityWriteError::AmbiguityRowInvalid {
        detail: "invalid ambiguity scope".to_owned(),
    })
}

fn scope_key_for_row(scope: &Value) -> Option<String> {
    let object = scope.as_object()?;
    match object.get("kind").and_then(Value::as_str) {
        Some("journal") if object.get("facet").is_none_or(Value::is_null) => {
            Some("journal".to_owned())
        }
        Some("facet") => object
            .get("facet")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|facet| format!("facet:{facet}")),
        _ => None,
    }
}

fn origin_key(origin: &Value) -> Result<String, EntityWriteError> {
    let mut origin = origin.clone();
    sort_json_keys(&mut origin);
    serialize_value(&origin, AsciiCompactFormatter).map_err(|error| {
        EntityWriteError::AmbiguityRowInvalid {
            detail: format!("cannot serialize ambiguity origin: {error}"),
        }
    })
}

fn serialize_ambiguity_rows(rows: &[Value]) -> Result<String, AtomicWriteError> {
    let mut contents = String::new();
    for row in rows {
        let line =
            serialize_value(row, PythonFormatter).map_err(|source| AtomicWriteError::Io {
                path: PathBuf::from("entities/ambiguities.jsonl"),
                source: io::Error::new(io::ErrorKind::InvalidData, source),
            })?;
        contents.push_str(&line);
        contents.push('\n');
    }
    Ok(contents)
}

fn serialize_value<F: serde_json::ser::Formatter>(
    value: &Value,
    formatter: F,
) -> Result<String, serde_json::Error> {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(bytes).expect("JSON serializer emits UTF-8"))
}

fn sort_json_keys(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let mut sorted = std::collections::BTreeMap::new();
            for (key, mut child) in std::mem::take(object) {
                sort_json_keys(&mut child);
                sorted.insert(key, child);
            }
            *object = sorted.into_iter().collect();
        }
        Value::Array(values) => values.iter_mut().for_each(sort_json_keys),
        _ => {}
    }
}

struct PythonFormatter;

impl serde_json::ser::Formatter for PythonFormatter {
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            writer.write_all(b", ")
        }
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }
}

struct AsciiCompactFormatter;

impl serde_json::ser::Formatter for AsciiCompactFormatter {
    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        for character in fragment.chars() {
            if character.is_ascii() {
                writer.write_all(&[character as u8])?;
            } else {
                write_ascii_escape(writer, character)?;
            }
        }
        Ok(())
    }
}

fn write_ascii_escape<W: ?Sized + io::Write>(writer: &mut W, character: char) -> io::Result<()> {
    let mut write_unit = |unit: u16| write!(writer, "\\u{unit:04x}");
    let value = character as u32;
    if value <= 0xFFFF {
        write_unit(value as u16)
    } else {
        let scalar = value - 0x1_0000;
        write_unit((0xD800 + (scalar >> 10)) as u16)?;
        write_unit((0xDC00 + (scalar & 0x3FF)) as u16)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Timelike, Utc};

    use super::{format_ambiguity_ts, format_history_ts};

    #[test]
    fn timestamp_formatters_match_the_entity_store_oracle() {
        let zero = Utc.with_ymd_and_hms(2026, 8, 5, 0, 32, 2).unwrap();
        let fractional = zero.with_nanosecond(582_506_000).unwrap();

        assert_eq!(format_history_ts(zero), "2026-08-05T00:32:02Z");
        assert_eq!(format_history_ts(fractional), "2026-08-05T00:32:02.582506Z");
        assert_eq!(format_ambiguity_ts(fractional), "2026-08-05T00:32:02Z");
    }
}
