// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A segment an import made, in the trash: what deleting it takes, and what it
//! needs to come back.
//!
//! An import keeps the file it was given in its record, `imports/<id>/`, and
//! that file still holds every segment the import made. So:
//!
//! - **Deleting an import's last segment deletes the import**: the segment, the
//!   record with its file, and its row on the Import page, as one item, the
//!   same one the Import page's delete makes ([`choose`]).
//! - **Deleting any other segment of it leaves the record**, and the segment's
//!   item names the record as what it needs ([`ImportSegments`]). While the
//!   import waits in the trash, the segment can't come back; erasing the import
//!   erases it too; the import never erases before it (`solstone-core-trash`,
//!   `Item::needs`).
//!
//! Only an import one import alone lists counts. A body import (Strava, Apple
//! Health, Oura) keeps its own delete, and a journal archive merge isn't one
//! thing, so their segments delete as segments. A segment two imports list
//! (calendar files placed by fixed keys) never takes either import.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_retention::notify::IndexNotify;
use solstone_core_retention::recording_trash::Recordings;
use solstone_core_trash::{Change, Chosen, Hold, Item, Kind, TrashError};

use crate::import_trash::{self, ImportThings};
use crate::metadata::{AttemptHolder, attempt_holder};

/// Why a segment can't come back yet: its import is in the trash. A code the
/// pages give their own words.
pub const IMPORT_IN_TRASH: &str = "import_in_trash";

/// Why a segment can't come back: its import was erased. Erasing an import
/// erases its segments with it, so only a stop between the two leaves one.
pub const IMPORT_GONE: &str = "import_gone";

/// Every segment an import lists, by the imports that list it: only imports
/// that delete as one thing and aren't body imports.
pub fn listing(journal: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut by_segment: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(journal.join("imports")) else {
        return by_segment;
    };
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !import_trash::is_import_id(&id) || !entry.path().is_dir() {
            continue;
        }
        let source = import_trash::source_of(journal, &id);
        if !counts(&source) {
            continue;
        }
        for segment in import_trash::listed_segments(journal, &id, &source) {
            by_segment.entry(segment).or_default().insert(id.clone());
        }
    }
    by_segment
}

/// Whether an import of `source` is one a segment delete can take.
fn counts(source: &str) -> bool {
    import_trash::deletes_here(source) && !crate::body_trash::is_body_source(source)
}

/// The one import that lists `segment`, if exactly one does.
fn sole_lister<'l>(listing: &'l BTreeMap<String, BTreeSet<String>>, segment: &str) -> Option<&'l str> {
    let listers = listing.get(segment)?;
    match (listers.first(), listers.len()) {
        (Some(id), 1) => Some(id.as_str()),
        _ => None,
    }
}

/// What deleting some segments would do to an import that made any of them:
/// for the delete dialog, before the owner confirms.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ImportPreview {
    pub id: String,
    pub source: String,
    /// The file the owner gave it, if any.
    pub label: Option<String>,
    /// How many original files it kept, as the Import page counts them.
    pub originals: usize,
    /// How many of those are files it was given and keeps in its record: what
    /// goes when the delete takes it.
    pub kept: usize,
    /// The delete takes it whole, its upload included.
    pub takes: bool,
    /// Its segments deleted earlier and waiting in the trash, which an erase
    /// of it now erases too.
    pub waiting: usize,
}

/// How many segments of each import wait in the trash needing it, by import
/// id: what erasing that import now erases too.
pub fn waiting_by_import(journal: &Path) -> BTreeMap<String, usize> {
    let mut waiting = BTreeMap::new();
    for item in solstone_core_trash::list(journal).unwrap_or_default() {
        for need in &item.needs {
            if let Some(id) = need.strip_prefix("imports/") {
                *waiting.entry(id.to_owned()).or_insert(0) += 1;
            }
        }
    }
    waiting
}

