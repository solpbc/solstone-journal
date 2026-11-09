// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retention's index-notify boundary, implemented over the durable to-do list.

use std::path::Path;

use solstone_core_retention::notify::{IndexNotify, NotifyError, PruneCounts};
use solstone_core_retention::receipt::RemovedPath;

use crate::todo::enqueue_path;

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
        for removed in removed {
            enqueue_path(self.journal, Path::new(removed.as_str())).map_err(|error| {
                NotifyError {
                    reason: format!("the search update could not be queued: {error}"),
                }
            })?;
        }
        Ok(PruneCounts::default())
    }

    fn paths_returned(&self, returned: &[String]) -> Result<(), NotifyError> {
        // The drain reads each path afresh: present is indexed, absent pruned.
        for path in returned {
            enqueue_path(self.journal, Path::new(path)).map_err(|error| NotifyError {
                reason: format!("the search update could not be queued: {error}"),
            })?;
        }
        Ok(())
    }
}
