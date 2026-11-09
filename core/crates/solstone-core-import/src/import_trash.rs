// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An import in the trash: its record and everything it brought in, as one
//! thing.
//!
//! The trash (`solstone-core-trash`) moves the files; this is the import's side
//! of it. An import is:
//!
//! - its record, `imports/<id>/`, with any original it kept (an uploaded
//!   file, a pasted text, a downloaded recording, an archive's own records);
//! - every segment it lists that is still live, whole;
//! - in a segment it shares with another import that is still in the journal,
//!   only the files that are its own (a calendar file names its import on its
//!   first line). A shared segment otherwise stays with the import still here.
//!
//! An import's segments are listed in whichever of its records the importer
//! keeps: `imported.json` (every importer that publishes), `audio-import.json`
//! (audio, stopped runs included), `content_manifest.jsonl`, an archive's
//! `audio-archive.sqlite`, and a legacy `segments.json`.
//!
//! Its references are a recording's: a tombstone where each whole segment
//! was, the search index told about every file that left or came back, and
//! each day's stream marker. What was made from it elsewhere is not reached.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;
use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_retention::door::{SegmentHold, place_tombstone};
use solstone_core_retention::layout::{parse_segment_rel, segment_rel};
use solstone_core_retention::notify::IndexNotify;
use solstone_core_retention::receipt::{RemovedPath, Target};
use solstone_core_trash::{Change, Hold, Item, Kind, TrashError};

use crate::metadata::{AttemptHolder, attempt_holder};

/// Where every import's record lives.
const IMPORTS_DIR: &str = "imports";

/// Why an import can't move while its run goes on: the words a set's refusal
/// is keyed on.
pub const STILL_IMPORTING: &str = "this import is still running; try again when it's done";

/// Whether an import of `source` deletes as a whole here. A journal archive
/// merge isn't one thing (each thing it brought in is its own); Strava,
/// Apple Health and Oura imports move with the body pages' lane.
pub fn deletes_here(source: &str) -> bool {
    !matches!(
        source,
        "journal_archive" | "strava" | "apple_health" | "oura"
    )
}

/// What kind of failure a refused delete was, for the page: a removal
/// already in progress (`busy`), an import still running (`importing`), or
/// anything else (`other`).
pub fn failure_key(error: &TrashError) -> &'static str {
    match error {
        TrashError::InFlight | TrashError::Unfinished(_) => "busy",
        TrashError::Held(reason) if reason == STILL_IMPORTING => "importing",
        _ => "other",
    }
}

/// The name the owner knew the import by: the file they gave it, if any.
pub fn import_label(journal: &Path, id: &str) -> Option<String> {
    let metadata = read_json(&journal.join(IMPORTS_DIR).join(id).join("import.json"))?;
    let name = str_of(&metadata, "original_filename")?;
    Some(name.chars().take(200).collect())
}

/// How many original files the import kept: what it was given, kept in its
/// record, and the originals in its own segments.
pub fn kept_originals(journal: &Path, things: &ImportThings) -> usize {
    let record = journal.join(&things.record);
    let in_record = read_json(&record.join("import.json"))
        .and_then(|metadata| {
            let path = str_of(&metadata, "file_path")?.to_owned();
            Some(Path::new(&path).starts_with(&record) && Path::new(&path).is_file())
        })
        .unwrap_or(false);
    let in_segments = things
        .segments
        .iter()
        .filter_map(|segment| std::fs::read_dir(journal.join(segment)).ok())
        .flat_map(|listing| listing.flatten())
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.starts_with("original.") || name.starts_with("imported_audio.")
            })
        })
        .count();
    usize::from(in_record) + in_segments
}

/// What one import holds, journal-relative: what its trash item moves.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportThings {
    /// What kind of import it is: `document`, `audio`, `ics`, `strava`, …
    pub source: String,
    /// Its record: `imports/<id>`.
    pub record: String,
    /// Its whole segments.
    pub segments: Vec<String>,
    /// Its own files in segments another import shares.
    pub files: Vec<String>,
    /// Segments it lists that another import still in the journal also
    /// lists, and that it keeps nothing of its own in: they stay.
    pub shared: Vec<String>,
}

impl ImportThings {
    /// Every path the trash moves, record first.
    pub fn paths(&self) -> Vec<String> {
        let mut paths = vec![self.record.clone()];
        paths.extend(self.segments.iter().cloned());
        paths.extend(self.files.iter().cloned());
        paths
    }

    /// The days its segments fall on, first and last.
    pub fn days(&self) -> Option<(String, String)> {
        let days: BTreeSet<String> = self
            .segments
            .iter()
            .chain(&self.files)
            .filter_map(|path| path.split('/').nth(1).map(str::to_owned))
            .collect();
        Some((days.first()?.clone(), days.last()?.clone()))
    }
}

