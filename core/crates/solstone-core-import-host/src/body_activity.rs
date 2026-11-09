// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Activity records for body experiences.
//!
//! Each workout, night of sleep or mindfulness session in a body window gets one
//! activity record, in the facet its window has, on its window's day. The record
//! refers to the account by its stream and row id and copies nothing else: what
//! it shows is composed from the window when read, for a connection allowed to
//! read that window. Code creates a record once and later changes only the
//! fields it wrote, when its account changes; nothing else but the owner's
//! edits, hides and separates writes it.
//!
//! Strava's tiles are read as body rows through the Strava adapter, one record
//! per workout. A workout whose tiles an earlier reading of the segment already
//! filed an activity record for keeps that record as its only one.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_facets::{
    AppendOutcome, BODY_RECORD_SOURCE, BodyAccountRefresh, append_activity_record,
    refresh_body_account,
};
use solstone_core_format::content::body_records::{
    BODY_RECORDS_FILE, BodyRow, RowKind, decode_rows, mapped_workout_type, parse_source_time,
    recorder_id,
};
use solstone_core_format::content::strava_body::{STRAVA_SOURCE, STRAVA_TILE_FILE, strava_rows};
use solstone_core_import_sources::body_stream::{BodySource, RecordSource, STRAVA, WindowFile};

pub(crate) const TOMBSTONE: &str = "tombstone.json";
/// Separates the stream from the row id in an account key: neither holds it.
const KEY_SEPARATOR: char = '\u{1f}';
const KINDS: [&str; 3] = ["workout", "sleep", "mindfulness"];

/// What writing one day's records did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RecordCounts {
    pub written: u64,
    /// Records whose account's times, type or recorder changed, set to the new ones.
    pub refreshed: u64,
    /// Experiences in a window with no facet, which get no record.
    pub no_facet: u64,
    /// Strava workouts an earlier reading of their tiles already filed a
    /// record for, which get no other.
    pub sensed: u64,
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
pub(crate) fn window_facet(window: &Path) -> Option<String> {
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

/// What a stored record holds that code wrote: the facet whose file holds it,
/// its `activity`, and its account's `start`, `end`, `recorder` and `type`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recorded {
    facet: String,
    fields: [Option<String>; 5],
}

/// Every activity record on `day`, in every facet folder, muted ones included,
/// with its facet, read line by line as bytes: a line that can't be read is
/// skipped, and the rest are still read. A day file that exists but can't be
/// read at all is an error, naming its facet.
fn each_day_record(
    journal: &Path,
    day: &str,
    mut each: impl FnMut(&str, &Map<String, Value>),
) -> Result<(), (String, String)> {
    let Ok(facets) = fs::read_dir(journal.join("facets")) else {
        return Ok(());
    };
    for facet in facets.flatten() {
        // A facet is a folder; a plain file beside them (a facet tool's list of
        // candidates, say) holds no records.
        if !facet.path().is_dir() {
            continue;
        }
        let name = facet.file_name().to_string_lossy().into_owned();
        let path = facet.path().join("activities").join(format!("{day}.jsonl"));
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err((name, error.to_string())),
        };
        for line in bytes.split(|&byte| byte == b'\n') {
            if let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(line) {
                each(&name, &record);
            }
        }
    }
    Ok(())
}

/// Whether a body record names `key` on `day`, in any facet folder.
pub(crate) fn has_record(journal: &Path, day: &str, key: &str) -> Result<bool, (String, String)> {
    recorded_keys(journal, day).map(|keys| keys.contains_key(key))
}

