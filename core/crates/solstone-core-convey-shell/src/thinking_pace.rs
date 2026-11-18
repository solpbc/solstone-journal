// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Thoughts per hour: how fast the journal's thinking finishes, by clock hour.
//!
//! A thought is one finished talent run: one day-index row whose `status` is
//! `completed`, keyed by its use id with the last row winning. A row with any
//! other status counts apart as unfinished. A row is placed in the owner-zone
//! clock hour its run finished: `ts` (when the use was asked for) plus its
//! `runtime_seconds`, which cortex measures to the finish or error event, or
//! `ts` alone when it has none. Counting by finish keeps a batch queued at once
//! from piling into one hour, and makes an hour complete once it has passed.
//! A thought is about an older day when its `day` is before the day before the
//! one it finished on, and recent otherwise. Retired newsletter runs are left
//! out, as Thinking's run list leaves them out.
//!
//! Cortex files each row under the run's target `day`, so a run catching up on
//! an old day appends to that old day's file. A file's name says nothing about
//! when its rows ran, so every file modified inside the window is read. Reading
//! them all on each view would be far too slow on a busy journal, so the index
//! keeps a cursor per file and, on each read, parses only the bytes appended
//! since. A row is appended once its run has finished, so a file last modified
//! before the window holds no row inside it and is never opened.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_journal_config::owner_zone;

use crate::JournalRoot;

const HOUR: i64 = 3600;
/// The longest view, in full hours.
const WINDOW_HOURS: i64 = 30 * 24;
/// Rows are kept a day longer than the longest view so the window can slide.
const KEEP_SECONDS: i64 = (WINDOW_HOURS + 24) * HOUR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Recent,
    Older,
    Unfinished,
}

#[derive(Clone, Copy, Debug)]
struct Row {
    /// When the run finished, epoch seconds.
    at: i64,
    kind: Kind,
}

#[derive(Default)]
struct FileState {
    identity: Option<(u64, u64)>,
    offset: u64,
    rows: HashMap<u64, Row>,
}

#[derive(Default)]
struct Index {
    zone: String,
    files: HashMap<PathBuf, FileState>,
}

fn indexes() -> &'static Mutex<HashMap<PathBuf, Index>> {
    static INDEXES: OnceLock<Mutex<HashMap<PathBuf, Index>>> = OnceLock::new();
    INDEXES.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Deserialize)]
struct DayIndexRow<'a> {
    #[serde(borrow, default)]
    use_id: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    agent_id: Option<Cow<'a, str>>,
    #[serde(default)]
    ts: Option<f64>,
    #[serde(default)]
    runtime_seconds: Option<f64>,
    #[serde(borrow, default)]
    name: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    day: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    status: Option<Cow<'a, str>>,
}

fn use_key(id: &str) -> u64 {
    // A fixed-key hash: the same id must map to the same key on every read.
    let mut hasher =
        std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default()
            .build_hasher();
    hasher.write(id.as_bytes());
    hasher.finish()
}

/// The owner's day before the one `seconds` falls on.
fn yesterday_of(zone: &TimeZone, seconds: i64) -> Option<String> {
    let at = Timestamp::from_second(seconds).ok()?;
    let yesterday = zone.to_datetime(at).date().yesterday().ok()?;
    Some(solstone_core_time::day_key(yesterday))
}

fn parse_row(line: &[u8], zone: &TimeZone) -> Option<(u64, Row)> {
    let row: DayIndexRow<'_> = serde_json::from_slice(line).ok()?;
    if row.name.as_deref() == Some("facet_newsletter") {
        return None;
    }
    let id = row.use_id.or(row.agent_id)?;
    let ts = row.ts?;
    if !ts.is_finite() || ts <= 0.0 {
        return None;
    }
    // Rows carry epoch milliseconds; tolerate seconds.
    let start = if ts > 1e11 {
        (ts / 1000.0) as i64
    } else {
        ts as i64
    };
    let runtime = row
        .runtime_seconds
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .unwrap_or(0.0);
    let at = start + runtime.round() as i64;
    let kind = if row.status.as_deref() != Some("completed") {
        Kind::Unfinished
    } else {
        let yesterday = yesterday_of(zone, at)?;
        match row.day.as_deref() {
            Some(day) if !day.is_empty() && day < yesterday.as_str() => Kind::Older,
            _ => Kind::Recent,
        }
    };
    Some((use_key(&id), Row { at, kind }))
}

fn is_day_file(name: &str) -> bool {
    name.len() == 14
        && name.ends_with(".jsonl")
        && name.as_bytes()[..8].iter().all(u8::is_ascii_digit)
}

#[cfg(unix)]
fn identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    let created = metadata.created().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((created.as_secs(), u64::from(created.subsec_nanos())))
}

fn modified_seconds(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |duration| duration.as_secs() as i64)
}

