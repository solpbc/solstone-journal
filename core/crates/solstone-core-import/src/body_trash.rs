// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a body import holds, as the trash moves it: a Strava download's
//! workouts, an Apple Health export's or an Oura import's rows.
//!
//! A body import holds what it first wrote, never what it only listed:
//!
//! - **Strava:** every live tile whose `workout.json` names the import as its
//!   `import_id`. A tile has one owner.
//! - **Apple Health, Oura:** every row whose `run` is the import, on the days the
//!   import touched (its `body-touched.jsonl`, and the day a stopped run of it was
//!   writing). Moves and refiles stay inside the owner's day, so those days hold
//!   all of it. A window holding only its rows (marks aside) is moved whole. In a
//!   window that holds other rows too, its rows are split out into
//!   `body.<import>.jsonl` beside them just before that file moves, so the other
//!   rows stay where they are and nothing of this import is left behind.
//!
//! What a source keeps for itself across imports stays: its zone, Oura's
//! coverage, its record of what it fetched and its connection, the order of
//! Apple runs, Strava's refile and conversion lists. A stopped run's own folder
//! is scratch: deleting its import removes it.
//!
//! A Strava workout is the same tiles, chosen by the workout's own id.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use solstone_core_format::content::body_records::{
    BODY_RECORDS_FILE, BodyRow, RowKind, decode_rows, encode_rows, grid_length, sort_rows,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockOptions, atomic_replace, hold_lock,
};
use solstone_core_retention::layout::segment_rel;

/// The body sources whose imports this reads.
pub const STRAVA: &str = "strava";
pub const APPLE_HEALTH: &str = "apple_health";
pub const OURA: &str = "oura";

const STRAVA_STREAM: &str = "import.strava";
const STRAVA_TILE: &str = "workout.json";
const LISTING_FILE: &str = "body-touched.jsonl";
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// How long a delete waits for a source's own lock before saying an import of
/// it is running.
const LOCK_WAIT: Duration = Duration::from_secs(2);

/// The streams body sources write their windows and workouts to.
const BODY_STREAMS: &[(&str, &str)] = &[
    (STRAVA_STREAM, STRAVA),
    ("import.apple_health", APPLE_HEALTH),
    ("import.oura", OURA),
];

/// Whether `stream` is a body source's.
pub fn is_body_stream(stream: &str) -> bool {
    BODY_STREAMS.iter().any(|(name, _)| *name == stream)
}

/// Whether an import of the body source writing `stream` holds its source's
/// lock now: it is running, and can still write that stream.
pub fn stream_importing(journal: &Path, stream: &str) -> bool {
    BODY_STREAMS
        .iter()
        .find(|(name, _)| *name == stream)
        .and_then(|(_, source)| lock_rel(source))
        .is_some_and(|lock| {
            solstone_core_journal_io::lock_is_held(journal.join(lock)).unwrap_or(false)
        })
}

/// Whether `source` is a body source, whose imports hold what they first wrote.
pub fn is_body_source(source: &str) -> bool {
    matches!(source, STRAVA | APPLE_HEALTH | OURA)
}

/// The lock a source's import holds while it writes: a delete holds it too, so
/// no import of the source writes while the item moves.
fn lock_rel(source: &str) -> Option<&'static str> {
    match source {
        STRAVA => Some(crate::strava_names::STRAVA_LOCK),
        APPLE_HEALTH => Some("imports/.apple_health"),
        OURA => Some("imports/.oura"),
        _ => None,
    }
}

/// A run's own folder, for the sources that keep one.
fn run_dir_rel(source: &str) -> Option<&'static str> {
    match source {
        APPLE_HEALTH => Some("imports/.apple_health-run"),
        OURA => Some("imports/.oura-run"),
        _ => None,
    }
}

