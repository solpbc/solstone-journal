// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Transcript-sidecar retry handling.

use std::fs;
use std::path::Path;

use crate::TranscribeError;

/// Remove a persisted embedding sidecar that has no matching transcript.
pub(crate) fn remove_orphan_npz(jsonl_path: &Path, npz_path: &Path) -> Result<(), TranscribeError> {
    if npz_path.is_file() && !jsonl_path.exists() {
        fs::remove_file(npz_path).map_err(|error| TranscribeError::OrphanNpzRemove {
            path: npz_path.to_path_buf(),
            detail: error.to_string(),
        })?;
    }
    Ok(())
}
