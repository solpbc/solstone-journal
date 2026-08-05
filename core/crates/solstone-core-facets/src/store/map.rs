// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_journal_io::{DirEntryKind, list_dir_entries, path_lexists};

use super::error::FacetStoreError;
use super::paths::{facet_entities_dir, facet_entity_link_path};

/// List immediate facet entity directories that contain an `entity.json` relationship.
pub fn list_facet_entity_directories(
    journal_root: &Path,
    facet_dir: &str,
) -> Result<Vec<String>, FacetStoreError> {
    let entities_dir = facet_entities_dir(journal_root, facet_dir)?;
    let mut directories = Vec::new();
    for entry in list_dir_entries(&entities_dir)? {
        if entry.kind != DirEntryKind::Directory {
            continue;
        }
        let entity_dir = entry.name.to_string_lossy().into_owned();
        let relationship_path = facet_entity_link_path(journal_root, facet_dir, &entity_dir)?;
        if path_lexists(&relationship_path)? {
            directories.push(entity_dir);
        }
    }
    Ok(directories)
}
