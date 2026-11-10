// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strava records an earlier reading of a workout's tiles filed become the
//! workout's code record.
//!
//! The one rule that keeps a stop safe: a workout's old lines are the last thing
//! removed. Whatever stops part way leaves a line to find, and whatever finds it
//! next finishes the job from disk. Owner fields are written onto the code
//! record before any line goes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::{Map, Value, json};
use solstone_core_facets::{
    BodyMoveOutcome, BodyRecordPlace, LineRemoval, StravaConvertedLine, activity_value_truthy,
    append_strava_converted, complete_body_move, move_body_record, open_body_moves,
    remove_activity_line_if, require_declared_facet, write_body_record_with,
};
use solstone_core_format::content::body_records::{BodyRow, RowKind, parse_source_time};
use solstone_core_format::content::strava_body::{STRAVA_SOURCE, STRAVA_STREAM, STRAVA_TILE_FILE};
use solstone_core_import::strava_refile::{add_strava_refiles, remove_strava_refiles};
use solstone_core_import_sources::apple_health::facet_file_text;
use solstone_core_import_sources::apple_health_run::sync_folder;
use solstone_core_import_sources::archive::hold_lock_if_no_merge_running;
use solstone_core_import_sources::body_stream::WindowFile;
use solstone_core_import_sources::strava::{
    clear_strava_converted_marker, misread_tile_keys, strava_conversion_epoch,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockOptions, atomic_replace, hold_lock,
};

use crate::body_activity::{
    TOMBSTONE, account_key, day_windows, record_for, record_id, strava_cut, window_facet,
    window_rows,
};
use crate::strava_decision::{
    RECORD_VISIBLE, SenseLine, choose_primary, decide, owner_hide, record_digest,
};
use crate::strava_filing::LiveWorkouts;

const STRAVA_LOCK: &str = "imports/.strava";
const MARKER: &str = "imports/.strava-converted";
const ACTIVITY_STATE: &str = "awareness/activity_state/import.strava.json";
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };
/// A component a line of which changes this many times waits for the next pass.
const ATTEMPTS: usize = 3;

#[cfg(test)]
thread_local! {
    /// Stop after this many durable writes, for a test of every stop point.
    pub(crate) static STOP_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    pub(crate) static WRITES_SEEN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Runs once, just before a pass removes its lines.
    static BEFORE_REMOVING: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// Runs once, when the marker's last look begins.
    static BEFORE_MARKER_SCAN: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
    /// Runs once, between a sweep's walk and taking the lock for its first component.
    static BEFORE_LOCK: std::cell::Cell<Option<fn(&Path)>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn stop_point() -> Result<(), String> {
    let seen = WRITES_SEEN.get() + 1;
    WRITES_SEEN.set(seen);
    if STOP_AFTER.get() == Some(seen) {
        return Err("stopped".to_owned());
    }
    Ok(())
}

#[cfg(not(test))]
#[allow(clippy::unnecessary_wraps)]
fn stop_point() -> Result<(), String> {
    Ok(())
}

/// What a pass over the journal did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ConversionCounts {
    pub converted: u64,
    /// Components that couldn't finish: an unreadable file or no facet to file under.
    pub waiting: u64,
    /// Components left because a Strava import held the lock.
    pub skipped_locked: u64,
    /// Lines naming no live workout, which stay as they are.
    pub no_workout: u64,
    /// Moves that couldn't be finished.
    pub moves_open: u64,
    /// Lines already folded into a code record that couldn't be removed.
    pub lines_left: u64,
}

/// Why a component didn't finish, and how many of its lines are left over.
struct Unfinished {
    reason: String,
    lines_left: u64,
}

impl From<String> for Unfinished {
    fn from(reason: String) -> Self {
        Self {
            reason,
            lines_left: 0,
        }
    }
}

/// A workout's tile, and the facet it's filed under.
#[derive(Debug, Clone)]
struct Tile {
    day: String,
    key: String,
    index: u64,
    facet: Option<String>,
}

/// How surely a key belongs to a workout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Misplaced,
    Cut,
    Live,
}

#[derive(Debug, Clone)]
struct Workout {
    activity: String,
    row: BodyRow,
    tiles: Vec<Tile>,
    keys: BTreeMap<(String, String), Rank>,
}

impl Workout {
    fn account(&self) -> String {
        account_key(STRAVA_STREAM, &self.activity)
    }

    /// Tile 0 when it's live, otherwise the lowest live tile.
    fn first_tile(&self) -> Option<&Tile> {
        self.tiles.iter().min_by_key(|tile| tile.index)
    }

    fn candidate_days(&self) -> BTreeSet<String> {
        self.keys.keys().map(|(day, _)| day.clone()).collect()
    }
}

/// How the pass files what it converts.
#[derive(Clone, Copy)]
pub(crate) struct ConversionScope<'a> {
    /// Inside a Strava import where the owner chose a facet: that facet, for
    /// the workouts in the download.
    pub chosen: Option<(&'a BTreeSet<String>, &'a str)>,
    /// A sweep publishes moved tiles itself; an import leaves them on the
    /// refile list for its own publish slice.
    pub publishes_tiles: bool,
}