/// Every account key a body record names on `day`, in any facet, with what
/// code wrote on it, read line by line as bytes: a line that can't be read
/// names none, and the rest still name theirs. A day file that exists but
/// can't be read at all is an error, so no record is written twice beside it.
/// A store write keeps a bad line as it is.
fn recorded_keys(
    journal: &Path,
    day: &str,
) -> Result<BTreeMap<String, Recorded>, (String, String)> {
    let mut keys = BTreeMap::new();
    each_day_record(journal, day, |facet, record| {
        if solstone_core_facets::is_body_record(record)
            && let Some(key) = record
                .get("account")
                .and_then(|account| account.get("key"))
                .and_then(Value::as_str)
        {
            let text = |path: &[&str]| {
                path.iter()
                    .try_fold(&Value::Object(record.clone()), |value, step| {
                        value.get(step)
                    })
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            keys.entry(key.to_owned()).or_insert_with(|| Recorded {
                facet: facet.to_owned(),
                fields: [
                    text(&["activity"]),
                    text(&["account", "start"]),
                    text(&["account", "end"]),
                    text(&["account", "recorder"]),
                    text(&["account", "type"]),
                ],
            });
        }
    })?;
    Ok(keys)
}

/// Every segment key an activity record of Strava's stream names on `day`, in
/// any facet folder, muted ones included: what an earlier reading of Strava's
/// tiles filed. A record from any other stream names none here.
fn sensed_keys(journal: &Path, day: &str) -> Result<BTreeSet<String>, (String, String)> {
    let mut keys = BTreeSet::new();
    each_day_record(journal, day, |_, record| {
        if record.get("stream").and_then(Value::as_str) == Some(STRAVA.stream) {
            keys.extend(
                record
                    .get("segments")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned),
            );
        }
    })?;
    Ok(keys)
}

/// The rows a live window of `file` holds, or `None` when it's deleted or
/// can't be read.
pub(crate) fn window_rows(window: &Path, file: WindowFile) -> Option<Vec<BodyRow>> {
    if window.join(TOMBSTONE).exists() {
        return None;
    }
    match file {
        WindowFile::BodyRecords => {
            decode_rows(&fs::read_to_string(window.join(BODY_RECORDS_FILE)).ok()?).ok()
        }
        WindowFile::StravaTile => {
            strava_rows(&fs::read_to_string(window.join(STRAVA_TILE_FILE)).ok()?)
        }
    }
}

