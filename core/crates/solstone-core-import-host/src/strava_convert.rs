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
use serde_json::{Map, Value};
use solstone_core_facets::{
    BodyMoveOutcome, BodyRecordPlace, LineRemoval, StravaConvertedLine, append_strava_converted,
    complete_body_move, move_body_record, open_body_moves, remove_activity_line_if,
    write_body_record_with,
};
use solstone_core_format::content::body_records::{BodyRow, RowKind, parse_source_time};
use solstone_core_format::content::strava_body::{STRAVA_SOURCE, STRAVA_STREAM, STRAVA_TILE_FILE};
use solstone_core_import::strava_refile::{add_strava_refiles, remove_strava_refiles};
use solstone_core_import_sources::apple_health::facet_file_text;
use solstone_core_import_sources::apple_health_run::sync_folder;
use solstone_core_import_sources::body_stream::WindowFile;
use solstone_core_import_sources::strava::misread_tile_keys;
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockOptions, atomic_replace, hold_lock,
};

use crate::body_activity::{
    account_key, day_windows, record_for, record_id, strava_cut, window_facet, window_rows,
};
use crate::strava_decision::{SenseLine, choose_primary, decide, owner_hide, record_digest};
use crate::strava_filing::LiveWorkouts;

const STRAVA_LOCK: &str = "imports/.strava";
const MARKER: &str = "imports/.strava-converted";
const EPOCH: &str = "imports/.strava-epoch";
const ACTIVITY_STATE: &str = "awareness/activity_state/import.strava.json";
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };
/// A component a line of which changes this many times waits for the next pass.
const ATTEMPTS: usize = 3;

#[cfg(test)]
thread_local! {
    /// Stop after this many durable writes, for a test of every stop point.
    static STOP_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static WRITES_SEEN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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
    let mut unreadable = 0;
    let Ok(facets) = fs::read_dir(journal.join("facets")) else {
        return (lines, unreadable);
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
                Ok(bytes) => lines.extend(sense_lines_in(&bytes, day, &name)),
                Err(_) => unreadable += 1,
            }
        }
    }
    lines.sort_by_key(SenseLine::key);
    (lines, unreadable)
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

/// Every workout with a live tile, with the keys it can go by.
fn live_workouts(journal: &Path) -> Vec<Workout> {
    let mut workouts: BTreeMap<String, Workout> = BTreeMap::new();
    let Ok(days) = fs::read_dir(journal.join("chronicle")) else {
        return Vec::new();
    };
    let mut days = days
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    days.sort();
    for day in days {
        for window in day_windows(journal, &day, STRAVA_STREAM).unwrap_or_default() {
            let Some(key) = window
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            let Some(row) = window_rows(&window, WindowFile::StravaTile)
                .and_then(|rows| rows.into_iter().next())
            else {
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
    workouts
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
        .collect()
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

/// A code record for this account wherever it's filed on these days.
fn find_code_record(
    journal: &Path,
    days: &BTreeSet<String>,
    key: &str,
) -> Result<Option<(BodyRecordPlace, Map<String, Value>)>, String> {
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
    journal
        .join("facets")
        .join(facet)
        .join("facet.json")
        .is_file()
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

/// Clear the skip marker and record that a conversion started.
fn clear_marker(journal: &Path) -> Result<(), String> {
    match fs::remove_file(journal.join(MARKER)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    if let Some(parent) = journal.join(MARKER).parent() {
        sync_folder(parent)?;
    }
    bump_epoch(journal)
}

fn read_epoch(journal: &Path) -> u64 {
    fs::read_to_string(journal.join(EPOCH))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

fn bump_epoch(journal: &Path) -> Result<(), String> {
    let next = read_epoch(journal).saturating_add(1);
    fs::create_dir_all(journal.join("imports")).map_err(|error| error.to_string())?;
    atomic_replace(journal.join(EPOCH), next.to_string().as_bytes(), PRIVATE)
        .map_err(|error| error.to_string())
}

/// What a merge does: a merged journal may hold old records again.
pub(crate) fn mark_conversion_needed(journal: &Path) -> Result<(), String> {
    clear_marker(journal)
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
        _ => BodyRecordPlace {
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
        decide(current.unwrap_or(fresh), named, spanned, &primary, now)
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
) -> Result<(), String> {
    clear_marker(journal)?;
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
        for found in current.iter().flatten() {
            remove_outputs(journal, found)?;
            digests.push((found.clone(), found.digest()));
        }
        remove_activity_state(journal)?;
        stop_point()?;
        let mut changed = false;
        for (line, digest) in digests {
            let place = BodyRecordPlace {
                day: line.day.clone(),
                facet: line.facet.clone(),
            };
            let removal = remove_activity_line_if(journal, &place, &line.id, |record| {
                record_digest(record) == digest
            })
            .map_err(|error| error.to_string())?;
            changed |= removal == LineRemoval::Changed;
            stop_point()?;
        }
        if !changed {
            return Ok(());
        }
    }
    Err("a record kept changing while it was being updated".to_owned())
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
    counts.waiting += unreadable;
    let workouts = live_workouts(journal);
    let (parts, no_workout) = components(&workouts, &lines);
    counts.no_workout = no_workout;
    let at = now_text(now);
    for part in &parts {
        match convert_component(journal, &workouts, &lines, part, scope, held, &at) {
            Ok(()) => counts.converted += 1,
            Err(error) => {
                log::warn!("Strava conversion: a workout's records couldn't be updated: {error}");
                counts.waiting += 1;
            }
        }
    }
    counts
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
    let mut held_back = BTreeSet::new();
    let workouts = live_workouts(journal)
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
    let epoch = read_epoch(journal);
    let (lines, unreadable) = all_sense_lines(journal);
    counts.waiting += unreadable;
    let workouts = live_workouts(journal);
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
    let found_any = !parts.is_empty();
    for part in &parts {
        let Ok(held) = hold_lock(journal.join(STRAVA_LOCK), lock_options.clone()) else {
            counts.skipped_locked += 1;
            continue;
        };
        match convert_component(journal, &workouts, &lines, part, scope, &held, &at) {
            Ok(()) => counts.converted += 1,
            Err(error) => {
                log::warn!("Strava conversion: a workout's records couldn't be updated: {error}");
                counts.waiting += 1;
            }
        }
    }
    if !found_any && counts.waiting == 0 && counts.skipped_locked == 0 {
        if let Ok(_held) = hold_lock(journal.join(STRAVA_LOCK), lock_options) {
            if read_epoch(journal) == epoch && fs::create_dir_all(journal.join("imports")).is_ok() {
                let _ = atomic_replace(journal.join(MARKER), b"", PRIVATE);
            }
        }
    }
    counts
}

#[cfg(test)]
#[path = "strava_convert_tests.rs"]
mod tests;
