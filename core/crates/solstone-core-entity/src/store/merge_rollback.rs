// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_journal_io::JournalSnapshot;
use solstone_core_journal_io::SnapshotError;
use solstone_core_journal_io::capture_snapshot;
use solstone_core_journal_io::restore_snapshot;

#[derive(Debug, Default)]
pub(crate) struct MergeRollback {
    snapshots: Vec<JournalSnapshot>,
}

impl MergeRollback {
    pub(super) fn capture(&mut self, journal: &Path, path: &str) -> Result<(), SnapshotError> {
        self.snapshots.push(capture_snapshot(journal, path)?);
        Ok(())
    }

    pub(super) fn restore(&self, journal: &Path) -> Result<(), SnapshotError> {
        for snapshot in self.snapshots.iter().rev() {
            restore_snapshot(journal, snapshot)?;
        }
        Ok(())
    }
}
