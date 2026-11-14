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
//! - **What was made from it**, through the [`MadeFrom`] the kind was built
//!   with (a recording, not an agent's memory: nothing is made from a memory).

use std::path::Path;

use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_trash::{Change, Hold, Item, Kind, TrashError};

use crate::door::SegmentHold;
use crate::made_from::MadeFrom;
use crate::notify::IndexNotify;
use crate::receipt::{RemovedPath, Target};

/// The trash's kind for recordings.
pub struct Recordings<'a> {
    index: &'a dyn IndexNotify,
    made: &'a dyn MadeFrom,
}

impl<'a> Recordings<'a> {
    pub fn new(index: &'a dyn IndexNotify, made: &'a dyn MadeFrom) -> Self {
        Self { index, made }
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

struct RecordingHold {
    segment: SegmentHold,
    _entity: solstone_core_entity::EntityTrustLock,
}
impl Hold for RecordingHold {
    fn check(&self) -> Result<(), String> {
        self.segment.check()
    }
}

impl Kind for Recordings<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Segment { .. } | Unit::Memory { .. })
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let target = target_of(item)
            .ok_or_else(|| "this isn't a recording your journal can place".to_owned())?;
        let segment = SegmentHold::acquire(journal, &target)?;
        let entity = solstone_core_entity::faceprints::hold_source_removal(journal)?;
        Ok(Box::new(RecordingHold {
            segment,
            _entity: entity,
        }))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        if path.starts_with("config/evernote-imports/") {
            return journal.join(path).is_file();
        }
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

    fn companions(&self, journal: &Path, item: &Item) -> Result<Vec<String>, String> {
        evernote_companions(journal, &item.paths)
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
            solstone_core_entity::faceprints::remove_segment_rows(
                journal,
                &crate::layout::segment_rel(&target.day, &target.stream, &target.dir),
            )?;
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
            .map_err(|error| format!("the day was not queued for follow-up: {error}"))?;
        if matches!(item.unit, Unit::Segment { .. }) {
            self.made
                .recordings_changed(journal, item, change, std::slice::from_ref(&target))?;
        }
        Ok(())
    }
}

/// Evernote's full-text comparison records for the whole note segments moving
/// now. A shared import segment is absent from `paths`, so its records stay.
/// Record paths come from the private store, never from fields in its JSON.
pub fn evernote_companions(journal: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    use std::collections::BTreeSet;
    use std::fs;

    let segments: BTreeSet<_> = paths
        .iter()
        .filter_map(|path| crate::layout::parse_segment_rel(path))
        .filter(|(_, stream, _)| stream == "import.evernote")
        .map(|(day, _, segment)| (day, segment))
        .collect();
    if segments.is_empty() {
        return Ok(Vec::new());
    }
    let store = journal.join("config/evernote-imports");
    let notebooks = match fs::read_dir(&store) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", store.display())),
    };
    let mut found = Vec::new();
    for notebook in notebooks {
        let notebook = notebook.map_err(|error| error.to_string())?;
        if !notebook
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let notes_dir = notebook.path().join("notes");
        let notes = match fs::read_dir(&notes_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{}: {error}", notes_dir.display())),
        };
        for entry in notes {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let read = || -> Result<serde_json::Value, String> {
                let bytes = fs::read(&path).map_err(|error| error.to_string())?;
                serde_json::from_slice(&bytes).map_err(|error| error.to_string())
            };
            let note = read().map_err(|error| format!("{}: {error}", path.display()))?;
            let day = note.get("day").and_then(serde_json::Value::as_str);
            let segment = note.get("segment").and_then(serde_json::Value::as_str);
            let (Some(day), Some(segment)) = (day, segment) else {
                return Err(format!("{}: missing note placement", path.display()));
            };
            if segments.contains(&(day.to_owned(), segment.to_owned())) {
                let notebook_name = notebook.file_name();
                let note_name = entry.file_name();
                let notebook_name = notebook_name.to_str().ok_or("invalid notebook filename")?;
                let note_name = note_name.to_str().ok_or("invalid note filename")?;
                found.push(format!(
                    "config/evernote-imports/{notebook_name}/notes/{note_name}"
                ));
            }
        }
    }
    found.sort();
    Ok(found)
}