/// Whether `id` names an import the Import page lists: `YYYYMMDD_HHMMSS`.
pub fn is_import_id(id: &str) -> bool {
    id.len() == 15
        && id.bytes().enumerate().all(|(index, byte)| {
            if index == 8 {
                byte == b'_'
            } else {
                byte.is_ascii_digit()
            }
        })
}

/// Work out what import `id` holds now. `None` when it has no record.
pub fn import_things(journal: &Path, id: &str) -> Option<ImportThings> {
    if !is_import_id(id) {
        return None;
    }
    let record = format!("{IMPORTS_DIR}/{id}");
    if !journal.join(&record).is_dir() {
        return None;
    }
    let source = source_of(journal, id);
    let mine = listed_segments(journal, id, &source);
    // Every other import still in the journal, and what it lists.
    let mut others: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if let Ok(listing) = std::fs::read_dir(journal.join(IMPORTS_DIR)) {
        for entry in listing.flatten() {
            let Some(other) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if other == id || !is_import_id(&other) || !entry.path().is_dir() {
                continue;
            }
            let other_source = source_of(journal, &other);
            for segment in listed_segments(journal, &other, &other_source) {
                others.entry(segment).or_default().insert(other.clone());
            }
        }
    }
    let mut things = ImportThings {
        source,
        record,
        ..ImportThings::default()
    };
    for segment in mine {
        if !live_segment(journal, &segment) {
            continue;
        }
        if !others.contains_key(&segment) {
            things.segments.push(segment);
            continue;
        }
        let own = own_files(journal, &segment, id);
        if own.is_empty() {
            things.shared.push(segment);
        } else {
            things.files.extend(own);
        }
    }
    Some(things)
}

/// What kind of import `id` is, from its records.
fn source_of(journal: &Path, id: &str) -> String {
    let projection = crate::projection::project_import_result(journal, id);
    let source = projection.source_type.trim();
    if source.is_empty() {
        "import".to_owned()
    } else {
        source.to_owned()
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Every segment `id`'s records name, live or not, journal-relative.
fn listed_segments(journal: &Path, id: &str, source: &str) -> BTreeSet<String> {
    let dir = journal.join(IMPORTS_DIR).join(id);
    let mut found = BTreeSet::new();
    let mut add = |day: &str, stream: &str, key: &str| {
        if valid_part(day) && valid_part(stream) && valid_part(key) {
            found.insert(segment_rel(day, stream, key));
        }
    };
    let default_stream = format!("import.{source}");
    // Every importer that publishes.
    if let Some(record) = read_json(&dir.join("imported.json")) {
        for segment in array(&record, "segments") {
            if let (Some(day), Some(key), Some(stream)) = (
                str_of(segment, "day"),
                str_of(segment, "segment"),
                str_of(segment, "stream"),
            ) {
                add(day, stream, key);
            }
        }
    }
    // Audio: every segment a run made, a stopped run's included.
    if let Some(record) = read_json(&dir.join("audio-import.json")) {
        for segment in array(&record, "created_segments") {
            if let (Some(day), Some(key), Some(stream)) = (
                str_of(segment, "day"),
                str_of(segment, "key"),
                str_of(segment, "stream"),
            ) {
                add(day, stream, key);
            }
        }
    }
    // The Import page's content rows.
    if let Ok(text) = std::fs::read_to_string(dir.join("content_manifest.jsonl")) {
        for row in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            for segment in array(&row, "segments") {
                if let (Some(day), Some(key)) = (str_of(segment, "day"), str_of(segment, "key")) {
                    let stream = str_of(segment, "stream").unwrap_or(&default_stream);
                    add(day, stream, key);
                }
            }
        }
    }
    // An audio archive's tiles.
    if dir
        .join(crate::audio_archive::AUDIO_ARCHIVE_RECORD_FILENAME)
        .is_file()
        && let Ok(Some(record)) = crate::audio_archive::read_audio_archive(journal, id)
    {
        for part in &record.parts {
            add(&part.day, &record.settings.stream, &part.segment_key);
        }
    }
    // A legacy record: keys of one day, with no stream.
    if let Some(record) = read_json(&dir.join("segments.json"))
        && let Some(day) = str_of(&record, "day")
    {
        for key in array(&record, "segments").iter().filter_map(Value::as_str) {
            if let Some(path) = legacy_segment(journal, day, key) {
                found.insert(path);
            }
        }
    }
    found
}

/// Where a legacy record's segment is: the direct layout, else the one
/// import stream that holds it.
fn legacy_segment(journal: &Path, day: &str, key: &str) -> Option<String> {
    if !valid_part(day) || !valid_part(key) {
        return None;
    }
    let direct = segment_rel(day, solstone_core_journal_io::paths::DEFAULT_STREAM, key);
    if journal.join(&direct).is_dir() {
        return Some(direct);
    }
    let mut found = std::fs::read_dir(journal.join("chronicle").join(day))
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|stream| stream.starts_with("import."))
        .map(|stream| segment_rel(day, &stream, key))
        .filter(|path| journal.join(path).is_dir());
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn array<'v>(value: &'v Value, key: &str) -> &'v [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn str_of<'v>(value: &'v Value, key: &str) -> Option<&'v str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// One path component a record may name: no separators, no dots alone.
fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.starts_with('.')
        && !part.contains(['/', '\\'])
}

