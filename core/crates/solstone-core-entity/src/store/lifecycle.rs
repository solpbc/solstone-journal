// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing journal entity lifecycle operations.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::remove_dir_all;

use crate::{EntityTrustLockError, hold_entity_trust_lock};

use super::error::EntityStoreError;
use super::history::{guard_restore_does_not_cross_merge, read_visible_history};
use super::identity::read_entity_identity;
use super::map::{read_identity_group_map, read_identity_map};
use super::write::{
    EntityOperationContext, EntityOperationKind, EntityWriteError, rewrite_identity_map_cache,
    save_entity_identity,
};

/// Failure while running an owner-facing entity lifecycle operation.
#[derive(Debug)]
pub enum EntityLifecycleError {
    TrustLock(EntityTrustLockError),
    Store(EntityStoreError),
    Write(EntityWriteError),
    EntityNotFound {
        entity_id: String,
    },
    EntityNotBlocked {
        entity_id: String,
    },
    HistoryVersionNotFound {
        entity_id: String,
        version_id: String,
    },
    RestoreTargetsRecordedMerge,
    RestoreCrossesRecordedMerge,
    RestoreSnapshotNotObject {
        entity_id: String,
        version_id: String,
    },
    RestoreSnapshotIdentityMismatch {
        entity_id: String,
        version_id: String,
        snapshot_id: Option<String>,
    },
    RestoreWouldCreateSecondPrincipal {
        entity_id: String,
        existing_entity_id: String,
    },
}

impl fmt::Display for EntityLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::EntityNotFound { entity_id } => write!(formatter, "entity not found: {entity_id}"),
            Self::EntityNotBlocked { entity_id } => write!(formatter, "entity is not blocked: {entity_id}"),
            Self::HistoryVersionNotFound {
                entity_id,
                version_id,
            } => write!(formatter, "history version not found for {entity_id}: {version_id}"),
            Self::RestoreTargetsRecordedMerge => formatter.write_str(
                "generic identity restore cannot target a recorded merge event; use recorded-merge undo instead",
            ),
            Self::RestoreCrossesRecordedMerge => formatter.write_str(
                "generic identity restore cannot cross a recorded merge event; use recorded-merge undo instead",
            ),
            Self::RestoreSnapshotNotObject {
                entity_id,
                version_id,
            } => write!(formatter, "restore snapshot is not an object for {entity_id}: {version_id}"),
            Self::RestoreSnapshotIdentityMismatch {
                entity_id,
                version_id,
                snapshot_id,
            } => write!(
                formatter,
                "restore snapshot identity mismatch for {entity_id} at {version_id}: {}",
                snapshot_id.as_deref().unwrap_or("<missing>")
            ),
            Self::RestoreWouldCreateSecondPrincipal {
                entity_id,
                existing_entity_id,
            } => write!(
                formatter,
                "restoring {entity_id} would create a second principal alongside {existing_entity_id}"
            ),
        }
    }
}

impl Error for EntityLifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::EntityNotFound { .. }
            | Self::EntityNotBlocked { .. }
            | Self::HistoryVersionNotFound { .. }
            | Self::RestoreTargetsRecordedMerge
            | Self::RestoreCrossesRecordedMerge
            | Self::RestoreSnapshotNotObject { .. }
            | Self::RestoreSnapshotIdentityMismatch { .. }
            | Self::RestoreWouldCreateSecondPrincipal { .. } => None,
        }
    }
}

impl From<EntityTrustLockError> for EntityLifecycleError {
    fn from(error: EntityTrustLockError) -> Self {
        Self::TrustLock(error)
    }
}

impl From<EntityStoreError> for EntityLifecycleError {
    fn from(error: EntityStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<EntityWriteError> for EntityLifecycleError {
    fn from(error: EntityWriteError) -> Self {
        Self::Write(error)
    }
}

/// Return the first principal identity in deterministic entity-directory order.
pub fn read_journal_principal(journal_root: &Path) -> Result<Option<Value>, EntityLifecycleError> {
    let mut entity_dirs = read_identity_group_map(journal_root)?
        .groups
        .into_values()
        .flatten()
        .collect::<Vec<_>>();
    entity_dirs.sort();
    for entity_dir in entity_dirs {
        let Some(identity) = read_entity_identity(journal_root, &entity_dir)? else {
            continue;
        };
        if identity
            .value()
            .get("is_principal")
            .is_some_and(value_is_truthy)
        {
            return Ok(Some(identity.value().clone()));
        }
    }
    Ok(None)
}

/// Whether any journal entity is marked as the principal.
pub fn has_journal_principal(journal_root: &Path) -> Result<bool, EntityLifecycleError> {
    Ok(read_journal_principal(journal_root)?.is_some())
}

/// Clear a blocked entity's flag and record an update history event.
pub fn unblock_journal_entity(
    journal_root: &Path,
    entity_id: &str,
) -> Result<Value, EntityLifecycleError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    let entity_dir = resolve_entity_dir(journal_root, entity_id)?;
    let identity = read_entity_identity(journal_root, &entity_dir)?
        .expect("resolved identity-map directory contains an identity");
    if !identity.value().get("blocked").is_some_and(value_is_truthy) {
        return Err(EntityLifecycleError::EntityNotBlocked {
            entity_id: entity_id.to_owned(),
        });
    }

    let mut identity = identity.value().clone();
    let object = identity
        .as_object_mut()
        .expect("identity reader returns an object");
    object.remove("blocked");
    object.insert("updated_at".to_owned(), Value::String(now_iso()));
    let operation = update_operation();
    let saved = save_entity_identity(journal_root, entity_id, &identity, Some(&operation))?;
    Ok(saved
        .event
        .expect("an explicit operation always produces a history event"))
}

/// Remove one resolved entity directory and rebuild the durable identity-map cache.
pub fn delete_entity_directory(
    journal_root: &Path,
    entity_id: &str,
) -> Result<(), EntityLifecycleError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    let entity_dir = resolve_entity_dir(journal_root, entity_id)?;
    remove_dir_all(journal_root, &format!("entities/{entity_dir}"))
        .map_err(EntityStoreError::from)?;
    rewrite_identity_map_cache(journal_root)?;
    Ok(())
}

