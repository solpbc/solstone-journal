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
    MissingDayFile { path: PathBuf },
    Path(PathError),
    Lock(LockError),
    Read(ReadError),
    Write(solstone_core_journal_io::AtomicWriteError),
    Json(serde_json::Error),
    Facet(super::error::FacetStoreError),
}

impl std::fmt::Display for ActivityRecordStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDayFile { path } => write!(
                formatter,
                "activity day file is missing: {}",
                path.display()
            ),
            Self::Path(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::Facet(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ActivityRecordStoreError {}

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

#[must_use]
pub enum AppendOutcome {
    Written(ActivityRecord),
    AlreadyExists,
}

fn day_path(root: &Path, facet: &str, day: &str) -> Result<PathBuf, ActivityRecordStoreError> {
    contained_path(root, &format!("facets/{facet}/activities/{day}.jsonl")).map_err(Into::into)
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

fn write_rows(path: &Path, rows: &[ActivityRecord]) -> Result<(), ActivityRecordStoreError> {
    let mut bytes = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut bytes, row)?;
        bytes.push(b'\n');
    }
    atomic_replace(path, &bytes, AtomicWriteOptions { mode: Some(0o600) })?;
    Ok(())
}

fn locked_modify_day_records<T>(
    root: &Path,
    facet: &str,
    day: &str,
    create_if_missing: bool,
    modify: impl FnOnce(
        Vec<ActivityRecord>,
    ) -> Result<(Vec<ActivityRecord>, T), ActivityRecordStoreError>,
) -> Result<T, ActivityRecordStoreError> {
    let _entity =
        solstone_core_entity::hold_entity_trust_lock(root).map_err(|error| match error {
            solstone_core_entity::EntityTrustLockError::Path(error) => {
                ActivityRecordStoreError::Path(error)
            }
            solstone_core_entity::EntityTrustLockError::Lock(error) => {
                ActivityRecordStoreError::Lock(error)
            }
        })?;
    let path = day_path(root, facet, day)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let existed = path.exists();
    if !existed && !create_if_missing {
        return Err(ActivityRecordStoreError::MissingDayFile { path });
    }
    let current = read_rows(&path)?;
    let (updated, result) = modify(current.clone())?;
    if !existed && updated.is_empty() {
        return Ok(result);
    }
    if existed && updated == current {
        return Ok(result);
    }
    write_rows(&path, &updated)?;
    Ok(result)
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
        .filter(|row| include_hidden || !hidden(row))
        .collect())
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
    locked_modify_day_records(root, facet, day, true, |rows| {
        let record_id = id(&record).to_owned();
        if !record_id.is_empty() && rows.iter().any(|row| id(row) == record_id) {
            return Ok((rows, AppendOutcome::AlreadyExists));
        }
        let written = normalize(record);
        let mut updated = rows;
        updated.push(written.clone());
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
    locked_modify_day_records(root, facet, day, false, |rows| {
        let mut result = None;
        let updated = rows
            .into_iter()
            .map(|row| {
                if id(&row) != record_id {
                    return row;
                }
                let mut merged = row;
                for (key, value) in patch {
                    merged.insert(key.clone(), value.clone());
                }
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
    locked_modify_day_records(root, facet, day, false, |rows| {
        let mut result = None;
        let updated = rows
            .into_iter()
            .map(|row| {
                if id(&row) != record_id {
                    return row;
                }
                let mut normalized = normalize(row);
                if hidden(&normalized) != hidden_value {
                    normalized.insert("hidden".to_owned(), Value::Bool(hidden_value));
                    normalized = append_edit(
                        normalized,
                        actor,
                        vec!["hidden".to_owned()],
                        reason.unwrap_or(if hidden_value { "muted" } else { "unmuted" }),
                        timestamp,
                    );
                }
                result = Some(normalized.clone());
                normalized
            })
            .collect();
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

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn append_is_fill_only() {
        let root = tempfile::tempdir().expect("root");
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
}
