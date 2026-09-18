// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::durability::{
    ArtifactId, DurableObservation, DurableRead, observe_json_durable, read_json_durable, set_aside,
};
use solstone_core_journal_io::{ReadError, contained_path, path_lexists};

use super::error::EntityStoreError;
use super::paths::identity_path;

/// One effective entity identity with its durable JSON object intact.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentitySnapshot {
    entity_id: String,
    written: bool,
    value: Value,
}

impl IdentitySnapshot {
    /// Effective written-or-directory identity id.
    pub fn entity_id(&self) -> &str {
        &self.entity_id
    }

    /// Whether the effective id came from a non-empty durable `id` field.
    pub fn was_written(&self) -> bool {
        self.written
    }

    /// Full identity object, including the effective stamped id.
    pub fn value(&self) -> &Value {
        &self.value
    }
}

/// Read one durable identity, treating missing or empty JSON as absent.
pub fn read_entity_identity(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<Option<IdentitySnapshot>, EntityStoreError> {
    let path = identity_path(journal_root, entity_dir)?;
    let value = match observe_json_durable(ArtifactId::Entity, &path) {
        DurableObservation::Present(value) => value,
        DurableObservation::Absent
        | DurableObservation::Malformed { .. }
        | DurableObservation::Unreadable { .. } => return Ok(None),
    };
    identity_snapshot(entity_dir, value)
}

pub(super) fn read_entity_identity_repairing(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<Option<IdentitySnapshot>, EntityStoreError> {
    let path = identity_path(journal_root, entity_dir)?;
    let value: Value = match read_json_durable(ArtifactId::Entity, &path).map_err(|source| {
        EntityStoreError::from(ReadError::Io {
            path: path.clone(),
            source,
        })
    })? {
        DurableRead::Present(value) => value,
        DurableRead::Absent | DurableRead::SetAside(_) | DurableRead::Unreadable { .. } => {
            return Ok(None);
        }
    };
    identity_snapshot_repairing(path, entity_dir, value)
}

fn identity_snapshot(
    entity_dir: &str,
    mut value: Value,
) -> Result<Option<IdentitySnapshot>, EntityStoreError> {
    if value.is_null() {
        return Ok(None);
    }
    let Some(object) = value.as_object_mut() else {
        return Ok(None);
    };
    let written = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    let entity_id = written.clone().unwrap_or_else(|| entity_dir.to_owned());
    object.insert("id".to_owned(), Value::String(entity_id.clone()));
    Ok(Some(IdentitySnapshot {
        entity_id,
        written: written.is_some(),
        value,
    }))
}

fn identity_snapshot_repairing(
    path: PathBuf,
    entity_dir: &str,
    value: Value,
) -> Result<Option<IdentitySnapshot>, EntityStoreError> {
    if value.is_null() {
        return Ok(None);
    }
    if !value.is_object() {
        set_aside(&path).map_err(|source| {
            EntityStoreError::from(ReadError::Io {
                path: path.clone(),
                source,
            })
        })?;
        return Ok(None);
    }
    identity_snapshot(entity_dir, value)
}

/// Return whether the literal identity destination exists, including an empty
/// file, JSON `null`, or a dangling symlink.
pub fn entity_identity_destination_occupied(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<bool, EntityStoreError> {
    let path = identity_destination_path(journal_root, entity_dir)?;
    path_lexists(&path).map_err(Into::into)
}

pub(super) fn identity_destination_path(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<PathBuf, EntityStoreError> {
    let directory = contained_path(journal_root, &format!("entities/{entity_dir}"))?;
    Ok(directory.join("entity.json"))
}