/// A segment that is there, and not only the tombstone a removal left or a
/// removal of it part-way through. Unreadable counts as there.
fn live_segment(journal: &Path, path: &str) -> bool {
    if parse_segment_rel(path).is_none() {
        return false;
    }
    let place = journal.join(path);
    place.is_dir()
        && !matches!(
            solstone_core_segment::owner_deletion_state(&place),
            Ok(state) if state != solstone_core_segment::OwnerDeletionState::Live
        )
}

/// The files in a shared segment that are `id`'s own: calendar files whose
/// first line names it.
fn own_files(journal: &Path, segment: &str, id: &str) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(journal.join(segment)) else {
        return Vec::new();
    };
    let mut own: Vec<String> = listing
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            if !(name.starts_with("calendar_") && name.ends_with(".jsonl")) {
                return None;
            }
            let text = std::fs::read_to_string(entry.path()).ok()?;
            let first: Value = serde_json::from_str(text.lines().next()?).ok()?;
            (str_of(&first, "import") == Some(id)).then(|| format!("{segment}/{name}"))
        })
        .collect();
    own.sort();
    own
}

/// The trash's kind for imports.
pub struct Imports<'a> {
    index: &'a dyn IndexNotify,
}

impl<'a> Imports<'a> {
    pub fn new(index: &'a dyn IndexNotify) -> Self {
        Self { index }
    }

    /// Move import `id` to the trash, as the owner's `action` from `surface`.
    pub fn trash(
        &self,
        journal: &Path,
        id: &str,
        action: Action,
        surface: Surface,
    ) -> Result<Item, TrashError> {
        let things =
            import_things(journal, id).ok_or_else(|| TrashError::Missing("import".into()))?;
        let unit = Unit::Import {
            source: things.source.clone(),
            id: id.to_owned(),
        };
        solstone_core_trash::detach_labelled(
            journal,
            self,
            unit,
            action,
            surface,
            things.paths(),
            import_label(journal, id),
        )
    }
}

/// The import an item holds.
fn import_id_of(item: &Item) -> Option<&str> {
    match &item.unit {
        Unit::Import { id, .. } => Some(id.as_str()),
        _ => None,
    }
}

/// The segments an item touches: each whole one, and each a file of it sits in.
fn targets_of(item: &Item) -> Vec<Target> {
    let mut targets: Vec<Target> = item
        .paths
        .iter()
        .filter_map(|path| {
            parse_segment_rel(path).or_else(|| {
                let (parent, _) = path.rsplit_once('/')?;
                parse_segment_rel(parent)
            })
        })
        .map(|(day, stream, dir)| Target { day, stream, dir })
        .collect();
    targets.sort_by(|a, b| (&a.day, &a.stream, &a.dir).cmp(&(&b.day, &b.stream, &b.dir)));
    targets.dedup();
    targets
}

/// The import's segments held, and its run not going on.
struct ImportHold {
    segments: SegmentHold,
    journal: std::path::PathBuf,
    id: String,
}

impl Hold for ImportHold {
    fn check(&self) -> Result<(), String> {
        if attempt_holder(&self.journal, &self.id) == AttemptHolder::Held {
            return Err(STILL_IMPORTING.to_owned());
        }
        self.segments.check()
    }
}

impl Kind for Imports<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Import { .. })
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let id = import_id_of(item).ok_or_else(|| "this isn't an import".to_owned())?;
        Ok(Box::new(ImportHold {
            segments: SegmentHold::acquire_many(journal, &targets_of(item))?,
            journal: journal.to_path_buf(),
            id: id.to_owned(),
        }))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        if parse_segment_rel(path).is_some() {
            return live_segment(journal, path);
        }
        let place = journal.join(path);
        if path.starts_with("imports/") {
            return place.is_dir();
        }
        place.is_file()
    }

    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        if change == Change::Detached {
            for path in &item.paths {
                let Some((day, stream, dir)) = parse_segment_rel(path) else {
                    continue;
                };
                let prefix = format!("{path}/");
                let manifest: Vec<String> = files
                    .iter()
                    .filter(|file| file.starts_with(&prefix))
                    .cloned()
                    .collect();
                place_tombstone(
                    journal,
                    &Target { day, stream, dir },
                    manifest,
                    &item.trashed_at,
                    &item.id,
                )
                .map_err(|error| format!("no tombstone was left: {error}"))?;
            }
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
        let days: BTreeSet<String> = targets_of(item)
            .into_iter()
            .map(|target| target.day)
            .collect();
        for day in days {
            solstone_core_journal_io::bump_stream_marker(journal, &day)
                .map_err(|error| format!("the day was not queued for follow-up: {error}"))?;
        }
        Ok(())
    }
}