/// Take `source`'s import lock, or `None` when the source has none. `Err`, in
/// the lock's own words, when an import of it is running.
pub fn hold_source_lock(journal: &Path, source: &str) -> Result<Option<FileLock>, String> {
    let Some(rel) = lock_rel(source) else {
        return Ok(None);
    };
    hold_lock(
        journal.join(rel),
        LockOptions {
            timeout: LOCK_WAIT,
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map(Some)
    .map_err(|error| error.to_string())
}

/// Why an import of a body source waits: a delete or recovery of one of that
/// source's imports is part way, and its run must not write where that moves.
pub const TRASH_MOVING: &str = "a delete or recovery from imports of this kind hasn't finished, so nothing was changed. it usually finishes within the hour or the next time your journal starts; try again then";

/// Why an import of a body source waits when the step has been part way for
/// longer than the sweep that should have finished it: it couldn't.
pub const TRASH_STUCK: &str = "a delete or recovery from imports of this kind stopped partway and your journal couldn't finish it, so nothing was changed. your journal keeps trying";

/// How long a step may be part way before its sweep has had two tries at it.
const STUCK_AFTER: chrono::Duration = chrono::Duration::hours(2);

/// Whether a trash item of `source` is moving now or stopped part way. Call it
/// holding the source's lock, before writing. A trash that can't be read counts
/// as moving: nothing is written on a guess.
pub fn source_trash_moving(journal: &Path, source: &str) -> bool {
    source_trash_held(journal, source, "").is_some()
}

/// [`source_trash_moving`], leaving out item `except`: what a trash step of
/// `source` asks before it moves anything, so two never interleave.
pub fn source_trash_moving_except(journal: &Path, source: &str, except: &str) -> bool {
    source_trash_held(journal, source, except).is_some()
}

/// The words for why `source` waits on its trash, if it does: [`TRASH_MOVING`],
/// or [`TRASH_STUCK`] once a step has been part way past two sweeps.
pub fn source_trash_held(journal: &Path, source: &str, except: &str) -> Option<&'static str> {
    use solstone_core_removals::Unit;
    let Ok(items) = solstone_core_trash::moving(journal) else {
        return Some(TRASH_MOVING);
    };
    let mut held = None;
    for item in items.iter().filter(|item| item.id != except) {
        let of = match &item.unit {
            Unit::Import { source: of, .. } | Unit::Workout { source: of, .. } => of,
            _ => continue,
        };
        if of != source {
            continue;
        }
        let since = item
            .pending
            .as_ref()
            .and_then(|pending| chrono::DateTime::parse_from_rfc3339(&pending.at).ok())
            .or_else(|| chrono::DateTime::parse_from_rfc3339(&item.trashed_at).ok());
        let stuck = since.is_some_and(|since| chrono::Utc::now() - since.to_utc() > STUCK_AFTER);
        if stuck {
            return Some(TRASH_STUCK);
        }
        held = Some(TRASH_MOVING);
    }
    held
}

/// What a body import holds now, journal-relative.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BodyThings {
    /// Its own windows or tiles, whole.
    pub segments: Vec<String>,
    /// In each window it shares, the file its rows are split into as it moves.
    pub files: Vec<String>,
    /// For Strava: the workouts its tiles are.
    pub workouts: BTreeSet<String>,
    /// Windows, or its listing, that couldn't be read: the delete refuses.
    pub unreadable: usize,
    /// The days on which something it brought in again stays, because an
    /// earlier import still here wrote it first.
    pub shared_days: usize,
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// One path component a record may name.
fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.starts_with('.')
        && !part.contains(['/', '\\'])
}

fn names_in(dir: &Path) -> Vec<String> {
    listed(dir).unwrap_or_default()
}

/// A folder's names, `None` when it is there and can't be listed. A folder that
/// isn't there has none.
fn listed(dir: &Path) -> Option<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = entry.ok()?.file_name();
        if let Some(name) = name.to_str().filter(|name| valid_part(name)) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Some(names)
}

