// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Moving one body record from one day file to another, so that a stop at any
//! point loses nothing and leaves one copy to read.
//!
//! The move is written down first, in an intent log, with the exact copy the
//! target gets. While an intent is open the source line is frozen: the store
//! refuses to change it, so finishing a move is always the same two steps, add
//! the intent's copy if the target lacks the key, then remove the source. The
//! target stays writable. Whatever finds an open intent next finishes it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_format::content::{is_body_record, owner_edit};
use solstone_core_journal_io::{FileLock, LockOptions, append_text, hold_lock};

use super::activity_records::{
    ActivityRecord, ActivityRecordStoreError, admit_activity_destination, id,
    locked_modify_day_records, normalize,
};

const MOVES_REL: &str = "imports/.body-moves.jsonl";
const MOVES_LOCK_REL: &str = "imports/.body-moves";

/// Where a body record is filed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BodyRecordPlace {
    pub day: String,
    pub facet: String,
}

/// A move that was started and hasn't finished.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenBodyMove {
    pub id: String,
    pub key: String,
    pub record_id: String,
    pub from: BodyRecordPlace,
    pub to: BodyRecordPlace,
    pub copy: ActivityRecord,
    /// Whether this move's own append reached the target.
    pub appended: bool,
}

/// What asking to move a record did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyMoveOutcome {
    Moved,
    /// It's already filed there.
    AlreadyThere,
    /// No body record with that id is in the source file.
    NotFound,
}

/// A body record's account key.
#[must_use]
pub fn body_record_key(record: &ActivityRecord) -> Option<&str> {
    record
        .get("account")
        .and_then(|account| account.get("key"))
        .and_then(Value::as_str)
}

fn moves_path(root: &Path) -> PathBuf {
    root.join(MOVES_REL)
}

fn moving_error(detail: impl std::fmt::Display) -> ActivityRecordStoreError {
    ActivityRecordStoreError::BodyRecordMoving {
        detail: detail.to_string(),
    }
}

fn place_of(value: Option<&Value>) -> Option<BodyRecordPlace> {
    let value = value?;
    Some(BodyRecordPlace {
        day: value.get("day")?.as_str()?.to_owned(),
        facet: value.get("facet")?.as_str()?.to_owned(),
    })
}

fn place_json(place: &BodyRecordPlace) -> Value {
    json!({"day": place.day, "facet": place.facet})
}

/// Every move not yet finished, in the order they were started. A torn or bad
/// line never reached its append, so it names no move.
///
/// # Errors
///
/// The log exists and can't be read.
pub fn open_body_moves(root: &Path) -> io::Result<Vec<OpenBodyMove>> {
    let bytes = match std::fs::read(moves_path(root)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut order = Vec::new();
    let mut moves = BTreeMap::new();
    for line in bytes.split(|&byte| byte == b'\n') {
        let Ok(Value::Object(entry)) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if let Some(done) = entry.get("done").and_then(Value::as_str) {
            moves.remove(done);
        } else if let Some(appended) = entry.get("appended").and_then(Value::as_str) {
            if let Some(open) = moves.get_mut(appended) {
                let open: &mut OpenBodyMove = open;
                open.appended = true;
            }
        } else if let Some(parsed) = parse_move(&entry) {
            order.push(parsed.id.clone());
            moves.insert(parsed.id.clone(), parsed);
        }
    }
    Ok(order.iter().filter_map(|id| moves.remove(id)).collect())
}

fn parse_move(entry: &Map<String, Value>) -> Option<OpenBodyMove> {
    Some(OpenBodyMove {
        id: entry.get("move")?.as_str()?.to_owned(),
        key: entry.get("key")?.as_str()?.to_owned(),
        record_id: entry.get("record")?.as_str()?.to_owned(),
        from: place_of(entry.get("from"))?,
        to: place_of(entry.get("to"))?,
        copy: entry.get("copy")?.as_object()?.clone(),
        appended: false,
    })
}

/// Append one line to the log, ending a torn tail first so it can't swallow
/// this one.
fn append_line(root: &Path, line: &Value) -> Result<(), ActivityRecordStoreError> {
    let path = moves_path(root);
    let torn = std::fs::read(&path).is_ok_and(|bytes| bytes.last().is_some_and(|&b| b != b'\n'));
    let text = if torn {
        format!("\n{line}")
    } else {
        line.to_string()
    };
    append_text(&path, &text).map_err(moving_error)
}

fn hold_moves(root: &Path) -> Result<FileLock, ActivityRecordStoreError> {
    Ok(hold_lock(
        root.join(MOVES_LOCK_REL),
        LockOptions::default(),
    )?)
}

fn move_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{nanos:x}-{:x}", std::process::id())
}

