// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A recording in the trash: one segment directory, agent memories included.
//!
//! The trash (`solstone-core-trash`) moves the files; this is the recording's
//! side of it. Its hold is the door's own for a whole segment (admission,
//! location lock, segment lock), so a recording moves only when nothing else is
//! part-way through the journal. Its references:
//!
//! - **Where it was**: a tombstone, the one every reader already treats as a
//!   removed recording (a stream's chain, backup teardown, memory reads, import
//!   pages). Recovery sets the tombstone aside, and a recording that arrives
//!   again replaces it.
//! - **The search index**, told about every file that left or came back.
//! - **The day's stream marker**, which queues the day's follow-up processing
//!   (location rebuilds from what remains).
//!
//! What was made from the recording elsewhere is not reached.

use std::path::Path;

use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_trash::{Change, Hold, Item, Kind, TrashError};

use crate::door::SegmentHold;
use crate::notify::IndexNotify;
use crate::receipt::{RemovedPath, Target};

/// The trash's kind for recordings.
pub struct Recordings<'a> {
    index: &'a dyn IndexNotify,
}

impl<'a> Recordings<'a> {
    pub fn new(index: &'a dyn IndexNotify) -> Self {
        Self { index }
    }

    /// Move one recording to the trash, as the owner's `action` from `surface`.
    pub fn trash(
        &self,
        journal: &Path,
        target: &Target,
        unit: Unit,
        action: Action,
        surface: Surface,
    ) -> Result<Item, TrashError> {
        let path = crate::layout::segment_rel(&target.day, &target.stream, &target.dir);
        solstone_core_trash::detach(journal, self, unit, action, surface, vec![path])
    }
}

/// The recording an item holds, from where it was.
fn target_of(item: &Item) -> Option<Target> {
    let path = item.paths.first()?;
    let (day, stream, dir) = crate::layout::parse_segment_rel(path)?;
    Some(Target { day, stream, dir })
}

impl Hold for SegmentHold {
    fn check(&self) -> Result<(), String> {
        SegmentHold::check(self)
    }
}

impl Kind for Recordings<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Segment { .. } | Unit::Memory { .. })
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let target = target_of(item)
            .ok_or_else(|| "this isn't a recording your journal can place".to_owned())?;
        Ok(Box::new(SegmentHold::acquire(journal, &target)?))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        // A live segment: there, and not only the tombstone a removal left or
        // a removal of it part-way through. Unreadable counts as there, so it
        // is never taken for gone.
        if crate::layout::parse_segment_rel(path).is_none() {
            return false;
        }
        let place = journal.join(path);
        if !place.is_dir() {
            return false;
        }
        !matches!(
            solstone_core_segment::owner_deletion_state(&place),
            Ok(state) if state != solstone_core_segment::OwnerDeletionState::Live
        )
    }

    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        let target = target_of(item)
            .ok_or_else(|| "this isn't a recording your journal can place".to_owned())?;
        if change == Change::Detached {
            crate::door::place_tombstone(
                journal,
                &target,
                files.to_vec(),
                &item.trashed_at,
                &item.id,
            )
            .map_err(|error| format!("no tombstone was left: {error}"))?;
        }
        match change {
            Change::Detached => {
                let removed: Vec<RemovedPath> =
                    files.iter().cloned().map(RemovedPath::confirmed).collect();
                self.index.paths_removed(&removed).map(|_| ())
            }
            Change::Attached => self.index.paths_returned(files),
        }
        .map_err(|error| format!("search was not updated: {}", error.reason))?;
        solstone_core_journal_io::bump_stream_marker(journal, &target.day)
            .map(|_| ())
            .map_err(|error| format!("the day was not queued for follow-up: {error}"))
    }
}
