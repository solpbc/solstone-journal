// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Activities are written once their stream's evidence has settled.
//!
//! Some capture sources deliver a backlog late, out of order and newest first:
//! a phone relaying a watch's saved recordings sends whatever it captured
//! recently ahead of the older queue, and the rest can follow minutes or hours
//! apart. Writing an activity where live thinking sees it end writes it before
//! its evidence has arrived: the stream shows a gap the backlog is about to
//! fill. So a live stream's activities are written from one place only, the
//! capture-order rebuild of the stream's day, and only once that rebuild has
//! shown an activity ended, with the same segments, for [`SETTLE_WINDOW_MS`],
//! while nothing captured just after it is still arriving or being thought.
//! A stream's last activity is closed once the stream has been quiet for
//! [`QUIET_CLOSE_MS`], and a finished day publishes what is left once it is
//! quiet.
//!
//! What is pending is kept per stream in `awareness/activity_settle/`: the
//! days to check, when each candidate was first seen with its segments, and
//! when the next check is due. Candidates are derived again on every check,
//! so losing the file restarts their clocks and never drops one.
//!
//! An activity this publisher wrote can grow: when later evidence extends it
//! (the same ID, on the same stream, holding every segment it held and more),
//! the record takes the new segments and its talents run again. A record
//! written any other way is never changed here, and no activity that shares a
//! segment with one is written.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_facets::{
    AppendOutcome, get_activity_record, load_activity_records, observe_declared_facet_inventory,
    update_activity_record,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, DEFAULT_STREAM, LockOptions, PathOrDay, atomic_replace, hold_lock,
    iter_segments,
};
use solstone_core_system::activity_state::ActivityStateMachine;
use solstone_core_system_health::newest_segment_input_ms;

use crate::context::ThinkContext;
use crate::dispatch::ModeResult;
use crate::run_log::RunLogWriter;
use crate::segment::{
    EndedActivity, LIVE_ORDER_HOLD_MS, activity_context, activity_event, append_own_activity,
    filter_declared_facets, modified_ms, record_publication, rethink_if_input_changed,
    run_activity_talents, stream_coordinate, valid_activity_sense,
};

/// How long an ended activity's segments must stay unchanged, and how long
/// ago anything captured just after it must have been thought, before it is
/// written.
pub(crate) const SETTLE_WINDOW_MS: i64 = 5 * 60 * 1000;

/// How long a stream must be quiet before its last activity is closed; the
/// supervisor's idle flush uses the same hour.
pub(crate) const QUIET_CLOSE_MS: i64 = 60 * 60 * 1000;

/// How long a check waits for another writer of the same stream.
const SETTLE_TURN_WAIT: Duration = Duration::from_secs(60);