/// Every Strava tile in the journal that is live, with its `workout.json`, and
/// how many places couldn't be read: a day's tiles that can't be listed, or a
/// tile whose record can't be read, could be anyone's.
fn strava_tiles(journal: &Path, live: &dyn Fn(&str) -> bool) -> (Vec<(String, Value)>, usize) {
    let mut tiles = Vec::new();
    let mut unreadable = 0;
    let chronicle = journal.join("chronicle");
    for day in names_in(&chronicle) {
        let Some(keys) = listed(&chronicle.join(&day).join(STRAVA_STREAM)) else {
            unreadable += 1;
            continue;
        };
        for key in keys {
            let rel = segment_rel(&day, STRAVA_STREAM, &key);
            if !live(&rel) {
                continue;
            }
            match read_json(&journal.join(&rel).join(STRAVA_TILE)) {
                Some(tile) => tiles.push((rel, tile)),
                None if journal.join(&rel).join(STRAVA_TILE).exists() => unreadable += 1,
                None => {}
            }
        }
    }
    (tiles, unreadable)
}

fn text_of(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A Strava workout's tiles, by its own id, and how many places couldn't be
/// read (one of them could be a tile of it).
pub fn workout_tiles_checked(
    journal: &Path,
    activity_id: &str,
    live: &dyn Fn(&str) -> bool,
) -> (Vec<String>, usize) {
    let (tiles, unreadable) = strava_tiles(journal, live);
    let tiles = tiles
        .into_iter()
        .filter(|(_, tile)| text_of(tile, "activity_id").as_deref() == Some(activity_id))
        .map(|(rel, _)| rel)
        .collect();
    (tiles, unreadable)
}

/// A Strava workout's tiles, by its own id.
pub fn workout_tiles(
    journal: &Path,
    activity_id: &str,
    live: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    workout_tiles_checked(journal, activity_id, live).0
}

/// Whether the trash holds the Strava tile that was at `rel` for a workout other
/// than `activity_id`: recovering it would put it back there, so another
/// workout arriving now steps past the key. A trash that can't be read counts
/// as holding it.
pub fn held_for_another_workout(journal: &Path, rel: &str, activity_id: &str) -> bool {
    let Ok(holders) = solstone_core_trash::holders(journal, rel, "") else {
        return true;
    };
    holders.iter().any(|item| {
        solstone_core_trash::held_file(journal, item, &format!("{rel}/{STRAVA_TILE}"))
            .ok()
            .and_then(|path| read_json(&path))
            .and_then(|tile| text_of(&tile, "activity_id"))
            .is_none_or(|held| held != activity_id)
    })
}

/// The day a workout began on and its first tile's clock, from its tiles' paths.
pub fn workout_start(paths: &[String]) -> Option<(String, String)> {
    let first = paths.iter().min()?;
    let mut parts = first.split('/');
    let (_, day, _, key) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let clock = key.split_once('_').map_or(key, |(clock, _)| clock);
    Some((day.to_owned(), clock.to_owned()))
}

/// What an Apple Health or Oura import's listing says it touched: each window
/// it brought in again (rows updated or found unchanged), by day, and whether
/// the listing could be read. A last line a stop cut short is the day its run
/// was writing, which the run's state names.
struct Listing {
    days: BTreeSet<String>,
    again: BTreeSet<String>,
    readable: bool,
}

fn listing_of(journal: &Path, id: &str, source: &str) -> Listing {
    let stream = format!("import.{source}");
    let mut listing = Listing {
        days: BTreeSet::new(),
        again: BTreeSet::new(),
        readable: true,
    };
    match std::fs::read_to_string(journal.join("imports").join(id).join(LISTING_FILE)) {
        Ok(text) => {
            let lines: Vec<&str> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            for (index, line) in lines.iter().enumerate() {
                let Ok(line) = serde_json::from_str::<Value>(line) else {
                    if index + 1 != lines.len() {
                        listing.readable = false;
                    }
                    continue;
                };
                let Some(day) = line
                    .get("day")
                    .and_then(Value::as_str)
                    .filter(|day| valid_part(day))
                else {
                    listing.readable = false;
                    continue;
                };
                listing.days.insert(day.to_owned());
                for entry in line
                    .get("windows")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice)
                {
                    let count = |at: usize| entry.get(at).and_then(Value::as_u64).unwrap_or(0);
                    if let Some(key) = entry.get(0).and_then(Value::as_str)
                        && valid_part(key)
                        && count(2) + count(3) > 0
                    {
                        listing.again.insert(segment_rel(day, &stream, key));
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => listing.readable = false,
    }
    // A run's state that is there and can't be read could be this import's,
    // writing a day its listing doesn't name yet.
    if let Some(rel) = run_dir_rel(source)
        && journal.join(rel).join("state.json").exists()
        && read_json(&journal.join(rel).join("state.json")).is_none()
    {
        listing.readable = false;
    }
    if let Some(state) = run_state_of(journal, id, source)
        && let Some(day) = state
            .get("pending")
            .and_then(|pending| pending.get("day"))
            .and_then(Value::as_str)
            .filter(|day| valid_part(day))
    {
        listing.days.insert(day.to_owned());
    }
    listing
}

/// A stopped run's state, when the run was `id`'s.
fn run_state_of(journal: &Path, id: &str, source: &str) -> Option<Value> {
    let state = read_json(&journal.join(run_dir_rel(source)?).join("state.json"))?;
    (state.get("import_id").and_then(Value::as_str) == Some(id)).then_some(state)
}

/// The rows a window holds, or `None` when they can't be read. A window with no
/// rows file holds none.
fn window_rows(journal: &Path, window: &str) -> Option<Vec<BodyRow>> {
    match std::fs::read_to_string(journal.join(window).join(BODY_RECORDS_FILE)) {
        Ok(text) => decode_rows(&text).ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

/// Whose a window's rows are, as far as `id` goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Share {
    /// None of its rows is `id`'s.
    None,
    /// Every row in it is `id`'s (marks aside): it moves whole.
    Whole,
    /// It holds `id`'s rows and others: `id`'s split out.
    Part,
    /// It can't be read.
    Unreadable,
}

/// How a window's rows stand for `id`.
pub fn share_of(journal: &Path, window: &str, id: &str) -> Share {
    let Some(rows) = window_rows(journal, window) else {
        return Share::Unreadable;
    };
    let rows: Vec<&BodyRow> = rows
        .iter()
        .filter(|row| row.kind != RowKind::Moved)
        .collect();
    let mine = rows
        .iter()
        .filter(|row| row.run.as_deref() == Some(id))
        .count();
    // Another import's rows split out beside these, on their way to or from the
    // trash, are that import's: the window is never taken whole with them.
    let others_split = names_in(&journal.join(window))
        .iter()
        .any(|name| split_of(&format!("{window}/{name}")).is_some_and(|(_, owner)| owner != id));
    let others = rows.len() - mine + usize::from(others_split);
    match (mine, others) {
        (0, _) => Share::None,
        (_, 0) => Share::Whole,
        _ => Share::Part,
    }
}

/// The file import `id`'s rows are split into in a window it shares.
pub fn split_file(window: &str, id: &str) -> String {
    format!("{window}/body.{id}.jsonl")
}

/// The window a split file sits in, and the import it is, when `path` is one.
pub fn split_of(path: &str) -> Option<(&str, &str)> {
    let (window, name) = path.rsplit_once('/')?;
    let id = name.strip_prefix("body.")?.strip_suffix(".jsonl")?;
    (valid_part(id) && !id.is_empty()).then_some((window, id))
}

/// What body import `id` of `source` holds now. `live` says whether a segment is
/// there to take.
pub fn body_things(
    journal: &Path,
    id: &str,
    source: &str,
    live: &dyn Fn(&str) -> bool,
) -> BodyThings {
    let mut things = BodyThings::default();
    if source == STRAVA {
        let (tiles, unreadable) = strava_tiles(journal, live);
        things.unreadable += unreadable;
        for (rel, tile) in tiles {
            if text_of(&tile, "import_id").as_deref() == Some(id) {
                if let Some(workout) = text_of(&tile, "activity_id") {
                    things.workouts.insert(workout);
                }
                things.segments.push(rel);
            }
        }
        return things;
    }
    let stream = format!("import.{source}");
    let listing = listing_of(journal, id, source);
    if !listing.readable {
        things.unreadable += 1;
    }
    let here = imports_here(journal);
    let mut shared_days = BTreeSet::new();
    for day in &listing.days {
        let Some(keys) = listed(&journal.join("chronicle").join(day).join(&stream)) else {
            things.unreadable += 1;
            continue;
        };
        for key in keys {
            if grid_length(&key).is_none() {
                continue;
            }
            let window = segment_rel(day, &stream, &key);
            if !live(&window) {
                continue;
            }
            match share_of(journal, &window, id) {
                Share::None => {
                    if journal.join(split_file(&window, id)).is_file() {
                        things.files.push(split_file(&window, id));
                    }
                }
                Share::Whole => things.segments.push(window.clone()),
                Share::Part => things.files.push(split_file(&window, id)),
                Share::Unreadable => {
                    things.unreadable += 1;
                    continue;
                }
            }
            // What it brought in again that an earlier import still here wrote
            // first stays with that import.
            if listing.again.contains(&window)
                && window_rows(journal, &window).is_some_and(|rows| {
                    rows.iter().any(|row| {
                        row.kind != RowKind::Moved
                            && row
                                .run
                                .as_deref()
                                // Ids are when each import began, so an earlier one
                                // sorts first: only what one of those wrote stays.
                                .is_some_and(|run| run < id && here.contains(run))
                    })
                })
            {
                shared_days.insert(day.clone());
            }
        }
    }
    things.shared_days = shared_days.len();
    things
}

/// The ids of the imports in the journal now.
fn imports_here(journal: &Path) -> BTreeSet<String> {
    names_in(&journal.join("imports"))
        .into_iter()
        .filter(|name| journal.join("imports").join(name).is_dir())
        .collect()
}

/// Split import `id`'s rows out of the window `split` names into `split`
/// itself, leaving every other row where it is. Both writes replace a file
/// whole, and the file is written first, so a stop between them leaves the rows
/// in both, which running this again settles. Call it holding the window.
pub fn split_out(journal: &Path, split: &str) -> Result<(), String> {
    let (window, id) = split_of(split).ok_or_else(|| format!("{split} isn't a split file"))?;
    let Some(rows) = window_rows(journal, window) else {
        return Err(format!("{window}: its rows couldn't be read"));
    };
    let mut kept = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(journal.join(split)) {
        for row in decode_rows(&text).map_err(|error| format!("{split}: {error}"))? {
            kept.insert(row.id.clone(), row);
        }
    }
    let (mine, others): (Vec<BodyRow>, Vec<BodyRow>) = rows
        .into_iter()
        .partition(|row| row.run.as_deref() == Some(id));
    if !mine.iter().any(|row| row.kind != RowKind::Moved) && kept.is_empty() {
        return Ok(());
    }
    for row in mine {
        kept.insert(row.id.clone(), row);
    }
    let mut kept: Vec<BodyRow> = kept.into_values().collect();
    sort_rows(&mut kept);
    atomic_replace(journal.join(split), encode_rows(&kept).as_bytes(), PRIVATE)
        .map_err(|error| format!("{split}: {error}"))?;
    let mut others = others;
    sort_rows(&mut others);
    atomic_replace(
        journal.join(window).join(BODY_RECORDS_FILE),
        encode_rows(&others).as_bytes(),
        PRIVATE,
    )
    .map_err(|error| format!("{window}: {error}"))
}

/// Put a split file's rows back into its window, they winning where a row of
/// the same id has arrived since (the trash's overwrite), then remove the file.
/// Safe to run again. Call it holding the window. Returns the window's records
/// file, for search.
pub fn merge_back(journal: &Path, split: &str) -> Result<Option<String>, String> {
    let (window, _) = split_of(split).ok_or_else(|| format!("{split} isn't a split file"))?;
    let place = journal.join(split);
    let text = match std::fs::read_to_string(&place) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{split}: {error}")),
    };
    let returning = decode_rows(&text).map_err(|error| format!("{split}: {error}"))?;
    let Some(here) = window_rows(journal, window) else {
        return Err(format!("{window}: its rows couldn't be read"));
    };
    let ids: BTreeSet<&str> = returning.iter().map(|row| row.id.as_str()).collect();
    let mut rows: Vec<BodyRow> = here
        .into_iter()
        .filter(|row| !ids.contains(row.id.as_str()))
        .collect();
    rows.extend(returning);
    sort_rows(&mut rows);
    atomic_replace(
        journal.join(window).join(BODY_RECORDS_FILE),
        encode_rows(&rows).as_bytes(),
        PRIVATE,
    )
    .map_err(|error| format!("{window}: {error}"))?;
    // System internal: the split's own file, its rows now back in the window.
    std::fs::remove_file(&place).map_err(|error| format!("{split}: {error}"))?;
    Ok(Some(format!("{window}/{BODY_RECORDS_FILE}")))
}

/// Whether rows an item split out of `split`'s window are back there now, from
/// a re-import: recovering would replace those copies.
pub fn split_returned(journal: &Path, item_id: &str, split: &str) -> bool {
    let Some((window, _)) = split_of(split) else {
        return false;
    };
    let Some(held) = solstone_core_trash::held_file(journal, item_id, split)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| decode_rows(&text).ok())
    else {
        return false;
    };
    let ids: BTreeSet<String> = held.into_iter().map(|row| row.id).collect();
    window_rows(journal, window)
        .unwrap_or_default()
        .iter()
        .any(|row| row.kind != RowKind::Moved && ids.contains(&row.id))
}

/// The rows import `id` takes at `paths`, by day, read before they move: what an
/// Oura import's delete adds to the sync's record.
pub fn rows_taken(journal: &Path, paths: &[String], id: &str) -> BTreeMap<String, Vec<String>> {
    let mut taken: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in paths {
        let (window, whole) = match split_of(path) {
            Some((window, _)) => (window, false),
            None if path.starts_with("chronicle/") => (path.as_str(), true),
            None => continue,
        };
        let Some(day) = window.split('/').nth(1) else {
            continue;
        };
        let mut ids: Vec<String> = window_rows(journal, window)
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.kind != RowKind::Moved && (whole || row.run.as_deref() == Some(id)))
            .map(|row| row.id)
            .collect();
        if let Ok(text) = std::fs::read_to_string(journal.join(path))
            && !whole
            && let Ok(rows) = decode_rows(&text)
        {
            ids.extend(
                rows.into_iter()
                    .filter(|row| row.kind != RowKind::Moved)
                    .map(|row| row.id),
            );
        }
        taken.entry(day.to_owned()).or_default().extend(ids);
    }
    taken
}

/// A stopped run's folder, gone with the import whose run it was: its scratch.
pub fn remove_run_folder(journal: &Path, id: &str, source: &str) -> Result<(), String> {
    let Some(rel) = run_dir_rel(source) else {
        return Ok(());
    };
    // Its own run's, or one whose state can't say whose it is: scratch either way.
    let state = journal.join(rel).join("state.json");
    if !journal.join(rel).exists()
        || (read_json(&state).is_some() && run_state_of(journal, id, source).is_none())
    {
        return Ok(());
    }
    #[expect(
        clippy::disallowed_methods,
        reason = "system internal: a stopped run's own staging, scratch for the import it belonged to"
    )]
    let removed = solstone_core_journal_io::remove_dir_all(journal, rel);
    removed.map_err(|error| format!("{rel}: {error}"))
}

/// The workouts Strava tiles at `paths` are, read from item `item_id`'s own
/// copies once they have moved.
pub fn workouts_held(journal: &Path, item_id: &str, paths: &[String]) -> BTreeSet<String> {
    paths
        .iter()
        .filter(|path| path.contains(&format!("/{STRAVA_STREAM}/")))
        .filter_map(|path| {
            solstone_core_trash::held_file(journal, item_id, &format!("{path}/{STRAVA_TILE}")).ok()
        })
        .filter_map(|file| read_json(&file))
        .filter_map(|tile| text_of(&tile, "activity_id"))
        .collect()
}
