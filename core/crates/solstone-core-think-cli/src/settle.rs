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
//! the record takes the new segments (and unheld description) and its talents
//! run again. When a newly written or grown settled activity strictly subsumes
//! earlier machine-derived settled rows on the same `(facet, day, stream)`,
//! owner-held fields and edits are carried forward and those strict-subset
//! sibling rows are retired atomically with their outputs, pending work, and
//! provenance. A record written any other way is never changed here, and no
//! activity that shares a segment with one is written.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_facets::{
    ActivityRecord, activity_field_held, admit_activity_destination, append_edit, is_body_record,
    is_retired_plan, load_activity_records, observe_declared_facet_inventory, remake_activity_day,
};
use solstone_core_format::content::{
    EDIT_STRETCH, STRETCH_ID, edit_names, edit_removed, is_strava_reading, owner_edit,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, DEFAULT_STREAM, LockOptions, PathOrDay, atomic_replace, hold_lock,
    iter_segments,
};
use solstone_core_system::activity_state::{ActivityStateMachine, Rederived, rederive};
use solstone_core_system_health::newest_segment_input_ms;

use crate::context::ThinkContext;
use crate::dispatch::ModeResult;
use crate::run_log::RunLogWriter;
use crate::segment::{
    EndedActivity, LIVE_ORDER_HOLD_MS, OwnAppend, activity_context, activity_event,
    append_own_activity, filter_declared_facets, modified_ms, record_publication,
    rethink_if_input_changed, run_activity_talents, stream_coordinate, valid_activity_sense,
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

/// Like [`defers`], also settling immediately (`0` min wait) for historical
/// (`day < today`) syncing-source backfills.
pub(crate) fn defers_in(journal: &Path, day: &str, stream: Option<&str>, refresh: bool) -> bool {
    if !defers(stream, refresh) {
        return false;
    }
    let today = solstone_core_system::daily_coverage::local_day(journal, jiff::Timestamp::now());
    if day < today.as_str()
        && stream.is_some_and(|name| solstone_core_segment::is_backfill_stream(journal, name))
    {
        return false;
    }
    true
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
#[cfg(test)]
pub(crate) fn list_publishing(
    journal: &Path,
    stream: &str,
    days: BTreeSet<String>,
) -> Option<PathBuf> {
    list_publishing_checked(journal, stream, days, &|| Ok(()))
}

fn list_publishing_checked(
    journal: &Path,
    stream: &str,
    days: BTreeSet<String>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Option<PathBuf> {
    checkpoint().ok()?;
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
                checkpoint().ok()?;
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
        i64::try_from(Timestamp::now().as_nanosecond()).unwrap_or_default()
    ));
    let bytes = serde_json::to_vec(&Publishing { days }).ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    checkpoint().ok()?;
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

fn write_pending(
    path: &Path,
    pending: &Pending,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
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
    checkpoint()?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
}

/// Mark `days` of `stream` for the next settle check.
pub(crate) fn note(context: &ThinkContext, stream: &str, days: &[&str]) -> Result<(), String> {
    context.checkpoint()?;
    let journal = &context.journal;
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
    context.checkpoint()?;
    let mut pending = read_pending(&path);
    let before = pending.days.len();
    for day in days {
        pending.days.entry((*day).to_owned()).or_default();
    }
    if pending.days.len() == before {
        return Ok(());
    }
    write_pending(&path, &pending, &|| context.checkpoint())
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
    context.checkpoint()?;
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
                errors.push(solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.settle.save",
                    [("error", &(error))],
                ));
                None
            }
        };
        context.checkpoint()?;
        let mut pending = read_pending(&path);
        // A day this run is already working past is over too.
        let today = local_day(&context.journal, now).max(context.day.clone());
        let yesterday = local_day(&context.journal, now - 24 * 60 * 60 * 1000);
        let mut days = pending.days.keys().cloned().collect::<BTreeSet<_>>();
        days.extend([today.clone(), yesterday]);
        let mut due: Option<i64> = None;
        for day in days {
            context.checkpoint()?;
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
            && let Err(error) = write_pending(&path, &pending, &|| context.checkpoint())
        {
            errors.push(error);
        }
        // Listed before the turn is given up, so the days stay held from
        // the moment they leave the pending list until their talents finish.
        publishing = list_publishing_checked(
            &context.journal,
            stream,
            talents
                .iter()
                .map(|talent| talent.day().to_owned())
                .collect(),
            &|| context.checkpoint(),
        );
    }
    // Talents can run for minutes; no writer waits on them.
    if let Err(error) = run_activity_talents(context, log, talents, false, max_concurrency) {
        errors.push(error);
    }
    if let Some(publishing) = publishing {
        context.checkpoint()?;
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
        Timestamp::from_millisecond(now_ms).unwrap_or_else(|_| Timestamp::now()),
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
        let has_subset_to_fold = held
            .iter()
            .any(|row| is_absorbable_subset(row, id, stream, &record_segments));
        if record_segments.is_subset(&written) && !has_subset_to_fold {
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

/// A record's canonical stream coordinate: its stored `stream`, or
/// [`DEFAULT_STREAM`] when absent.
fn exact_stream(record: &Map<String, Value>) -> &str {
    record
        .get("stream")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_STREAM)
}

fn row_id(record: &Map<String, Value>) -> &str {
    record.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn has_non_null_source(record: &Map<String, Value>) -> bool {
    record.get("source").is_some_and(|value| !value.is_null())
}

/// Whether `row` is an existing machine-derived settled record on the same
/// stream whose segments are a strict subset of `target_segments`.
fn is_absorbable_subset(
    row: &Map<String, Value>,
    target_id: &str,
    stream: &str,
    target_segments: &BTreeSet<String>,
) -> bool {
    if row_id(row) == target_id
        || is_retired_plan(row)
        || !row.contains_key("settled_at")
        || has_non_null_source(row)
        || is_body_record(row)
        || is_strava_reading(row)
        || exact_stream(row) != stream
    {
        return false;
    }
    let segments = segments_of_map(row);
    !segments.is_empty()
        && segments.len() < target_segments.len()
        && segments.is_subset(target_segments)
}

enum Published {
    Written,
    Grown,
    Refused,
}

enum FoldOutcome {
    AppendOnly {
        target_id: String,
    },
    Written {
        record: ActivityRecord,
        absorbed_ids: Vec<String>,
    },
    Grown {
        record: ActivityRecord,
        absorbed_ids: Vec<String>,
    },
    Refused,
}

/// Structural fields that belong to the target span and are never overwritten
/// from an absorbed subset row.
const NON_CARRIED_FIELDS: [&str; 10] = [
    "id",
    "facet",
    "stream",
    "segments",
    "settled_at",
    "created_at",
    "destination_id",
    "level_avg",
    "active_entities",
    "edits",
];

fn edit_stretch_id(edit: &Map<String, Value>) -> Option<&str> {
    edit.get(EDIT_STRETCH)?.get(STRETCH_ID)?.as_str()
}

fn edit_timestamp(edit: &Map<String, Value>) -> &str {
    edit.get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Carry forward owner-held field values and active owner edits from absorbed
/// strict-subset sibling rows onto `target`. Any field already held by the
/// owner on `target` keeps `target`'s value and edit precedence.
fn carry_forward_owner_edits(target: &mut ActivityRecord, absorbed: &[&ActivityRecord]) {
    let mut target_held = BTreeSet::new();
    for key in target.keys() {
        if !NON_CARRIED_FIELDS.contains(&key.as_str()) && activity_field_held(target, key) {
            target_held.insert(key.clone());
        }
    }
    if let Some(edits) = target.get("edits").and_then(Value::as_array) {
        for edit in edits.iter().filter_map(Value::as_object) {
            if owner_edit(edit) && !edit_removed(edit) {
                for field in edit
                    .get("fields")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !NON_CARRIED_FIELDS.contains(&field) {
                        target_held.insert(field.to_owned());
                    }
                }
            }
        }
    }

    let mut existing_edits = target
        .get("edits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen_stretches: BTreeSet<String> = existing_edits
        .iter()
        .filter_map(Value::as_object)
        .filter(|edit| !edit_removed(edit))
        .filter_map(edit_stretch_id)
        .map(str::to_owned)
        .collect();

    let mut pending_edits = Vec::new();
    for row in absorbed {
        let Some(edits) = row.get("edits").and_then(Value::as_array) else {
            continue;
        };
        let first_segment = segments_of_map(row).into_iter().next().unwrap_or_default();
        let id = row_id(row).to_owned();
        for (idx, edit) in edits.iter().enumerate() {
            if let Some(edit) = edit.as_object()
                && owner_edit(edit)
                && !edit_removed(edit)
            {
                pending_edits.push((
                    edit_timestamp(edit).to_owned(),
                    first_segment.clone(),
                    id.clone(),
                    idx,
                    row,
                    edit,
                ));
            }
        }
    }
    pending_edits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
    });

    let mut carried_before_owner = Vec::<Value>::new();
    let mut carried_after_existing = Vec::<Value>::new();
    let mut carried_fields = Vec::<(String, Option<Value>)>::new();

    for (_, _, _, _, row, edit) in pending_edits {
        if let Some(stretch_id) = edit_stretch_id(edit)
            && !seen_stretches.insert(stretch_id.to_owned())
        {
            continue;
        }
        let edit_value = Value::Object(edit.clone());
        if existing_edits.contains(&edit_value)
            || carried_before_owner.contains(&edit_value)
            || carried_after_existing.contains(&edit_value)
        {
            continue;
        }
        let overlaps_target_hold = target_held.iter().any(|field| edit_names(edit, field));
        for field in edit
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !NON_CARRIED_FIELDS.contains(&field)
                && !target_held.contains(field)
                && activity_field_held(row, field)
            {
                carried_fields.push((field.to_owned(), row.get(field).cloned()));
            }
        }
        if overlaps_target_hold {
            carried_before_owner.push(edit_value);
        } else {
            carried_after_existing.push(edit_value);
        }
    }

    for (field, value) in carried_fields {
        if let Some(value) = value {
            target.insert(field, value);
        }
    }

    if !carried_before_owner.is_empty() || !carried_after_existing.is_empty() {
        let insert_at = existing_edits
            .iter()
            .position(|edit| edit.as_object().is_some_and(owner_edit))
            .unwrap_or(existing_edits.len());
        if !carried_before_owner.is_empty() {
            existing_edits.splice(insert_at..insert_at, carried_before_owner);
        }
        existing_edits.extend(carried_after_existing);
        target.insert("edits".to_owned(), Value::Array(existing_edits));
    }
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
    if stream == solstone_core_format::content::strava_body::STRAVA_STREAM {
        return Ok(Published::Refused);
    }
    let Some(mut record) = record.as_object().cloned() else {
        return Ok(Published::Refused);
    };
    let facet = record
        .get("facet")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let initial_id = row_id(&record).to_owned();
    if stream != DEFAULT_STREAM {
        record.insert("stream".to_owned(), Value::String(stream.to_owned()));
    }
    record.insert("settled_at".to_owned(), Value::from(context.now_ms));

    match rederive(&context.journal, day, &record) {
        Rederived::Unchanged => {}
        Rederived::Changed(next) => record = next,
        Rederived::Removed => return Ok(Published::Refused),
    }
    let target_segments = segments_of_map(&record);
    if target_segments.is_empty() {
        return Ok(Published::Refused);
    }

    let (published, absorbed_ids) =
        publish_or_grow_and_fold(context, &facet, day, stream, &mut record, &target_segments)?;
    if matches!(published, Published::Refused) {
        return Ok(published);
    }
    let id = row_id(&record).to_owned();
    let id = if id.is_empty() { initial_id } else { id };

    if !absorbed_ids.is_empty() {
        cleanup_absorbed(context, &facet, day, &absorbed_ids);
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

/// Write or grow a settled activity and atomically retire any machine-derived
/// settled strict-subset sibling rows on `(facet, day, stream)` through
/// [`remake_activity_day`], carrying forward owner-held fields and edits.
fn publish_or_grow_and_fold(
    context: &ThinkContext,
    facet: &str,
    day: &str,
    stream: &str,
    record: &mut ActivityRecord,
    target_segments: &BTreeSet<String>,
) -> Result<(Published, Vec<String>), String> {
    let initial_id = row_id(record).to_owned();
    context.checkpoint()?;
    let facet_id = admit_activity_destination(&context.journal, facet)
        .map_err(|error| format!("{facet}/{initial_id}: {error}"))?;
    let timestamp = solstone_core_time::utc_rfc3339(
        Timestamp::from_millisecond(context.now_ms).unwrap_or_else(|_| Timestamp::now()),
    );

    let fold_day = |record: &ActivityRecord| {
        remake_activity_day(&context.journal, facet, day, |rows| {
            if let Err(error) = context.checkpoint() {
                return (None, Err(error));
            }
            let Some(activity) = record.get("activity").and_then(Value::as_str) else {
                return (None, Ok(FoldOutcome::Refused));
            };
            let mut target_id = row_id(record).to_owned();
            if let Some(existing) = rows.iter().find(|row| {
                !is_retired_plan(row) && !is_body_record(row) && row_id(row) == target_id
            }) {
                let held_by_other_stream = existing
                    .get("stream")
                    .and_then(Value::as_str)
                    .is_some_and(|held| held != stream)
                    || (existing.contains_key("settled_at") && exact_stream(existing) != stream);
                if held_by_other_stream
                    && safe_stream(stream)
                    && let Some(since) = target_id.strip_prefix(&format!("{activity}_"))
                {
                    target_id = format!("{activity}_{stream}_{since}");
                }
            }
            let blocked_by_unsettled = rows.iter().any(|row| {
                !is_retired_plan(row)
                    && !is_body_record(row)
                    && !row.contains_key("settled_at")
                    && same_stream(row, stream)
                    && !segments_of_map(row).is_disjoint(target_segments)
            });
            if blocked_by_unsettled {
                return (None, Ok(FoldOutcome::Refused));
            }
            let existing_idx = rows.iter().position(|row| {
                !is_retired_plan(row) && !is_body_record(row) && row_id(row) == target_id
            });
            let absorbed_indices: BTreeSet<usize> = rows
                .iter()
                .enumerate()
                .filter(|(idx, row)| {
                    Some(*idx) != existing_idx
                        && is_absorbable_subset(row, &target_id, stream, target_segments)
                })
                .map(|(idx, _)| idx)
                .collect();
            let absorbed_rows: Vec<&ActivityRecord> =
                absorbed_indices.iter().map(|idx| &rows[*idx]).collect();
            let absorbed_ids: Vec<String> = absorbed_rows
                .iter()
                .map(|row| row_id(row).to_owned())
                .collect();

            if let Some(idx) = existing_idx {
                let held = &rows[idx];
                let held_segments = segments_of_map(held);
                if !held.contains_key("settled_at")
                    || exact_stream(held) != stream
                    || has_non_null_source(held)
                    || is_body_record(held)
                    || is_strava_reading(held)
                    || !held_segments.is_subset(target_segments)
                    || (held_segments == *target_segments && absorbed_indices.is_empty())
                {
                    return (None, Ok(FoldOutcome::Refused));
                }
                let mut updated = held.clone();
                if !absorbed_rows.is_empty() {
                    carry_forward_owner_edits(&mut updated, &absorbed_rows);
                }
                if held_segments != *target_segments {
                    let mut patch_fields = Vec::new();
                    for field in ["segments", "level_avg", "active_entities"] {
                        if let Some(value) = record.get(field) {
                            updated.insert(field.to_owned(), value.clone());
                            patch_fields.push(field.to_owned());
                        }
                    }
                    if !activity_field_held(&updated, "description")
                        && let Some(description) = record.get("description")
                        && updated.get("description") != Some(description)
                    {
                        updated.insert("description".to_owned(), description.clone());
                        patch_fields.push("description".to_owned());
                    }
                    updated.insert("destination_id".to_owned(), Value::String(facet_id.clone()));
                    updated = append_edit(
                        updated,
                        "activity",
                        patch_fields,
                        "grew with later evidence",
                        &timestamp,
                    );
                }
                let mut next = Vec::with_capacity(rows.len() - absorbed_indices.len());
                for (i, row) in rows.iter().enumerate() {
                    if absorbed_indices.contains(&i) {
                        continue;
                    }
                    if i == idx {
                        next.push(updated.clone());
                    } else {
                        next.push(row.clone());
                    }
                }
                (
                    Some(next),
                    Ok(FoldOutcome::Grown {
                        record: updated,
                        absorbed_ids,
                    }),
                )
            } else if absorbed_indices.is_empty() {
                // Plain append: leave existing day-file lines untouched byte-for-byte.
                (None, Ok(FoldOutcome::AppendOnly { target_id }))
            } else {
                let mut new_row = record.clone();
                new_row.insert("id".to_owned(), Value::String(target_id));
                new_row.insert("destination_id".to_owned(), Value::String(facet_id.clone()));
                carry_forward_owner_edits(&mut new_row, &absorbed_rows);
                let mut next = Vec::with_capacity(rows.len() - absorbed_indices.len() + 1);
                for (i, row) in rows.iter().enumerate() {
                    if !absorbed_indices.contains(&i) {
                        next.push(row.clone());
                    }
                }
                next.push(new_row.clone());
                (
                    Some(next),
                    Ok(FoldOutcome::Written {
                        record: new_row,
                        absorbed_ids,
                    }),
                )
            }
        })
        .map_err(|error| format!("{facet}/{initial_id}: {error}"))
    };

    let apply_append = |record: &mut ActivityRecord| -> Result<(Published, Vec<String>), String> {
        match append_own_activity(context, facet, day, record)
            .map_err(|error| format!("{facet}/{initial_id}: {error}"))?
        {
            OwnAppend::Written => Ok((Published::Written, Vec::new())),
            OwnAppend::Skipped => Ok((Published::Refused, Vec::new())),
            OwnAppend::AlreadyExists => match fold_day(record)? {
                Some(outcome) => match outcome? {
                    FoldOutcome::Written {
                        record: written,
                        absorbed_ids,
                    } => {
                        *record = written;
                        Ok((Published::Written, absorbed_ids))
                    }
                    FoldOutcome::Grown {
                        record: grown,
                        absorbed_ids,
                    } => {
                        *record = grown;
                        Ok((Published::Grown, absorbed_ids))
                    }
                    FoldOutcome::AppendOnly { .. } | FoldOutcome::Refused => {
                        Ok((Published::Refused, Vec::new()))
                    }
                },
                None => Ok((Published::Refused, Vec::new())),
            },
        }
    };

    match fold_day(record)? {
        Some(outcome) => match outcome? {
            FoldOutcome::AppendOnly { target_id } => {
                record.insert("id".to_owned(), Value::String(target_id));
                apply_append(record)
            }
            FoldOutcome::Written {
                record: written,
                absorbed_ids,
            } => {
                *record = written;
                Ok((Published::Written, absorbed_ids))
            }
            FoldOutcome::Grown {
                record: grown,
                absorbed_ids,
            } => {
                *record = grown;
                Ok((Published::Grown, absorbed_ids))
            }
            FoldOutcome::Refused => Ok((Published::Refused, Vec::new())),
        },
        None => apply_append(record),
    }
}

/// Remove per-activity output folders, pending activity-work entries, and
/// provenance receipts for absorbed strict-subset rows, and enqueue modified
/// paths for search reindexing.
fn cleanup_absorbed(context: &ThinkContext, facet: &str, day: &str, absorbed_ids: &[String]) {
    let day_dir = context
        .journal
        .join("facets")
        .join(facet)
        .join("activities")
        .join(day);
    for absorbed_id in absorbed_ids {
        let outputs = day_dir.join(absorbed_id);
        if let Err(error) = std::fs::remove_dir_all(&outputs)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("settle: absorbed outputs {facet}/{absorbed_id} not removed: {error}");
        }
        if let Err(error) = context.index.enqueue_path(&context.journal, &outputs) {
            log::warn!(
                "settle: search update for absorbed outputs {facet}/{absorbed_id} not queued: {error}"
            );
        }
        crate::activity_work::discard_retired(context, day, facet, absorbed_id);
    }
    let day_jsonl = context
        .journal
        .join("facets")
        .join(facet)
        .join("activities")
        .join(format!("{day}.jsonl"));
    if let Err(error) = context.index.enqueue_path(&context.journal, &day_jsonl) {
        log::warn!("settle: search update for {facet}/{day}.jsonl not queued: {error}");
    }
}
