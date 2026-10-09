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
    EntityAlreadyExists {
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
            Self::EntityNotFound { entity_id } => {
                write!(formatter, "entity not found: {entity_id}")
            }
            Self::EntityAlreadyExists { entity_id } => {
                write!(formatter, "entity already exists: {entity_id}")
            }
            Self::EntityNotBlocked { entity_id } => {
                write!(formatter, "entity is not blocked: {entity_id}")
            }
            Self::HistoryVersionNotFound {
                entity_id,
                version_id,
            } => write!(
                formatter,
                "history version not found for {entity_id}: {version_id}"
            ),
            Self::RestoreTargetsRecordedMerge => formatter
                .write_str("that version is a merge, and a merge can't be restored or undone"),
            Self::RestoreCrossesRecordedMerge => formatter
                .write_str("that version is from before a merge, and a merge can't be undone"),
            Self::RestoreSnapshotNotObject {
                entity_id,
                version_id,
            } => write!(
                formatter,
                "restore snapshot is not an object for {entity_id}: {version_id}"
            ),
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
            | Self::EntityAlreadyExists { .. }
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

/// Whether an identity is the journal's principal: marked `is_principal: true`
/// and of type `Person`. A flag on any other type, or any value other than
/// JSON `true`, names no one.
pub fn identity_is_principal(identity: &Value) -> bool {
    identity.get("is_principal") == Some(&Value::Bool(true))
        && identity.get("type").and_then(Value::as_str) == Some("Person")
}

/// Remove an `is_principal` that names no one: anything but `true` on a
/// `Person`. For a write, `before` is the identity it replaces, and a flag
/// does not survive a change of type in either direction. Every identity read
/// applies this with no `before`, and every identity write with one, so a flag
/// left at rest on another type is never seen and is dropped when the entity
/// is next written. Returns whether a set flag (any truthy value) was removed.
pub(crate) fn drop_stray_principal_flag(before: Option<&Value>, identity: &mut Value) -> bool {
    let Some(object) = identity.as_object_mut() else {
        return false;
    };
    let Some(flag) = object.get("is_principal") else {
        return false;
    };
    let is_person = |value: Option<&Value>| value.and_then(Value::as_str) == Some("Person");
    let keeps = flag == &Value::Bool(true)
        && is_person(object.get("type"))
        && before.is_none_or(|before| is_person(before.get("type")));
    if keeps {
        return false;
    }
    object
        .remove("is_principal")
        .is_some_and(|flag| value_is_truthy(&flag))
}

/// Every live entity marked as the principal, with its folder, in folder
/// order. Each identity carries its effective id in `id`.
fn principal_identities(journal_root: &Path) -> Result<Vec<(String, Value)>, EntityStoreError> {
    let mut found = Vec::new();
    for (entity_id, entity_dir) in read_identity_map(journal_root)?.resolved {
        let Some(identity) = read_entity_identity(journal_root, &entity_dir)? else {
            continue;
        };
        if identity_is_principal(identity.value()) {
            let mut value = identity.value().clone();
            if let Some(object) = value.as_object_mut() {
                object.insert("id".to_owned(), Value::String(entity_id));
            }
            found.push((entity_dir, value));
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(found)
}

/// The journal's principal and the folder that holds it: the one live,
/// unblocked `Person` marked as the principal, as speaker admission resolves
/// it. With none, or with more than one, no one is the principal.
pub fn journal_principal_entry(
    journal_root: &Path,
) -> Result<Option<(String, Value)>, EntityStoreError> {
    let mut found = principal_identities(journal_root)?
        .into_iter()
        .filter(|(_, identity)| !identity.get("blocked").is_some_and(value_is_truthy))
        .collect::<Vec<_>>();
    if found.len() != 1 {
        return Ok(None);
    }
    Ok(Some(found.remove(0)))
}

/// The journal's principal identity, as [`journal_principal_entry`] resolves it.
pub fn read_journal_principal(journal_root: &Path) -> Result<Option<Value>, EntityLifecycleError> {
    Ok(journal_principal_entry(journal_root)?.map(|(_, identity)| identity))
}

/// Whether any live `Person` is marked as the principal, blocked or not, so
/// that no second one is marked beside it.
pub fn has_journal_principal(journal_root: &Path) -> Result<bool, EntityLifecycleError> {
    Ok(!principal_identities(journal_root)?.is_empty())
}

/// Mark the one person named as the owner as the journal's principal, when
/// the journal has none yet.
///
/// Journals started before entity creation marked the owner hold a person
/// with the owner's configured name and no principal. This adopts that person
/// only when exactly one admissible person matches the preferred name, full
/// name or an alias; with none or several it changes nothing. Returns the
/// adopted entity's id.
pub fn adopt_configured_principal(
    journal_root: &Path,
) -> Result<Option<String>, EntityLifecycleError> {
    let _trust = hold_entity_trust_lock(journal_root)?;
    if has_journal_principal(journal_root)? {
        return Ok(None);
    }
    let names = super::create::journal_identity_names(journal_root);
    if names.is_empty() {
        return Ok(None);
    }
    let mut matches = super::journal_entities::load_all_journal_entities(journal_root)?
        .into_iter()
        .filter(super::journal_entities::is_admissible_person)
        .filter(|entity| {
            let name = entity
                .value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let aka = entity
                .value
                .get("aka")
                .and_then(Value::as_array)
                .map(|aka| {
                    aka.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                });
            super::derived::entity_matches_identity_name(name, aka.as_deref(), &names)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Ok(None);
    }
    let entity = matches.remove(0);
    let mut identity = entity.value;
    let object = identity
        .as_object_mut()
        .expect("identity reader returns an object");
    object.insert("is_principal".to_owned(), Value::Bool(true));
    save_entity_identity(
        journal_root,
        &entity.id,
        &identity,
        Some(&update_operation()),
    )?;
    Ok(Some(entity.id))
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
    if identity_is_principal(&snapshot) {
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

pub(crate) fn resolve_entity_dir(
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
        if identity.entity_id() != entity_id && identity_is_principal(identity.value()) {
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

pub(crate) fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