/// Restore one visible identity snapshot after merge and principal guards pass.
pub fn restore_journal_entity_version(
    journal_root: &Path,
    entity_id: &str,
    version_id: &str,
    caller: Option<Value>,
) -> Result<Value, EntityLifecycleError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    let entity_dir = resolve_entity_dir(journal_root, entity_id)?;
    let events = read_visible_history(journal_root, &entity_dir)?;
    let target = events
        .iter()
        .find(|event| event.value().get("version_id").and_then(Value::as_str) == Some(version_id))
        .ok_or_else(|| EntityLifecycleError::HistoryVersionNotFound {
            entity_id: entity_id.to_owned(),
            version_id: version_id.to_owned(),
        })?;
    guard_restore_does_not_cross_merge(target, &events).map_err(map_restore_guard_error)?;

    let snapshot = target.value().get("identity_after").cloned();
    let Some(snapshot) = snapshot.filter(Value::is_object) else {
        return Err(EntityLifecycleError::RestoreSnapshotNotObject {
            entity_id: entity_id.to_owned(),
            version_id: version_id.to_owned(),
        });
    };
    let snapshot_id = snapshot
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if snapshot_id.as_deref() != Some(entity_id) {
        return Err(EntityLifecycleError::RestoreSnapshotIdentityMismatch {
            entity_id: entity_id.to_owned(),
            version_id: version_id.to_owned(),
            snapshot_id,
        });
    }
    if snapshot.get("is_principal").is_some_and(value_is_truthy) {
        guard_restore_principal(journal_root, entity_id)?;
    }

    let operation = EntityOperationContext {
        kind: EntityOperationKind::Restore,
        caller: caller.unwrap_or(Value::Null),
        actor: Value::Null,
        metadata: json!({"restored_version_id": version_id}),
    };
    let saved = save_entity_identity(journal_root, entity_id, &snapshot, Some(&operation))?;
    Ok(saved
        .event
        .expect("an explicit restore operation always produces a history event"))
}

fn resolve_entity_dir(
    journal_root: &Path,
    entity_id: &str,
) -> Result<String, EntityLifecycleError> {
    read_identity_map(journal_root)?
        .resolved
        .get(entity_id)
        .cloned()
        .ok_or_else(|| EntityLifecycleError::EntityNotFound {
            entity_id: entity_id.to_owned(),
        })
}

fn guard_restore_principal(
    journal_root: &Path,
    entity_id: &str,
) -> Result<(), EntityLifecycleError> {
    for entity_dir in read_identity_group_map(journal_root)?
        .groups
        .into_values()
        .flatten()
    {
        let Some(identity) = read_entity_identity(journal_root, &entity_dir)? else {
            continue;
        };
        if identity.entity_id() != entity_id
            && identity
                .value()
                .get("is_principal")
                .is_some_and(value_is_truthy)
        {
            return Err(EntityLifecycleError::RestoreWouldCreateSecondPrincipal {
                entity_id: entity_id.to_owned(),
                existing_entity_id: identity.entity_id().to_owned(),
            });
        }
    }
    Ok(())
}

fn map_restore_guard_error(error: EntityStoreError) -> EntityLifecycleError {
    match error {
        EntityStoreError::RestoreTargetsRecordedMerge => {
            EntityLifecycleError::RestoreTargetsRecordedMerge
        }
        EntityStoreError::RestoreCrossesRecordedMerge => {
            EntityLifecycleError::RestoreCrossesRecordedMerge
        }
        other => EntityLifecycleError::Store(other),
    }
}

fn update_operation() -> EntityOperationContext {
    EntityOperationContext {
        kind: EntityOperationKind::Update,
        caller: Value::Null,
        actor: Value::Null,
        metadata: json!({}),
    }
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