/// What deleting `segments` (journal-relative) would do to each import that
/// alone made any of them, in the order first met. A segment two imports list
/// takes neither, and says nothing of either.
pub fn preview(journal: &Path, segments: &[String]) -> Vec<ImportPreview> {
    let listing = listing(journal);
    let chosen: BTreeSet<String> = segments.iter().cloned().collect();
    let waiting = waiting_by_import(journal);
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    for segment in segments {
        if let Some(id) = sole_lister(&listing, segment) {
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let Some(things) = import_trash::import_things(journal, id) else {
                continue;
            };
            found.push(ImportPreview {
                takes: taken_whole(journal, id, &things, &chosen),
                originals: import_trash::kept_originals(journal, &things),
                kept: import_trash::kept_uploads(journal, &things),
                waiting: waiting.get(id).copied().unwrap_or(0),
                label: import_trash::import_label(journal, id),
                source: things.source,
                id: id.to_owned(),
            });
        }
    }
    found
}

/// Whether deleting `chosen` segments now removes every segment import `id`
/// still has: each is its alone, none is shared, and its run isn't going on
/// (a run still going can make more).
fn taken_whole(journal: &Path, id: &str, things: &ImportThings, chosen: &BTreeSet<String>) -> bool {
    counts(&things.source)
        && things.unreadable == 0
        && things.files.is_empty()
        && things.shared.is_empty()
        && !things.segments.is_empty()
        && things.segments.iter().all(|segment| chosen.contains(segment))
        && attempt_holder(journal, id) != AttemptHolder::Held
}

/// One thing a segment delete moves, and which of the segments asked for it
/// answers.
#[derive(Clone, Debug)]
pub struct Choice {
    pub chosen: Chosen,
    /// Indexes into the segments given to [`choose`].
    pub covers: Vec<usize>,
    /// The import it takes whole, when it does.
    pub import: Option<String>,
}

/// Group the segments an owner chose to delete: an import whose every
/// remaining segment is among them goes whole, as one import; the rest go as
/// themselves. Each import comes where its first segment was.
pub fn choose(journal: &Path, segments: Vec<Chosen>) -> Vec<Choice> {
    let listing = listing(journal);
    let chosen_paths: BTreeSet<String> = segments
        .iter()
        .filter(|chosen| matches!(chosen.unit, Unit::Segment { .. }))
        .filter_map(|chosen| chosen.paths.first().cloned())
        .collect();
    let mut whole: BTreeMap<String, Option<ImportThings>> = BTreeMap::new();
    let mut placed: BTreeMap<String, usize> = BTreeMap::new();
    let mut choices: Vec<Choice> = Vec::new();
    for (index, chosen) in segments.into_iter().enumerate() {
        let import = match (&chosen.unit, chosen.paths.first()) {
            (Unit::Segment { .. }, Some(path)) => sole_lister(&listing, path).and_then(|id| {
                let things = whole.entry(id.to_owned()).or_insert_with(|| {
                    import_trash::import_things(journal, id)
                        .filter(|things| taken_whole(journal, id, things, &chosen_paths))
                });
                things.as_ref().map(|things| (id.to_owned(), things.clone()))
            }),
            _ => None,
        };
        let Some((id, things)) = import else {
            choices.push(Choice {
                chosen,
                covers: vec![index],
                import: None,
            });
            continue;
        };
        if let Some(&at) = placed.get(&id)
            && let Some(choice) = choices.get_mut(at)
        {
            choice.covers.push(index);
            continue;
        }
        placed.insert(id.clone(), choices.len());
        choices.push(Choice {
            chosen: Chosen {
                unit: Unit::Import {
                    source: things.source.clone(),
                    id: id.clone(),
                },
                paths: things.paths(),
                label: import_trash::import_label(journal, &id),
            },
            covers: vec![index],
            import: Some(id),
        });
    }
    choices
}