/// Whether the record `record_id` in `rows` at this place has an open move out
/// of it. Only a body record reads the log, so no other write pays for it.
pub(super) fn refuse_while_moving(
    root: &Path,
    place: &BodyRecordPlace,
    rows: &[ActivityRecord],
    record_id: &str,
) -> Result<(), ActivityRecordStoreError> {
    let Some(key) = rows
        .iter()
        .find(|row| id(row) == record_id && is_body_record(row))
        .and_then(body_record_key)
    else {
        return Ok(());
    };
    let moves = open_body_moves(root).map_err(moving_error)?;
    if moves
        .iter()
        .any(|open| open.key == key && open.from == *place)
    {
        return Err(moving_error("this record has an open move"));
    }
    Ok(())
}

/// Refuses creating a record whose key has an open move anywhere.
pub(super) fn refuse_create_while_moving(
    root: &Path,
    record: &ActivityRecord,
) -> Result<(), ActivityRecordStoreError> {
    if !is_body_record(record) {
        return Ok(());
    }
    let Some(key) = body_record_key(record) else {
        return Ok(());
    };
    let moves = open_body_moves(root).map_err(moving_error)?;
    if moves.iter().any(|open| open.key == key) {
        return Err(moving_error("this record has an open move"));
    }
    Ok(())
}

/// Move one body record, finishing the move before it returns. If it stops
/// part way, the next [`complete_body_move`] finishes it.
///
/// # Errors
///
/// The store's errors for either file, or the log can't be written.
pub fn move_body_record(
    root: &Path,
    record_id: &str,
    from: &BodyRecordPlace,
    to: &BodyRecordPlace,
) -> Result<BodyMoveOutcome, ActivityRecordStoreError> {
    if from == to {
        return Ok(BodyMoveOutcome::AlreadyThere);
    }
    let _moves = hold_moves(root)?;
    let from_id = admit_activity_destination(root, &from.facet)?;
    let to_id = admit_activity_destination(root, &to.facet)?;
    let started =
        locked_modify_day_records(root, &from.facet, &from.day, false, &from_id, |rows| {
            let Some(row) = rows
                .iter()
                .find(|row| id(row) == record_id && is_body_record(row))
            else {
                return Ok((rows, None));
            };
            let Some(key) = body_record_key(row).map(str::to_owned) else {
                return Ok((rows, None));
            };
            let mut copy = normalize(row.clone());
            copy.insert("destination_id".to_owned(), Value::String(to_id.clone()));
            let open = OpenBodyMove {
                id: move_id(),
                key,
                record_id: record_id.to_owned(),
                from: from.clone(),
                to: to.clone(),
                copy,
                appended: false,
            };
            append_line(
                root,
                &json!({
                    "move": open.id,
                    "key": open.key,
                    "record": open.record_id,
                    "from": place_json(from),
                    "to": place_json(to),
                    "copy": open.copy,
                }),
            )?;
            Ok((rows, Some(open)))
        });
    let open = match started {
        Ok(Some(open)) => open,
        Ok(None) | Err(ActivityRecordStoreError::MissingDayFile { .. }) => {
            return Ok(BodyMoveOutcome::NotFound);
        }
        Err(error) => return Err(error),
    };
    complete_held(root, &open)?;
    Ok(BodyMoveOutcome::Moved)
}

/// Finish a move that was started and didn't finish.
///
/// # Errors
///
/// The store's errors for either file; the intent stays open.
pub fn complete_body_move(
    root: &Path,
    open: &OpenBodyMove,
) -> Result<(), ActivityRecordStoreError> {
    let _moves = hold_moves(root)?;
    // Someone else may have finished it while this waited for the lock.
    let still_open = open_body_moves(root)
        .map_err(moving_error)?
        .into_iter()
        .find(|candidate| candidate.id == open.id);
    match still_open {
        Some(current) => complete_held(root, &current),
        None => Ok(()),
    }
}

/// Adds to the target copy the owner edits only the source holds, known by
/// their time, actor and what they carried.
fn with_source_edits(mut target: ActivityRecord, source: &ActivityRecord) -> ActivityRecord {
    let identity = |edit: &Map<String, Value>| {
        (
            edit.get("timestamp").cloned(),
            edit.get("actor").cloned(),
            edit.get("carried").cloned(),
        )
    };
    let held = target
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(identity)
        .collect::<Vec<_>>();
    let missing = source
        .get("edits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|edit| owner_edit(edit) && !held.contains(&identity(edit)))
        .cloned()
        .map(Value::Object)
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        let edits = target
            .entry("edits".to_owned())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(edits) = edits {
            edits.extend(missing);
        }
    }
    target
}