// Tests that do not exercise the window run on fixed clocks far from their
// files' times; for them an ended activity settles at once and the idle flush
// closes the tail. Tests of the window turn this off.
#[cfg(test)]
thread_local! {
    static AT_ONCE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

#[cfg(test)]
pub(crate) fn settle_at_once(at_once: bool) {
    AT_ONCE.with(|cell| cell.set(at_once));
}

fn at_once() -> bool {
    #[cfg(test)]
    {
        AT_ONCE.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Whether a stream's ended activities wait to settle. A finite import is
/// written where its replay ends, and an explicit refresh rewrites what is
/// already settled.
pub(crate) fn defers(stream: Option<&str>, refresh: bool) -> bool {
    !refresh && !stream.is_some_and(|stream| stream.starts_with("import."))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Pending {
    #[serde(default)]
    due_ms: Option<i64>,
    #[serde(default)]
    days: BTreeMap<String, BTreeMap<String, Seen>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Seen {
    since: i64,
    segments: Vec<String>,
    #[serde(default)]
    refused: bool,
}

/// Stream names come from segment directory names; anything else is never
/// used as a file name.
fn safe_stream(stream: &str) -> bool {
    !stream.is_empty()
        && !stream.starts_with('.')
        && stream
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn pending_dir(journal: &Path) -> PathBuf {
    journal.join("awareness/activity_settle")
}

fn pending_path(journal: &Path, stream: &str) -> PathBuf {
    pending_dir(journal).join(format!("{stream}.json"))
}

/// While a check runs the talents of what it wrote, the days it wrote are
/// listed here, one file per check, so a day is never read as settled while
/// its activities are still being written.
fn publishing_dir(journal: &Path) -> PathBuf {
    pending_dir(journal).join("publishing")
}

/// A listing older than this was left by a check that never finished.
const PUBLISHING_STALE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Default, Serialize, Deserialize)]
struct Publishing {
    days: BTreeSet<String>,
}

/// List `days` as being written by this check, and drop listings this
/// stream's earlier checks left behind. Returns the listing to remove once
/// the talents have run.
pub(crate) fn list_publishing(
    journal: &Path,
    stream: &str,
    days: BTreeSet<String>,
) -> Option<PathBuf> {
    let dir = publishing_dir(journal);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.filter_map(Result::ok) {
            let stale = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= PUBLISHING_STALE);
            let ours = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&format!("{stream}-")));
            if stale && ours {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    if days.is_empty() {
        return None;
    }
    let path = dir.join(format!(
        "{stream}-{}-{}.json",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let bytes = serde_json::to_vec(&Publishing { days }).ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    atomic_replace(&path, &bytes, AtomicWriteOptions::default()).ok()?;
    Some(path)
}

fn read_pending(path: &Path) -> Pending {
    match solstone_core_journal_io::durability::read_json_durable::<Pending>(
        solstone_core_journal_io::durability::ArtifactId::ActivitySettle,
        path,
    ) {
        Ok(solstone_core_journal_io::durability::DurableRead::Present(pending)) => pending,
        _ => Pending::default(),
    }
}

fn write_pending(path: &Path, pending: &Pending) -> Result<(), String> {
    if pending.days.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec(pending).map_err(|error| error.to_string())?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
}

/// Mark `days` of `stream` for the next settle check.
pub(crate) fn note(journal: &Path, stream: &str, days: &[&str]) -> Result<(), String> {
    if !safe_stream(stream) {
        return Ok(());
    }
    let path = pending_path(journal, stream);
    let _turn = hold_lock(
        &path,
        LockOptions {
            timeout: SETTLE_TURN_WAIT,
            ..LockOptions::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let mut pending = read_pending(&path);
    let before = pending.days.len();
    for day in days {
        pending.days.entry((*day).to_owned()).or_default();
    }
    if pending.days.len() == before {
        return Ok(());
    }
    write_pending(&path, &pending)
}

/// Streams whose next settle check is due by `now_ms`.
pub(crate) fn due_streams(journal: &Path, now_ms: i64) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(pending_dir(journal)) else {
        return Vec::new();
    };
    let mut due = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let stream = path
                .file_name()?
                .to_str()?
                .strip_suffix(".json")?
                .to_owned();
            (safe_stream(&stream)
                && read_pending(&path)
                    .due_ms
                    .is_some_and(|due_ms| due_ms <= now_ms))
            .then_some(stream)
        })
        .collect::<Vec<_>>();
    due.sort();
    due
}

/// Whether any stream's pending checks include `day`, or a check is still
/// running the talents of activities it wrote there.
pub(crate) fn holds_day(journal: &Path, day: &str) -> bool {
    let pending = std::fs::read_dir(pending_dir(journal))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            let path = entry.path();
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".json"))
                .is_some_and(safe_stream)
                && read_pending(&path).days.contains_key(day)
        });
    pending
        || std::fs::read_dir(publishing_dir(journal))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .any(|entry| {
                let fresh = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_none_or(|age| age < PUBLISHING_STALE);
                fresh
                    && std::fs::read(entry.path())
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<Publishing>(&bytes).ok())
                        .is_some_and(|publishing| publishing.days.contains(day))
            })
}