fn now_text(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Every Strava-stream record in every day file, and how many files couldn't
/// be read. One unreadable file doesn't stop the others.
fn all_sense_lines(journal: &Path) -> (Vec<SenseLine>, u64) {
    let mut lines = Vec::new();
    let unreadable = each_day_file(journal, |day, facet, bytes| {
        lines.extend(sense_lines_in(bytes, day, facet));
    });
    lines.sort_by_key(SenseLine::key);
    (lines, unreadable)
}

/// Run `visit` over every activity day file with its day and facet, and say how
/// many files couldn't be read.
fn each_day_file(journal: &Path, mut visit: impl FnMut(&str, &str, &[u8])) -> u64 {
    let mut unreadable = 0;
    let Ok(facets) = fs::read_dir(journal.join("facets")) else {
        return unreadable;
    };
    for facet in facets.flatten() {
        if !facet.path().is_dir() {
            continue;
        }
        let name = facet.file_name().to_string_lossy().into_owned();
        let Ok(days) = fs::read_dir(facet.path().join("activities")) else {
            continue;
        };
        for entry in days.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(day) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            match fs::read(&path) {
                Ok(bytes) => visit(day, &name, &bytes),
                Err(_) => unreadable += 1,
            }
        }
    }
    unreadable
}

/// Every line key a code record says it holds, with the digest the line had
/// when it was folded in.
fn folded_digests(journal: &Path) -> BTreeMap<String, String> {
    const CONVERTED: &[u8] = b"\"converted\"";
    let mut folded = BTreeMap::new();
    each_day_file(journal, |_, _, bytes| {
        for line in bytes.split(|&byte| byte == b'\n') {
            if !line.windows(CONVERTED.len()).any(|at| at == CONVERTED) {
                continue;
            }
            let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if !solstone_core_facets::is_body_record(&record) {
                continue;
            }
            let held = record
                .get("converted")
                .and_then(|converted| converted.get("lines"))
                .and_then(Value::as_object);
            for (key, entry) in held.into_iter().flatten() {
                let digest = entry.get("sha256").and_then(Value::as_str);
                folded.insert(key.clone(), digest.unwrap_or_default().to_owned());
            }
        }
    });
    folded
}

/// Lines a code record holds that no live workout names any more, because the
/// workout was deleted after its record was written, each with its digest.
fn folded_orphans<'a>(
    journal: &Path,
    lines: &'a [SenseLine],
    parts: &[Component],
) -> Vec<(&'a SenseLine, String)> {
    let claimed = parts
        .iter()
        .flat_map(|part| part.lines.iter().copied())
        .collect::<BTreeSet<_>>();
    if claimed.len() == lines.len() {
        return Vec::new();
    }
    let folded = folded_digests(journal);
    lines
        .iter()
        .enumerate()
        .filter(|(at, _)| !claimed.contains(at))
        .filter_map(|(_, line)| {
            let digest = folded.get(&line.key())?;
            Some((line, digest.clone()))
        })
        .collect()
}

/// The last step of a conversion, for a line whose workout has gone. Only a
/// line that still says what was folded in goes; one that's changed stays
/// hidden and waits.
fn remove_folded_orphan(journal: &Path, line: &SenseLine, folded: &str) -> Result<(), String> {
    let Some(current) = reload(journal, std::slice::from_ref(line))?.pop().flatten() else {
        return Ok(());
    };
    if current.digest() != folded {
        return Err("it has changed since it was folded in".to_owned());
    }
    remove_outputs(journal, &current)?;
    let place = BodyRecordPlace {
        day: current.day.clone(),
        facet: current.facet.clone(),
    };
    match remove_activity_line_if(journal, &place, &current.id, |record| {
        record_digest(record) == folded
    })
    .map_err(|error| error.to_string())?
    {
        LineRemoval::Changed => Err("it changed as it was being removed".to_owned()),
        _ => Ok(()),
    }
}

/// Retry each orphan; `held` is the Strava lock.
fn remove_folded_orphans(
    journal: &Path,
    orphans: &[(&SenseLine, String)],
    _held: &FileLock,
    counts: &mut ConversionCounts,
) {
    for (line, folded) in orphans {
        if let Err(error) = remove_folded_orphan(journal, line, folded) {
            log::warn!("Strava conversion: a folded line couldn't be removed: {error}");
            counts.waiting += 1;
            counts.lines_left += 1;
        }
    }
}

fn sense_lines_in(bytes: &[u8], day: &str, facet: &str) -> Vec<SenseLine> {
    bytes
        .split(|&byte| byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|value| match value {
            Value::Object(record) => Some(record),
            _ => None,
        })
        .filter(|record| {
            record.get("stream").and_then(Value::as_str) == Some(STRAVA_STREAM)
                && !solstone_core_facets::is_body_record(record)
        })
        .filter_map(|record| {
            Some(SenseLine {
                day: day.to_owned(),
                facet: facet.to_owned(),
                id: record.get("id")?.as_str()?.to_owned(),
                record,
            })
        })
        .collect()
}