/// Move one segment to the trash as the owner's `action` from `surface`: the
/// import it is the last of, whole, or the segment alone. Returns the item and
/// the import it took, if it took one.
pub fn trash_segment(
    journal: &Path,
    index: &dyn IndexNotify,
    unit: Unit,
    path: String,
    action: Action,
    surface: Surface,
) -> Result<(Item, Option<String>), TrashError> {
    let mut choices = choose(
        journal,
        vec![Chosen {
            unit,
            paths: vec![path],
            label: None,
        }],
    );
    let Some(choice) = choices.pop() else {
        return Err(TrashError::Missing("recording".into()));
    };
    match (&choice.import, &choice.chosen.unit) {
        (Some(id), _) => import_trash::Imports::new(index)
            .trash(journal, id, action, surface)
            .map(|item| (item, choice.import.clone())),
        (None, unit) => solstone_core_trash::detach(
            journal,
            &ImportSegments::new(index),
            unit.clone(),
            action,
            surface,
            choice.chosen.paths,
        )
        .map(|item| (item, None)),
    }
}

/// The trash's kind for segments: a recording's, with the import it needs.
pub struct ImportSegments<'a> {
    recordings: Recordings<'a>,
    /// Which imports list which segments, read once per use.
    listing: RefCell<Option<BTreeMap<String, BTreeSet<String>>>>,
}

impl<'a> ImportSegments<'a> {
    pub fn new(index: &'a dyn IndexNotify) -> Self {
        Self {
            recordings: Recordings::new(index),
            listing: RefCell::new(None),
        }
    }
}

/// A recording's hold, and, coming back, the import it needs in the journal.
struct NeedsHold<'h> {
    inner: Box<dyn Hold + 'h>,
    journal: std::path::PathBuf,
    item: String,
    /// What it needs, when it is coming back rather than going.
    returning: Option<Vec<String>>,
}

impl NeedsHold<'_> {
    fn needs_met(&self) -> Result<(), String> {
        for need in self.returning.iter().flatten() {
            let held = solstone_core_trash::holders(&self.journal, need, &self.item)
                .map_err(|error| error.to_string())?;
            if !held.is_empty() {
                return Err(serde_json::json!({ "code": IMPORT_IN_TRASH }).to_string());
            }
            if !self.journal.join(need).is_dir() {
                return Err(serde_json::json!({ "code": IMPORT_GONE }).to_string());
            }
        }
        Ok(())
    }
}

impl Hold for NeedsHold<'_> {
    fn check(&self) -> Result<(), String> {
        self.needs_met()?;
        self.inner.check()
    }

    fn moving(&self, path: &str) -> Result<(), String> {
        // The engine asks `check` again under its lock before a recover
        // starts; once started, it finishes.
        self.inner.moving(path)
    }
}

impl Kind for ImportSegments<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Segment { .. })
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let inner = self.recordings.hold(journal, item)?;
        // Only a recover starting now is refused: one a stop left part way is
        // finished, never left stuck between the trash and the journal.
        let returning = (item.state == solstone_core_trash::State::Trashed)
            .then(|| item.needs.clone());
        Ok(Box::new(NeedsHold {
            inner,
            journal: journal.to_path_buf(),
            item: item.id.clone(),
            returning,
        }))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        self.recordings.present(journal, path)
    }

    fn needs(&self, journal: &Path, unit: &Unit) -> Vec<String> {
        let Unit::Segment {
            day,
            stream,
            segment,
        } = unit
        else {
            return Vec::new();
        };
        let path = solstone_core_retention::layout::segment_rel(day, stream, segment);
        let mut listing = self.listing.borrow_mut();
        let listing = listing.get_or_insert_with(|| listing_of(journal));
        sole_lister(listing, &path)
            .map(|id| vec![format!("imports/{id}")])
            .unwrap_or_default()
    }

    fn companions(&self, journal: &Path, item: &Item) -> Result<Vec<String>, String> {
        self.recordings.companions(journal, item)
    }

    fn returns(&self, journal: &Path, item: &Item, path: &str) -> bool {
        self.recordings.returns(journal, item, path)
    }

    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        self.recordings.references(journal, item, change, files)
    }
}

fn listing_of(journal: &Path) -> BTreeMap<String, BTreeSet<String>> {
    listing(journal)
}

#[cfg(test)]
#[path = "segment_trash_tests.rs"]
mod tests;
