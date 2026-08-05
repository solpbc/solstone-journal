// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_journal_io::{AtomicWriteOptions, path_lexists, read_text, write_text};

use crate::hold_facet_trust_lock;

use super::error::{FacetStoreError, FacetWriteError};
use super::paths::facet_entity_observations_path;

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