fn complete_held(root: &Path, open: &OpenBodyMove) -> Result<(), ActivityRecordStoreError> {
    let to_id = admit_activity_destination(root, &open.to.facet)?;
    let from_id = admit_activity_destination(root, &open.from.facet)?;
    let appended = locked_modify_day_records(
        root,
        &open.to.facet,
        &open.to.day,
        true,
        &to_id,
        |mut rows| {
            let found = rows
                .iter()
                .position(|row| is_body_record(row) && body_record_key(row) == Some(&open.key));
            match found {
                // A copy this move didn't write arrived from elsewhere, a merge
                // say: it keeps what it has and gains what only the source held.
                Some(position) if !open.appended => {
                    let target = rows.remove(position);
                    rows.insert(position, with_source_edits(target, &open.copy));
                    Ok((rows, false))
                }
                Some(_) => Ok((rows, false)),
                None => {
                    rows.push(open.copy.clone());
                    Ok((rows, true))
                }
            }
        },
    )?;
    if appended {
        append_line(root, &json!({"appended": open.id}))?;
    }
    let removed = locked_modify_day_records(
        root,
        &open.from.facet,
        &open.from.day,
        false,
        &from_id,
        |mut rows| {
            rows.retain(|row| !(is_body_record(row) && body_record_key(row) == Some(&open.key)));
            Ok((rows, ()))
        },
    );
    match removed {
        Ok(()) | Err(ActivityRecordStoreError::MissingDayFile { .. }) => {}
        Err(error) => return Err(error),
    }
    append_line(root, &json!({"done": open.id}))?;
    // Nothing open: the log is finished with, and the next store write reads nothing.
    if open_body_moves(root).map_err(moving_error)?.is_empty() {
        match std::fs::remove_file(moves_path(root)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(moving_error(error)),
        }
    }
    Ok(())
}

/// Create or rewrite one body record under its day file's lock, from the record
/// as it stands then. Refused while the record has an open move: nothing writes
/// the key anywhere until the move finishes.
///
/// # Errors
///
/// The store's errors for the file, or an open move.
pub fn write_body_record_with(
    root: &Path,
    place: &BodyRecordPlace,
    record_id: &str,
    make: impl FnOnce(Option<ActivityRecord>) -> ActivityRecord,
) -> Result<ActivityRecord, ActivityRecordStoreError> {
    let facet_id = admit_activity_destination(root, &place.facet)?;
    locked_modify_day_records(
        root,
        &place.facet,
        &place.day,
        true,
        &facet_id,
        |mut rows| {
            let found = rows
                .iter()
                .position(|row| id(row) == record_id && is_body_record(row));
            let mut written = match found {
                Some(position) => {
                    refuse_while_moving(root, place, &rows, record_id)?;
                    make(Some(rows.remove(position)))
                }
                None => {
                    let made = make(None);
                    refuse_create_while_moving(root, &made)?;
                    made
                }
            };
            written.insert("destination_id".to_owned(), Value::String(facet_id.clone()));
            let at = found.unwrap_or(rows.len());
            rows.insert(at, written.clone());
            Ok((rows, written))
        },
    )
}

/// What removing a line that must still match did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineRemoval {
    Removed,
    /// It's there and no longer matches, so it stays.
    Changed,
    /// It's already gone.
    Absent,
}

/// Remove the record `record_id` at this place, if `matches` still says yes for
/// it under the day file's lock.
///
/// # Errors
///
/// The store's errors for the file.
pub fn remove_activity_line_if(
    root: &Path,
    place: &BodyRecordPlace,
    record_id: &str,
    matches: impl FnOnce(&ActivityRecord) -> bool,
) -> Result<LineRemoval, ActivityRecordStoreError> {
    let facet_id = admit_activity_destination(root, &place.facet)?;
    let removed = locked_modify_day_records(
        root,
        &place.facet,
        &place.day,
        false,
        &facet_id,
        |mut rows| {
            let Some(position) = rows.iter().position(|row| id(row) == record_id) else {
                return Ok((rows, LineRemoval::Absent));
            };
            if !matches(&rows[position]) {
                return Ok((rows, LineRemoval::Changed));
            }
            rows.remove(position);
            Ok((rows, LineRemoval::Removed))
        },
    );
    match removed {
        Err(ActivityRecordStoreError::MissingDayFile { .. }) => Ok(LineRemoval::Absent),
        other => other,
    }
}

#[cfg(test)]
#[path = "body_moves_tests.rs"]
mod tests;
