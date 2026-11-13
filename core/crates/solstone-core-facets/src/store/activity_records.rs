// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockError, LockOptions, PathError, ReadError, atomic_replace,
    contained_path, hold_lock, read_text,
};

use super::activities::read_activity_file;

pub type ActivityRecord = Map<String, Value>;

#[derive(serde::Deserialize)]
struct DefaultsFixture {
    activities: Vec<DefaultActivity>,
}
#[derive(serde::Deserialize)]
struct DefaultActivity {
    id: String,
    always_on: bool,
}

fn defaults() -> &'static [DefaultActivity] {
    static DEFAULTS: OnceLock<Vec<DefaultActivity>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        serde_json::from_str::<DefaultsFixture>(include_str!(
            "../../../../fixtures/activity_defaults.json"
        ))
        .expect("activity defaults fixture")
        .activities
    })
}

pub fn activity_is_available(
    root: &Path,
    facet: &str,
    activity_id: &str,
) -> Result<bool, ActivityRecordStoreError> {
    if facet.is_empty() {
        return Ok(defaults().iter().any(|item| item.id == activity_id));
    }
    let explicit = read_activity_file(root, facet, "activities.jsonl")?.unwrap_or_default();
    let ids = explicit
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|row| row.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Ok(defaults().iter().any(|item| item.id == activity_id));
    }
    Ok(ids.iter().any(|id| id == activity_id)
        || defaults()
            .iter()
            .any(|item| item.always_on && item.id == activity_id))
}

#[derive(Debug)]
pub enum ActivityRecordStoreError {
    MissingDayFile {
        path: PathBuf,
    },
    MissingRecord {
        facet: String,
        day: String,
        record_id: String,
    },
    Path(PathError),
    Lock(LockError),
    Read(ReadError),
    Write(solstone_core_journal_io::AtomicWriteError),
    Json(serde_json::Error),
    Facet(super::error::FacetStoreError),
    Identity(super::declaration::FacetIdentityError),
    DestinationMuted {
        facet: String,
    },
    PublicationRefused {
        detail: String,
    },
    EditRefused(EditRefusal),
    /// The record has an open move: only its destination can change.
    BodyRecordMoving {
        detail: String,
    },
}

/// Why an edit could not be removed. Nothing is written when one is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditRefusal {
    /// No edit is at that position.
    OutOfRange,
    /// The edit at that position has another timestamp or actor.
    Mismatch,
    /// Hiding and unhiding have their own inverse.
    NotRemovable,
    /// The edit was saved before edits recorded their values.
    NotRecorded,
    AlreadyRemoved,
}

impl EditRefusal {
    /// The stable reason code for this refusal.
    #[must_use]
    pub fn reason_code(self) -> &'static str {
        match self {
            Self::OutOfRange => "activity_edit_not_found",
            Self::Mismatch => "activity_edit_mismatch",
            Self::NotRemovable => "activity_edit_not_removable",
            Self::NotRecorded => "activity_edit_not_recorded",
            Self::AlreadyRemoved => "activity_edit_already_removed",
        }
    }
}

impl std::fmt::Display for ActivityRecordStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDayFile { path } => write!(
                formatter,
                "activity day file is missing: {}",
                path.display()
            ),
            Self::MissingRecord {
                facet,
                day,
                record_id,
            } => write!(
                formatter,
                "activity row is missing: {facet}/{day}/{record_id}"
            ),
            Self::Path(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::Facet(error) => error.fmt(formatter),
            Self::Identity(error) => error.fmt(formatter),
            Self::DestinationMuted { facet } => {
                write!(formatter, "destination facet is muted: {facet}")
            }
            Self::PublicationRefused { detail } | Self::BodyRecordMoving { detail } => {
                write!(formatter, "{detail}")
            }
            Self::EditRefused(refusal) => write!(
                formatter,
                "{}",
                match refusal {
                    EditRefusal::OutOfRange => "no edit is at that position",
                    EditRefusal::Mismatch => "the edit at that position is a different edit",
                    EditRefusal::NotRemovable => "hiding and unhiding are undone, not removed",
                    EditRefusal::NotRecorded => "that edit did not record its values",
                    EditRefusal::AlreadyRemoved => "that edit is already removed",
                }
            ),
        }
    }
}

impl std::error::Error for ActivityRecordStoreError {}

impl ActivityRecordStoreError {
    /// Whether the store gave up waiting for the day file's lock.
    #[must_use]
    pub fn is_lock_timeout(&self) -> bool {
        matches!(self, Self::Lock(LockError::Timeout(_)))
    }
}

impl From<PathError> for ActivityRecordStoreError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}
impl From<LockError> for ActivityRecordStoreError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}
impl From<ReadError> for ActivityRecordStoreError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}
impl From<solstone_core_journal_io::AtomicWriteError> for ActivityRecordStoreError {
    fn from(error: solstone_core_journal_io::AtomicWriteError) -> Self {
        Self::Write(error)
    }
}
impl From<serde_json::Error> for ActivityRecordStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl From<super::error::FacetStoreError> for ActivityRecordStoreError {
    fn from(error: super::error::FacetStoreError) -> Self {
        Self::Facet(error)
    }
}
impl From<super::declaration::FacetIdentityError> for ActivityRecordStoreError {
    fn from(error: super::declaration::FacetIdentityError) -> Self {
        Self::Identity(error)
    }
}

#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub enum AppendOutcome {
    Written(ActivityRecord),
    AlreadyExists,
}

fn day_path(root: &Path, facet: &str, day: &str) -> Result<PathBuf, ActivityRecordStoreError> {
    contained_path(root, &format!("facets/{facet}/activities/{day}.jsonl")).map_err(Into::into)
}

pub fn list_activity_day_stems(
    root: &Path,
    facet: &str,
) -> Result<Vec<String>, ActivityRecordStoreError> {
    let dir = contained_path(root, &format!("facets/{facet}/activities"))
        .map_err(ActivityRecordStoreError::from)?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir).map_err(|source| ReadError::Io {
        path: dir.clone(),
        source,
    })?;
    let mut stems = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ReadError::Io {
            path: dir.clone(),
            source,
        })?;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if let Some(stem) = name_str.strip_suffix(".jsonl")
            && stem.len() == 8
            && stem.bytes().all(|b| b.is_ascii_digit())
        {
            stems.push(stem.to_owned());
        }
    }
    stems.sort();
    Ok(stems)
}

/// A day's records for a reader: a file that isn't text can't be read, and a
/// line that isn't a record is skipped.
fn read_rows(path: &Path) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(read_text(path, String::new())?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| value.as_object().cloned())
        .collect())
}

/// A day's records, every line that is one; a line that isn't costs only
/// itself. Unlike [`read_activity_records_strict`], a half-written line
/// doesn't make the day unreadable.
///
/// # Errors
///
/// When the file can't be read at all.
pub fn read_activity_records_readable(
    root: &Path,
    facet: &str,
    day: &str,
) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    Ok(read_day_lines(&day_path(root, facet, day)?)?
        .rows
        .into_iter()
        .map(normalize)
        .collect())
}

fn read_rows_strict(path: &Path) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = read_text(path, String::new())?;
    parse_rows_strict(&text)
}

fn parse_rows_strict(text: &str) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record = serde_json::from_str::<ActivityRecord>(trimmed)
            .map_err(ActivityRecordStoreError::Json)?;
        rows.push(record);
    }
    Ok(rows)
}

/// A day file as a write reads it: its rows, and every other line, kept as it
/// is with how many rows came before it. A write never loses a line it can't
/// read: a half-written line, or one that isn't a record, goes back verbatim.
#[derive(Default)]
struct DayLines {
    rows: Vec<ActivityRecord>,
    kept: Vec<(usize, Vec<u8>)>,
}

fn read_day_lines(path: &Path) -> Result<DayLines, ActivityRecordStoreError> {
    let mut day = DayLines::default();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(day),
        Err(source) => {
            return Err(ReadError::Io {
                path: path.to_owned(),
                source,
            }
            .into());
        }
    };
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(line) {
            Ok(Value::Object(row)) => day.rows.push(row),
            _ => day.kept.push((day.rows.len(), line.to_vec())),
        }
    }
    Ok(day)
}

fn write_rows(
    path: &Path,
    rows: &[ActivityRecord],
    kept: &[(usize, Vec<u8>)],
) -> Result<(), ActivityRecordStoreError> {
    let mut bytes = Vec::new();
    let mut kept = kept.iter().peekable();
    for (index, row) in rows.iter().enumerate() {
        while let Some((_, line)) = kept.next_if(|(before, _)| *before <= index) {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }
        serde_json::to_writer(&mut bytes, row)?;
        bytes.push(b'\n');
    }
    for (_, line) in kept {
        bytes.extend_from_slice(line);
        bytes.push(b'\n');
    }
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })?;
    Ok(())
}

/// Reconcile one current activity after backfill or a participation change.
/// It is safe to queue this coordinate: no stale record snapshot is accepted.
pub fn reconcile_faceprints_for_activity(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
) -> Result<usize, String> {
    if !solstone_core_entity::faceprints::enabled(root)? {
        return Ok(0);
    }
    let _trust = solstone_core_entity::hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    let path = day_path(root, facet, day).map_err(|e| e.to_string())?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(|e| e.to_string())?;
    let records = read_day_lines(&path).map_err(|e| e.to_string())?;
    if !records.kept.is_empty() {
        return Err("unreadable activity rows; face reconciliation deferred".into());
    }
    let absent = Map::from_iter([("id".to_owned(), Value::String(record_id.to_owned()))]);
    let record = records
        .rows
        .iter()
        .find(|r| id(r) == record_id)
        .unwrap_or(&absent);
    solstone_core_entity::faceprints::accumulation::reconcile_activity(root, facet, day, record)
}