/// Parse the complete lines appended to `path` since `state.offset`. Returns
/// false when the byte before the cursor is no longer a line end: the file was
/// rewritten under the same identity, so the caller reads it from the start.
fn read_appended(path: &Path, state: &mut FileState, zone: &TimeZone, keep_from: i64) -> bool {
    let Ok(mut file) = File::open(path) else {
        return true;
    };
    let from = state.offset.saturating_sub(1);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return true;
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return true;
    }
    let bytes = if state.offset > 0 {
        match bytes.split_first() {
            Some((b'\n', rest)) => rest,
            _ => return false,
        }
    } else {
        &bytes[..]
    };
    // A row still being written has no newline yet: leave it for the next read.
    let Some(end) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        return true;
    };
    for line in bytes[..end].split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if let Some((key, row)) = parse_row(line, zone) {
            if row.at >= keep_from {
                state.rows.insert(key, row);
            } else {
                state.rows.remove(&key);
            }
        }
    }
    state.offset += end as u64 + 1;
    true
}

impl Index {
    fn refresh(&mut self, talents: &Path, zone: &TimeZone, now: i64) {
        let zone_name = zone.iana_name().unwrap_or_default().to_owned();
        if zone_name != self.zone {
            // Which day a start falls on moved with the zone: start over.
            self.files.clear();
            self.zone = zone_name;
        }
        let keep_from = now - KEEP_SECONDS;
        let Ok(entries) = fs::read_dir(talents) else {
            self.files.clear();
            return;
        };
        let mut seen = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !is_day_file(name) {
                continue;
            }
            let path = entry.path();
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let known = self.files.contains_key(&path);
            if !known && modified_seconds(&metadata) < keep_from {
                continue;
            }
            seen.push(path.clone());
            let state = self.files.entry(path.clone()).or_default();
            let current = identity(&metadata);
            if state.identity != current || metadata.len() < state.offset {
                // Replaced or truncated: read it again from the start.
                *state = FileState {
                    identity: current,
                    ..FileState::default()
                };
            }
            if metadata.len() > state.offset && !read_appended(&path, state, zone, keep_from) {
                *state = FileState {
                    identity: current,
                    ..FileState::default()
                };
                read_appended(&path, state, zone, keep_from);
            }
            state.rows.retain(|_, row| row.at >= keep_from);
        }
        seen.sort();
        self.files
            .retain(|path, _| seen.binary_search(path).is_ok());
    }

    fn rows(&self) -> impl Iterator<Item = &Row> {
        self.files.values().flat_map(|state| state.rows.values())
    }
}

#[derive(Clone, Copy, Default)]
struct Counts {
    recent: u32,
    older: u32,
    unfinished: u32,
}

impl Counts {
    fn thoughts(&self) -> u32 {
        self.recent + self.older
    }
}

#[derive(Serialize, Debug, PartialEq)]
struct HourBar {
    start: i64,
    day: String,
    hour: i8,
    recent: u32,
    older: u32,
}

#[derive(Serialize, Debug, PartialEq)]
struct DayBar {
    day: String,
    /// Full hours of this day inside the view (fewer at either edge).
    hours: u32,
    /// Hours with at least one thought.
    thinking_hours: u32,
    /// The day's own typical thinking hour, split by the day's share of
    /// recent and older thoughts, so a bar reads against the view's median.
    /// Zero when the day had no thinking.
    recent: f64,
    older: f64,
    /// The day's own totals.
    thoughts: u32,
    older_thoughts: u32,
}

#[derive(Serialize, Debug, PartialEq)]
struct Busiest {
    start: i64,
    day: String,
    hour: i8,
    thoughts: u32,
}

#[derive(Serialize, Debug, PartialEq)]
struct View {
    key: &'static str,
    hours: u32,
    thinking_hours: u32,
    thoughts: u32,
    older: u32,
    unfinished: u32,
    /// Median thoughts over the hours that had any; null when none did.
    median: Option<f64>,
    busiest: Option<Busiest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hour_bars: Vec<HourBar>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    day_bars: Vec<DayBar>,
}

#[derive(Serialize, Debug, PartialEq)]
struct Pace {
    zone: String,
    /// The end of the last full hour, epoch seconds.
    through: i64,
    /// Thoughts finished so far in the hour in progress.
    this_hour: u32,
    views: Vec<View>,
}

/// The owner-zone start of the clock hour holding `seconds`.
fn hour_start(zone: &TimeZone, seconds: i64) -> i64 {
    let Ok(at) = Timestamp::from_second(seconds) else {
        return seconds - seconds.rem_euclid(HOUR);
    };
    let local = zone.to_datetime(at);
    seconds - i64::from(local.minute()) * 60 - i64::from(local.second())
}

fn label(zone: &TimeZone, start: i64) -> (String, i8) {
    match Timestamp::from_second(start) {
        Ok(at) => {
            let local = zone.to_datetime(at);
            (solstone_core_time::day_key(local.date()), local.hour())
        }
        Err(_) => (String::new(), 0),
    }
}

fn median(values: &mut [u32]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        f64::from(values[middle])
    } else {
        (f64::from(values[middle - 1]) + f64::from(values[middle])) / 2.0
    })
}