/// `journal think --settle`: check one stream, or every stream with pending
/// days.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    stream: Option<&str>,
    max_concurrency: i64,
    skip_activity_prompts: bool,
) -> Result<ModeResult, String> {
    let streams = match stream {
        Some(stream) => vec![stream.to_owned()],
        None => std::fs::read_dir(pending_dir(&context.journal))
            .map(|entries| {
                let mut streams = entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| {
                        entry
                            .file_name()
                            .to_str()?
                            .strip_suffix(".json")
                            .map(str::to_owned)
                    })
                    .collect::<Vec<_>>();
                streams.sort();
                streams
            })
            .unwrap_or_default(),
    };
    let errors = streams
        .iter()
        .filter_map(|stream| {
            check(
                context,
                log,
                stream,
                false,
                skip_activity_prompts,
                max_concurrency,
            )
            .err()
        })
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(ModeResult::default())
    } else {
        Err(errors.join("; "))
    }
}

/// Write every settled activity of `stream`'s pending days, plus today and
/// yesterday, and record when the next check is due. `idle` is the
/// supervisor's idle flush: it closes the stream's last activity once nothing
/// of the stream has arrived or been thought within the window.
pub(crate) fn check(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    stream: &str,
    idle: bool,
    skip_activity_prompts: bool,
    max_concurrency: i64,
) -> Result<(), String> {
    if !safe_stream(stream) {
        return Ok(());
    }
    let now = context.now_ms;
    let path = pending_path(&context.journal, stream);
    let mut talents = Vec::new();
    let mut errors = Vec::new();
    let publishing;
    {
        // Another check holding this stream reads the same evidence. A turn
        // that cannot be made at all is not fatal: what has settled is still
        // written, though what is pending is not remembered.
        let turn = match hold_lock(
            &path,
            LockOptions {
                timeout: SETTLE_TURN_WAIT,
                ..LockOptions::default()
            },
        ) {
            Ok(turn) => Some(turn),
            Err(solstone_core_journal_io::LockError::Timeout(_)) => return Ok(()),
            Err(error) => {
                errors.push(format!("activity settle state not saved: {error}"));
                None
            }
        };
        let mut pending = read_pending(&path);
        // A day this run is already working past is over too.
        let today = local_day(&context.journal, now).max(context.day.clone());
        let yesterday = local_day(&context.journal, now - 24 * 60 * 60 * 1000);
        let mut days = pending.days.keys().cloned().collect::<BTreeSet<_>>();
        days.extend([today.clone(), yesterday]);
        let mut due: Option<i64> = None;
        for day in days {
            let mut seen = pending.days.remove(&day).unwrap_or_default();
            let settled = activity_context(context, &day).and_then(|day_context| {
                settle_day(
                    &day_context,
                    log,
                    stream,
                    &today,
                    idle,
                    &mut seen,
                    skip_activity_prompts,
                    &mut talents,
                )
            });
            match settled {
                Ok(Some(next)) => {
                    due = Some(due.map_or(next, |due| due.min(next)));
                    pending.days.insert(day, seen);
                }
                Ok(None) => {}
                Err(error) => {
                    errors.push(error);
                    due = Some(due.map_or(now + SETTLE_WINDOW_MS, |due| {
                        due.min(now + SETTLE_WINDOW_MS)
                    }));
                    pending.days.insert(day, seen);
                }
            }
        }
        pending.due_ms = due;
        #[cfg(test)]
        crate::segment::state_probe::at(&context.journal, "settled");
        if turn.is_some()
            && let Err(error) = write_pending(&path, &pending)
        {
            errors.push(error);
        }
        // Listed before the turn is given up, so the days stay held from
        // the moment they leave the pending list until their talents finish.
        publishing = list_publishing(
            &context.journal,
            stream,
            talents
                .iter()
                .map(|talent| talent.day().to_owned())
                .collect(),
        );
    }
    // Talents can run for minutes; no writer waits on them.
    if let Err(error) = run_activity_talents(context, log, talents, false, max_concurrency) {
        errors.push(error);
    }
    if let Some(publishing) = publishing {
        let _ = std::fs::remove_file(publishing);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn local_day(journal: &Path, now_ms: i64) -> String {
    solstone_core_system::daily_coverage::local_day(
        journal,
        Utc.timestamp_millis_opt(now_ms)
            .single()
            .unwrap_or_else(Utc::now),
    )
}

/// One segment of the stream's day: when its input last changed and when its
/// Sense was written.
struct Segment {
    key: String,
    dir: PathBuf,
    arrived: Option<i64>,
    sensed: Option<i64>,
}

/// Settle one stream-day. Returns when it next needs a check, or `None` when
/// nothing is left to write there.
#[allow(clippy::too_many_arguments)]
fn settle_day(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    stream: &str,
    today: &str,
    idle: bool,
    seen: &mut BTreeMap<String, Seen>,
    skip_activity_prompts: bool,
    talents: &mut Vec<EndedActivity>,
) -> Result<Option<i64>, String> {
    let now = context.now_ms;
    let day = context.day.as_str();
    let mut segments = iter_segments(&context.journal, PathOrDay::Day(day))
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|entry| stream_coordinate(entry.path(), day) == stream)
        .map(|entry| Segment {
            key: entry.name().to_string_lossy().into_owned(),
            dir: entry.path().to_path_buf(),
            arrived: newest_segment_input_ms(entry.path()).ok().flatten(),
            sensed: modified_ms(&entry.path().join("talents/sense.json")),
        })
        .collect::<Vec<_>>();
    segments.sort_by(|a, b| a.key.cmp(&b.key));
    if segments.is_empty() {
        seen.clear();
        return Ok(None);
    }
    let latest = segments
        .iter()
        .filter_map(|segment| segment.arrived.max(segment.sensed))
        .max()
        .unwrap_or(0);
    // A finished day is over and nothing of it has arrived or been thought
    // within the window, so a segment still being thought is never left out.
    let finished = day < today && (at_once() || now - latest >= SETTLE_WINDOW_MS);
    let close_tail = finished
        || now - latest >= QUIET_CLOSE_MS
        || (idle && (at_once() || now - latest >= SETTLE_WINDOW_MS));

    // The day in capture order, over every segment thought so far.
    let inventory =
        observe_declared_facet_inventory(&context.journal).map_err(|error| error.to_string())?;
    let mut machine = ActivityStateMachine::default();
    for segment in segments.iter().filter(|segment| segment.sensed.is_some()) {
        let Ok(solstone_core_journal_io::durability::DurableRead::Present(sense)) =
            solstone_core_journal_io::durability::read_json_durable::<Value>(
                solstone_core_journal_io::durability::ArtifactId::SegmentSense,
                &segment.dir.join("talents/sense.json"),
            )
        else {
            continue;
        };
        if !valid_activity_sense(&sense) {
            continue;
        }
        let mut filtered = sense;
        if let Some(object) = filtered.as_object_mut() {
            let (facets, _) = filter_declared_facets(
                object.get("facets"),
                &inventory,
                context,
                log,
                &segment.key,
                Some(stream),
                false,
            )?;
            object.insert("facets".to_owned(), facets);
        }
        machine.update(&filtered, &segment.key, day, None, now);
    }
    let ended = machine.completed_activities().len();
    let mut open = false;
    if let Some(last) = machine.last_segment_key().map(ToOwned::to_owned) {
        if close_tail {
            let _ = machine.close_active(&last, now);
        } else {
            open = !machine.close_active(&last, now).is_empty();
        }
    }
    let completed = machine.completed_activities();
    let candidates = if close_tail {
        &completed[..]
    } else {
        &completed[..ended]
    };

    let mut due: Option<i64> = open.then_some(latest + QUIET_CLOSE_MS);
    // A finished day's last activity is written as soon as its last segment
    // has settled, not an hour after it.
    if open && day < today {
        due = due.map(|due| due.min(latest + SETTLE_WINDOW_MS));
    }
    let mut keep = BTreeSet::new();
    let mut failures = Vec::new();
    let mut held_by_facet: BTreeMap<String, Vec<Map<String, Value>>> = BTreeMap::new();
    for (index, record) in candidates.iter().enumerate() {
        let (Some(facet), Some(id)) = (
            record.get("facet").and_then(Value::as_str),
            record.get("id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let record_segments = segments_of(record);
        let Some(last) = record_segments.iter().max().cloned() else {
            continue;
        };
        let key = format!("{facet}/{id}");
        let entry = seen.entry(key.clone()).or_insert_with(|| Seen {
            since: now,
            segments: record_segments.iter().cloned().collect(),
            refused: false,
        });
        if entry.segments.iter().cloned().collect::<BTreeSet<_>>() != record_segments {
            *entry = Seen {
                since: now,
                segments: record_segments.iter().cloned().collect(),
                refused: false,
            };
        }
        if entry.refused {
            keep.insert(key);
            continue;
        }
        if !held_by_facet.contains_key(facet) {
            let held = load_activity_records(&context.journal, facet, day, true)
                .map_err(|error| error.to_string())?
                .into_iter()
                .filter(|held| same_stream(held, stream))
                .collect();
            held_by_facet.insert(facet.to_owned(), held);
        }
        let held = &held_by_facet[facet];
        let written = held
            .iter()
            .flat_map(segments_of_map)
            .collect::<BTreeSet<_>>();
        if record_segments.is_subset(&written) {
            // Written already. If its segments were thought again since, its
            // talents run again when what they read has changed.
            if let Some(held) = held
                .iter()
                .find(|held| segments_of_map(held) == record_segments)
                && !skip_activity_prompts
                && segments.iter().any(|segment| {
                    record_segments.contains(&segment.key)
                        && segment.sensed.is_some_and(|sensed| {
                            held.get("created_at")
                                .and_then(Value::as_i64)
                                .is_none_or(|created| sensed > created)
                        })
                })
                && let Err(error) = rethink_if_input_changed(context, day, facet, held, talents)
            {
                failures.push(error);
            }
            continue;
        }
        // Records written any other way are never extended or overlapped.
        if held.iter().any(|held| {
            !held.contains_key("settled_at") && !segments_of_map(held).is_disjoint(&record_segments)
        }) {
            entry.refused = true;
            keep.insert(key);
            continue;
        }
        let tail_closed = index >= ended;
        let since = entry.since;
        let moving = gap_moving(&segments, &last, now);
        let settled = at_once()
            || finished
            || tail_closed
            || (now - since >= SETTLE_WINDOW_MS && moving.is_none());
        if !settled {
            let next = (since + SETTLE_WINDOW_MS).max(moving.unwrap_or(0));
            due = Some(due.map_or(next, |due| due.min(next)));
            keep.insert(key);
            continue;
        }
        match publish(
            context,
            log,
            stream,
            &last,
            record,
            skip_activity_prompts,
            talents,
        ) {
            Ok(Published::Written | Published::Grown) => {
                held_by_facet.remove(facet);
            }
            Ok(Published::Refused) => {
                entry.refused = true;
                keep.insert(key);
            }
            Err(error) => {
                failures.push(error);
                keep.insert(key);
            }
        }
    }
    seen.retain(|key, _| keep.contains(key));
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    // A refused candidate needs no check of its own; it is looked at again
    // when its segments change, which another segment's thinking notices.
    let waiting = seen.values().any(|seen| !seen.refused);
    Ok((open || waiting).then(|| due.unwrap_or(now + SETTLE_WINDOW_MS)))
}

/// When an activity ending at `last` may still be extended: a segment
/// captured after it that was thought within the window, or that has arrived
/// and is still being thought, means a gap may be filling from its far end.
/// Returns when that stops holding, or `None` when nothing holds it.
fn gap_moving(segments: &[Segment], last: &str, now: i64) -> Option<i64> {
    for segment in segments
        .iter()
        .filter(|segment| segment.key.as_str() > last)
    {
        match segment.sensed {
            Some(sensed) if now - sensed >= SETTLE_WINDOW_MS => return None,
            Some(sensed) => return Some(sensed + SETTLE_WINDOW_MS),
            None => {
                if let Some(arrived) = segment.arrived
                    && now - arrived < LIVE_ORDER_HOLD_MS
                {
                    return Some(now + SETTLE_WINDOW_MS);
                }
            }
        }
    }
    None
}

fn segments_of(record: &Value) -> BTreeSet<String> {
    record.as_object().map(segments_of_map).unwrap_or_default()
}

fn segments_of_map(record: &Map<String, Value>) -> BTreeSet<String> {
    record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// A record of this stream: one that names it, or one that names no stream,
/// which may be an older record of this stream or one of the direct layout.
fn same_stream(record: &Map<String, Value>, stream: &str) -> bool {
    record
        .get("stream")
        .and_then(Value::as_str)
        .is_none_or(|held| held == stream)
}

enum Published {
    Written,
    Grown,
    Refused,
}

fn publish(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    stream: &str,
    last: &str,
    record: &Value,
    skip_activity_prompts: bool,
    talents: &mut Vec<EndedActivity>,
) -> Result<Published, String> {
    let day = context.day.as_str();
    let Some(mut record) = record.as_object().cloned() else {
        return Ok(Published::Refused);
    };
    let facet = record
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let id = record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if stream != DEFAULT_STREAM {
        record.insert("stream".to_owned(), Value::String(stream.to_owned()));
    }
    record.insert("settled_at".to_owned(), Value::from(context.now_ms));
    let outcome = append_own_activity(&context.journal, &facet, day, &mut record)
        .map_err(|error| format!("{facet}/{id}: {error}"))?;
    let published = match outcome {
        AppendOutcome::Written(_) => Published::Written,
        AppendOutcome::AlreadyExists => grow(context, &facet, &record)?,
    };
    if matches!(published, Published::Refused) {
        return Ok(published);
    }
    log.log(
        "activity.detected",
        context.now_ms,
        activity_event(
            context,
            last,
            day,
            Map::from_iter([
                ("activity".to_owned(), Value::String(id.clone())),
                ("facet".to_owned(), Value::String(facet.clone())),
                ("state".to_owned(), Value::String("ended".to_owned())),
            ]),
        ),
    );
    record_publication(
        context,
        log,
        last,
        day,
        &facet,
        &record,
        &id,
        true,
        false,
        skip_activity_prompts,
        talents,
    )?;
    Ok(published)
}

/// Extend a record this publisher wrote, when `record` holds every segment
/// it holds and more. Its ID, and everything the owner or a talent wrote on
/// it, stay.
fn grow(
    context: &ThinkContext,
    facet: &str,
    record: &Map<String, Value>,
) -> Result<Published, String> {
    let day = context.day.as_str();
    let Some(id) = record.get("id").and_then(Value::as_str) else {
        return Ok(Published::Refused);
    };
    let Some(held) = get_activity_record(&context.journal, facet, day, id)
        .map_err(|error| format!("{facet}/{id}: {error}"))?
    else {
        return Ok(Published::Refused);
    };
    let held_segments = segments_of_map(&held);
    let segments = segments_of_map(record);
    if !held.contains_key("settled_at")
        || held.get("stream") != record.get("stream")
        || !held_segments.is_subset(&segments)
        || held_segments == segments
    {
        return Ok(Published::Refused);
    }
    let patch = ["segments", "level_avg", "active_entities"]
        .into_iter()
        .filter_map(|field| {
            record
                .get(field)
                .map(|value| (field.to_owned(), value.clone()))
        })
        .collect::<Map<String, Value>>();
    let timestamp = Utc
        .timestamp_millis_opt(context.now_ms)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339();
    update_activity_record(
        &context.journal,
        facet,
        day,
        id,
        &patch,
        "activity",
        "grew with later evidence",
        &timestamp,
    )
    .map_err(|error| format!("{facet}/{id}: {error}"))?
    .map_or(Ok(Published::Refused), |_| Ok(Published::Grown))
}