/// Replay private face evidence from current activity records, without inference.
/// The maintenance scheduler calls this after an interrupted derived write. The
/// same trust lock as participation writers prevents an old assignment winning.
pub fn reconcile_faceprints(root: &Path) -> Result<usize, String> {
    if !solstone_core_entity::faceprints::enabled(root)? {
        return Ok(0);
    }
    let _trust = solstone_core_entity::hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    let entities = solstone_core_entity::live_journal_entities(root).map_err(|e| e.to_string())?;
    let mut live = std::collections::BTreeSet::new();
    let mut added = 0;
    for name in super::map::list_declared_facet_names(root).map_err(|e| e.to_string())? {
        let name = name.as_str();
        let activity_dir = root.join("facets").join(name).join("activities");
        if !activity_dir.is_dir() {
            continue;
        }
        for day in
            solstone_core_journal_io::list_dir_entries(&activity_dir).map_err(|e| e.to_string())?
        {
            if day.kind != solstone_core_journal_io::DirEntryKind::File
                || day.path.extension().and_then(|s| s.to_str()) != Some("jsonl")
            {
                continue;
            }
            let day_name = day
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("invalid activity day")?;
            let _lock = hold_lock(&day.path, LockOptions::default()).map_err(|e| e.to_string())?;
            let records = read_day_lines(&day.path).map_err(|e| e.to_string())?;
            if !records.kept.is_empty() {
                return Err("unreadable activity rows; face reconciliation deferred".into());
            }
            for row in records.rows {
                live.insert(
                    serde_json::to_string(&[name, day_name, id(&row)])
                        .map_err(|e| e.to_string())?,
                );
                added += solstone_core_entity::faceprints::accumulation::reconcile_activity_with_entities(
                    root, name, day_name, &row, &entities,
                )?;
            }
        }
    }
    solstone_core_entity::faceprints::remove_orphan_activities(root, &live)?;
    Ok(added)
}

/// Point rows a facet merge moved here at this facet. A merge copies a
/// facet's activity rows as they are, so a moved row still names the merged
/// facet's id, and every write-side identity check refuses it. A row whose
/// stored id is a retired facet id whose merge or rename chain ends at
/// `current_id` is given `current_id`; nothing else in it changes. Rows with a
/// deleted facet's id, a live facet's id or no id are left alone, as is every
/// row when the journal's facets can't be read.
fn adopt_moved_rows(
    root: &Path,
    current_id: &str,
    rows: Vec<ActivityRecord>,
) -> Vec<ActivityRecord> {
    let moved = |row: &ActivityRecord| {
        row.get("destination_id")
            .and_then(Value::as_str)
            .is_some_and(|stored| stored != current_id)
    };
    if !rows.iter().any(moved) {
        return rows;
    }
    let Ok(facets) =
        solstone_core_indexer_store::classification::FacetDeclarationSet::from_journal(root)
    else {
        return rows;
    };
    rows.into_iter()
        .map(|mut row| {
            let adopt = row
                .get("destination_id")
                .and_then(Value::as_str)
                .is_some_and(|stored| facets.id_resolves_to(stored, current_id));
            if adopt {
                row.insert(
                    "destination_id".to_owned(),
                    Value::String(current_id.to_owned()),
                );
            }
            row
        })
        .collect()
}

/// Adopt one day's rows moved here by a facet merge, before automated work
/// reads them. Returns how many rows changed. A day with no file changes
/// nothing.
pub fn adopt_moved_activity_rows(
    root: &Path,
    facet: &str,
    day: &str,
) -> Result<usize, ActivityRecordStoreError> {
    // Look first, without locks or side effects: most days have nothing to
    // adopt, and a missing day or an unready facet has nothing to change.
    let path = day_path(root, facet, day)?;
    if !path.exists() {
        return Ok(0);
    }
    let super::declaration::DestinationObservation::Ready { id, .. } =
        super::declaration::observe_facet_destination(root, facet)?
    else {
        return Ok(0);
    };
    let unadopted = read_rows(&path)?.iter().any(|row| {
        row.get("destination_id")
            .and_then(Value::as_str)
            .is_some_and(|stored| stored != id)
    });
    if !unadopted {
        return Ok(0);
    }
    let current_id = admit_activity_destination(root, facet)?;
    let result = locked_modify_day_records(root, facet, day, false, &current_id, |rows| {
        let before = rows.clone();
        let adopted = adopt_moved_rows(root, &current_id, rows);
        let changed = adopted
            .iter()
            .zip(&before)
            .filter(|(after, before)| after != before)
            .count();
        Ok((adopted, changed))
    });
    match result {
        Err(ActivityRecordStoreError::MissingDayFile { .. }) => Ok(0),
        other => other,
    }
}

pub(super) fn locked_modify_day_records<T>(
    root: &Path,
    facet: &str,
    day: &str,
    create_if_missing: bool,
    expected_facet_id: &str,
    modify: impl FnOnce(
        Vec<ActivityRecord>,
    ) -> Result<(Vec<ActivityRecord>, T), ActivityRecordStoreError>,
) -> Result<T, ActivityRecordStoreError> {
    let (result, written) = write_day_records(
        root,
        facet,
        day,
        create_if_missing,
        expected_facet_id,
        modify,
    )?;
    if let Some(Written {
        path,
        hidden_changed,
        face_ids,
    }) = written
    {
        super::publication::publish_saved(root, &path);
        // Whoever changed a row's hidden, its outputs are read again, so
        // search leaves out what a hidden activity made, or takes it back.
        for id in hidden_changed {
            super::publication::publish_activity_outputs(root, facet, day, &id);
        }
        // Publication and its notifications have completed before derived work.
        // Re-read under the source locks: a newer assignment always wins.
        for id in face_ids {
            if let Err(error) = reconcile_faceprints_for_activity(root, facet, day, &id) {
                log::warn!("face activity reconciliation deferred: {error}");
            }
        }
    }
    Ok(result)
}

/// A day file [`write_day_records`] wrote, with the rows whose hidden it
/// changed.
struct Written {
    path: PathBuf,
    hidden_changed: Vec<String>,
    face_ids: std::collections::BTreeSet<String>,
}

/// The ids whose hidden differs between `before` and `after`. A row that is
/// absent reads as not hidden, so a hidden row that arrives or leaves counts.
fn hidden_changed(before: &[ActivityRecord], after: &[ActivityRecord]) -> Vec<String> {
    let hidden_by_id = |rows: &[ActivityRecord]| {
        let mut by_id = std::collections::BTreeMap::new();
        for row in rows {
            let entry = by_id.entry(id(row).to_owned()).or_insert(false);
            *entry |= hidden(row);
        }
        by_id
    };
    let (before, after) = (hidden_by_id(before), hidden_by_id(after));
    let ids: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    ids.into_iter()
        .filter(|id| !id.is_empty())
        .filter(|id| {
            before.get(*id).copied().unwrap_or(false) != after.get(*id).copied().unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// The locked half of [`locked_modify_day_records`]: the day file when it was
/// written.
fn write_day_records<T>(
    root: &Path,
    facet: &str,
    day: &str,
    create_if_missing: bool,
    expected_facet_id: &str,
    modify: impl FnOnce(
        Vec<ActivityRecord>,
    ) -> Result<(Vec<ActivityRecord>, T), ActivityRecordStoreError>,
) -> Result<(T, Option<Written>), ActivityRecordStoreError> {
    let _entity =
        solstone_core_entity::hold_entity_trust_lock(root).map_err(|error| match error {
            solstone_core_entity::EntityTrustLockError::Path(error) => {
                ActivityRecordStoreError::Path(error)
            }
            solstone_core_entity::EntityTrustLockError::Lock(error) => {
                ActivityRecordStoreError::Lock(error)
            }
        })?;
    super::declaration::require_observed_facet_write_identity(root, facet, expected_facet_id)?;
    let path = day_path(root, facet, day)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let existed = path.exists();
    if !existed && !create_if_missing {
        return Err(ActivityRecordStoreError::MissingDayFile { path });
    }
    let DayLines {
        rows: current,
        kept,
    } = read_day_lines(&path)?;
    let (updated, result) = modify(current.clone())?;
    if !existed && updated.is_empty() {
        return Ok((result, None));
    }
    if existed && updated == current {
        return Ok((result, None));
    }
    write_rows(&path, &updated, &kept)?;
    let face_ids = current
        .iter()
        .chain(&updated)
        .filter(|row| {
            current.iter().find(|r| id(r) == id(row)) != updated.iter().find(|r| id(r) == id(row))
        })
        .map(|r| id(r).to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    let hidden_changed = hidden_changed(&current, &updated);
    Ok((
        result,
        Some(Written {
            path,
            hidden_changed,
            face_ids,
        }),
    ))
}

/// Plan records the retired `schedule` talent inferred. They stay on disk as
/// written; no reader returns them.
pub fn is_retired_plan(record: &ActivityRecord) -> bool {
    record.get("source").and_then(Value::as_str) == Some("anticipated")
}

pub use solstone_core_format::content::{
    BODY_RECORD_SOURCE, EDIT_REMOVED, EDIT_STRETCH, STRETCH_ID, STRETCH_SEGMENTS,
    activity_field_held, correction_segments, edit_removed, is_body_record,
};
use solstone_core_format::content::{edit_names, owner_edit};

pub fn load_activity_records(
    root: &Path,
    facet: &str,
    day: &str,
    include_hidden: bool,
) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    let rows = read_rows(&day_path(root, facet, day)?)?
        .into_iter()
        .map(normalize)
        .collect::<Vec<_>>();
    let superseded = super::superseded::superseded_record_ids(root, facet, day, &rows);
    Ok(rows
        .into_iter()
        .filter(|row| {
            !is_retired_plan(row)
                && !is_body_record(row)
                && (include_hidden || !hidden(row))
                && !superseded.contains(id(row))
        })
        .collect())
}

/// [`load_activity_records`] for the owner's day view: a line that isn't text,
/// or isn't a record, costs only itself rather than the day.
///
/// # Errors
///
/// When the file can't be read at all.
pub fn load_activity_records_readable(
    root: &Path,
    facet: &str,
    day: &str,
    include_hidden: bool,
) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    let rows = read_activity_records_readable(root, facet, day)?;
    let superseded = super::superseded::superseded_record_ids(root, facet, day, &rows);
    Ok(rows
        .into_iter()
        .filter(|row| {
            !is_retired_plan(row)
                && !is_body_record(row)
                && (include_hidden || !hidden(row))
                && !superseded.contains(id(row))
        })
        .collect())
}

pub fn read_activity_records_strict(
    root: &Path,
    facet: &str,
    day: &str,
) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    let rows = read_rows_strict(&day_path(root, facet, day)?)?;
    Ok(rows.into_iter().map(normalize).collect())
}

/// Apply the maintained strict projection to bytes already admitted by a
/// bounded native reader. This never reopens a pathname during consumption.
pub fn parse_activity_records_strict(
    bytes: &[u8],
) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    let text = std::str::from_utf8(bytes).map_err(|error| ReadError::Io {
        path: PathBuf::from("activity rows"),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
    })?;
    Ok(parse_rows_strict(text)?
        .into_iter()
        .map(normalize)
        .collect())
}

