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
/// merge isn't one thing (each thing it brought in is its own; `archive` is
/// its older name); Strava, Apple Health and Oura imports move with the body
/// pages' lane. Every other import, one whose kind went unrecorded included,
/// is its record and the segments it lists.
pub fn deletes_here(source: &str) -> bool {
    !matches!(
        source,
        "journal_archive" | "archive" | "strava" | "apple_health" | "oura"
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
    let in_record = std::iter::once(&things.record)
        .chain(&things.companions)
        .filter(|rel| {
            let dir = journal.join(rel);
            read_json(&dir.join("import.json"))
                .and_then(|metadata| {
                    let path = journal.join(str_of(&metadata, "file_path")?);
                    Some(path.starts_with(&dir) && path.is_file())
                })
                .unwrap_or(false)
        })
        .count();
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
    in_record + in_segments
}

/// What one import holds, journal-relative: what its trash item moves.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImportThings {
    /// What kind of import it is: `document`, `audio`, `ics`, `strava`, …
    pub source: String,
    /// Its record: `imports/<id>`.
    pub record: String,
    /// Records that exist only for it: the hidden selection an archive uploaded
    /// from the Import page leaves beside its run.
    pub companions: Vec<String>,
    /// Its whole segments.
    pub segments: Vec<String>,
    /// Its own files in segments another import shares.
    pub files: Vec<String>,
    /// Segments it lists that another import still in the journal also
    /// lists, and that it keeps nothing of its own in: they stay.
    pub shared: Vec<String>,
}

impl ImportThings {
    /// What it names but doesn't take: the segments it shares, and those it
    /// takes only its own files from.
    pub fn refs(&self) -> Vec<String> {
        let mut refs: BTreeSet<String> = self.shared.iter().cloned().collect();
        refs.extend(
            self.files
                .iter()
                .filter_map(|file| file.rsplit_once('/').map(|(parent, _)| parent.to_owned())),
        );
        refs.into_iter().collect()
    }
}

impl ImportThings {
    /// Every path the trash moves, record first.
    pub fn paths(&self) -> Vec<String> {
        let mut paths = vec![self.record.clone()];
        paths.extend(self.companions.iter().cloned());
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
    let companions = if source == "audio_archive" {
        archive_selections(journal, id)
    } else {
        Vec::new()
    };
    let mut things = ImportThings {
        source,
        record,
        companions,
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

/// The hidden selection records that belong to archive run `id`.
fn archive_selections(journal: &Path, id: &str) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(journal.join(IMPORTS_DIR)) else {
        return Vec::new();
    };
    let mut found: Vec<String> = listing
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            if name == id || !is_import_id(&name) {
                return None;
            }
            let metadata = read_json(&entry.path().join("import.json"))?;
            if str_of(&metadata, "source_hint") != Some("audio_archive") {
                return None;
            }
            let digest = metadata
                .get("preview")?
                .get("size")?
                .get("archive_identity")?
                .as_str()?;
            let run = crate::audio_archive::read_matching_archive_run(journal, digest).ok()??;
            (run == id).then(|| format!("{IMPORTS_DIR}/{name}"))
        })
        .collect();
    found.sort();
    found
}