/// Every workout with a live tile, with the keys it can go by, and how many
/// folders couldn't be read. A deleted tile isn't one of those.
fn live_workouts(journal: &Path) -> (Vec<Workout>, u64) {
    let mut workouts: BTreeMap<String, Workout> = BTreeMap::new();
    let mut unreadable = 0;
    let days = match fs::read_dir(journal.join("chronicle")) {
        Ok(days) => days,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), 0);
        }
        Err(_) => return (Vec::new(), 1),
    };
    let mut days = days
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    days.sort();
    for day in days {
        let Some(windows) = day_windows(journal, &day, STRAVA_STREAM) else {
            if journal
                .join("chronicle")
                .join(&day)
                .join(STRAVA_STREAM)
                .exists()
            {
                unreadable += 1;
            }
            continue;
        };
        for window in windows {
            let Some(key) = window
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            let Some(row) = window_rows(&window, WindowFile::StravaTile)
                .and_then(|rows| rows.into_iter().next())
            else {
                if !window.join(TOMBSTONE).exists() && window.join(STRAVA_TILE_FILE).exists() {
                    unreadable += 1;
                }
                continue;
            };
            let activity = row.of.clone().unwrap_or_else(|| row.id.clone());
            let index = if row.kind == RowKind::Experience {
                0
            } else {
                row.index.unwrap_or(u64::MAX)
            };
            let workout = workouts.entry(activity.clone()).or_insert_with(|| Workout {
                activity,
                row: row.clone(),
                tiles: Vec::new(),
                keys: BTreeMap::new(),
            });
            workout.keys.insert((day.clone(), key.clone()), Rank::Live);
            workout.tiles.push(Tile {
                day: day.clone(),
                key,
                index,
                facet: window_facet(&window),
            });
        }
    }
    let zone = solstone_core_journal_config::placed_zone(journal, STRAVA_SOURCE);
    let workouts = workouts
        .into_values()
        .map(|mut workout| {
            let span = workout
                .row
                .start
                .as_deref()
                .and_then(parse_source_time)
                .zip(workout.row.end.as_deref().and_then(parse_source_time));
            if let Some((start, end)) = span {
                for at in strava_cut(start, end, zone) {
                    workout.keys.entry(at).or_insert(Rank::Cut);
                }
                if let Some(zone) = zone {
                    add_misplaced(&mut workout, start, end, zone);
                }
            }
            workout
        })
        .collect();
    (workouts, unreadable)
}

/// Read a component's workouts again, now its lock is held: a delete may have
/// come since the walk read them. False when a workout has no tile left, which
/// leaves the whole component for the next pass to work out afresh.
fn refresh_workouts(journal: &Path, workouts: &mut [Workout], component: &Component) -> bool {
    for at in &component.workouts {
        let workout = &mut workouts[*at];
        workout.tiles.retain_mut(|tile| {
            let window = journal
                .join("chronicle")
                .join(&tile.day)
                .join(STRAVA_STREAM)
                .join(&tile.key);
            tile.facet = window_facet(&window);
            window_rows(&window, WindowFile::StravaTile).is_some()
        });
        if workout.tiles.is_empty() {
            return false;
        }
    }
    true
}

fn add_misplaced(
    workout: &mut Workout,
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    zone: chrono_tz::Tz,
) {
    let elapsed = u64::try_from((end - start).num_seconds()).unwrap_or(1);
    for at in misread_tile_keys(start, elapsed, zone) {
        workout.keys.entry(at).or_insert(Rank::Misplaced);
    }
}

/// The workouts a line's segments name, only the surest kind of match.
fn named_workouts(
    line: &SenseLine,
    index: &BTreeMap<(String, String), Vec<(usize, Rank)>>,
) -> Vec<usize> {
    let mut found = BTreeMap::new();
    for segment in line
        .record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        for (workout, rank) in index
            .get(&(line.day.clone(), segment.to_owned()))
            .into_iter()
            .flatten()
        {
            let best = found.entry(*workout).or_insert(*rank);
            *best = (*best).max(*rank);
        }
    }
    let Some(top) = found.values().copied().max() else {
        return Vec::new();
    };
    found
        .into_iter()
        .filter(|(_, rank)| *rank == top)
        .map(|(workout, _)| workout)
        .collect()
}

/// One connected part of the graph of workouts and the lines naming them.
struct Component {
    workouts: Vec<usize>,
    lines: Vec<usize>,
    names: BTreeMap<usize, Vec<usize>>,
}

