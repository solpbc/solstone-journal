// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A speakers wipe in the trash: every voice file the journal keeps
//! ([`crate::artifact_wipe::wipe_paths`]), moved as one item.
//!
//! The trash (`solstone-core-trash`) moves the files; this is the wipe's side
//! of it.
//!
//! - **Hold:** the journal's whole-segment lease first, then the entity trust
//!   lock. The lease holds admission and the location lock, so a recording's
//!   trash and location uploads wait while a wipe moves, or comes back. Each
//!   file then takes its own lock as it moves, where its home is still there:
//!   a recording's segment lock, or the file's own lock beside it. Where the
//!   home is gone nothing is locked and nothing is created.
//! - **Recover** refuses while a home of its files waits in the trash (a
//!   recording, an import, a person), naming what to recover first. A home gone
//!   for good keeps its files with the item, and they go when it does; the
//!   recover records them erased.
//! - **References:** search is told what left and came back, and the discovery
//!   groups, a cache of pointers into the wiped embeddings, are cleared both
//!   ways so they're built again from what is there.
//!
//! People and recordings already in the trash keep their voice files: a wipe
//! reaches only what is in the journal.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_journal_io::{FileLock, LockOptions, hold_lock};
use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_retention::door::SegmentHold;
use solstone_core_retention::notify::IndexNotify;
use solstone_core_retention::receipt::{RemovedPath, Target};
pub use solstone_core_trash::TrashError;
use solstone_core_trash::{Change, Hold, Item, Kind, State};

use crate::artifact_wipe::{WipeReport, report, wipe_paths};

/// A hold's refusal as the trash page reads it: a compact JSON object whose
/// `code` the page puts in its own words.
fn refusal(fields: Value) -> String {
    fields.to_string()
}

/// A lock, the journal's lease, or another removal is in the way; trying again
/// later works. The detail goes to the log, not the owner.
pub fn busy(detail: impl std::fmt::Display) -> String {
    log::info!("speakers wipe: held: {detail}");
    refusal(json!({"code": "busy"}))
}

/// The `code` of a hold's refusal, for a route that answers the owner itself.
pub fn refusal_code(reason: &str) -> Option<String> {
    serde_json::from_str::<Value>(reason)
        .ok()?
        .get("code")?
        .as_str()
        .map(str::to_owned)
}

/// The trash's kind for a speakers wipe.
pub struct Speakers<'a> {
    index: &'a dyn IndexNotify,
}

/// What a committed wipe did.
#[derive(Debug)]
pub struct Wiped {
    /// What it took, by category.
    pub report: WipeReport,
    /// Its item in the trash: id and erase date. None when there was nothing
    /// to take.
    pub trash: Option<(String, String)>,
    /// Erased at once, as the owner asked.
    pub erased: bool,
    /// The owner asked it erased at once and the erase was refused; it waits
    /// in the trash.
    pub erase_failed: bool,
}

impl Wiped {
    /// The report's fields, with `trash` (`id`, `erase_at`) while it waits in
    /// the trash, or `erased`.
    pub fn to_json(&self) -> Value {
        let mut value = serde_json::to_value(&self.report).unwrap_or_else(|_| json!({}));
        if let Some(object) = value.as_object_mut() {
            if self.erased {
                object.insert("erased".to_owned(), Value::Bool(true));
            } else if let Some((id, erase_at)) = &self.trash {
                object.insert("trash".to_owned(), json!({"id": id, "erase_at": erase_at}));
                if self.erase_failed {
                    object.insert("erased".to_owned(), Value::Bool(false));
                    object.insert("erase_failed".to_owned(), Value::Bool(true));
                }
            }
        }
        value
    }
}

/// Move every voice file to the trash, telling the journal's own search, and
/// erase it at once when the owner asks.
pub fn wipe(journal: &Path, surface: Surface, erase: bool) -> Result<Wiped, TrashError> {
    let index = solstone_core_indexer_store::RetentionIndex::new(journal);
    Speakers::new(&index).wipe(journal, surface, erase)
}

impl<'a> Speakers<'a> {
    pub fn new(index: &'a dyn IndexNotify) -> Self {
        Self { index }
    }

