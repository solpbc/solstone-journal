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
            Self::PublicationRefused { detail } => write!(formatter, "{detail}"),
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

fn read_rows_strict(path: &Path) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = read_text(path, String::new())?;
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

fn write_rows(path: &Path, rows: &[ActivityRecord]) -> Result<(), ActivityRecordStoreError> {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row)?;
        bytes.push(b'\n');
    }
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })?;
    Ok(())
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

fn locked_modify_day_records<T>(
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
    if let Some(path) = written {
        super::publication::publish_saved(root, &path);
    }
    Ok(result)
}

/// The locked half of [`locked_modify_day_records`]: the day file's path when
/// it was written.
fn write_day_records<T>(
    root: &Path,
    facet: &str,
    day: &str,
    create_if_missing: bool,
    expected_facet_id: &str,
    modify: impl FnOnce(
        Vec<ActivityRecord>,
    ) -> Result<(Vec<ActivityRecord>, T), ActivityRecordStoreError>,
) -> Result<(T, Option<PathBuf>), ActivityRecordStoreError> {
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
    let current = read_rows(&path)?;
    let (updated, result) = modify(current.clone())?;
    if !existed && updated.is_empty() {
        return Ok((result, None));
    }
    if existed && updated == current {
        return Ok((result, None));
    }
    write_rows(&path, &updated)?;
    Ok((result, Some(path)))
}

/// Plan records the retired `schedule` talent inferred. They stay on disk as
/// written; no reader returns them.
pub fn is_retired_plan(record: &ActivityRecord) -> bool {
    record.get("source").and_then(Value::as_str) == Some("anticipated")
}

pub fn load_activity_records(
    root: &Path,
    facet: &str,
    day: &str,
    include_hidden: bool,
) -> Result<Vec<ActivityRecord>, ActivityRecordStoreError> {
    let rows = read_rows(&day_path(root, facet, day)?)?;
    Ok(rows
        .into_iter()
        .map(normalize)
        .filter(|row| !is_retired_plan(row) && (include_hidden || !hidden(row)))
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
) -> Result<Option<ActivityRecord>, ActivityRecordStoreError> {
    let current_id = admit_activity_destination(root, facet)?;
    let mut derive = Some(derive);
    locked_modify_day_records(root, facet, day, false, &current_id, |rows| {
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
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
                if id(&row) != record_id {
                    return row;
                }
                let Some(patch) = derive.take().and_then(|derive| derive(&row)) else {
                    return row;
                };
                let mut merged = row;
                for (key, value) in &patch {
                    merged.insert(key.clone(), value.clone());
                }
                merged.insert(
                    "destination_id".to_owned(),
                    Value::String(current_id.clone()),
                );
                let edited = append_edit(
                    normalize(merged),
                    actor,
                    patch.keys().cloned().collect(),
                    note,
                    timestamp,
                );
                result = Some(edited.clone());
                edited
            })
            .collect();
        checkpoint().map_err(|detail| ActivityRecordStoreError::PublicationRefused { detail })?;
        Ok((updated, result))
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
                if id(&row) != record_id {
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

fn normalize(mut record: ActivityRecord) -> ActivityRecord {
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

pub fn activity_field_held(record: &ActivityRecord, field: &str) -> bool {
    let Some(edits) = record.get("edits").and_then(Value::as_array) else {
        return false;
    };
    for edit in edits.iter().rev() {
        let Some(obj) = edit.as_object() else {
            continue;
        };
        let Some(fields) = obj.get("fields").and_then(Value::as_array) else {
            continue;
        };
        let lists_field = fields.iter().any(|f| f.as_str() == Some(field));
        if lists_field {
            let actor = obj.get("actor").and_then(Value::as_str).unwrap_or_default();
            return actor.starts_with("owner:") || actor.starts_with("cli:");
        }
    }
    false
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
fn id(record: &ActivityRecord) -> &str {
    record.get("id").and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
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
