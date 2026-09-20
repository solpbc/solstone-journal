// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_journal_io::{AtomicWriteOptions, path_lexists, read_text, write_text};

use crate::hold_facet_trust_lock;

use super::declaration::require_declared_facet;
use super::error::{FacetStoreError, FacetWriteError};
use super::paths::{FacetContentKind, content_file_path};

/// Read a news markdown file without interpretation.
pub fn read_news_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
) -> Result<Option<String>, FacetStoreError> {
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::News,
        relative_path,
    )?;
    if !path_lexists(&path)? {
        return Ok(None);
    }
    read_text(&path, String::new())
        .map(Some)
        .map_err(Into::into)
}

/// Atomically replace a news markdown file without interpretation.
pub fn write_news_file(
    journal_root: &Path,
    facet_dir: &str,
    relative_path: &str,
    contents: &str,
) -> Result<(), FacetWriteError> {
    let _trust = hold_facet_trust_lock(journal_root)?;
    require_declared_facet(journal_root, facet_dir)?;
    let path = content_file_path(
        journal_root,
        facet_dir,
        FacetContentKind::News,
        relative_path,
    )?;
    write_text(&path, contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(FacetWriteError::ContentWrite)
}

/// Prepared newsletter replacement, retained in the admitting daily unit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreparedNewsReplacement {
    pub facet: String,
    pub facet_id: String,
    pub relative_path: String,
    pub before: Option<String>,
    pub after: String,
}

pub fn prepare_news_replacement(
    root: &Path,
    facet: &str,
    relative: &str,
    after: &str,
) -> Result<PreparedNewsReplacement, String> {
    let _trust = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    let facet_id = super::declaration::facet_write_identity(root, facet)?;
    Ok(PreparedNewsReplacement {
        facet: facet.into(),
        facet_id,
        relative_path: relative.into(),
        before: read_news_file(root, facet, relative).map_err(|e| e.to_string())?,
        after: after.into(),
    })
}

pub fn publish_news_replacement(
    root: &Path,
    batch: &PreparedNewsReplacement,
    allow_before: bool,
    receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let _trust = hold_facet_trust_lock(root).map_err(|e| e.to_string())?;
    super::declaration::require_facet_write_identity(root, &batch.facet, &batch.facet_id)?;
    let current =
        read_news_file(root, &batch.facet, &batch.relative_path).map_err(|e| e.to_string())?;
    if current.as_deref() != Some(batch.after.as_str()) {
        if !allow_before || current != batch.before {
            return Err("conflict: newsletter changed after preparation".into());
        }
        write_news_file(root, &batch.facet, &batch.relative_path, &batch.after)
            .map_err(|e| e.to_string())?;
    }
    receipt()
}