    /// Move every voice file to the trash as one item, as the owner's wipe
    /// from `surface`, then erase it at once when `erase`.
    pub fn wipe(&self, journal: &Path, surface: Surface, erase: bool) -> Result<Wiped, TrashError> {
        let paths = wipe_paths(journal).map_err(|error| TrashError::Io(error.to_string()))?;
        if paths.is_empty() {
            return Ok(nothing());
        }
        let item = match solstone_core_trash::detach(
            journal,
            self,
            Unit::Speakers,
            Action::SpeakersWipe,
            surface,
            paths,
        ) {
            Ok(item) => item,
            // Worked out again under the hold: nothing there now, or a file
            // went between the two looks.
            Err(TrashError::Missing(_)) => {
                let again =
                    wipe_paths(journal).map_err(|error| TrashError::Io(error.to_string()))?;
                if again.is_empty() {
                    return Ok(nothing());
                }
                return Err(TrashError::Held(busy(
                    "the voice files changed under the wipe",
                )));
            }
            // It left the journal and is in the trash; a step after the move
            // failed, and the trash's sweep finishes it. Not erased now.
            Err(TrashError::Unfinished(id)) => {
                let item =
                    solstone_core_trash::item(journal, &id)?.ok_or(TrashError::NotInTrash)?;
                return Ok(Wiped {
                    report: held_report(journal, &item),
                    trash: Some((item.id, item.erase_at)),
                    erased: false,
                    erase_failed: erase,
                });
            }
            Err(error) => return Err(error),
        };
        let report = held_report(journal, &item);
        let erased = erase
            && match solstone_core_trash::erase(journal, &item.id, Action::SpeakersWipe, surface) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("trash item {}: erase now was refused: {error}", item.id);
                    false
                }
            };
        Ok(Wiped {
            report,
            trash: Some((item.id, item.erase_at)),
            erased,
            erase_failed: erase && !erased,
        })
    }
}

fn nothing() -> Wiped {
    Wiped {
        report: report(false, &[], |_| 0),
        trash: None,
        erased: false,
        erase_failed: false,
    }
}

/// What an item holds, sized from its own copies.
fn held_report(journal: &Path, item: &Item) -> WipeReport {
    report(false, &item.paths, |path| {
        solstone_core_trash::held_file(journal, &item.id, path)
            .ok()
            .and_then(|held| std::fs::symlink_metadata(held).ok())
            .map_or(0, |metadata| metadata.len())
    })
}

/// A recording that is there, and not only the marker a removal left or a
/// removal of it part-way through. Unreadable counts as there.
pub(crate) fn live_segment(journal: &Path, segment: &str) -> bool {
    let place = journal.join(segment);
    place.is_dir()
        && !matches!(
            solstone_core_segment::owner_deletion_state(&place),
            Ok(state) if state != solstone_core_segment::OwnerDeletionState::Live
        )
}

/// The recording folder a held file sits in: its parent, or for a label or
/// correction, the folder above `agents/` or `talents/`.
fn segment_of(path: &str) -> Option<String> {
    if !path.starts_with("chronicle/") {
        return None;
    }
    let (parent, _) = path.rsplit_once('/')?;
    match parent.rsplit_once('/') {
        Some((segment, "agents" | "talents")) => Some(segment.to_owned()),
        _ => Some(parent.to_owned()),
    }
}

/// The entity folder a held file sits in.
fn entity_of(path: &str) -> Option<String> {
    let rest = path.strip_prefix("entities/")?;
    let (dir, _) = rest.split_once('/')?;
    Some(format!("entities/{dir}"))
}

/// The folder each held file needs to come back into.
fn homes(paths: &[String]) -> BTreeSet<String> {
    paths
        .iter()
        .filter_map(|path| segment_of(path).or_else(|| entity_of(path)))
        .collect()
}

/// A recording's place, for the lease and its lock. A folder literally named
/// `_default` under a day has none; it moves under the lease alone.
fn target_of(segment: &str) -> Option<Target> {
    solstone_core_retention::layout::parse_segment_rel(segment).map(|(day, stream, dir)| Target {
        day,
        stream,
        dir,
    })
}

fn targets_of(paths: &[String]) -> Vec<Target> {
    let segments: BTreeSet<String> = paths.iter().filter_map(|path| segment_of(path)).collect();
    segments
        .iter()
        .filter_map(|segment| target_of(segment))
        .collect()
}

/// The lease, the entity trust lock, one file's lock at a time, and, when the
/// wipe is coming back, what it needs to be out of the trash.
struct SpeakersHold {
    /// The one lock held now, and what it is for: a recording folder, or one
    /// file. Dropped first.
    moving: RefCell<Option<(String, FileLock)>>,
    _trust: solstone_core_entity::EntityTrustLock,
    segments: SegmentHold,
    journal: PathBuf,
    /// The item and the homes of its files, when it is coming back.
    returning: Option<(String, BTreeSet<String>)>,
}

impl Hold for SpeakersHold {
    fn check(&self) -> Result<(), String> {
        if let Some((id, homes)) = &self.returning {
            let waiting = waiting_homes(&self.journal, id, homes).map_err(busy)?;
            if !waiting.is_empty() {
                return Err(waits_wipe(&waiting));
            }
        }
        self.segments.check().map_err(busy)
    }

    fn moving(&self, path: &str) -> Result<(), String> {
        let segment = segment_of(path);
        let key = segment.clone().unwrap_or_else(|| path.to_owned());
        // The files of one recording move together under its one lock.
        if self
            .moving
            .borrow()
            .as_ref()
            .is_some_and(|(held, _)| *held == key)
        {
            return self
                .segments
                .check_one(segment.as_deref().and_then(target_of).as_ref())
                .map_err(busy);
        }
        // Let go of the last lock before taking the next.
        self.moving.borrow_mut().take();
        match segment {
            Some(segment) => {
                let target = target_of(&segment);
                self.segments.check_one(target.as_ref()).map_err(busy)?;
                // A recording folder that's gone has no home: no lock, and
                // nothing made where it was.
                if let Some(target) = target
                    && self.journal.join(&segment).is_dir()
                {
                    let lock = self.segments.lock_one(&target).map_err(busy)?;
                    *self.moving.borrow_mut() = Some((key, lock));
                }
            }
            None => {
                self.segments.check_one(None).map_err(busy)?;
                // The file's own lock, which its writers take, while its folder
                // is there: a person erased or merged away gets no folder back.
                let place = self.journal.join(path);
                if place.parent().is_some_and(Path::is_dir) {
                    let lock = hold_lock(&place, LockOptions::default()).map_err(busy)?;
                    *self.moving.borrow_mut() = Some((key, lock));
                }
            }
        }
        Ok(())
    }
}

