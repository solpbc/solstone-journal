// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retention's index-notify boundary, implemented over this crate's prune.

use std::path::Path;

use solstone_core_retention::notify::{IndexNotify, NotifyError, PruneCounts};
use solstone_core_retention::receipt::RemovedPath;

use crate::db::prune_by_paths;

/// The search index, as retention's removal door addresses it.
pub struct RetentionIndex<'a> {
    journal: &'a Path,
}

impl<'a> RetentionIndex<'a> {
    pub fn new(journal: &'a Path) -> Self {
        Self { journal }
    }
}

impl IndexNotify for RetentionIndex<'_> {
    fn paths_removed(&self, removed: &[RemovedPath]) -> Result<PruneCounts, NotifyError> {
        let rels: Vec<&str> = removed.iter().map(RemovedPath::as_str).collect();
        match prune_by_paths(self.journal, &rels) {
            Ok(Some(counts)) => Ok(PruneCounts {
                chunks: counts.chunks,
                files: counts.files,
            }),
            Ok(None) => Ok(PruneCounts::default()),
            Err(error) => Err(NotifyError {
                reason: format!("the search index could not be updated: {error}"),
            }),
        }
    }
}
