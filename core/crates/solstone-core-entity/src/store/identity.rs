// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_journal_io::{MalformedPolicy, contained_path, path_lexists, read_json};

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
    let mut value: Value = read_json(&path, Value::Null, MalformedPolicy::Raise)?;
    if value.is_null() {
        return Ok(None);
    }
    let Some(object) = value.as_object_mut() else {
        return Err(EntityStoreError::IdentityNotObject { path });
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