/// The items other than `id`, in any state but erasing, that hold one of
/// `homes` or a folder above one: what has to come back first. Read once, not
/// once per home, since a wipe can have a home for every recording.
fn waiting_homes(journal: &Path, id: &str, homes: &BTreeSet<String>) -> Result<Vec<Item>, String> {
    let mut items = solstone_core_trash::moving(journal).map_err(|error| error.to_string())?;
    items.extend(solstone_core_trash::list(journal).map_err(|error| error.to_string())?);
    let mut found: Vec<Item> = Vec::new();
    for item in items {
        if item.id == id || found.iter().any(|seen| seen.id == item.id) {
            continue;
        }
        let holds = item.paths.iter().any(|held| {
            let below = format!("{held}/");
            homes.contains(held)
                || homes
                    .range(below.clone()..)
                    .next()
                    .is_some_and(|home| home.starts_with(&below))
        });
        if holds {
            found.push(item);
        }
    }
    found.sort_by(|left, right| left.trashed_at.cmp(&right.trashed_at));
    Ok(found)
}

/// What has to come back before the wipe can, counted by kind: recordings,
/// imports and people, named by the first person's own name.
fn waits_wipe(waiting: &[Item]) -> String {
    let (mut segments, mut imports, mut people) = (0_usize, 0_usize, 0_usize);
    let mut name = None;
    for item in waiting {
        match &item.unit {
            Unit::Import { .. } | Unit::Workout { .. } => imports += 1,
            Unit::Entity => {
                people += 1;
                if name.is_none() {
                    name = item.label.clone();
                }
            }
            _ => segments += 1,
        }
    }
    let mut fields = json!({
        "code": "waits_wipe",
        "segments": segments,
        "imports": imports,
        "people": people,
    });
    if people > 0
        && let Some(object) = fields.as_object_mut()
    {
        object.insert("name".to_owned(), name.map_or(Value::Null, Value::String));
    }
    refusal(fields)
}

impl Kind for Speakers<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Speakers)
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        // Admission outermost, then the entity trust lock, as other
        // whole-journal work takes them.
        let segments =
            SegmentHold::acquire_lease(journal, &targets_of(&item.paths)).map_err(busy)?;
        let trust = solstone_core_entity::hold_entity_trust_lock(journal)
            .map_err(|error| busy(format!("{error:?}")))?;
        let returning =
            (item.state != State::Detaching).then(|| (item.id.clone(), homes(&item.paths)));
        Ok(Box::new(SpeakersHold {
            moving: RefCell::new(None),
            _trust: trust,
            segments,
            journal: journal.to_path_buf(),
            returning,
        }))
    }

    fn current_paths(
        &self,
        journal: &Path,
        _unit: &Unit,
        _chosen: &[String],
    ) -> Option<Vec<String>> {
        // Under the hold, so a voice file written since the first look moves
        // too. A walk that fails keeps the first look's paths.
        match wipe_paths(journal) {
            Ok(paths) => Some(paths),
            Err(error) => {
                log::warn!("speakers wipe: the voice files couldn't be listed again: {error}");
                None
            }
        }
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        std::fs::symlink_metadata(journal.join(path)).is_ok_and(|metadata| metadata.is_file())
    }

    fn returns(&self, journal: &Path, _item: &Item, path: &str) -> bool {
        if let Some(segment) = segment_of(path) {
            // A recording erased since leaves its marker folder behind.
            return live_segment(journal, &segment);
        }
        if let Some(entity) = entity_of(path) {
            let dir = entity.strip_prefix("entities/").unwrap_or(&entity);
            return solstone_core_entity::entity_identity_path(journal, dir)
                .is_ok_and(|identity| identity.is_file());
        }
        true
    }

    fn references(
        &self,
        journal: &Path,
        _item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        crate::discovery_scan::clear_discovery_cache(journal, &|| Ok(()))
            .map_err(|error| format!("the voice groups weren't cleared: {error}"))?;
        match change {
            Change::Detached => {
                let removed: Vec<RemovedPath> =
                    files.iter().cloned().map(RemovedPath::confirmed).collect();
                self.index.paths_removed(&removed).map(|_| ())
            }
            Change::Attached => self.index.paths_returned(files),
        }
        .map_err(|error| format!("search was not updated: {}", error.reason))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code"
)]
#[path = "speakers_trash_tests.rs"]
mod tests;