/// The experiences a live window holds, with its facet.
fn window_experiences(window: &Path, file: WindowFile) -> Option<(Option<String>, Vec<BodyRow>)> {
    let experiences = window_rows(window, file)?
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

/// The window folders of `stream` on `day`, in name order.
pub(crate) fn day_windows(
    journal: &Path,
    day: &str,
    stream: &str,
) -> Option<Vec<std::path::PathBuf>> {
    let entries = fs::read_dir(journal.join("chronicle").join(day).join(stream)).ok()?;
    let mut windows = entries
        .flatten()
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    windows.sort();
    Some(windows)
}

/// A Strava tile's length, as Strava's import cuts them.
const STRAVA_TILE_SECONDS: i64 = 300;

/// What deciding whether Strava workouts were already sensed reads, once per day.
#[derive(Default)]
pub(crate) struct Sensed {
    /// Per day, the keys of each workout's live tiles.
    tiles: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    /// Per day, the segment keys Strava's activity records name.
    named: BTreeMap<String, Result<BTreeSet<String>, (String, String)>>,
}

impl Sensed {
    /// Whether an activity record of Strava's stream names any tile of the
    /// workout `row`, which starts on `day` in `window`, on any day it covers.
    /// A tile is known by what its window holds and by where Strava's import
    /// cuts it, so a tile the owner deleted still counts. A day file that
    /// can't be read decides nothing, so it is an error.
    pub(crate) fn check(
        &mut self,
        journal: &Path,
        day: &str,
        window: &str,
        row: &BodyRow,
    ) -> Result<bool, (String, String)> {
        let mut keys: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        keys.entry(day.to_owned())
            .or_default()
            .insert(window.to_owned());
        let span = row
            .start
            .as_deref()
            .and_then(parse_source_time)
            .zip(row.end.as_deref().and_then(parse_source_time));
        if let Some((start, end)) = span {
            let zone = solstone_core_journal_config::placed_zone(journal, STRAVA_SOURCE);
            for (on, key) in strava_cut(start, end, zone) {
                keys.entry(on).or_default().insert(key);
            }
            for on in covered_days(day, end, zone) {
                let tiles = self
                    .tiles
                    .entry(on.clone())
                    .or_insert_with(|| strava_tiles(journal, &on));
                if let Some(found) = tiles.get(&row.id) {
                    keys.entry(on).or_default().extend(found.iter().cloned());
                }
            }
        }
        for (on, tiles) in keys {
            let named = self
                .named
                .entry(on.clone())
                .or_insert_with(|| sensed_keys(journal, &on));
            match named {
                Ok(named) if tiles.iter().any(|key| named.contains(key)) => return Ok(true),
                Ok(_) => {}
                Err(refused) => return Err(refused.clone()),
            }
        }
        Ok(false)
    }
}

/// Every live Strava tile on `day`, by its workout's activity id.
fn strava_tiles(journal: &Path, day: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut tiles: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for window in day_windows(journal, day, STRAVA.stream).unwrap_or_default() {
        let Some(name) = window
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        for row in window_rows(&window, WindowFile::StravaTile).unwrap_or_default() {
            let workout = row.of.unwrap_or(row.id);
            tiles.entry(workout).or_default().insert(name.clone());
        }
    }
    tiles
}

/// The day and key of each tile Strava's import cuts a workout into: one every
/// five minutes from its start, keyed by its clock and length on the zone its
/// first import fixed, or on the start's own offset with none.
pub(crate) fn strava_cut(
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    zone: Option<chrono_tz::Tz>,
) -> Vec<(String, String)> {
    let elapsed = (end - start).num_seconds().max(1);
    (0..(elapsed + STRAVA_TILE_SECONDS - 1) / STRAVA_TILE_SECONDS)
        .map(|index| {
            let at = start + Duration::seconds(index * STRAVA_TILE_SECONDS);
            let seconds = STRAVA_TILE_SECONDS.min(elapsed - index * STRAVA_TILE_SECONDS);
            let clock = |format: &str| match zone {
                Some(zone) => at.with_timezone(&zone).format(format).to_string(),
                None => at.format(format).to_string(),
            };
            (clock("%Y%m%d"), format!("{}_{seconds}", clock("%H%M%S")))
        })
        .collect()
}

/// `day` and every later day a span ending at `end` reaches.
fn covered_days(day: &str, end: DateTime<FixedOffset>, zone: Option<chrono_tz::Tz>) -> Vec<String> {
    let Ok(first) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return vec![day.to_owned()];
    };
    // The last second it covers.
    let last = end - Duration::seconds(1);
    let last = match zone {
        Some(zone) => last.with_timezone(&zone).date_naive(),
        None => last.date_naive(),
    };
    first
        .iter_days()
        .take_while(|on| *on <= last.max(first))
        .take(15)
        .map(|on| on.format("%Y%m%d").to_string())
        .collect()
}

/// What code writes on a record for `row`: its `activity`, and its account's
/// `start`, `end`, `recorder` and workout `type`, in [`Recorded`]'s order.
fn written_fields(stream: &str, row: &BodyRow) -> [Option<String>; 5] {
    let kind = (row.experience.as_deref() == Some("workout"))
        .then(|| mapped_workout_type(&row.source, &row.record_type))
        .flatten();
    [
        row.experience.clone(),
        row.start.clone(),
        row.end.clone(),
        Some(recorder_id(stream, row.by.as_deref())),
        kind.map(str::to_owned),
    ]
}