/// Admit a write to an existing declaration, adopting only a historical missing ID.
pub fn admit_activity_destination(
    root: &Path,
    facet: &str,
) -> Result<String, ActivityRecordStoreError> {
    let _guard = crate::hold_facet_trust_lock(root).map_err(|e| {
        ActivityRecordStoreError::Identity(super::declaration::FacetIdentityError::Failed(
            e.to_string(),
        ))
    })?;
    use solstone_core_journal_io::durability::{
        ArtifactId, DurableObservation, observe_json_durable,
    };
    let path = super::paths::declaration_path(root, facet)?;
    match observe_json_durable::<Value>(ArtifactId::FacetDeclaration, &path) {
        DurableObservation::Absent => Err(ActivityRecordStoreError::Identity(
            super::declaration::FacetIdentityError::Absent,
        )),
        DurableObservation::Malformed { path, source } => Err(ActivityRecordStoreError::Identity(
            super::declaration::FacetIdentityError::Failed(format!("{}: {source}", path.display())),
        )),
        DurableObservation::Unreadable { path, source } => Err(ActivityRecordStoreError::Identity(
            super::declaration::FacetIdentityError::Failed(format!("{}: {source}", path.display())),
        )),
        DurableObservation::Present(value) => {
            let Some(object) = value.as_object() else {
                return Err(ActivityRecordStoreError::Identity(
                    super::declaration::FacetIdentityError::Failed(format!(
                        "{}: facet declaration is not an object",
                        path.display()
                    )),
                ));
            };
            if let Some(id) = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| super::facet_id::is_well_formed_facet_id(id))
            {
                Ok(id.to_owned())
            } else if object.contains_key("id") {
                Err(ActivityRecordStoreError::Identity(
                    super::declaration::FacetIdentityError::Invalid,
                ))
            } else {
                super::facet_id::ensure_daily_facet_id(root, facet).map_err(|e| {
                    ActivityRecordStoreError::Identity(
                        super::declaration::FacetIdentityError::Failed(e.to_string()),
                    )
                })?;
                Ok(super::declaration::observe_facet_write_identity(
                    root, facet,
                )?)
            }
        }
    }
}

/// Hold lifecycle exclusion across automated enrichment validation and publication.
pub fn hold_activity_enrichment(
    root: &Path,
    facet: &str,
    expected_id: &str,
) -> Result<crate::FacetTrustLock, ActivityRecordStoreError> {
    let guard = crate::hold_facet_trust_lock(root).map_err(|e| {
        ActivityRecordStoreError::Identity(super::declaration::FacetIdentityError::Failed(
            e.to_string(),
        ))
    })?;
    super::declaration::require_observed_facet_write_identity(root, facet, expected_id)?;
    if matches!(
        super::declaration::observe_facet_destination(root, facet)?,
        super::declaration::DestinationObservation::Ready { muted: true, .. }
    ) {
        return Err(ActivityRecordStoreError::DestinationMuted {
            facet: facet.to_owned(),
        });
    }
    Ok(guard)
}

pub fn get_activity_record(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    Ok(load_activity_records(root, facet, day, true)?
        .into_iter()
        .find(|row| id(row) == record_id))
}

pub fn append_activity_record(
    root: &Path,
    facet: &str,
    day: &str,
    record: ActivityRecord,
) -> Result<AppendOutcome, ActivityRecordStoreError> {
    append_activity_record_with_check(root, facet, day, record, || Ok(()))
}

pub fn append_activity_record_checked(
    root: &Path,
    facet: &str,
    day: &str,
    record: ActivityRecord,
    checkpoint: impl Fn() -> Result<(), String>,
) -> Result<AppendOutcome, ActivityRecordStoreError> {
    let _guard = crate::hold_facet_trust_lock(root).map_err(|error| {
        ActivityRecordStoreError::Identity(super::declaration::FacetIdentityError::Failed(
            error.to_string(),
        ))
    })?;
    checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
    append_activity_record_with_check(root, facet, day, record, checkpoint)
}

fn append_activity_record_with_check(
    root: &Path,
    facet: &str,
    day: &str,
    mut record: ActivityRecord,
    checkpoint: impl Fn() -> Result<(), String>,
) -> Result<AppendOutcome, ActivityRecordStoreError> {
    let facet_id = admit_activity_destination(root, facet)?;
    record.insert(
        "destination_id".to_string(),
        Value::String(facet_id.clone()),
    );
    locked_modify_day_records(root, facet, day, true, &facet_id, |rows| {
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        super::body_moves::refuse_create_while_moving(root, &record)?;
        let record_id = id(&record).to_owned();
        if !record_id.is_empty() && rows.iter().any(|row| id(row) == record_id) {
            return Ok((rows, AppendOutcome::AlreadyExists));
        }
        let written = normalize(record);
        let mut updated = rows;
        updated.push(written.clone());
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        Ok((updated, AppendOutcome::Written(written)))
    })
}

#[allow(clippy::too_many_arguments)] // These fields are the public record-mutation contract.
pub fn update_activity_record(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    patch: &Map<String, Value>,
    actor: &str,
    note: &str,
    timestamp: &str,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    update_activity_record_with(
        root,
        facet,
        day,
        record_id,
        |_| Some(patch.clone()),
        actor,
        note,
        timestamp,
    )
}