/// What kind of import `id` is, from its records.
/// The kind of import `id` is, as deleting reads it, and as the Import page
/// asks before offering a delete.
pub fn source_of(journal: &Path, id: &str) -> String {
    // A record that names its kind says so; the Import page reads the same.
    let dir = journal.join(IMPORTS_DIR).join(id);
    for name in ["manifest.json", "imported.json"] {
        if let Some(source) = read_json(&dir.join(name))
            .and_then(|record| str_of(&record, "source_type").map(str::to_owned))
        {
            return source;
        }
    }
    // A body card's upload that stopped before it wrote either record keeps the
    // card's own kind: its upload was saved as a generic one.
    if let Some(hint) = read_json(&dir.join("import.json"))
        .and_then(|record| str_of(&record, "source_hint").map(str::to_owned))
        .filter(|hint| matches!(hint.as_str(), "strava" | "apple_health" | "oura"))
    {
        return hint;
    }
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
    let mut legacy = Vec::new();
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
    // A record from before publication records: the files it created, as
    // `<day>/<stream>/<key>/<file>` or `<day>/<key>/<file>` below the
    // chronicle.
    if let Some(record) = read_json(&dir.join("imported.json")) {
        for file in array(&record, "all_created_files")
            .iter()
            .filter_map(Value::as_str)
        {
            if let Some(segment) = legacy_created_segment(journal, file) {
                legacy.push(segment);
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
    found.extend(legacy);
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

/// Where a legacy record's segment is: the one import stream that holds it,
/// else the direct layout.
fn legacy_segment(journal: &Path, day: &str, key: &str) -> Option<String> {
    if !valid_part(day) || !valid_part(key) {
        return None;
    }
    let direct = segment_rel(day, solstone_core_journal_io::paths::DEFAULT_STREAM, key);
    let mut found = std::fs::read_dir(journal.join("chronicle").join(day))
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|stream| stream.starts_with("import."))
        .map(|stream| segment_rel(day, &stream, key))
        .filter(|path| journal.join(path).is_dir());
    match (found.next(), found.next()) {
        (Some(only), None) => Some(only),
        (Some(_), Some(_)) => None,
        (None, _) => journal.join(&direct).is_dir().then_some(direct),
    }
}

/// The segment a legacy record's created file sits in, if that segment is
/// there to name.
fn legacy_created_segment(journal: &Path, file: &str) -> Option<String> {
    let is_day = |part: &&str| part.len() == 8 && part.bytes().all(|byte| byte.is_ascii_digit());
    // The older importer wrote absolute paths: read from the chronicle, or
    // from the first part that is a day.
    let parts: Vec<&str> = file.split('/').filter(|part| !part.is_empty()).collect();
    let start = parts
        .iter()
        .rposition(|part| *part == "chronicle")
        .map(|chronicle| chronicle + 1)
        .or_else(|| parts.iter().position(&is_day))?;
    let rest = parts.get(start..)?;
    if !rest.first().is_some_and(is_day) {
        return None;
    }
    let is_key = |part: &str| {
        part.len() > 7
            && part.as_bytes().iter().take(6).all(u8::is_ascii_digit)
            && part.as_bytes().get(6) == Some(&b'_')
    };
    let segment = match rest {
        [day, key, _, ..] if is_key(key) && valid_part(key) => {
            segment_rel(day, solstone_core_journal_io::paths::DEFAULT_STREAM, key)
        }
        [day, stream, key, _, ..] if valid_part(stream) && valid_part(key) => {
            segment_rel(day, stream, key)
        }
        _ => return None,
    };
    journal.join(&segment).is_dir().then_some(segment)
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

/// A file of this import came back into a segment whose other contents were
/// removed meanwhile, and no other item holds them: the removal's tombstone
/// gives way, so the import's own file is seen again.
fn reclaim_for_own_file(journal: &Path, segment: &str, item: &str) -> Result<(), String> {
    let tombstone = journal
        .join(segment)
        .join(solstone_core_retention::tombstone::TOMBSTONE_NAME);
    if !tombstone.is_file() {
        return Ok(());
    }
    let held =
        solstone_core_trash::holders(journal, segment, item).map_err(|error| error.to_string())?;
    if !held.is_empty() {
        return Ok(());
    }
    // System internal: a removal's marker giving way to what came back.
    std::fs::remove_file(&tombstone).map_err(|error| format!("{segment}: {error}"))
}

/// An archive still owed processing is owed it again once it's back: the
/// journal forgets a pending tile whose run has left.
fn restore_pending_tiles(journal: &Path, item: &Item) -> Result<(), String> {
    let Unit::Import { source, id } = &item.unit else {
        return Ok(());
    };
    if source != "audio_archive" {
        return Ok(());
    }
    // A record that can't be read owes nothing it can name.
    let tiles = match crate::audio_archive::ArchiveStore::open(journal, id, false)
        .and_then(|store| store.pending_tiles(u32::MAX))
    {
        Ok(tiles) => tiles,
        Err(error) => {
            log::warn!(
                "trash item {}: its pending work couldn't be read: {error}",
                item.id
            );
            return Ok(());
        }
    };
    tiles.iter().try_for_each(|(day, key)| {
        crate::audio_archive::claims::register_pending_tile(journal, id, day, key)
            .map_err(|error| format!("pending tiles weren't restored: {error}"))
    })
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

/// The segment a path is, or that a file of it sits in.
fn target_of_path(path: &str) -> Option<Target> {
    parse_segment_rel(path)
        .or_else(|| {
            let (parent, _) = path.rsplit_once('/')?;
            parse_segment_rel(parent)
        })
        .map(|(day, stream, dir)| Target { day, stream, dir })
}

/// The segments an item touches: each whole one, and each a file of it sits in.
fn targets_of(item: &Item) -> Vec<Target> {
    let mut targets: Vec<Target> = item
        .paths
        .iter()
        .filter_map(|path| target_of_path(path))
        .collect();
    targets.sort_by(|a, b| (&a.day, &a.stream, &a.dir).cmp(&(&b.day, &b.stream, &b.dir)));
    targets.dedup();
    targets
}

/// Why an import can't come back yet: a segment it shares is in the trash with
/// another import.
pub const SHARED_IN_TRASH: &str =
    "part of this import is in the trash with another import; recover that one first";

/// The import's segments held, its run not going on, and, when it comes back,
/// nothing it shares held by another item in the trash.
struct ImportHold {
    segments: SegmentHold,
    /// The one segment lock held now: the segment moving.
    moving: std::cell::RefCell<Option<solstone_core_journal_io::FileLock>>,
    journal: std::path::PathBuf,
    id: String,
    /// The item and what it shares, when it is coming back rather than going.
    returning: Option<(String, Vec<String>)>,
}

impl Hold for ImportHold {
    fn check(&self) -> Result<(), String> {
        if attempt_holder(&self.journal, &self.id) == AttemptHolder::Held {
            return Err(STILL_IMPORTING.to_owned());
        }
        if let Some((item, refs)) = &self.returning {
            for path in refs {
                let held = solstone_core_trash::holders(&self.journal, path, item)
                    .map_err(|error| error.to_string())?;
                if !held.is_empty() {
                    return Err(SHARED_IN_TRASH.to_owned());
                }
            }
        }
        self.segments.check()
    }

    fn moving(&self, path: &str) -> Result<(), String> {
        // Let go of the last segment before taking the next.
        self.moving.borrow_mut().take();
        if attempt_holder(&self.journal, &self.id) == AttemptHolder::Held {
            return Err(STILL_IMPORTING.to_owned());
        }
        let target = target_of_path(path);
        self.segments.check_one(target.as_ref())?;
        if let Some(target) = target {
            *self.moving.borrow_mut() = Some(self.segments.lock_one(&target)?);
        }
        Ok(())
    }
}

impl Kind for Imports<'_> {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Import { .. })
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let id = import_id_of(item).ok_or_else(|| "this isn't an import".to_owned())?;
        let returning = (item.state != solstone_core_trash::State::Detaching)
            .then(|| (item.id.clone(), item.refs.clone()));
        Ok(Box::new(ImportHold {
            segments: SegmentHold::acquire_lease(journal, &targets_of(item))?,
            moving: std::cell::RefCell::new(None),
            journal: journal.to_path_buf(),
            id: id.to_owned(),
            returning,
        }))
    }

    fn current_paths(&self, journal: &Path, unit: &Unit) -> Option<Vec<String>> {
        let Unit::Import { id, .. } = unit else {
            return None;
        };
        // Gone from the journal: nothing of it to move now.
        Some(import_things(journal, id).map_or_else(Vec::new, |things| things.paths()))
    }

    fn refs(&self, journal: &Path, unit: &Unit) -> Vec<String> {
        let Unit::Import { id, .. } = unit else {
            return Vec::new();
        };
        import_things(journal, id).map_or_else(Vec::new, |things| things.refs())
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
            Change::Attached => {
                for path in &item.paths {
                    if parse_segment_rel(path).is_none()
                        && let Some((parent, _)) = path.rsplit_once('/')
                    {
                        reclaim_for_own_file(journal, parent, &item.id)?;
                    }
                }
                restore_pending_tiles(journal, item)?;
                self.index.paths_returned(files)
            }
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]
mod tests {
    use super::*;
    use solstone_core_removals::Page;
    use solstone_core_retention::NoIndex;
    use std::fs;
    use tempfile::TempDir;

    const ID: &str = "20260801_090000";
    const OTHER: &str = "20260801_100000";

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn published(root: &Path, id: &str, segments: &[(&str, &str, &str)]) {
        let segments: Vec<Value> = segments
            .iter()
            .map(|(day, stream, key)| {
                serde_json::json!({"day": day, "segment": key, "stream": stream,
                    "outcome": {"status": "bound", "prev_day": null, "prev_segment": null, "seq": 1}})
            })
            .collect();
        write(
            root,
            &format!("imports/{id}/imported.json"),
            serde_json::json!({"schema": "solstone.import.publication.v2", "status": "success",
                "segments": segments, "indexing": {"queued": [], "declined": [], "errored": []},
                "day_markers": []})
            .to_string()
            .as_bytes(),
        );
        write(
            root,
            &format!("imports/{id}/import.json"),
            serde_json::json!({"import_id": id, "original_filename": "notes.ics"})
                .to_string()
                .as_bytes(),
        );
        write(
            root,
            &format!("imports/{id}/manifest.json"),
            br#"{"source_type": "ics"}"#,
        );
    }

    /// A journal with two calendar imports: each has a segment of its own, and
    /// they share one, where each keeps its own calendar file.
    fn journal() -> TempDir {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            "config/journal.json",
            br#"{"setup":{"completed_at":1700000000000}}"#,
        );
        published(
            root.path(),
            ID,
            &[
                ("20260801", "import.ics", "090000_300"),
                ("20260802", "import.ics", "090000_300"),
            ],
        );
        published(
            root.path(),
            OTHER,
            &[
                ("20260802", "import.ics", "090000_300"),
                ("20260803", "import.ics", "090000_300"),
            ],
        );
        for (day, owner) in [("20260801", ID), ("20260803", OTHER)] {
            write(
                root.path(),
                &format!("chronicle/{day}/import.ics/090000_300/event_transcript.md"),
                owner.as_bytes(),
            );
        }
        let shared = "chronicle/20260802/import.ics/090000_300";
        write(
            root.path(),
            &format!("{shared}/event_transcript.md"),
            b"last writer",
        );
        for id in [ID, OTHER] {
            write(
                root.path(),
                &format!("{shared}/calendar_{id}.jsonl"),
                format!("{{\"import\":\"{id}\"}}\n").as_bytes(),
            );
        }
        root
    }

    fn surface() -> Surface {
        Surface::Web { page: Page::Import }
    }

    fn trash(root: &Path) -> Item {
        Imports::new(&NoIndex)
            .trash(root, ID, Action::ImportDelete, surface())
            .unwrap()
    }

    #[test]
    fn an_import_lists_its_own_segments_and_only_its_files_in_a_shared_one() {
        let root = journal();
        let things = import_things(root.path(), ID).unwrap();
        assert_eq!(things.source, "ics");
        assert_eq!(things.record, format!("imports/{ID}"));
        assert_eq!(
            things.segments,
            vec!["chronicle/20260801/import.ics/090000_300".to_owned()]
        );
        assert_eq!(
            things.files,
            vec![format!(
                "chronicle/20260802/import.ics/090000_300/calendar_{ID}.jsonl"
            )]
        );
    }

    #[test]
    fn trash_moves_the_import_whole_and_leaves_the_other_import_as_it_was() {
        let root = journal();
        let item = trash(root.path());
        assert_eq!(item.label.as_deref(), Some("notes.ics"));
        assert!(!root.path().join("imports").join(ID).exists());
        // Its own segment holds only the tombstone now.
        let own = root.path().join("chronicle/20260801/import.ics/090000_300");
        let names: Vec<_> = fs::read_dir(&own)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["tombstone.json".to_owned()]);
        // The shared segment keeps the other import's file and the transcript.
        let shared = root.path().join("chronicle/20260802/import.ics/090000_300");
        assert!(shared.join(format!("calendar_{OTHER}.jsonl")).is_file());
        assert!(shared.join("event_transcript.md").is_file());
        assert!(!shared.join(format!("calendar_{ID}.jsonl")).exists());
        // The other import is untouched.
        assert!(root.path().join("imports").join(OTHER).is_dir());
        assert!(
            root.path()
                .join("chronicle/20260803/import.ics/090000_300/event_transcript.md")
                .is_file()
        );
        assert!(import_things(root.path(), ID).is_none());
    }

    #[test]
    fn recover_puts_every_part_back_exactly() {
        let root = journal();
        let before = import_things(root.path(), ID).unwrap();
        let item = trash(root.path());
        crate::trash_kinds::with_kinds(root.path(), |kinds| {
            solstone_core_trash::attach(root.path(), kinds, &item.id, Action::Recover, surface())
        })
        .unwrap();
        assert_eq!(import_things(root.path(), ID).unwrap(), before);
        assert_eq!(
            fs::read(
                root.path()
                    .join("chronicle/20260801/import.ics/090000_300/event_transcript.md")
            )
            .unwrap(),
            ID.as_bytes()
        );
        assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn erase_unlinks_the_import_and_nothing_else() {
        let root = journal();
        let item = trash(root.path());
        solstone_core_trash::erase(root.path(), &item.id, Action::Erase, surface()).unwrap();
        assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
        assert!(!root.path().join("imports").join(ID).exists());
        assert!(root.path().join("imports").join(OTHER).is_dir());
        assert!(
            root.path()
                .join(format!(
                    "chronicle/20260802/import.ics/090000_300/calendar_{OTHER}.jsonl"
                ))
                .is_file()
        );
    }

    #[test]
    fn a_set_of_imports_moves_each_as_its_own_item() {
        let root = journal();
        let chosen: Vec<solstone_core_trash::Chosen> = [ID, OTHER]
            .iter()
            .map(|id| {
                let things = import_things(root.path(), id).unwrap();
                solstone_core_trash::Chosen {
                    unit: Unit::Import {
                        source: things.source.clone(),
                        id: (*id).to_owned(),
                    },
                    paths: things.paths(),
                    label: None,
                }
            })
            .collect();
        let results = crate::trash_kinds::with_kinds(root.path(), |kinds| {
            solstone_core_trash::detach_set(
                root.path(),
                kinds,
                chosen,
                Action::ImportDelete,
                surface(),
                false,
            )
        })
        .unwrap();
        assert!(
            results
                .iter()
                .all(|result| matches!(result, solstone_core_trash::SetResult::Moved { .. }))
        );
        assert_eq!(solstone_core_trash::list(root.path()).unwrap().len(), 2);
        assert!(!root.path().join("imports").join(OTHER).exists());
        // The segment both shared went with the last of them: nothing of
        // either is left to read.
        let shared = "chronicle/20260802/import.ics/090000_300";
        assert!(!live_segment(root.path(), shared));
        assert_eq!(
            solstone_core_trash::holders(root.path(), shared, "")
                .unwrap()
                .len(),
            1
        );
    }

    fn recover(root: &Path, id: &str) -> Result<Item, TrashError> {
        crate::trash_kinds::with_kinds(root, |kinds| {
            solstone_core_trash::attach(root, kinds, id, Action::Recover, surface())
        })
    }

    fn trash_other(root: &Path) -> Item {
        Imports::new(&NoIndex)
            .trash(root, OTHER, Action::ImportDelete, surface())
            .unwrap()
    }

    #[test]
    fn an_import_whose_shared_segment_went_with_another_waits_for_it() {
        let root = journal();
        let first = trash(root.path());
        let second = trash_other(root.path());
        // The first one's own file sits in a segment the second one took.
        let error = recover(root.path(), &first.id).unwrap_err();
        assert!(matches!(&error, TrashError::Held(reason) if reason == SHARED_IN_TRASH));
        recover(root.path(), &second.id).unwrap();
        recover(root.path(), &first.id).unwrap();
        let shared = root.path().join("chronicle/20260802/import.ics/090000_300");
        for file in [
            format!("calendar_{ID}.jsonl"),
            format!("calendar_{OTHER}.jsonl"),
            "event_transcript.md".to_owned(),
        ] {
            assert!(shared.join(&file).is_file(), "{file}");
        }
        assert!(live_segment(
            root.path(),
            "chronicle/20260802/import.ics/090000_300"
        ));
        assert!(solstone_core_trash::list(root.path()).unwrap().is_empty());
    }

    #[test]
    fn an_import_comes_back_into_a_shared_segment_another_erased() {
        let root = journal();
        let first = trash(root.path());
        let second = trash_other(root.path());
        solstone_core_trash::erase(root.path(), &second.id, Action::Erase, surface()).unwrap();
        recover(root.path(), &first.id).unwrap();
        // Its own file is back and seen; what only the other held is gone.
        let shared = "chronicle/20260802/import.ics/090000_300";
        assert!(live_segment(root.path(), shared));
        assert!(
            root.path()
                .join(shared)
                .join(format!("calendar_{ID}.jsonl"))
                .is_file()
        );
        assert!(
            !root
                .path()
                .join(shared)
                .join(format!("calendar_{OTHER}.jsonl"))
                .exists()
        );
    }

    #[test]
    fn a_running_import_is_held() {
        let root = journal();
        let now = 1_700_000_000_000;
        crate::admit_running_attempt(root.path(), ID, now, None).unwrap();
        let error = Imports::new(&NoIndex)
            .trash(root.path(), ID, Action::ImportDelete, surface())
            .unwrap_err();
        assert_eq!(failure_key(&error), "importing");
        assert!(root.path().join("imports").join(ID).is_dir());
        crate::metadata::release_attempt(root.path(), ID);
    }

    #[test]
    fn a_legacy_record_names_its_segments_through_the_files_it_created() {
        let root = TempDir::new().unwrap();
        write(
            root.path(),
            &format!("imports/{ID}/imported.json"),
            serde_json::json!({"all_created_files": [
                "20260805/import.ics/090000_300/event_transcript.md",
                "20260805/import.ics/093000_300/event_transcript.md"
            ], "segments": ["090000_300", "093000_300"], "source_type": "ics"})
            .to_string()
            .as_bytes(),
        );
        for key in ["090000_300", "093000_300"] {
            write(
                root.path(),
                &format!("chronicle/20260805/import.ics/{key}/event_transcript.md"),
                b"event",
            );
        }
        let things = import_things(root.path(), ID).unwrap();
        assert_eq!(
            things.segments,
            vec![
                "chronicle/20260805/import.ics/090000_300".to_owned(),
                "chronicle/20260805/import.ics/093000_300".to_owned(),
            ]
        );
    }

    #[test]
    fn a_legacy_record_with_absolute_paths_names_its_segments_and_what_it_shares() {
        let root = journal();
        let shared = "chronicle/20260802/import.ics/090000_300";
        write(
            root.path(),
            "imports/20260801_120000/imported.json",
            serde_json::json!({"all_created_files": [
                format!("/home/owner/journal/{shared}/event_transcript.md")
            ], "source_type": "ics"})
            .to_string()
            .as_bytes(),
        );
        // `OTHER` goes; the older record still names the segment, so `ID` can't
        // take it whole.
        trash_other(root.path());
        let things = import_things(root.path(), ID).unwrap();
        assert!(!things.segments.contains(&shared.to_owned()));
        assert_eq!(things.files, vec![format!("{shared}/calendar_{ID}.jsonl")]);
    }

    #[test]
    fn an_erased_archive_leaves_nothing_in_the_claims_database() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("imports").join(ID)).unwrap();
        let conn =
            rusqlite::Connection::open(crate::audio_archive::claims::claim_db_path(root.path()))
                .unwrap();
        conn.execute_batch(
            "CREATE TABLE audio_archive_run (archive_identity TEXT PRIMARY KEY, import_id TEXT);
             INSERT INTO audio_archive_run VALUES ('live','20260801_090000'),
                ('trashed','20260801_100000'),('erased','20260801_110000');",
        )
        .unwrap();
        drop(conn);
        let held = |id: &str| id == OTHER;
        let forgotten = crate::audio_archive::claims::forget_gone_runs(root.path(), &held).unwrap();
        assert_eq!(forgotten, 1);
        let conn =
            rusqlite::Connection::open(crate::audio_archive::claims::claim_db_path(root.path()))
                .unwrap();
        let left: Vec<String> = conn
            .prepare("SELECT archive_identity FROM audio_archive_run ORDER BY 1")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(left, vec!["live".to_owned(), "trashed".to_owned()]);
    }

    #[test]
    fn a_body_cards_upload_that_stopped_early_is_still_its_cards_kind() {
        // Saved as a generic upload, stopped before it wrote a manifest or a
        // publication record: the card's hint says what it is.
        let journal = tempfile::tempdir().unwrap();
        let dir = journal.path().join(IMPORTS_DIR).join("20261109_225511");
        std::fs::create_dir_all(&dir).unwrap();
        for (hint, kind) in [
            ("apple_health", "apple_health"),
            ("oura", "oura"),
            ("", "text"),
        ] {
            let mut record = serde_json::json!({"source": "text"});
            if !hint.is_empty() {
                record["source_hint"] = serde_json::json!(hint);
            }
            std::fs::write(dir.join("import.json"), record.to_string()).unwrap();
            let source = source_of(journal.path(), "20261109_225511");
            assert!(
                source == kind || (hint.is_empty() && deletes_here(&source)),
                "{hint:?}: {source}"
            );
            assert_eq!(deletes_here(&source), hint.is_empty(), "{hint:?}");
        }
    }

    #[test]
    fn deletes_here_leaves_the_body_lane_and_merges_alone() {
        for source in [
            "strava",
            "apple_health",
            "oura",
            "journal_archive",
            "archive",
        ] {
            assert!(!deletes_here(source), "{source}");
        }
        for source in [
            "document",
            "image",
            "text",
            "audio",
            "audio_archive",
            "ics",
            "chatgpt",
            // A kind that went unrecorded is still its record and segments.
            "import",
        ] {
            assert!(deletes_here(source), "{source}");
        }
    }
}
