// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_journal_io::{AtomicWriteOptions, path_lexists, read_text, write_text};

use crate::hold_facet_trust_lock;

use super::error::{FacetStoreError, FacetWriteError};
use super::paths::{FacetContentKind, content_file_path};

/// Read a facet log JSONL file without interpretation.
pub fn read_log_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
) -> Result<Option<String>, FacetStoreError> {
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::Logs,
        relative_path,
    )?;
    if !path_lexists(&path)? {
        return Ok(None);
    }
    read_text(&path, String::new())
        .map(Some)
        .map_err(Into::into)
}

/// Atomically replace a facet log JSONL file without interpretation.
pub fn write_log_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
    contents: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::Logs,
        relative_path,
    )?;
    write_text(&path, contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(FacetWriteError::ContentWrite)
}