/// Like `update_activity_record`, with the patch derived from the record as
/// it stands under the day file's lock, so a patch that replaces a whole array
/// can never be computed from an older copy of it. `derive` returning `None`
/// writes no edit and returns `None`.
#[allow(clippy::too_many_arguments)]
pub fn update_activity_record_with(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    derive: impl FnOnce(&ActivityRecord) -> Option<Map<String, Value>>,
    actor: &str,
    note: &str,
    timestamp: &str,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    update_activity_record_with_check(
        root,
        facet,
        day,
        record_id,
        derive,
        actor,
        note,
        timestamp,
        || Ok(()),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn update_activity_record_checked(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    patch: &Map<String, Value>,
    actor: &str,
    note: &str,
    timestamp: &str,
    checkpoint: impl Fn() -> Result<(), String>,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    let _guard = crate::hold_facet_trust_lock(root).map_err(|error| {
        ActivityRecordStoreError::Identity(super::declaration::FacetIdentityError::Failed(
            error.to_string(),
        ))
    })?;
    // Derived publication must recheck manual holds while the normal facet
    // trust lock excludes edits through the eventual patch publication. A
    // pre-Generate observation cannot protect an edit made during Generate.
    if matches!(actor, "story" | "participation") {
        let rows = read_activity_records_strict(root, facet, day)?;
        if rows
            .iter()
            .filter(|row| id(row) == record_id)
            .any(|row| patch.keys().any(|key| activity_field_held(row, key)))
        {
            return Err(ActivityRecordStoreError::PublicationRefused {
                detail: "activity output is held by owner".into(),
            });
        }
    }
    checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
    update_activity_record_with_check(
        root,
        facet,
        day,
        record_id,
        |_| Some(patch.clone()),
        actor,
        note,
        timestamp,
        checkpoint,
        None,
    )
}

/// [`update_activity_record`] for an owner edit that carries `extra` in its
/// history entry, beside what it set and what those fields held before.
#[allow(clippy::too_many_arguments)]
pub fn update_activity_record_noting(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    patch: &Map<String, Value>,
    actor: &str,
    note: &str,
    timestamp: &str,
    extra: &Map<String, Value>,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    update_activity_record_with_check(
        root,
        facet,
        day,
        record_id,
        |_| Some(patch.clone()),
        actor,
        note,
        timestamp,
        || Ok(()),
        Some(extra),
    )
}

#[allow(clippy::too_many_arguments)]
fn update_activity_record_with_check(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    derive: impl FnOnce(&ActivityRecord) -> Option<Map<String, Value>>,
    actor: &str,
    note: &str,
    timestamp: &str,
    checkpoint: impl Fn() -> Result<(), String>,
    extra: Option<&Map<String, Value>>,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    let current_id = admit_activity_destination(root, facet)?;
    let mut derive = Some(derive);
    locked_modify_day_records(root, facet, day, false, &current_id, |rows| {
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        refuse_while_moving(root, facet, day, &rows, record_id)?;
        let rows = adopt_moved_rows(root, &current_id, rows);
        let mut result = None;
        for row in &rows {
            if id(row) == record_id
                && let Some(stored_id) = row.get("destination_id").and_then(Value::as_str)
            {
                super::declaration::require_observed_facet_write_identity(root, facet, stored_id)?;
            }
        }
        let updated = rows
            .into_iter()
            .map(|row| {
                if id(&row) != record_id || !actor_may_write(&row, actor) {
                    return row;
                }
                let Some(patch) = derive.take().and_then(|derive| derive(&row)) else {
                    return row;
                };
                let before = records_values(actor).then(|| {
                    patch
                        .keys()
                        .filter_map(|key| Some((key.clone(), row.get(key)?.clone())))
                        .collect::<Map<_, _>>()
                });
                let mut merged = row;
                for (key, value) in &patch {
                    merged.insert(key.clone(), value.clone());
                }
                merged.insert(
                    "destination_id".to_owned(),
                    Value::String(current_id.clone()),
                );
                let mut edited = append_edit(
                    normalize(merged),
                    actor,
                    patch.keys().cloned().collect(),
                    note,
                    timestamp,
                );
                if let Some(before) = before
                    && let Some(Value::Object(edit)) = edited
                        .get_mut("edits")
                        .and_then(Value::as_array_mut)
                        .and_then(|edits| edits.last_mut())
                {
                    edit.insert(EDIT_SET.to_owned(), Value::Object(patch.clone()));
                    edit.insert(EDIT_BEFORE.to_owned(), Value::Object(before));
                }
                if let Some(extra) = extra
                    && let Some(Value::Object(edit)) = edited
                        .get_mut("edits")
                        .and_then(Value::as_array_mut)
                        .and_then(|edits| edits.last_mut())
                {
                    edit.extend(extra.clone());
                }
                result = Some(edited.clone());
                edited
            })
            .collect();
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        Ok((updated, result))
    })
}

/// What refreshing a body record's account did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyAccountRefresh {
    /// The account's fields changed to the ones given.
    Refreshed,
    /// They already were the ones given; nothing was written.
    Unchanged,
    /// No body record with that id is in the day file.
    NotFound,
}

/// The fields code wrote on a body record's account, set to what its account
/// says now: `account.start`, `end`, `recorder` and `type`, and the record's
/// `activity`. A `None` type removes it. The record is re-read under the day
/// file's lock and nothing else in it changes, so an owner's title, hide,
/// `apart` or edit made a moment before stays. Lines the store can't read stay
/// as they are.
///
/// # Errors
///
/// The store's errors for the day file and the facet.
#[allow(clippy::too_many_arguments)]
pub fn refresh_body_account(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    activity: &str,
    start: &str,
    end: &str,
    recorder: &str,
    workout_type: Option<&str>,
) -> Result<BodyAccountRefresh, ActivityRecordStoreError> {
    let current_id = admit_activity_destination(root, facet)?;
    locked_modify_day_records(root, facet, day, false, &current_id, |rows| {
        refuse_while_moving(root, facet, day, &rows, record_id)?;
        let mut outcome = BodyAccountRefresh::NotFound;
        let updated = rows
            .into_iter()
            .map(|mut row| {
                if id(&row) != record_id || !is_body_record(&row) {
                    return row;
                }
                let Some(Value::Object(account)) = row.get_mut("account") else {
                    return row;
                };
                let mut changed = false;
                for (field, value) in [("start", start), ("end", end), ("recorder", recorder)] {
                    if account.get(field).and_then(Value::as_str) != Some(value) {
                        account.insert(field.to_owned(), Value::String(value.to_owned()));
                        changed = true;
                    }
                }
                match workout_type {
                    Some(kind) if account.get("type").and_then(Value::as_str) != Some(kind) => {
                        account.insert("type".to_owned(), Value::String(kind.to_owned()));
                        changed = true;
                    }
                    None if account.remove("type").is_some() => changed = true,
                    _ => {}
                }
                if row.get("activity").and_then(Value::as_str) != Some(activity) {
                    row.insert("activity".to_owned(), Value::String(activity.to_owned()));
                    changed = true;
                }
                outcome = if changed {
                    BodyAccountRefresh::Refreshed
                } else {
                    BodyAccountRefresh::Unchanged
                };
                row
            })
            .collect();
        Ok((updated, outcome))
    })
}

#[allow(clippy::too_many_arguments)] // These fields are the public hidden-state mutation contract.
pub fn set_activity_hidden(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    hidden_value: bool,
    actor: &str,
    reason: Option<&str>,
    timestamp: &str,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    set_activity_hidden_with_check(
        root,
        facet,
        day,
        record_id,
        hidden_value,
        actor,
        reason,
        timestamp,
        || Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn set_activity_hidden_checked(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    hidden_value: bool,
    actor: &str,
    reason: Option<&str>,
    timestamp: &str,
    checkpoint: impl Fn() -> Result<(), String>,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    let _guard = crate::hold_facet_trust_lock(root).map_err(|error| {
        ActivityRecordStoreError::Identity(super::declaration::FacetIdentityError::Failed(
            error.to_string(),
        ))
    })?;
    checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
    set_activity_hidden_with_check(
        root,
        facet,
        day,
        record_id,
        hidden_value,
        actor,
        reason,
        timestamp,
        checkpoint,
    )
}

#[allow(clippy::too_many_arguments)]
fn set_activity_hidden_with_check(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    hidden_value: bool,
    actor: &str,
    reason: Option<&str>,
    timestamp: &str,
    checkpoint: impl Fn() -> Result<(), String>,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    let current_id = admit_activity_destination(root, facet)?;
    locked_modify_day_records(root, facet, day, false, &current_id, |rows| {
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        refuse_while_moving(root, facet, day, &rows, record_id)?;
        let rows = adopt_moved_rows(root, &current_id, rows);
        let mut result = None;
        for row in &rows {
            if id(row) == record_id
                && let Some(stored_id) = row.get("destination_id").and_then(Value::as_str)
            {
                super::declaration::require_observed_facet_write_identity(root, facet, stored_id)?;
            }
        }
        let updated = rows
            .into_iter()
            .map(|row| {
                if id(&row) != record_id || !actor_may_write(&row, actor) {
                    return row;
                }
                let mut normalized = normalize(row);
                if hidden(&normalized) != hidden_value {
                    normalized.insert("hidden".to_owned(), Value::Bool(hidden_value));
                    normalized.insert(
                        "destination_id".to_owned(),
                        Value::String(current_id.clone()),
                    );
                    let edited = append_edit(
                        normalized,
                        actor,
                        vec!["hidden".to_owned()],
                        reason.unwrap_or(if hidden_value { "muted" } else { "unmuted" }),
                        timestamp,
                    );
                    result = Some(edited.clone());
                    edited
                } else {
                    result = Some(normalized.clone());
                    normalized
                }
            })
            .collect();
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        Ok((updated, result))
    })
}

/// Append an activity edit using the store's canonical edit shape.
pub fn append_edit(
    mut record: ActivityRecord,
    actor: &str,
    fields: Vec<String>,
    note: &str,
    timestamp: &str,
) -> ActivityRecord {
    let mut edits = record
        .get("edits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    edits.push(
        serde_json::json!({"timestamp": timestamp, "actor": actor, "fields": fields, "note": note}),
    );
    record.insert("edits".to_owned(), Value::Array(edits));
    record
}

fn refuse_while_moving(
    root: &Path,
    facet: &str,
    day: &str,
    rows: &[ActivityRecord],
    record_id: &str,
) -> Result<(), ActivityRecordStoreError> {
    super::body_moves::refuse_while_moving(
        root,
        &super::body_moves::BodyRecordPlace {
            day: day.to_owned(),
            facet: facet.to_owned(),
        },
        rows,
        record_id,
    )
}

/// Only the owner writes a body record. To anyone else it reads as no record,
/// as it does to every default reader.
fn actor_may_write(record: &ActivityRecord, actor: &str) -> bool {
    !is_body_record(record) || actor.starts_with("owner:")
}

pub(super) fn normalize(mut record: ActivityRecord) -> ActivityRecord {
    let title = title(&record);
    record.insert("title".to_owned(), Value::String(title));
    record.insert(
        "details".to_owned(),
        Value::String(string(&record, "details")),
    );
    record.insert("hidden".to_owned(), Value::Bool(hidden(&record)));
    let edits = record
        .get("edits")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.is_object())
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    record.insert("edits".to_owned(), Value::Array(edits));
    record
}

fn title(record: &ActivityRecord) -> String {
    for key in ["title", "description", "activity", "id"] {
        let value = string(record, key);
        if !value.is_empty() {
            return if matches!(key, "activity" | "id") {
                value
                    .replace('_', " ")
                    .split_whitespace()
                    .map(|part| {
                        let mut chars = part.chars();
                        chars
                            .next()
                            .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                value
            };
        }
    }
    "untitled activity".to_owned()
}
fn string(record: &ActivityRecord, key: &str) -> String {
    activity_value_or_empty(record.get(key)).trim().to_owned()
}
fn hidden(record: &ActivityRecord) -> bool {
    record.get("hidden").is_some_and(activity_value_truthy)
}

/// What a recorded edit set, per field.
pub const EDIT_SET: &str = "set";
/// What each field an edit set held just before. A field that was absent is
/// absent here too.
pub const EDIT_BEFORE: &str = "before";
/// The actor of an edit recording who the owner said spoke some of the
/// activity's lines.
pub const OWNER_LINES_ACTOR: &str = "owner:lines";
/// The lines an owner lines edit labelled: each line's stream, segment,
/// source and sentence id, the `set_at` token its answer was written with and
/// the owner's answer it replaced (`prior`, or null).
pub const EDIT_LINES: &str = "lines";
/// Who an owner lines edit says spoke its lines, in tagged form.
pub const EDIT_SPEAKER: &str = "speaker";

fn records_values(actor: &str) -> bool {
    matches!(actor, "owner:update" | "cli:update")
}

fn lines_edit(edit: &Map<String, Value>) -> bool {
    edit.get("actor").and_then(Value::as_str) == Some(OWNER_LINES_ACTOR)
}

fn edit_recorded(edit: &Map<String, Value>) -> bool {
    edit.get(EDIT_SET).is_some_and(Value::is_object)
        && edit.get(EDIT_BEFORE).is_some_and(Value::is_object)
}

/// Whether an edit can be removed: an update that recorded its values and
/// has not been removed yet.
pub fn edit_removable(edit: &Map<String, Value>) -> bool {
    !edit_removed(edit)
        && if lines_edit(edit) {
            edit.get(EDIT_LINES).is_some_and(Value::is_array)
        } else {
            edit.get("actor")
                .and_then(Value::as_str)
                .is_some_and(records_values)
                && edit_recorded(edit)
        }
}

/// The value `field` takes once the edits that are removed no longer count:
/// the newest remaining owner edit's value, else what the field held before
/// the oldest owner edit since the journal last wrote it. `None` is absent.
/// `Err` leaves the field as it is, for a history this cannot read.
fn field_after_removal(edits: &[Map<String, Value>], field: &str) -> Result<Option<Value>, ()> {
    let naming = edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| edit_names(edit, field))
        .collect::<Vec<_>>();
    let window = naming
        .iter()
        .rposition(|(_, edit)| !owner_edit(edit))
        .map_or(&naming[..], |last| &naming[last + 1..]);
    let recorded = |edit: &Map<String, Value>, key: &str| {
        edit.get(key)
            .and_then(Value::as_object)
            .map(|values| values.get(field).cloned())
    };
    let after = match window.iter().rposition(|(_, edit)| !edit_removed(edit)) {
        Some(newest) => {
            if let Some(set) = recorded(window[newest].1, EDIT_SET) {
                return set.map(Some).ok_or(());
            }
            newest + 1
        }
        None => 0,
    };
    window[after..]
        .iter()
        .find_map(|(_, edit)| edit_recorded(edit).then(|| recorded(edit, EDIT_BEFORE)))
        .flatten()
        .ok_or(())
}

/// Remove the edit at `position` in the record's edit list, once its
/// timestamp and actor match, and give every field it set the value the
/// remaining edits leave it. The edit stays in the history, marked removed.
#[allow(clippy::too_many_arguments)] // These fields name the one edit removed.
pub fn remove_activity_edit(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    position: usize,
    edit_timestamp: &str,
    edit_actor: &str,
    removed_at: &str,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    modify_record(root, facet, day, record_id, |mut record| {
        let mut edits = record
            .get("edits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .cloned()
            .collect::<Vec<_>>();
        let refuse = |refusal| Err(ActivityRecordStoreError::EditRefused(refusal));
        let Some(edit) = edits.get_mut(position) else {
            return refuse(EditRefusal::OutOfRange);
        };
        if edit.get("timestamp").and_then(Value::as_str) != Some(edit_timestamp)
            || edit.get("actor").and_then(Value::as_str) != Some(edit_actor)
        {
            return refuse(EditRefusal::Mismatch);
        }
        let lines = lines_edit(edit);
        if !lines
            && !edit
                .get("actor")
                .and_then(Value::as_str)
                .is_some_and(records_values)
        {
            return refuse(EditRefusal::NotRemovable);
        }
        if edit_removed(edit) {
            return refuse(EditRefusal::AlreadyRemoved);
        }
        if !edit_removable(edit) {
            return refuse(EditRefusal::NotRecorded);
        }
        edit.insert(
            EDIT_REMOVED.to_owned(),
            Value::String(removed_at.to_owned()),
        );
        // A lines edit sets no field; its lines are put back by the caller.
        let fields = edit
            .get(EDIT_SET)
            .and_then(Value::as_object)
            .filter(|_| !lines)
            .map(|set| set.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for field in fields {
            match field_after_removal(&edits, &field) {
                Ok(Some(value)) => {
                    record.insert(field, value);
                }
                Ok(None) => {
                    record.remove(&field);
                }
                Err(()) => {}
            }
        }
        record.insert(
            "edits".to_owned(),
            Value::Array(edits.into_iter().map(Value::Object).collect()),
        );
        Ok(record)
    })
}

/// Record who the owner said spoke some of the activity's lines: append an
/// [`OWNER_LINES_ACTOR`] edit carrying `lines` and `speaker`. It sets no
/// field, so no talent output is held by it. `checkpoint` runs under the
/// day's lock before anything is written; an error from it writes nothing.
#[allow(clippy::too_many_arguments)] // The edit and where it goes.
pub fn append_activity_lines_edit(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    lines: Vec<Value>,
    speaker: Value,
    timestamp: &str,
    checkpoint: impl FnOnce() -> Result<(), String>,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    let mut checkpoint = Some(checkpoint);
    modify_record(root, facet, day, record_id, |record| {
        if let Some(checkpoint) = checkpoint.take() {
            checkpoint()
                .map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        }
        let mut record = append_edit(record, OWNER_LINES_ACTOR, Vec::new(), "", timestamp);
        if let Some(Value::Object(edit)) = record
            .get_mut("edits")
            .and_then(Value::as_array_mut)
            .and_then(|edits| edits.last_mut())
        {
            edit.insert(EDIT_LINES.to_owned(), Value::Array(lines));
            edit.insert(EDIT_SPEAKER.to_owned(), speaker);
        }
        Ok(record)
    })
}

/// Change the one record `record_id` names under the day's lock, after its
/// stored destination is confirmed. `Ok(None)` when no record has that id.
fn modify_record(
    root: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
    change: impl FnOnce(ActivityRecord) -> Result<ActivityRecord, ActivityRecordStoreError>,
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    let current_id = admit_activity_destination(root, facet)?;
    let mut change = Some(change);
    locked_modify_day_records(root, facet, day, false, &current_id, |rows| {
        refuse_while_moving(root, facet, day, &rows, record_id)?;
        let rows = adopt_moved_rows(root, &current_id, rows);
        let mut result = None;
        let mut updated = Vec::with_capacity(rows.len());
        for row in rows {
            if id(&row) != record_id || result.is_some() {
                updated.push(row);
                continue;
            }
            if let Some(stored_id) = row.get("destination_id").and_then(Value::as_str) {
                super::declaration::require_observed_facet_write_identity(root, facet, stored_id)?;
            }
            let Some(change) = change.take() else {
                updated.push(row);
                continue;
            };
            let mut record = change(normalize(row))?;
            record.insert(
                "destination_id".to_owned(),
                Value::String(current_id.clone()),
            );
            let record = normalize(record);
            result = Some(record.clone());
            updated.push(record);
        }
        Ok((updated, result))
    })
}

/// Python's `bool(value)` for activity-record compatibility.
pub fn activity_value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value
            .as_i64()
            .map(|value| value != 0)
            .or_else(|| value.as_u64().map(|value| value != 0))
            .or_else(|| value.as_f64().map(|value| value != 0.0))
            .unwrap_or(false),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

/// Python's `str(value)` for activity request and record coercion.
pub fn activity_value_string(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(value) => {
            if *value {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(activity_value_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    python_quote(key),
                    activity_value_repr(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python's `str(value or "")` for activity request and record coercion.
pub fn activity_value_or_empty(value: Option<&Value>) -> String {
    value
        .filter(|value| activity_value_truthy(value))
        .map(activity_value_string)
        .unwrap_or_default()
}

fn activity_value_repr(value: &Value) -> String {
    match value {
        Value::String(value) => python_quote(value),
        Value::Array(_) | Value::Object(_) => activity_value_string(value),
        _ => activity_value_string(value),
    }
}

fn python_quote(value: &str) -> String {
    format!(
        "'{}'",
        value
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}
pub(super) fn id(record: &ActivityRecord) -> &str {
    record.get("id").and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_body_record_is_read_by_no_default_reader_and_written_only_by_the_owner() {
        use super::*;
        let root = tempfile::tempdir().unwrap();
        crate::create_facet(root.path(), "body", "Body", "Test", "#123456", "", None).unwrap();
        let day = "20261016";
        let capture = serde_json::json!({"id":"run_070000_300","activity":"running","segments":["070000_300"]});
        let body = serde_json::json!({"id":"body_0123456789abcdef","source":"body","activity":"workout",
            "account":{"key":"import.apple_health\u{1f}w1","start":"2026-10-16 07:00:00 -0600","end":"2026-10-16 07:40:00 -0600"}});
        for row in [&body, &capture] {
            let _ =
                append_activity_record(root.path(), "body", day, row.as_object().unwrap().clone())
                    .unwrap();
        }
        let ids = |rows: Vec<ActivityRecord>| {
            rows.iter()
                .map(|row| id(row).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids(load_activity_records(root.path(), "body", day, true).unwrap()),
            ["run_070000_300"]
        );
        assert!(
            get_activity_record(root.path(), "body", day, "body_0123456789abcdef")
                .unwrap()
                .is_none()
        );
        let patch = Map::from_iter([("title".into(), Value::from("Renamed"))]);
        for actor in ["cli:update", "talent:story"] {
            let before = std::fs::read(day_path(root.path(), "body", day).unwrap()).unwrap();
            let updated = update_activity_record(
                root.path(),
                "body",
                day,
                "body_0123456789abcdef",
                &patch,
                actor,
                "n",
                "2026-10-16T12:00:00Z",
            )
            .unwrap();
            let muted = set_activity_hidden(
                root.path(),
                "body",
                day,
                "body_0123456789abcdef",
                true,
                actor,
                None,
                "2026-10-16T12:00:00Z",
            )
            .unwrap();
            assert!(updated.is_none() && muted.is_none(), "{actor}");
            assert_eq!(
                std::fs::read(day_path(root.path(), "body", day).unwrap()).unwrap(),
                before,
                "{actor}"
            );
        }
        let updated = update_activity_record(
            root.path(),
            "body",
            day,
            "body_0123456789abcdef",
            &patch,
            "owner:update",
            "n",
            "2026-10-16T12:00:00Z",
        )
        .unwrap()
        .expect("the owner edits it");
        assert_eq!(updated["title"], "Renamed");
        assert!(activity_field_held(&updated, "title"));
    }

    #[test]
    fn only_a_lock_timeout_reads_as_a_lock_timeout() {
        let timeout = super::ActivityRecordStoreError::Lock(super::LockError::Timeout(
            solstone_core_journal_io::LockTimeout {
                path: "activities.jsonl".into(),
                timeout: std::time::Duration::from_millis(1),
            },
        ));
        assert!(timeout.is_lock_timeout());
        let missing = super::ActivityRecordStoreError::MissingDayFile {
            path: "activities.jsonl".into(),
        };
        assert!(!missing.is_lock_timeout());
    }

    use super::*;

    #[test]
    fn checked_derived_publication_preserves_owner_holds_and_unrelated_fields() {
        for (actor, field) in [("story", "story"), ("participation", "participation")] {
            let root = tempfile::tempdir().unwrap();
            crate::create_facet(root.path(), "home", "Home", "Test", "#123456", "", None).unwrap();
            let row = serde_json::json!({"id":"writing_090000_300","activity":"writing","facet":"home","segments":["090000_300"]});
            let _ = append_activity_record(
                root.path(),
                "home",
                "20261016",
                row.as_object().unwrap().clone(),
            )
            .unwrap();
            let patch = Map::from_iter([(field.into(), Value::from("owner value"))]);
            update_activity_record(
                root.path(),
                "home",
                "20261016",
                "writing_090000_300",
                &patch,
                "owner:test",
                "hold",
                "2026-10-16T12:00:00Z",
            )
            .unwrap();
            let before = std::fs::read(day_path(root.path(), "home", "20261016").unwrap()).unwrap();
            let derived = Map::from_iter([(field.into(), Value::from("derived replacement"))]);
            let error = update_activity_record_checked(
                root.path(),
                "home",
                "20261016",
                "writing_090000_300",
                &derived,
                actor,
                "derived",
                "2026-10-16T12:01:00Z",
                || Ok(()),
            )
            .unwrap_err();
            assert_eq!(error.to_string(), "activity output is held by owner");
            assert_eq!(
                std::fs::read(day_path(root.path(), "home", "20261016").unwrap()).unwrap(),
                before
            );
            let unheld = Map::from_iter([("unrelated".into(), Value::from("unheld output"))]);
            update_activity_record_checked(
                root.path(),
                "home",
                "20261016",
                "writing_090000_300",
                &unheld,
                actor,
                "unheld",
                "2026-10-16T12:02:00Z",
                || Ok(()),
            )
            .unwrap()
            .unwrap();
            let row = get_activity_record(root.path(), "home", "20261016", "writing_090000_300")
                .unwrap()
                .unwrap();
            assert_eq!(row[field], "owner value");
            assert_eq!(row["unrelated"], "unheld output");
        }
    }

    const DAY: &str = "20261016";
    const ROW: &str = "writing_090000_300";

    fn corrected_root(original: Value) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        crate::create_facet(root.path(), "home", "Home", "Test", "#123456", "", None).unwrap();
        let mut row = serde_json::json!({"id":ROW,"activity":"writing","segments":["090000_300"]});
        if let Value::Object(fields) = original {
            row.as_object_mut().unwrap().extend(fields);
        }
        let _ = append_activity_record(root.path(), "home", DAY, row.as_object().unwrap().clone())
            .unwrap();
        root
    }

    fn owner_set(root: &Path, field: &str, value: Value, at: &str) -> ActivityRecord {
        let patch = Map::from_iter([(field.to_owned(), value)]);
        update_activity_record(root, "home", DAY, ROW, &patch, "owner:update", "", at)
            .unwrap()
            .unwrap()
    }

    fn remove_at(root: &Path, position: usize) -> Result<ActivityRecord, ActivityRecordStoreError> {
        let record = get_activity_record(root, "home", DAY, ROW)
            .unwrap()
            .unwrap();
        let edit = record["edits"][position].clone();
        remove_activity_edit(
            root,
            "home",
            DAY,
            ROW,
            position,
            edit["timestamp"].as_str().unwrap_or_default(),
            edit["actor"].as_str().unwrap_or_default(),
            "2026-10-16T13:00:00Z",
        )
        .map(|record| record.unwrap())
    }

    #[test]
    fn an_owner_update_records_what_it_set_and_what_was_there() {
        let root = corrected_root(serde_json::json!({"description":"original"}));
        let record = owner_set(root.path(), "description", Value::from("A"), "t1");
        let edit = &record["edits"][0];
        assert_eq!(edit[EDIT_SET], serde_json::json!({"description":"A"}));
        assert_eq!(
            edit[EDIT_BEFORE],
            serde_json::json!({"description":"original"})
        );
        let record = owner_set(root.path(), "playback", Value::Bool(true), "t2");
        let edit = record["edits"][1].as_object().unwrap();
        assert_eq!(edit[EDIT_SET], serde_json::json!({"playback":true}));
        assert_eq!(edit[EDIT_BEFORE], serde_json::json!({}));
        assert!(activity_field_held(&record, "playback"));
        let story = Map::from_iter([("story".to_owned(), Value::from("body"))]);
        let record =
            update_activity_record(root.path(), "home", DAY, ROW, &story, "story", "", "t3")
                .unwrap()
                .unwrap();
        assert!(
            !record["edits"][2]
                .as_object()
                .unwrap()
                .contains_key(EDIT_SET)
        );
    }

    #[test]
    fn removing_edits_newest_first_returns_the_field_to_the_journal() {
        for (field, original) in [
            ("description", Value::from("original")),
            ("details", Value::from("")),
        ] {
            let root = corrected_root(serde_json::json!({ field: original.clone() }));
            owner_set(root.path(), field, Value::from("A"), "t1");
            owner_set(root.path(), field, Value::from("B"), "t2");
            let record = remove_at(root.path(), 1).unwrap();
            assert_eq!(record[field], "A", "{field}");
            assert!(activity_field_held(&record, field), "{field}");
            let record = remove_at(root.path(), 0).unwrap();
            assert_eq!(record[field], original, "{field}");
            assert!(!activity_field_held(&record, field), "{field}");
        }
        // The journal's line write can set the description again.
        let root = corrected_root(serde_json::json!({"description":"original"}));
        owner_set(root.path(), "description", Value::from("A"), "t1");
        remove_at(root.path(), 0).unwrap();
        let line = Map::from_iter([("description".to_owned(), Value::from("journal line"))]);
        let record = update_activity_record_checked(
            root.path(),
            "home",
            DAY,
            ROW,
            &line,
            "story",
            "",
            "t3",
            || Ok(()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(record["description"], "journal line");
    }

    #[test]
    fn removing_edits_oldest_first_keeps_the_newer_correction() {
        for (field, original) in [
            ("description", Value::from("original")),
            ("details", Value::from("")),
        ] {
            let root = corrected_root(serde_json::json!({ field: original.clone() }));
            owner_set(root.path(), field, Value::from("A"), "t1");
            owner_set(root.path(), field, Value::from("B"), "t2");
            let record = remove_at(root.path(), 0).unwrap();
            assert_eq!(record[field], "B", "{field}");
            assert!(activity_field_held(&record, field), "{field}");
            let record = remove_at(root.path(), 1).unwrap();
            assert_eq!(record[field], original, "{field}");
            assert!(!activity_field_held(&record, field), "{field}");
        }
    }

    #[test]
    fn removing_a_correction_of_an_absent_field_deletes_it() {
        let root = corrected_root(serde_json::json!({}));
        owner_set(root.path(), "playback", Value::Bool(true), "t1");
        let record = remove_at(root.path(), 0).unwrap();
        assert!(!record.contains_key("playback"));
        assert!(!activity_field_held(&record, "playback"));
        assert_eq!(record["edits"][0][EDIT_REMOVED], "2026-10-16T13:00:00Z");
    }

    #[test]
    fn a_journal_write_after_a_removed_correction_is_what_a_later_removal_returns_to() {
        let root = corrected_root(serde_json::json!({"description":"original"}));
        owner_set(root.path(), "description", Value::from("A"), "t1");
        remove_at(root.path(), 0).unwrap();
        let line = Map::from_iter([("description".to_owned(), Value::from("journal line"))]);
        update_activity_record_checked(
            root.path(),
            "home",
            DAY,
            ROW,
            &line,
            "story",
            "",
            "t2",
            || Ok(()),
        )
        .unwrap();
        owner_set(root.path(), "description", Value::from("C"), "t3");
        let record = remove_at(root.path(), 2).unwrap();
        assert_eq!(record["description"], "journal line");
    }

    #[test]
    fn refused_removals_change_nothing() {
        let root = corrected_root(serde_json::json!({"description":"original"}));
        owner_set(root.path(), "description", Value::from("A"), "t1");
        set_activity_hidden(
            root.path(),
            "home",
            DAY,
            ROW,
            true,
            "owner:mute",
            None,
            "t2",
        )
        .unwrap();
        // An edit saved before edits recorded their values.
        update_activity_record_with(
            root.path(),
            "home",
            DAY,
            ROW,
            |_| Some(Map::new()),
            "story",
            "",
            "t3",
        )
        .unwrap();
        let path = day_path(root.path(), "home", DAY).unwrap();
        let mut row: Value =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        row["edits"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"timestamp":"t0","actor":"owner:update","fields":["title"],"note":""}));
        std::fs::write(&path, format!("{row}\n")).unwrap();
        remove_at(root.path(), 0).unwrap();
        let before = std::fs::read(&path).unwrap();
        let refused = |position, timestamp: &str, actor: &str| match remove_activity_edit(
            root.path(),
            "home",
            DAY,
            ROW,
            position,
            timestamp,
            actor,
            "t9",
        ) {
            Err(ActivityRecordStoreError::EditRefused(refusal)) => refusal,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            refused(0, "t1", "owner:update"),
            EditRefusal::AlreadyRemoved
        );
        assert_eq!(refused(1, "t2", "owner:mute"), EditRefusal::NotRemovable);
        assert_eq!(refused(3, "t0", "owner:update"), EditRefusal::NotRecorded);
        assert_eq!(refused(4, "t0", "owner:update"), EditRefusal::OutOfRange);
        assert_eq!(refused(0, "t0", "owner:update"), EditRefusal::Mismatch);
        assert_eq!(refused(0, "t1", "cli:update"), EditRefusal::Mismatch);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_derived_update_sees_the_record_as_stored_when_it_writes() {
        let root = tempfile::tempdir().expect("root");
        crate::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let mut record = ActivityRecord::new();
        record.insert("id".to_owned(), Value::String("a1".to_owned()));
        record.insert(
            "participation".to_owned(),
            serde_json::json!([{"name":"Ada"}]),
        );
        assert!(matches!(
            append_activity_record(root.path(), "work", "20260510", record).expect("write"),
            AppendOutcome::Written(_)
        ));
        // Another writer adds an entry after a caller's snapshot was taken.
        update_activity_record(
            root.path(),
            "work",
            "20260510",
            "a1",
            &Map::from_iter([(
                "participation".to_owned(),
                serde_json::json!([{"name":"Ada"},{"name":"Bea"}]),
            )]),
            "participation",
            "updated participation",
            "2026-05-10T00:00:00Z",
        )
        .expect("update");
        let path = day_path(root.path(), "work", "20260510").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(
            update_activity_record_with(
                root.path(),
                "work",
                "20260510",
                "a1",
                |_| None,
                "test",
                "nothing",
                "2026-05-10T00:00:01Z",
            )
            .expect("no-op")
            .is_none()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let updated = update_activity_record_with(
            root.path(),
            "work",
            "20260510",
            "a1",
            |current| {
                let mut people = current["participation"].as_array()?.clone();
                people.push(serde_json::json!({"name":"Cy"}));
                Some(Map::from_iter([(
                    "participation".to_owned(),
                    Value::Array(people),
                )]))
            },
            "test",
            "added",
            "2026-05-10T00:00:02Z",
        )
        .expect("derived update")
        .expect("record exists");
        let names = updated["participation"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Ada", "Bea", "Cy"]);
    }

    #[test]
    fn append_is_fill_only() {
        let root = tempfile::tempdir().expect("root");
        crate::create_facet(root.path(), "work", "Work", "", "", "", None).unwrap();
        let mut record = ActivityRecord::new();
        record.insert("id".to_owned(), Value::String("meeting_1".to_owned()));
        assert!(matches!(
            append_activity_record(root.path(), "work", "20260510", record.clone()).expect("write"),
            AppendOutcome::Written(_)
        ));
        assert!(matches!(
            append_activity_record(root.path(), "work", "20260510", record).expect("read"),
            AppendOutcome::AlreadyExists
        ));
    }

    #[test]
    fn rows_moved_by_a_facet_merge_are_adopted_and_others_stay_refused() {
        let root = tempfile::tempdir().expect("root");
        crate::create_facet(root.path(), "team", "Team", "", "", "", None).unwrap();
        crate::create_facet(root.path(), "personal", "Personal", "", "", "", None).unwrap();
        let team = crate::observe_facet_write_identity(root.path(), "team").unwrap();
        let personal = crate::observe_facet_write_identity(root.path(), "personal").unwrap();
        let merged = "77777777-7777-4777-8777-777777777777";
        let deleted = "66666666-6666-4666-8666-666666666666";
        let orphan = "55555555-5555-4555-8555-555555555555";
        std::fs::write(
            root.path().join("facets/retired.json"),
            serde_json::json!({"names": {
                "project": {"state": "merged", "id": merged, "successor": team},
                "gone": {"state": "deleted", "id": deleted},
                "orphaned": {"state": "merged", "id": null, "successor": team},
            }})
            .to_string(),
        )
        .unwrap();
        let rows = [
            ("moved", merged),
            ("from_deleted", deleted),
            ("from_personal", personal.as_str()),
            ("from_orphan", orphan),
        ];
        let path = root.path().join("facets/team/activities/20260510.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = rows
            .iter()
            .map(|(id, destination)| {
                serde_json::json!({"id": id, "title": id, "destination_id": destination})
                    .to_string()
                    + "\n"
            })
            .collect();
        std::fs::write(&path, &text).unwrap();
        // A second day, touched only by a hide, so each write path's
        // adoption is shown on its own.
        let hide_day = root.path().join("facets/team/activities/20260511.jsonl");
        std::fs::write(&hide_day, &text).unwrap();
        let mut patch = Map::new();
        patch.insert("title".to_owned(), Value::String("Edited".to_owned()));
        let edit = |record: &str| {
            update_activity_record(
                root.path(),
                "team",
                "20260510",
                record,
                &patch,
                "owner",
                "",
                "2026-05-10T10:00:00Z",
            )
        };

        for refused in ["from_deleted", "from_personal", "from_orphan"] {
            assert!(edit(refused).is_err(), "{refused}");
        }
        // A refused edit writes nothing, not even an adoption.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let edited = edit("moved").unwrap().expect("moved row");
        assert_eq!(edited["title"], "Edited");
        assert_eq!(edited["destination_id"], team.as_str());
        let hidden = set_activity_hidden(
            root.path(),
            "team",
            "20260511",
            "moved",
            true,
            "owner",
            None,
            "2026-05-10T10:00:00Z",
        )
        .unwrap()
        .expect("moved row");
        assert_eq!(hidden["hidden"], true);
        assert_eq!(hidden["destination_id"], team.as_str());
        let stored = |record: &str| {
            load_activity_records(root.path(), "team", "20260510", true)
                .unwrap()
                .into_iter()
                .find(|row| id(row) == record)
                .unwrap()["destination_id"]
                .clone()
        };
        assert_eq!(stored("from_deleted"), deleted);
        assert_eq!(stored("from_personal"), personal.as_str());
        assert_eq!(stored("from_orphan"), orphan);
        assert!(
            hold_activity_enrichment(root.path(), "team", stored("moved").as_str().unwrap())
                .is_ok()
        );
    }

    #[test]
    fn adopting_a_day_changes_only_moved_rows_and_a_missing_day_is_nothing() {
        let root = tempfile::tempdir().expect("root");
        crate::create_facet(root.path(), "team", "Team", "", "", "", None).unwrap();
        let team = crate::observe_facet_write_identity(root.path(), "team").unwrap();
        let merged = "77777777-7777-4777-8777-777777777777";
        std::fs::write(
            root.path().join("facets/retired.json"),
            serde_json::json!({"names": {
                "project": {"state": "merged", "id": merged, "successor": team},
            }})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            adopt_moved_activity_rows(root.path(), "team", "20260510").unwrap(),
            0
        );
        let path = root.path().join("facets/team/activities/20260510.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = format!(
            "{}\n{}\n",
            serde_json::json!({"id": "moved", "destination_id": merged, "note": "kept"}),
            serde_json::json!({"id": "home", "destination_id": team})
        );
        std::fs::write(&path, &text).unwrap();
        assert_eq!(
            adopt_moved_activity_rows(root.path(), "team", "20260510").unwrap(),
            1
        );
        let rows = load_activity_records(root.path(), "team", "20260510", true).unwrap();
        assert_eq!(rows[0]["destination_id"], team.as_str());
        assert_eq!(rows[0]["note"], "kept");
        assert_eq!(
            adopt_moved_activity_rows(root.path(), "team", "20260510").unwrap(),
            0
        );
    }

    #[test]
    fn append_refuses_undeclared_muted_or_malformed_destination() {
        let root = tempfile::tempdir().expect("root");
        let mut record = ActivityRecord::new();
        record.insert("id".to_owned(), Value::String("meeting_1".to_owned()));

        // Absent facet
        let err =
            append_activity_record(root.path(), "missing", "20260510", record.clone()).unwrap_err();
        assert!(matches!(
            err,
            ActivityRecordStoreError::Identity(
                super::super::declaration::FacetIdentityError::Absent
            )
        ));
        assert!(!root.path().join("facets/missing").exists());

        // Muted facets preserve owner edits while automated enrichment pauses.
        crate::create_facet(root.path(), "muted_facet", "Muted", "", "", "", None).unwrap();
        // A journal keeps one enabled facet; the sibling lets "muted_facet" mute.
        crate::create_facet(root.path(), "personal", "Personal", "", "", "", None).unwrap();
        crate::set_facet_muted(root.path(), "muted_facet", true).unwrap();
        let outcome =
            append_activity_record(root.path(), "muted_facet", "20260510", record.clone()).unwrap();
        assert!(matches!(outcome, AppendOutcome::Written(_)));
        let mut patch = Map::new();
        patch.insert("title".to_owned(), Value::String("Updated".to_owned()));
        let updated = update_activity_record(
            root.path(),
            "muted_facet",
            "20260510",
            "meeting_1",
            &patch,
            "test",
            "",
            "2026-05-10T10:00:00Z",
        )
        .unwrap();
        assert_eq!(updated.unwrap()["title"], "Updated");
        let identity = crate::observe_facet_write_identity(root.path(), "muted_facet").unwrap();
        assert!(matches!(
            hold_activity_enrichment(root.path(), "muted_facet", &identity),
            Err(ActivityRecordStoreError::DestinationMuted { .. })
        ));

        // Malformed facet declaration
        std::fs::create_dir_all(root.path().join("facets/bad")).unwrap();
        std::fs::write(root.path().join("facets/bad/facet.json"), b"not json").unwrap();
        let err =
            append_activity_record(root.path(), "bad", "20260510", record.clone()).unwrap_err();
        assert!(matches!(
            err,
            ActivityRecordStoreError::Identity(
                super::super::declaration::FacetIdentityError::Failed(_)
            )
        ));
        assert_eq!(
            std::fs::read(root.path().join("facets/bad/facet.json")).unwrap(),
            b"not json"
        );

        // Present declaration without ID mints ID at admission
        std::fs::create_dir_all(root.path().join("facets/noid")).unwrap();
        std::fs::write(
            root.path().join("facets/noid/facet.json"),
            b"{\"title\":\"No ID\"}",
        )
        .unwrap();
        let outcome =
            append_activity_record(root.path(), "noid", "20260510", record.clone()).unwrap();
        assert!(matches!(outcome, AppendOutcome::Written(_)));
        let decl = std::fs::read_to_string(root.path().join("facets/noid/facet.json")).unwrap();
        assert!(decl.contains("\"id\":"));

        // Replaced facet declaration fails update with Replaced
        crate::create_facet(root.path(), "repl", "Repl", "", "", "", None).unwrap();
        let outcome = append_activity_record(root.path(), "repl", "20260510", record).unwrap();
        assert!(matches!(outcome, AppendOutcome::Written(_)));
        std::fs::write(
            root.path().join("facets/repl/facet.json"),
            b"{\"id\":\"00000000-0000-4000-8000-000000000099\",\"title\":\"Replaced\"}",
        )
        .unwrap();
        let repl_err = update_activity_record(
            root.path(),
            "repl",
            "20260510",
            "meeting_1",
            &patch,
            "test",
            "",
            "2026-05-10T10:00:00Z",
        )
        .unwrap_err();
        assert!(matches!(
            repl_err,
            ActivityRecordStoreError::Identity(
                super::super::declaration::FacetIdentityError::Replaced
            )
        ));
    }

    #[test]
    fn python_value_coercion_preserves_truthiness_and_string_forms() {
        assert!(!activity_value_truthy(&Value::Null));
        assert!(!activity_value_truthy(&serde_json::json!(0)));
        assert!(!activity_value_truthy(&serde_json::json!([])));
        assert!(activity_value_truthy(&serde_json::json!([0])));
        assert_eq!(activity_value_or_empty(Some(&serde_json::json!(false))), "");
        assert_eq!(activity_value_string(&serde_json::json!(true)), "True");
        assert_eq!(
            activity_value_string(&serde_json::json!(["a", true])),
            "['a', True]"
        );

        let normalized = normalize(
            serde_json::json!({
                "id":"record_1", "activity":"meeting", "description":"Fallback",
                "title":false, "details":false, "hidden":"yes", "edits":{}
            })
            .as_object()
            .expect("record")
            .clone(),
        );
        assert_eq!(normalized["title"], "Fallback");
        assert_eq!(normalized["details"], "");
        assert_eq!(normalized["hidden"], true);
        assert_eq!(normalized["edits"], serde_json::json!([]));
    }

    #[test]
    fn list_activity_day_stems_filters_and_sorts_day_files() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let act_dir = root.path().join("facets/work/activities");
        std::fs::create_dir_all(&act_dir).unwrap();
        std::fs::write(act_dir.join("20261002.jsonl"), "").unwrap();
        std::fs::write(act_dir.join("20261001.jsonl"), "").unwrap();
        std::fs::write(act_dir.join("activities.jsonl"), "").unwrap();
        std::fs::write(act_dir.join("20261001.jsonl.lock"), "").unwrap();
        std::fs::create_dir_all(act_dir.join("20261003.jsonl")).unwrap(); // directory

        let stems = list_activity_day_stems(root.path(), "work").unwrap();
        assert_eq!(stems, vec!["20261001", "20261002"]);

        let missing = list_activity_day_stems(root.path(), "missing").unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn strict_reader_rejects_malformed_and_non_object_rows() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let act_dir = root.path().join("facets/work/activities");
        std::fs::create_dir_all(&act_dir).unwrap();

        // Missing file returns empty Ok
        let empty = read_activity_records_strict(root.path(), "work", "20261001").unwrap();
        assert!(empty.is_empty());

        // Valid rows return parsed and normalized
        std::fs::write(
            act_dir.join("20261001.jsonl"),
            "{\"id\":\"act1\",\"title\":\"Activity One\"}\n\n{\"id\":\"act2\",\"title\":\"Activity Two\"}\n",
        )
        .unwrap();
        let rows = read_activity_records_strict(root.path(), "work", "20261001").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "act1");
        assert_eq!(rows[1]["id"], "act2");

        // Malformed JSON syntax returns Json error
        std::fs::write(act_dir.join("20261002.jsonl"), "{\"id\":\"act1\"\n").unwrap();
        let err = read_activity_records_strict(root.path(), "work", "20261002").unwrap_err();
        assert!(matches!(err, ActivityRecordStoreError::Json(_)));

        // Non-object JSON (array) returns Json error
        std::fs::write(act_dir.join("20261003.jsonl"), "[1, 2, 3]\n").unwrap();
        let err2 = read_activity_records_strict(root.path(), "work", "20261003").unwrap_err();
        assert!(matches!(err2, ActivityRecordStoreError::Json(_)));
    }
}

#[cfg(test)]
mod kept_line_tests {
    use super::*;

    const DAY: &str = "20261016";
    const HALF: &str = r#"{"id":"run_0700","activity":"run"#;
    const NOT_A_RECORD: &str = r#"["not", "a", "record"]"#;

    fn journal_with(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        crate::create_facet(root.path(), "work", "Work", "Test", "#123456", "", None).unwrap();
        let path = root
            .path()
            .join(format!("facets/work/activities/{DAY}.jsonl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        (root, path)
    }

    fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn row(id: &str) -> String {
        serde_json::json!({"id": id, "activity": "meeting", "segments": ["090000_300"]}).to_string()
    }

    fn body(id: &str, hidden: bool) -> String {
        serde_json::json!({"id": id, "source": "body", "activity": "workout", "hidden": hidden,
            "title": "Hill repeats", "apart": ["body_0000000000000001"],
            "account": {"key": format!("import.apple_health\u{1f}{id}"), "start": "2026-10-16 07:00:00 -0600",
                "end": "2026-10-16 07:20:00 -0600", "recorder": "aaaaaaaaaaaa", "type": "running"}})
        .to_string()
    }

    #[test]
    fn a_refresh_changes_only_the_accounts_fields_keeps_a_bad_line_and_a_later_hide() {
        let first = body("body_0000000000000a", false);
        let (root, path) = journal_with(&[HALF, &first, NOT_A_RECORD]);
        // A hide lands after the importer read the day's keys and before it refreshes.
        set_activity_hidden(
            root.path(),
            "work",
            DAY,
            "body_0000000000000a",
            true,
            "owner:web",
            None,
            "t",
        )
        .unwrap();
        let hidden = lines(&path)[1].clone();
        let refresh = |end: &str, kind: Option<&str>| {
            refresh_body_account(
                root.path(),
                "work",
                DAY,
                "body_0000000000000a",
                "workout",
                "2026-10-16 07:00:00 -0600",
                end,
                "aaaaaaaaaaaa",
                kind,
            )
            .unwrap()
        };
        assert_eq!(
            refresh("2026-10-16 07:40:00 -0600", Some("running")),
            BodyAccountRefresh::Refreshed
        );
        let now = lines(&path);
        assert_eq!(now[0], HALF);
        assert_eq!(now[2], NOT_A_RECORD);
        let mut expected: Value = serde_json::from_str(&hidden).unwrap();
        expected["account"]["end"] = Value::from("2026-10-16 07:40:00 -0600");
        assert_eq!(serde_json::from_str::<Value>(&now[1]).unwrap(), expected);
        // The same fields again write nothing.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            refresh("2026-10-16 07:40:00 -0600", Some("running")),
            BodyAccountRefresh::Unchanged
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        // A type the shared vocabulary no longer maps goes.
        assert_eq!(
            refresh("2026-10-16 07:40:00 -0600", None),
            BodyAccountRefresh::Refreshed
        );
        let record: Value = serde_json::from_str(&lines(&path)[1]).unwrap();
        assert!(record["account"].get("type").is_none(), "{record}");
        // Only a body record is ever refreshed, even one carrying an account.
        let other = serde_json::json!({"id": "body_0000000000000a", "activity": "workout",
            "account": {"start": "2026-10-16 07:00:00 -0600", "end": "2026-10-16 07:20:00 -0600"}})
        .to_string();
        let (root, path) = journal_with(&[&other]);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            refresh_body_account(
                root.path(),
                "work",
                DAY,
                "body_0000000000000a",
                "workout",
                "a",
                "b",
                "c",
                None
            )
            .unwrap(),
            BodyAccountRefresh::NotFound
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn every_write_keeps_a_line_it_cannot_read_where_it_was() {
        let first = row("meet_0900");
        let second = row("meet_1000");
        let (root, path) = journal_with(&[&first, HALF, NOT_A_RECORD, &second]);
        let kept_in_place = |path: &Path| {
            let now = lines(path);
            let at = |text: &str| now.iter().position(|line| line == text);
            assert_eq!(at(HALF), Some(1), "{now:?}");
            assert_eq!(at(NOT_A_RECORD), Some(2), "{now:?}");
            now
        };

        // Hide, edit and append each rewrite the file; none loses a line.
        set_activity_hidden(
            root.path(),
            "work",
            DAY,
            "meet_1000",
            true,
            "owner:web",
            None,
            "t",
        )
        .unwrap()
        .expect("hidden");
        assert!(kept_in_place(&path)[3].contains(r#""hidden":true"#));
        let mut patch = Map::new();
        patch.insert("title".to_owned(), Value::String("Standup".to_owned()));
        update_activity_record(
            root.path(),
            "work",
            DAY,
            "meet_0900",
            &patch,
            "owner:web",
            "",
            "t",
        )
        .unwrap()
        .expect("edited");
        assert!(kept_in_place(&path)[0].contains("Standup"));
        let third = serde_json::from_str::<Value>(&row("meet_1100")).unwrap();
        let _ =
            append_activity_record(root.path(), "work", DAY, third.as_object().unwrap().clone())
                .unwrap();
        let now = kept_in_place(&path);
        assert_eq!(now.len(), 5, "{now:?}");
        assert!(now[4].contains("meet_1100"));
    }

    #[test]
    fn a_file_holding_only_a_bad_line_keeps_it() {
        let (root, path) = journal_with(&[HALF]);
        let before = std::fs::read(&path).unwrap();
        // Nothing to hide: the file isn't touched.
        assert!(
            set_activity_hidden(
                root.path(),
                "work",
                DAY,
                "run_0700",
                true,
                "owner:web",
                None,
                "t"
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // An append adds its row after the line, which stays.
        let added = serde_json::from_str::<Value>(&row("meet_0900")).unwrap();
        let _ =
            append_activity_record(root.path(), "work", DAY, added.as_object().unwrap().clone())
                .unwrap();
        let now = lines(&path);
        assert_eq!(now[0], HALF);
        assert!(now[1].contains("meet_0900"), "{now:?}");
    }
}

#[cfg(test)]
mod hidden_outputs_tests {
    use super::*;

    const DAY: &str = "20261016";

    fn journal() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        crate::create_facet(root.path(), "work", "Work", "Test", "#123456", "", None).unwrap();
        let path = root
            .path()
            .join(format!("facets/work/activities/{DAY}.jsonl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let rows = [
            r#"{"id":"standup","activity":"meeting","title":"Standup"}"#,
            r#"{"id":"review","activity":"meeting","title":"Review"}"#,
        ];
        std::fs::write(&path, rows.join("\n") + "\n").unwrap();
        root
    }

    /// The paths search has been asked to read again; the hints go with it,
    /// as a drain takes them.
    fn take_queued(root: &Path) -> Vec<String> {
        let todo = root.join("indexer/todo");
        let Ok(entries) = std::fs::read_dir(&todo) else {
            return Vec::new();
        };
        let mut keys: Vec<String> = entries
            .flatten()
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter_map(|entry| entry["key"].as_str().map(str::to_owned))
            .collect();
        keys.sort();
        std::fs::remove_dir_all(todo).unwrap();
        keys
    }

    fn outputs(id: &str) -> String {
        format!("facets/work/activities/{DAY}/{id}")
    }

    fn day_file() -> String {
        format!("facets/work/activities/{DAY}.jsonl")
    }

    fn write(root: &Path, change: impl FnOnce(&mut Vec<ActivityRecord>)) {
        let current_id = admit_activity_destination(root, "work").unwrap();
        locked_modify_day_records(root, "work", DAY, false, &current_id, |mut rows| {
            change(&mut rows);
            Ok((rows, ()))
        })
        .unwrap();
    }

    #[test]
    fn a_hide_the_journal_makes_queues_the_activity_outputs() {
        let root = journal();
        take_queued(root.path());
        // The journal's own hide of a cancelled plan, not the owner's mute.
        write(root.path(), |rows| {
            let row = rows.iter_mut().find(|row| id(row) == "review").unwrap();
            row.insert("hidden".to_owned(), Value::Bool(true));
            row.insert("cancelled".to_owned(), Value::Bool(true));
        });
        assert_eq!(take_queued(root.path()), [day_file(), outputs("review")]);

        // Twin: an edit that leaves hidden as it was queues only the day.
        write(root.path(), |rows| {
            let row = rows.iter_mut().find(|row| id(row) == "standup").unwrap();
            row.insert("title".to_owned(), Value::from("Daily standup"));
        });
        assert_eq!(take_queued(root.path()), [day_file()]);
    }

    #[test]
    fn an_owner_mute_and_unmute_queue_the_activity_outputs() {
        let root = journal();
        take_queued(root.path());
        let mute = |hidden| {
            set_activity_hidden(
                root.path(),
                "work",
                DAY,
                "standup",
                hidden,
                "owner:mute",
                None,
                "2026-10-16T12:00:00Z",
            )
            .unwrap()
            .unwrap()
        };
        assert_eq!(mute(true)["hidden"], true);
        assert_eq!(take_queued(root.path()), [day_file(), outputs("standup")]);
        // A repeat writes nothing, so the store has nothing to queue.
        mute(true);
        assert_eq!(take_queued(root.path()), Vec::<String>::new());
        assert_eq!(mute(false)["hidden"], false);
        assert_eq!(take_queued(root.path()), [day_file(), outputs("standup")]);
    }

    #[test]
    fn a_hidden_row_that_arrives_or_leaves_counts_as_a_change() {
        let before: Vec<ActivityRecord> = vec![
            serde_json::from_str(r#"{"id":"a","hidden":true}"#).unwrap(),
            serde_json::from_str(r#"{"id":"b"}"#).unwrap(),
        ];
        let after: Vec<ActivityRecord> = vec![
            serde_json::from_str(r#"{"id":"b"}"#).unwrap(),
            serde_json::from_str(r#"{"id":"c","hidden":true}"#).unwrap(),
            serde_json::from_str(r#"{"id":"d","hidden":false}"#).unwrap(),
        ];
        assert_eq!(hidden_changed(&before, &after), ["a", "c"]);
    }
}
