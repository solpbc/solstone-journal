// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Activity records for body experiences.
//!
//! Each workout, night of sleep or mindfulness session in a body window gets one
//! activity record, in the facet its window has, on its window's day. The record
//! refers to the account by its stream and row id and copies nothing else: what
//! it shows is composed from the window when read, for a connection allowed to
//! read that window. Code creates a record once and never edits it; the owner's
//! edits, hides and separates are the only later writes.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_facets::{AppendOutcome, BODY_RECORD_SOURCE, append_activity_record};
use solstone_core_format::content::body_records::{
    BODY_RECORDS_FILE, BodyRow, RowKind, decode_rows, mapped_workout_type, recorder_id,
};

/// The body stream whose experiences become records.
const STREAM: &str = "import.apple_health";
const TOMBSTONE: &str = "tombstone.json";
/// Separates the stream from the row id in an account key: neither holds it.
const KEY_SEPARATOR: char = '\u{1f}';
const KINDS: [&str; 3] = ["workout", "sleep", "mindfulness"];

/// What writing one day's records did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RecordCounts {
    pub written: u64,
    /// Experiences in a window with no facet, which get no record.
    pub no_facet: u64,
    /// Appends the store refused, with the facet and why. The next run tries again.
    pub refused: Vec<(String, String)>,
}

/// An account's key: its stream and the experience row's id.
#[must_use]
pub(crate) fn account_key(stream: &str, row_id: &str) -> String {
    format!("{stream}{KEY_SEPARATOR}{row_id}")
}

/// A record's id, fixed by its account key, so it is the same for life.
#[must_use]
pub(crate) fn record_id(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("body_{}", &hex[..16])
}

/// The facet a window was filed under, when it has one.
fn window_facet(window: &Path) -> Option<String> {
    let text = fs::read_to_string(window.join("talents/facets.json")).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    value
        .as_array()?
        .first()?
        .get("facet")?
        .as_str()
        .filter(|facet| !facet.is_empty())
        .map(str::to_owned)
}

/// Every account key a body record names on `day`, in any facet. A line that
/// can't be read names none, and a store write keeps it as it is.
fn recorded_keys(journal: &Path, day: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let Ok(facets) = fs::read_dir(journal.join("facets")) else {
        return keys;
    };
    for facet in facets.flatten() {
        let path = facet.path().join("activities").join(format!("{day}.jsonl"));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if solstone_core_facets::is_body_record(&record)
                && let Some(key) = record
                    .get("account")
                    .and_then(|account| account.get("key"))
                    .and_then(Value::as_str)
            {
                keys.insert(key.to_owned());
            }
        }
    }
    keys
}

/// The experiences a live window holds, with its facet.
fn window_experiences(window: &Path) -> Option<(Option<String>, Vec<BodyRow>)> {
    if window.join(TOMBSTONE).exists() {
        return None;
    }
    let text = fs::read_to_string(window.join(BODY_RECORDS_FILE)).ok()?;
    let rows = decode_rows(&text).ok()?;
    let experiences = rows
        .into_iter()
        .filter(|row| {
            row.kind == RowKind::Experience
                && row
                    .experience
                    .as_deref()
                    .is_some_and(|kind| KINDS.contains(&kind))
                && row.start.is_some()
                && row.end.is_some()
        })
        .collect::<Vec<_>>();
    Some((window_facet(window), experiences))
}

/// The record: a reference to the account, its instants, what groups read
/// without opening a window (the workout's shared type and an opaque recorder
/// id), and the owner's day it arrived on, by this code's clock. Nothing the
/// owner or the source wrote is copied.
fn record_for(key: &str, row: &BodyRow, arrived: &str) -> Map<String, Value> {
    let mut account = json!({
        "key": key,
        "start": row.start,
        "end": row.end,
        "recorder": recorder_id(STREAM, row.by.as_deref()),
        "arrived": arrived,
    });
    if row.experience.as_deref() == Some("workout")
        && let Some(kind) = mapped_workout_type(&row.source, &row.record_type)
    {
        account["type"] = Value::String(kind.to_owned());
    }
    let Value::Object(record) = json!({
        "id": record_id(key),
        "source": BODY_RECORD_SOURCE,
        "activity": row.experience,
        "account": account,
    }) else {
        unreachable!("a record is an object");
    };
    record
}

/// Create the record of every experience on `day` that has none, and nothing
/// else. It is safe to call for any day, any number of times: a day whose
/// experiences all have records appends nothing.
pub(crate) fn write_records(journal: &Path, day: &str) -> RecordCounts {
    let mut counts = RecordCounts::default();
    let stream_dir = journal.join("chronicle").join(day).join(STREAM);
    let Ok(entries) = fs::read_dir(&stream_dir) else {
        return counts;
    };
    let mut windows = entries
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    windows.sort();
    let mut recorded = recorded_keys(journal, day);
    let arrived = chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .format("%Y%m%d")
        .to_string();
    for window in windows {
        let Some((facet, experiences)) = window_experiences(&window) else {
            continue;
        };
        for row in experiences {
            let key = account_key(STREAM, &row.id);
            if recorded.contains(&key) {
                continue;
            }
            let Some(facet) = facet.as_deref() else {
                counts.no_facet += 1;
                continue;
            };
            match append_activity_record(journal, facet, day, record_for(&key, &row, &arrived)) {
                Ok(AppendOutcome::Written(_)) => {
                    counts.written += 1;
                    recorded.insert(key);
                }
                Ok(AppendOutcome::AlreadyExists) => {
                    recorded.insert(key);
                }
                Err(error) => counts.refused.push((facet.to_owned(), error.to_string())),
            }
        }
    }
    counts
}

#[cfg(test)]
#[path = "body_activity_tests.rs"]
mod tests;
