// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Map-resolved paths for durable entity records and memory directories.

use std::path::{Path, PathBuf};

use solstone_core_journal_io::ensure_directory;

use super::error::EntityStoreError;
use super::lifecycle::{EntityLifecycleError, resolve_entity_dir};
use super::paths::identity_path;

/// Return the identity-record path for a resolved entity id.
pub fn entity_path(journal_root: &Path, entity_id: &str) -> Result<PathBuf, EntityLifecycleError> {
    let entity_dir = resolve_entity_dir(journal_root, entity_id)?;
    identity_path(journal_root, &entity_dir).map_err(Into::into)
}

/// Return the memory directory for a resolved entity id, creating it when requested.
pub fn entity_memory_path(
    journal_root: &Path,
    entity_id: &str,
    create: bool,
) -> Result<PathBuf, EntityLifecycleError> {
    let path = entity_path(journal_root, entity_id)?;
    let directory = path
        .parent()
        .expect("identity path always has an entity directory")
        .to_path_buf();
    if create {
        ensure_directory(&directory).map_err(EntityStoreError::from)?;
    }
    Ok(directory)
}