fn view(key: &'static str, hours: &[(i64, Counts)], zone: &TimeZone, by_day: bool) -> View {
    let mut thinking = Vec::new();
    let mut busiest: Option<(i64, u32)> = None;
    let (mut thoughts, mut older, mut unfinished) = (0, 0, 0);
    for (start, counts) in hours {
        thoughts += counts.thoughts();
        older += counts.older;
        unfinished += counts.unfinished;
        if counts.thoughts() > 0 {
            thinking.push(counts.thoughts());
            if busiest.is_none_or(|(_, most)| counts.thoughts() > most) {
                busiest = Some((*start, counts.thoughts()));
            }
        }
    }
    let thinking_hours = thinking.len() as u32;
    let mut hour_bars = Vec::new();
    let mut day_bars = Vec::new();
    if by_day {
        // Per day: its full hours, its thinking hours' counts, and its totals.
        let mut days: BTreeMap<String, (u32, Vec<u32>, u32, u32)> = BTreeMap::new();
        for (start, counts) in hours {
            let entry = days.entry(label(zone, *start).0).or_default();
            entry.0 += 1;
            if counts.thoughts() > 0 {
                entry.1.push(counts.thoughts());
            }
            entry.2 += counts.recent;
            entry.3 += counts.older;
        }
        day_bars = days
            .into_iter()
            .map(|(day, (hours, mut counts, recent, older))| {
                let thinking_hours = counts.len() as u32;
                let typical = median(&mut counts).unwrap_or(0.0);
                let total = recent + older;
                let older_share = if total == 0 {
                    0.0
                } else {
                    f64::from(older) / f64::from(total)
                };
                DayBar {
                    day,
                    hours,
                    thinking_hours,
                    recent: typical * (1.0 - older_share),
                    older: typical * older_share,
                    thoughts: total,
                    older_thoughts: older,
                }
            })
            .collect();
    } else {
        hour_bars = hours
            .iter()
            .map(|(start, counts)| {
                let (day, hour) = label(zone, *start);
                HourBar {
                    start: *start,
                    day,
                    hour,
                    recent: counts.recent,
                    older: counts.older,
                }
            })
            .collect();
    }
    View {
        key,
        hours: hours.len() as u32,
        thinking_hours,
        thoughts,
        older,
        unfinished,
        median: median(&mut thinking),
        busiest: busiest.map(|(start, thoughts)| {
            let (day, hour) = label(zone, start);
            Busiest {
                start,
                day,
                hour,
                thoughts,
            }
        }),
        hour_bars,
        day_bars,
    }
}

fn pace<'a>(rows: impl Iterator<Item = &'a Row>, zone: &TimeZone, now: i64) -> Pace {
    let current = hour_start(zone, now);
    // Every full hour in the window, newest first. Walked back hour by hour in
    // the zone, so a half-hour clock change keeps the zone's own boundaries.
    let mut starts = Vec::with_capacity(WINDOW_HOURS as usize);
    let mut start = current;
    for _ in 0..WINDOW_HOURS {
        start = hour_start(zone, start - 1);
        starts.push(start);
    }
    let first = start;
    let mut buckets: HashMap<i64, Counts> = HashMap::new();
    let mut this_hour = 0;
    for row in rows {
        if row.at >= current {
            if row.kind != Kind::Unfinished {
                this_hour += 1;
            }
            continue;
        }
        if row.at < first {
            continue;
        }
        let bucket = buckets.entry(hour_start(zone, row.at)).or_default();
        match row.kind {
            Kind::Recent => bucket.recent += 1,
            Kind::Older => bucket.older += 1,
            Kind::Unfinished => bucket.unfinished += 1,
        }
    }
    // Oldest first; an hour with no rows is zero.
    let hours: Vec<(i64, Counts)> = starts
        .into_iter()
        .rev()
        .map(|start| (start, buckets.get(&start).copied().unwrap_or_default()))
        .collect();
    let tail = |count: usize| &hours[hours.len() - count..];
    Pace {
        zone: zone.iana_name().unwrap_or_default().to_owned(),
        through: current,
        this_hour,
        views: vec![
            view("day", tail(24), zone, false),
            view("week", tail(7 * 24), zone, false),
            view("month", &hours, zone, true),
        ],
    }
}

fn read_pace(journal: &Path, now: i64) -> Pace {
    let zone = owner_zone(journal);
    let mut indexes = indexes()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let index = indexes.entry(journal.to_path_buf()).or_default();
    index.refresh(&journal.join("talents"), &zone, now);
    pace(index.rows(), &zone, now)
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

pub(crate) async fn api_pace(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let root = journal.0.clone();
    match tokio::task::spawn_blocking(move || read_pace(&root, now_seconds())).await {
        Ok(pace) => Json(pace).into_response(),
        Err(_) => error_envelope(
            "pace_unavailable",
            "thoughts per hour couldn't be read.",
            "the pace read stopped before it finished",
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response(),
    }
}

#[cfg(test)]
#[path = "thinking_pace_tests.rs"]
mod tests;