fn components(workouts: &[Workout], lines: &[SenseLine]) -> (Vec<Component>, u64) {
    let mut index: BTreeMap<(String, String), Vec<(usize, Rank)>> = BTreeMap::new();
    for (at, workout) in workouts.iter().enumerate() {
        for (key, rank) in &workout.keys {
            index.entry(key.clone()).or_default().push((at, *rank));
        }
    }
    let mut parent = (0..workouts.len()).collect::<Vec<_>>();
    fn find(parent: &mut [usize], at: usize) -> usize {
        if parent[at] != at {
            let root = find(parent, parent[at]);
            parent[at] = root;
        }
        parent[at]
    }
    let mut names = BTreeMap::new();
    let mut no_workout = 0;
    for (at, line) in lines.iter().enumerate() {
        let named = named_workouts(line, &index);
        if named.is_empty() {
            no_workout += 1;
            continue;
        }
        for pair in named.windows(2) {
            let (left, right) = (find(&mut parent, pair[0]), find(&mut parent, pair[1]));
            parent[left] = right;
        }
        names.insert(at, named);
    }
    let mut grouped: BTreeMap<usize, (BTreeSet<usize>, Vec<usize>)> = BTreeMap::new();
    for (line, named) in &names {
        let root = find(&mut parent, named[0]);
        let entry = grouped.entry(root).or_default();
        entry.0.extend(named.iter().copied());
        entry.1.push(*line);
    }
    let parts = grouped
        .into_values()
        .map(|(workouts, lines)| Component {
            workouts: workouts.into_iter().collect(),
            names: lines
                .iter()
                .map(|line| (*line, names[line].clone()))
                .collect(),
            lines,
        })
        .collect();
    (parts, no_workout)
}

type CodeRecord = (BodyRecordPlace, Map<String, Value>);

/// A code record for this account wherever it's filed on these days.
fn find_code_record(
    journal: &Path,
    days: &BTreeSet<String>,
    key: &str,
) -> Result<Option<CodeRecord>, String> {
    let Ok(facets) = fs::read_dir(journal.join("facets")) else {
        return Ok(None);
    };
    let mut facet_names = facets
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    facet_names.sort();
    for day in days {
        for facet in &facet_names {
            let path = journal
                .join("facets")
                .join(facet)
                .join("activities")
                .join(format!("{day}.jsonl"));
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
            };
            let found = bytes
                .split(|&byte| byte == b'\n')
                .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
                .filter_map(|value| match value {
                    Value::Object(record) => Some(record),
                    _ => None,
                })
                .find(|record| {
                    solstone_core_facets::is_body_record(record)
                        && solstone_core_facets::body_record_key(record) == Some(key)
                });
            if let Some(record) = found {
                return Ok(Some((
                    BodyRecordPlace {
                        day: day.clone(),
                        facet: facet.clone(),
                    },
                    record,
                )));
            }
        }
    }
    Ok(None)
}

fn facet_declared(journal: &Path, facet: &str) -> bool {
    require_declared_facet(journal, facet).is_ok()
}

/// The facet a workout's first conversion files it under.
fn target_facet(journal: &Path, workout: &Workout, lines: &[&SenseLine]) -> Option<String> {
    let first_facet = workout.first_tile().and_then(|tile| tile.facet.clone());
    let visible =
        |line: &&&SenseLine| owner_hide(line).is_none() && facet_declared(journal, &line.facet);
    if let Some(facet) = &first_facet
        && lines
            .iter()
            .filter(visible)
            .any(|line| line.facet == *facet)
    {
        return Some(facet.clone());
    }
    if let Some(line) = lines.iter().find(visible) {
        return Some(line.facet.clone());
    }
    if let Some(facet) = first_facet.filter(|facet| facet_declared(journal, facet)) {
        return Some(facet);
    }
    lines
        .iter()
        .filter(|line| facet_declared(journal, &line.facet))
        .min_by_key(|line| {
            (
                line.record
                    .get("created_at")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX),
                line.key(),
            )
        })
        .map(|line| line.facet.clone())
}

/// A Sense id's last two parts name the segment it started on.
fn since_of(id: &str) -> Option<String> {
    let mut parts = id.rsplitn(3, '_');
    let length = parts.next()?;
    let clock = parts.next()?;
    Some(format!("{clock}_{length}"))
}

/// What a merge does once it has finished: a pass that began while it ran
/// mustn't write the marker.
pub(crate) fn mark_conversion_needed(journal: &Path) -> Result<(), String> {
    clear_strava_converted_marker(journal)
}

fn queue_tile(journal: &Path, tile: &(String, String)) {
    let rel = format!(
        "chronicle/{}/{STRAVA_STREAM}/{}/{STRAVA_TILE_FILE}",
        tile.0, tile.1
    );
    if let Err(error) = solstone_core_indexer_store::todo::enqueue_path(journal, Path::new(&rel)) {
        log::warn!("Strava conversion: a moved workout wasn't queued for search: {error}");
    }
}

/// Files the tiles under the target, through the refile list.
fn move_tiles(
    journal: &Path,
    tiles: &[&Tile],
    facet: &str,
    held: &FileLock,
    scope: ConversionScope<'_>,
) -> Result<(), String> {
    let changing = tiles
        .iter()
        .filter(|tile| tile.facet.as_deref() != Some(facet))
        .map(|tile| (tile.day.clone(), tile.key.clone()))
        .collect::<Vec<_>>();
    if changing.is_empty() {
        return Ok(());
    }
    add_strava_refiles(journal, &changing, held)?;
    for (day, key) in &changing {
        let dir = journal
            .join("chronicle")
            .join(day)
            .join(STRAVA_STREAM)
            .join(key);
        fs::create_dir_all(dir.join("talents")).map_err(|error| error.to_string())?;
        atomic_replace(
            dir.join("talents/facets.json"),
            facet_file_text(facet).as_bytes(),
            PRIVATE,
        )
        .map_err(|error| error.to_string())?;
        sync_folder(&dir)?;
    }
    if scope.publishes_tiles {
        for tile in &changing {
            queue_tile(journal, tile);
        }
        remove_strava_refiles(journal, &changing.iter().cloned().collect(), held)?;
    }
    Ok(())
}