/// The record: a reference to the account, its instants, what groups read
/// without opening a window (the workout's shared type and an opaque recorder
/// id), and the owner's day it arrived on, by this code's clock. Nothing the
/// owner or the source wrote is copied.
pub(crate) fn record_for(
    stream: &str,
    key: &str,
    row: &BodyRow,
    arrived: &str,
) -> Map<String, Value> {
    let mut account = json!({
        "key": key,
        "start": row.start,
        "end": row.end,
        "recorder": recorder_id(stream, row.by.as_deref()),
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

/// Create the record of every experience in `source`'s stream on `day` that has
/// none, and bring a record whose account changed (a night a later import ended
/// later, a revised type) to its account's times, type and recorder. Nothing else
/// is written: an owner's fields are never touched. It is safe to call for any
/// day, any number of times: a day whose records all match appends nothing.
/// `now` is the clock a record's arrival is read from.
pub(crate) fn write_records_at(
    journal: &Path,
    source: &BodySource,
    day: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> RecordCounts {
    write_stream_records_at(journal, &source.records(), day, now, &BTreeSet::new())
}

/// [`write_records_at`] over Strava's tiles on `day`: one record per workout,
/// on its first tile's day and in that tile's facet. A workout an activity
/// record of Strava's stream already names (any of its tiles, on any day it
/// covers, in any facet) gets none: that record stays its only one. A day file
/// that can't be read refuses the workouts it would decide, counted.
pub(crate) fn write_strava_records_at(
    journal: &Path,
    day: &str,
    now: chrono::DateTime<chrono::Utc>,
    held_back: &BTreeSet<String>,
) -> RecordCounts {
    write_stream_records_at(journal, &STRAVA, day, now, held_back)
}

fn write_stream_records_at(
    journal: &Path,
    source: &RecordSource,
    day: &str,
    now: chrono::DateTime<chrono::Utc>,
    held_back: &BTreeSet<String>,
) -> RecordCounts {
    let mut counts = RecordCounts::default();
    let Some(windows) = day_windows(journal, day, source.stream) else {
        return counts;
    };
    let mut recorded = match recorded_keys(journal, day) {
        Ok(keys) => keys,
        Err(refused) => {
            counts.refused.push(refused);
            return counts;
        }
    };
    let arrived = now
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .format("%Y%m%d")
        .to_string();
    let mut sensed = Sensed::default();
    for window in windows {
        let Some((facet, experiences)) = window_experiences(&window, source.file) else {
            continue;
        };
        for row in experiences {
            let key = account_key(source.stream, &row.id);
            if held_back.contains(&key) {
                counts.refused.push((
                    facet.clone().unwrap_or_default(),
                    "its record couldn't be placed yet".to_owned(),
                ));
                continue;
            }
            if let Some(stored) = recorded.get(&key) {
                let fields = written_fields(source.stream, &row);
                if stored.fields == fields {
                    continue;
                }
                let [Some(activity), Some(start), Some(end), Some(recorder), kind] = fields else {
                    continue;
                };
                match refresh_body_account(
                    journal,
                    &stored.facet,
                    day,
                    &record_id(&key),
                    &activity,
                    &start,
                    &end,
                    &recorder,
                    kind.as_deref(),
                ) {
                    Ok(BodyAccountRefresh::Refreshed) => counts.refreshed += 1,
                    Ok(BodyAccountRefresh::Unchanged | BodyAccountRefresh::NotFound) => {}
                    Err(error) => counts
                        .refused
                        .push((stored.facet.clone(), error.to_string())),
                }
                continue;
            }
            if source.file == WindowFile::StravaTile {
                let name = window
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match sensed.check(journal, day, &name, &row) {
                    Ok(false) => {}
                    Ok(true) => {
                        counts.sensed += 1;
                        continue;
                    }
                    Err(refused) => {
                        counts.refused.push(refused);
                        continue;
                    }
                }
            }
            let Some(facet) = facet.as_deref() else {
                counts.no_facet += 1;
                continue;
            };
            match append_activity_record(
                journal,
                facet,
                day,
                record_for(source.stream, &key, &row, &arrived),
            ) {
                Ok(outcome) => {
                    if matches!(outcome, AppendOutcome::Written(_)) {
                        counts.written += 1;
                    }
                    recorded.insert(
                        key,
                        Recorded {
                            facet: facet.to_owned(),
                            fields: written_fields(source.stream, &row),
                        },
                    );
                }
                Err(error) => counts.refused.push((facet.to_owned(), error.to_string())),
            }
        }
    }
    counts
}

/// [`write_records_at`] on the wall clock.
#[cfg(test)]
pub(crate) fn write_records(journal: &Path, source: &BodySource, day: &str) -> RecordCounts {
    write_records_at(journal, source, day, chrono::Utc::now())
}

/// [`write_strava_records_at`] on the wall clock.
#[cfg(test)]
pub(crate) fn write_strava_records(journal: &Path, day: &str) -> RecordCounts {
    write_strava_records_at(journal, day, chrono::Utc::now(), &BTreeSet::new())
}

#[cfg(test)]
#[path = "body_activity_tests.rs"]
mod tests;