/// Remove the talent outputs a line's record held, and queue their paths so
/// search drops them.
fn remove_outputs(journal: &Path, line: &SenseLine) -> Result<(), String> {
    let rel = format!("facets/{}/activities/{}/{}", line.facet, line.day, line.id);
    let dir = journal.join(&rel);
    let mut files = Vec::new();
    collect_files(&dir, &rel, &mut files);
    #[expect(
        clippy::disallowed_methods,
        reason = "system-internal: the journal replacing its own model output about a workout, never an owner's removal"
    )]
    solstone_core_journal_io::removal::remove_dir_all(journal, &rel)
        .map_err(|error| error.to_string())?;
    for file in files {
        if let Err(error) =
            solstone_core_indexer_store::todo::enqueue_path(journal, Path::new(&file))
        {
            log::warn!("Strava conversion: a removed output wasn't queued for search: {error}");
        }
    }
    Ok(())
}

fn collect_files(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = format!("{rel}/{name}");
        if entry.path().is_dir() {
            collect_files(&entry.path(), &child, out);
        } else {
            out.push(child);
        }
    }
}

fn remove_activity_state(journal: &Path) -> Result<(), String> {
    match fs::remove_file(journal.join(ACTIVITY_STATE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// The lines of a component as they stand now.
fn reload(journal: &Path, wanted: &[SenseLine]) -> Result<Vec<Option<SenseLine>>, String> {
    let mut files: BTreeMap<(String, String), Vec<SenseLine>> = BTreeMap::new();
    for line in wanted {
        let key = (line.facet.clone(), line.day.clone());
        if !files.contains_key(&key) {
            let path = journal
                .join("facets")
                .join(&line.facet)
                .join("activities")
                .join(format!("{}.jsonl", line.day));
            let found = match fs::read(&path) {
                Ok(bytes) => sense_lines_in(&bytes, &line.day, &line.facet),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.to_string()),
            };
            files.insert(key.clone(), found);
        }
    }
    Ok(wanted
        .iter()
        .map(|line| {
            files[&(line.facet.clone(), line.day.clone())]
                .iter()
                .find(|found| found.id == line.id)
                .cloned()
        })
        .collect())
}

/// Finish any open move of this account. A move that can't finish stops this
/// workout, so nothing writes the key meanwhile.
fn finish_open_move(journal: &Path, key: &str) -> Result<(), String> {
    for open in open_body_moves(journal).map_err(|error| error.to_string())? {
        if open.key == key {
            complete_body_move(journal, &open).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Fold one workout's lines into its code record, returning where it's filed.
fn write_workout(
    journal: &Path,
    workout: &Workout,
    named: &[&SenseLine],
    spanned: &dyn Fn(&SenseLine) -> bool,
    scope: ConversionScope<'_>,
    now: &str,
) -> Result<BodyRecordPlace, String> {
    let key = account_key(STRAVA_STREAM, &workout.activity);
    finish_open_move(journal, &key)?;
    let first = workout.first_tile().ok_or("a live workout has no tile")?;
    let mut days = workout.candidate_days();
    days.insert(first.day.clone());
    let existing = find_code_record(journal, &days, &key)?;
    let chosen = scope
        .chosen
        .filter(|(download, _)| download.contains(&workout.activity))
        .map(|(_, facet)| facet.to_owned());
    let decided = existing
        .as_ref()
        .is_some_and(|(_, record)| record.get("converted").is_some());
    let place = match (&existing, chosen) {
        (_, Some(facet)) => BodyRecordPlace {
            day: first.day.clone(),
            facet,
        },
        (Some((place, _)), None) if decided => place.clone(),
        // A code record the owner could already see keeps its facet.
        (Some((place, _)), None) => BodyRecordPlace {
            day: first.day.clone(),
            facet: place.facet.clone(),
        },
        (None, None) => BodyRecordPlace {
            day: first.day.clone(),
            facet: target_facet(journal, workout, named)
                .ok_or("no declared facet to file this workout under")?,
        },
    };
    if let Some((from, _)) = &existing
        && *from != place
    {
        match move_body_record(journal, &record_id(&key), from, &place)
            .map_err(|error| error.to_string())?
        {
            BodyMoveOutcome::Moved | BodyMoveOutcome::AlreadyThere | BodyMoveOutcome::NotFound => {}
        }
    }
    let primary = choose_primary(
        named,
        &place.facet,
        Some((first.day.as_str(), first.key.as_str())),
    )
    .ok_or("a workout's component has no line for it")?;
    let arrived = now.get(..10).unwrap_or("").replace('-', "");
    let fresh = record_for(STRAVA_STREAM, &key, &workout.row, &arrived);
    write_body_record_with(journal, &place, &record_id(&key), |current| {
        let base = match current {
            Some(mut record) => {
                let visible = !record.get("hidden").is_some_and(activity_value_truthy);
                if visible && !record.contains_key("converted") {
                    record.insert("converted".to_owned(), json!({RECORD_VISIBLE: true}));
                }
                record
            }
            None => fresh,
        };
        decide(base, named, spanned, &primary, now)
    })
    .map_err(|error| error.to_string())?;
    Ok(place)
}

/// Run a component's steps. An error leaves it for the next pass: every line
/// not yet removed is still on disk to find.
fn convert_component(
    journal: &Path,
    workouts: &[Workout],
    all_lines: &[SenseLine],
    component: &Component,
    scope: ConversionScope<'_>,
    held: &FileLock,
    now: &str,
) -> Result<(), Unfinished> {
    clear_strava_converted_marker(journal)?;
    let listed = component
        .lines
        .iter()
        .map(|at| StravaConvertedLine {
            day: all_lines[*at].day.clone(),
            facet: all_lines[*at].facet.clone(),
            id: all_lines[*at].id.clone(),
        })
        .collect::<Vec<_>>();
    append_strava_converted(journal, &listed).map_err(|error| error.to_string())?;
    stop_point()?;
    let mut lines_left = 0;

    let wanted = component
        .lines
        .iter()
        .map(|at| all_lines[*at].clone())
        .collect::<Vec<_>>();
    for _ in 0..ATTEMPTS {
        let current = reload(journal, &wanted)?;
        let mut digests = Vec::new();
        for workout_at in &component.workouts {
            let workout = &workouts[*workout_at];
            let named = component
                .lines
                .iter()
                .zip(&current)
                .filter(|(line, _)| component.names[*line].contains(workout_at))
                .filter_map(|(_, found)| found.as_ref())
                .collect::<Vec<_>>();
            if named.is_empty() {
                continue;
            }
            // A line spanning workouts gives its fields to the one its start names.
            let line_names = |line: &SenseLine| {
                component
                    .lines
                    .iter()
                    .zip(&wanted)
                    .find(|(_, candidate)| candidate.key() == line.key())
                    .map(|(at, _)| component.names[at].clone())
                    .unwrap_or_default()
            };
            let spanned = |line: &SenseLine| {
                let names = line_names(line);
                if names.len() < 2 {
                    return true;
                }
                let since = since_of(&line.id).map(|since| (line.day.clone(), since));
                let owner = names
                    .iter()
                    .find(|named| {
                        since
                            .as_ref()
                            .is_some_and(|key| workouts[**named].keys.contains_key(key))
                    })
                    .or_else(|| names.first());
                owner == Some(workout_at)
            };
            let place = write_workout(journal, workout, &named, &spanned, scope, now)?;
            stop_point()?;
            let tiles = workout.tiles.iter().collect::<Vec<_>>();
            move_tiles(journal, &tiles, &place.facet, held, scope)?;
            stop_point()?;
        }
        lines_left = 0;
        for found in current.iter().flatten() {
            // One that won't go doesn't hold the others back.
            match remove_outputs(journal, found) {
                Ok(()) => digests.push((found.clone(), found.digest())),
                Err(error) => {
                    log::warn!("Strava conversion: outputs couldn't be removed: {error}");
                    lines_left += 1;
                }
            }
        }
        remove_activity_state(journal)?;
        stop_point()?;
        #[cfg(test)]
        if let Some(hook) = BEFORE_REMOVING.take() {
            hook(journal);
        }
        let mut changed = false;
        for (line, digest) in digests {
            let place = BodyRecordPlace {
                day: line.day.clone(),
                facet: line.facet.clone(),
            };
            match remove_activity_line_if(journal, &place, &line.id, |record| {
                record_digest(record) == digest
            }) {
                Ok(LineRemoval::Changed) => changed = true,
                Ok(_) => {}
                Err(error) => {
                    log::warn!("Strava conversion: a folded line couldn't be removed: {error}");
                    lines_left += 1;
                }
            }
            stop_point()?;
        }
        if !changed {
            if lines_left > 0 {
                return Err(Unfinished {
                    reason: "not every folded line could be removed".to_owned(),
                    lines_left,
                });
            }
            return Ok(());
        }
    }
    Err(Unfinished {
        reason: "a record kept changing while it was being updated".to_owned(),
        lines_left,
    })
}

fn tally(counts: &mut ConversionCounts, result: Result<(), Unfinished>) {
    match result {
        Ok(()) => counts.converted += 1,
        Err(unfinished) => {
            log::warn!(
                "Strava conversion: a workout's records couldn't be updated: {}",
                unfinished.reason
            );
            counts.waiting += 1;
            counts.lines_left += unfinished.lines_left;
        }
    }
}

/// Convert every component in the journal while `held` is held the whole time:
/// the writer and filing, inside a Strava import.
pub(crate) fn convert_held(
    journal: &Path,
    held: &FileLock,
    scope: ConversionScope<'_>,
    now: DateTime<Utc>,
) -> ConversionCounts {
    let mut counts = ConversionCounts::default();
    let (lines, unreadable) = all_sense_lines(journal);
    let (workouts, folders) = live_workouts(journal);
    counts.waiting += unreadable + folders;
    let (parts, no_workout) = components(&workouts, &lines);
    counts.no_workout = no_workout;
    let at = now_text(now);
    for part in &parts {
        let result = convert_component(journal, &workouts, &lines, part, scope, held, &at);
        tally(&mut counts, result);
    }
    let orphans = folded_orphans(journal, &lines, &parts);
    remove_folded_orphans(journal, &orphans, held, &mut counts);
    counts
}

/// How many workouts in a download, already listed with the owner's
/// activities, a choice of `facet` would move, and the titles of the facets
/// they'd move from. `None` when it moves none.
pub(crate) fn listed_moves(
    journal: &Path,
    download: &BTreeSet<String>,
    facet: &str,
) -> Option<(usize, Vec<String>)> {
    summed_moves(journal, &listed_moves_by_workout(journal, download, facet))
}

/// A workout a choice would move, and the facet it's in now.
pub(crate) type ListedMove = (String, String);

/// Of the workouts counted before a run, the ones whose record is now in
/// `facet`, summed as [`listed_moves`] sums them. A workout whose group waited
/// isn't one of them, and nor is one whose move was held back: a stopped move
/// can leave a copy in the facet that isn't its place yet.
pub(crate) fn listed_moved_now(
    journal: &Path,
    counted: &[ListedMove],
    facet: &str,
    held_back: &BTreeSet<String>,
) -> Option<(usize, Vec<String>)> {
    let workouts = live_workouts(journal)
        .0
        .into_iter()
        .map(|workout| (workout.activity.clone(), workout))
        .collect::<BTreeMap<_, _>>();
    let moved = counted
        .iter()
        .filter(|(activity, _)| !held_back.contains(&account_key(STRAVA_STREAM, activity)))
        .filter(|(activity, _)| {
            workouts.get(activity).is_some_and(|workout| {
                find_code_record(journal, &workout.candidate_days(), &workout.account())
                    .ok()
                    .flatten()
                    .is_some_and(|(place, _)| place.facet == facet)
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    summed_moves(journal, &moved)
}

fn summed_moves(journal: &Path, moves: &[ListedMove]) -> Option<(usize, Vec<String>)> {
    let from = moves
        .iter()
        .map(|(_, facet)| facet_title(journal, facet))
        .collect::<BTreeSet<_>>();
    (!moves.is_empty()).then(|| (moves.len(), from.into_iter().collect()))
}

/// Each workout [`listed_moves`] counts.
pub(crate) fn listed_moves_by_workout(
    journal: &Path,
    download: &BTreeSet<String>,
    facet: &str,
) -> Vec<ListedMove> {
    let (workouts, _) = live_workouts(journal);
    let (lines, _) = all_sense_lines(journal);
    let (parts, _) = components(&workouts, &lines);
    let mut named: BTreeMap<usize, Vec<&SenseLine>> = BTreeMap::new();
    for part in &parts {
        for (line, workouts) in &part.names {
            for workout in workouts {
                named.entry(*workout).or_default().push(&lines[*line]);
            }
        }
    }
    let mut moves = Vec::new();
    for (at, workout) in workouts.iter().enumerate() {
        if !download.contains(&workout.activity) {
            continue;
        }
        let code = find_code_record(journal, &workout.candidate_days(), &workout.account())
            .ok()
            .flatten();
        let current = match &code {
            Some((place, _)) => Some(place.facet.clone()),
            None => named
                .get(&at)
                .and_then(|lines| target_facet(journal, workout, lines)),
        };
        if let Some(current) = current
            && current != facet
        {
            moves.push((workout.activity.clone(), current));
        }
    }
    moves
}

pub(crate) fn facet_title(journal: &Path, facet: &str) -> String {
    solstone_core_facets::read_facet_declaration(journal, facet)
        .ok()
        .flatten()
        .and_then(|snapshot| {
            snapshot
                .value()
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| facet.to_owned())
}

/// Put each code record of the download's workouts where its workout now is:
/// on its first tile's day, and under the facet the owner chose when they
/// chose one. Returns the accounts it couldn't place, which the writer leaves
/// alone so no second record appears beside them.
pub(crate) fn relocate_download(
    journal: &Path,
    live: &LiveWorkouts,
    chosen: Option<&str>,
) -> BTreeSet<String> {
    relocate_with_days(journal, live, chosen, &BTreeSet::new())
}

/// [`relocate_download`], also looking on `extra_days`.
pub(crate) fn relocate_with_days(
    journal: &Path,
    live: &LiveWorkouts,
    chosen: Option<&str>,
    extra_days: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut held_back = BTreeSet::new();
    let workouts = live_workouts(journal)
        .0
        .into_iter()
        .map(|workout| (workout.activity.clone(), workout))
        .collect::<BTreeMap<_, _>>();
    for (activity, (first, tiles)) in live {
        let activity = activity.to_string();
        let key = account_key(STRAVA_STREAM, &activity);
        let Some(day) = tiles
            .iter()
            .min_by_key(|tile| tile.index)
            .map(|tile| tile.day.clone())
            .or_else(|| first.as_ref().map(|(day, _)| day.clone()))
        else {
            continue;
        };
        let mut days = workouts
            .get(&activity)
            .map(Workout::candidate_days)
            .unwrap_or_default();
        days.insert(day.clone());
        days.extend(extra_days.iter().cloned());
        let placed = finish_open_move(journal, &key).and_then(|()| {
            let Some((from, _)) = find_code_record(journal, &days, &key)? else {
                return Ok(());
            };
            let to = BodyRecordPlace {
                day,
                facet: chosen.map_or_else(|| from.facet.clone(), str::to_owned),
            };
            if from != to {
                move_body_record(journal, &record_id(&key), &from, &to)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });
        if let Err(error) = placed {
            log::warn!("Strava import: a workout's record couldn't be placed: {error}");
            held_back.insert(key);
        }
    }
    held_back
}

/// Finish moves stopped part way. A source held by a running import is left
/// for it.
fn finish_open_moves(journal: &Path, counts: &mut ConversionCounts) {
    let Ok(open) = open_body_moves(journal) else {
        counts.moves_open += 1;
        return;
    };
    for one in open {
        let stream = one.key.split('\u{1f}').next().unwrap_or_default();
        let source = stream.strip_prefix("import.").unwrap_or(stream);
        let lock = hold_lock(
            journal.join("imports").join(format!(".{source}")),
            LockOptions {
                timeout: std::time::Duration::from_millis(50),
                mode: Some(0o600),
                ..LockOptions::default()
            },
        );
        let Ok(_lock) = lock else {
            counts.moves_open += 1;
            continue;
        };
        if let Err(error) = complete_body_move(journal, &one) {
            log::warn!(
                "Body records: a move couldn't finish for {}: {error}",
                one.key
            );
            counts.moves_open += 1;
        }
    }
}

/// The start of a journal: finish moves, then convert component by component,
/// releasing the Strava lock between them so a delete or a card never waits on
/// the whole walk. Best effort: anything left waits for the next pass.
pub fn sweep_strava_conversions(journal: &Path) {
    let counts = sweep(journal, Utc::now());
    if counts != ConversionCounts::default() {
        log::info!("Strava conversion: {counts:?}");
    }
}

pub(crate) fn sweep(journal: &Path, now: DateTime<Utc>) -> ConversionCounts {
    let mut counts = ConversionCounts::default();
    finish_open_moves(journal, &mut counts);
    if journal.join(MARKER).exists() {
        return counts;
    }
    let (lines, unreadable) = all_sense_lines(journal);
    let (mut workouts, folders) = live_workouts(journal);
    counts.waiting += unreadable + folders;
    let (parts, no_workout) = components(&workouts, &lines);
    counts.no_workout = no_workout;
    let at = now_text(now);
    let scope = ConversionScope {
        chosen: None,
        publishes_tiles: true,
    };
    let lock_options = LockOptions {
        timeout: std::time::Duration::from_secs(1),
        mode: Some(0o600),
        ..LockOptions::default()
    };
    for part in &parts {
        #[cfg(test)]
        if let Some(hook) = BEFORE_LOCK.take() {
            hook(journal);
        }
        let Ok(held) = hold_lock(journal.join(STRAVA_LOCK), lock_options) else {
            counts.skipped_locked += 1;
            continue;
        };
        if !refresh_workouts(journal, &mut workouts, part) {
            counts.waiting += 1;
            continue;
        }
        let result = convert_component(journal, &workouts, &lines, part, scope, &held, &at);
        tally(&mut counts, result);
    }
    let orphans = folded_orphans(journal, &lines, &parts);
    if !orphans.is_empty() {
        match hold_lock(journal.join(STRAVA_LOCK), lock_options) {
            Ok(held) => remove_folded_orphans(journal, &orphans, &held, &mut counts),
            Err(_) => counts.skipped_locked += 1,
        }
    }
    write_marker_if_finished(journal, lock_options);
    counts
}

/// Say nothing is left to convert, if that's so now: no line a live workout
/// names, and no line a code record holds, live or not, because readers stop
/// hiding those once the marker is there. A merge running or a conversion
/// starting meanwhile leaves the marker for another pass.
fn write_marker_if_finished(journal: &Path, lock_options: LockOptions) {
    // Held through the look, so no merge can start and finish inside it.
    let Some(_no_merge) = hold_lock_if_no_merge_running(journal) else {
        return;
    };
    #[cfg(test)]
    if let Some(hook) = BEFORE_MARKER_SCAN.take() {
        hook(journal);
    }
    let epoch = strava_conversion_epoch(journal);
    let (lines, unreadable) = all_sense_lines(journal);
    let (workouts, folders) = live_workouts(journal);
    let (parts, _) = components(&workouts, &lines);
    if unreadable + folders == 0
        && parts.is_empty()
        && folded_orphans(journal, &lines, &parts).is_empty()
        && let Ok(_held) = hold_lock(journal.join(STRAVA_LOCK), lock_options)
        && strava_conversion_epoch(journal) == epoch
        && fs::create_dir_all(journal.join("imports")).is_ok()
    {
        let _ = atomic_replace(journal.join(MARKER), b"", PRIVATE);
    }
}

#[cfg(test)]
#[path = "strava_convert_tests.rs"]
mod tests;
