// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Body records, composed for a reader.
//!
//! A body record in a facet's activity file refers to an account in a body
//! window by its stream and row id, and copies nothing else. Every reader that
//! shows one composes it here, from the window as it now is, and only for a
//! connection allowed to read that window: so nothing in a record file is ever
//! served as it stands, and a facet change or a narrower grant can't widen what
//! is read.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::{DateTime, FixedOffset, NaiveDate, Timelike};
use chrono_tz::Tz;
use serde_json::{Map, Value};
use solstone_core_facets::{activity_field_held, is_body_record, update_activity_record_with};
use solstone_core_format::content::body_records::{
    BODY_RECORDS_FILE, BodyRow, RowKind, copy_rank, decode_rows, parse_source_time, user_entered,
};
use solstone_core_format::content::strava_body::{STRAVA_STREAM, STRAVA_TILE_FILE, strava_rows};

const TOMBSTONE: &str = "tombstone.json";
/// Separates the stream from the row id in an account key.
const KEY_SEPARATOR: char = '\u{1f}';
/// The fields an owner edit can hold, shown in place of what the source says.
pub const OWNER_FIELDS: [&str; 4] = ["title", "description", "details", "playback"];

/// What the account's window says now.
#[derive(Debug, Clone, PartialEq)]
pub enum Account {
    /// The experience row, as its window holds it.
    Live(Box<BodyRow>),
    /// The window of its start was deleted and a later tile of its span is
    /// still live: that tile's span, `by` and `label`.
    PartRemoved(Box<BodyRow>),
    /// The owner deleted the window of its start and every tile of its span.
    RemovedByYou,
    /// The window of its start and every tile of its span are gone, and not by
    /// the owner's delete: retention reached them, say, or an import run was
    /// released.
    Removed,
    /// Its window is live and holds no such row.
    NotFound,
    /// Its window couldn't be read.
    Unreadable,
}

/// A body record composed for one reader.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposedBody {
    pub id: String,
    pub facet: String,
    pub day: String,
    /// `workout`, `sleep` or `mindfulness`.
    pub activity: String,
    pub key: String,
    pub stream: String,
    /// The instants stored when the record was created. Groups use these.
    pub stored_start: String,
    pub stored_end: String,
    /// The window of its start, when one is known.
    pub window: Option<String>,
    pub account: Account,
    /// Owner-held fields only, by name.
    pub owner: BTreeMap<String, Value>,
    pub hidden: bool,
    /// Account keys this record is kept apart from.
    pub apart: Vec<String>,
}

impl ComposedBody {
    /// Whether its window, or a later piece of its span, still holds it.
    #[must_use]
    pub fn still_there(&self) -> bool {
        matches!(self.account, Account::Live(_) | Account::PartRemoved(_))
    }

    /// The row to show: the experience, or the surviving tile.
    #[must_use]
    pub fn row(&self) -> Option<&BodyRow> {
        match &self.account {
            Account::Live(row) | Account::PartRemoved(row) => Some(row),
            _ => None,
        }
    }
}

/// Why a day's records couldn't be composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeError(pub String);

impl std::fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ComposeError {}

/// Whether this reader may read a body window: its day, its stream and, when
/// one is known, its name. The owner's own view reads every one.
pub type Readable<'a> = &'a dyn Fn(&str, &str, Option<&str>) -> bool;

/// One body window's rows, as far as it could be read.
#[derive(Debug)]
enum Held {
    Rows(Vec<BodyRow>),
    /// Tombstoned: by the owner's own delete, or not, and whether it was an
    /// import run's release (which a later import puts back beside it).
    Deleted {
        by_owner: bool,
        released: bool,
    },
    Unreadable,
}

/// The one tombstone reason that is the owner's delete of a window.
const OWNER_DELETE: &str = "owner_segment_delete";
/// The tombstone reason of an import run's release: its pieces come back
/// beside it on a later import, so it removes nothing from an account.
const RUN_RELEASE: &str = "import_run_release";

#[derive(Debug)]
struct Window {
    name: String,
    start: u32,
    end: u32,
    held: Held,
}

/// `HHMMSS_<len>[_...]` as seconds of the day, start and exclusive end.
fn window_span(name: &str) -> Option<(u32, u32)> {
    let (clock, rest) = name.split_once('_')?;
    if clock.len() != 6 || !clock.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let length =
        if let Some(length) = solstone_core_format::content::body_records::grid_length(name) {
            u32::try_from(length).ok()?
        } else {
            rest.split('_').next()?.parse::<u32>().ok()?
        };
    let hours = clock[0..2].parse::<u32>().ok()?;
    let minutes = clock[2..4].parse::<u32>().ok()?;
    let seconds = clock[4..6].parse::<u32>().ok()?;
    let start = hours * 3600 + minutes * 60 + seconds;
    Some((start, start + length.max(1)))
}

/// What one window of `stream` holds now. A Strava tile keeps its own file,
/// read as body rows; every other stream's window holds `body.jsonl`.
fn read_held(path: &Path, stream: &str) -> Held {
    if path.join(TOMBSTONE).exists() {
        // A tombstone that can't be read says nobody's delete in particular.
        let reason = fs::read(path.join(TOMBSTONE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|tombstone| tombstone["reason"].as_str().map(str::to_owned));
        return Held::Deleted {
            by_owner: reason.as_deref() == Some(OWNER_DELETE),
            released: reason.as_deref() == Some(RUN_RELEASE),
        };
    }
    let strava = stream == STRAVA_STREAM;
    let file = if strava {
        STRAVA_TILE_FILE
    } else {
        BODY_RECORDS_FILE
    };
    match fs::read_to_string(path.join(file)) {
        Ok(text) => if strava {
            strava_rows(&text)
        } else {
            decode_rows(&text).ok()
        }
        .map_or(Held::Unreadable, Held::Rows),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Held::Rows(Vec::new()),
        Err(_) => Held::Unreadable,
    }
}

/// The experience row `row_id` a window holds, if it's live and holds it.
fn experience_row<'a>(held: &'a Held, row_id: &str) -> Option<&'a BodyRow> {
    match held {
        Held::Rows(rows) => rows
            .iter()
            .find(|row| row.kind == RowKind::Experience && row.id == row_id),
        _ => None,
    }
}

fn read_windows(journal: &Path, day: &str, stream: &str) -> Vec<Window> {
    let dir = journal.join("chronicle").join(day).join(stream);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Some((start, end)) = window_span(&name) else {
            continue;
        };
        let held = read_held(&entry.path(), stream);
        windows.push(Window {
            name,
            start,
            end,
            held,
        });
    }
    windows.sort_by(|left, right| (left.start, &left.name).cmp(&(right.start, &right.name)));
    windows
}

/// Seconds into `day` of an instant, on the clock that placed the source's
/// windows. `None` when it falls on another day.
fn seconds_on(day: &str, time: DateTime<FixedOffset>, zone: Option<Tz>) -> Option<u32> {
    let local = zone.map_or_else(
        || time.naive_local(),
        |zone| time.with_timezone(&zone).naive_local(),
    );
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    (local.date() == date).then(|| local.num_seconds_from_midnight())
}

fn source_of(stream: &str) -> &str {
    stream.strip_prefix("import.").unwrap_or(stream)
}

struct Day {
    windows: Vec<Window>,
    zone: Option<Tz>,
}

impl Day {
    fn surviving_tile(&self, row_id: &str) -> Option<&BodyRow> {
        self.windows.iter().find_map(|window| match &window.held {
            Held::Rows(rows) => rows.iter().find(|row| {
                row.kind == RowKind::ExperienceTile && row.of.as_deref() == Some(row_id)
            }),
            _ => None,
        })
    }

    /// How far into `day` a span ending at `end` reaches, in seconds: all of
    /// it when it ends later, none when it ended before.
    fn seconds_reached(&self, day: &str, end: DateTime<FixedOffset>) -> u32 {
        if let Some(seconds) = seconds_on(day, end, self.zone) {
            return seconds;
        }
        let local = self.zone.map_or_else(
            || end.naive_local(),
            |zone| end.with_timezone(&zone).naive_local(),
        );
        match NaiveDate::parse_from_str(day, "%Y%m%d") {
            Ok(date) if local.date() > date => 86_400,
            _ => 0,
        }
    }

    fn covering(&self, day: &str, start: &str) -> Vec<&Window> {
        let Some(seconds) =
            parse_source_time(start).and_then(|start| seconds_on(day, start, self.zone))
        else {
            return Vec::new();
        };
        self.windows
            .iter()
            .filter(|window| window.start <= seconds && seconds < window.end)
            .collect()
    }
}

/// Tiles of a span sit in later windows, possibly on later days: the days a
/// surviving tile is looked for in, after the start's own.
const TILE_DAYS: i64 = 14;

fn later_days(day: &str) -> impl Iterator<Item = String> {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok();
    (1..=TILE_DAYS).filter_map(move |offset| {
        date.and_then(|date| date.checked_add_signed(chrono::Duration::days(offset)))
            .map(|date| date.format("%Y%m%d").to_string())
    })
}

/// The window an account's experience row sits in, and what it says now.
fn locate(
    journal: &Path,
    day: &str,
    stream: &str,
    row_id: &str,
    (start, end): (&str, &str),
    days: &mut BTreeMap<(String, String), Day>,
) -> (Option<String>, Account) {
    let load = |days: &mut BTreeMap<(String, String), Day>, on: &str| {
        days.entry((on.to_owned(), stream.to_owned()))
            .or_insert_with(|| Day {
                windows: read_windows(journal, on, stream),
                zone: solstone_core_journal_config::placed_zone(journal, source_of(stream)),
            });
    };
    load(days, day);
    let today = &days[&(day.to_owned(), stream.to_owned())];
    let covering = today.covering(day, start);
    // A window that can't be read might hold the row: nothing more is known.
    if let Some(window) = covering
        .iter()
        .find(|window| matches!(window.held, Held::Unreadable))
    {
        return (Some(window.name.clone()), Account::Unreadable);
    }
    let deleted = covering
        .iter()
        .filter_map(|window| match window.held {
            Held::Deleted { by_owner, .. } => Some((window.name.clone(), by_owner)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let first = covering.first().map(|window| window.name.clone());
    let Some((name, _)) = deleted.first().cloned() else {
        return (first, Account::NotFound);
    };
    // A tombstone doesn't name its rows: the owner removed it only if every
    // deleted window that covers it says so.
    let by_owner = deleted.iter().all(|(_, by_owner)| *by_owner);
    // Deleted: say so only when no tile of the span survives, and nothing that
    // can't be read might hold one.
    let end = parse_source_time(end);
    let begins = parse_source_time(start)
        .and_then(|start| seconds_on(day, start, today.zone))
        .unwrap_or(0);
    for on in std::iter::once(day.to_owned()).chain(later_days(day)) {
        load(days, &on);
        let that_day = &days[&(on.clone(), stream.to_owned())];
        if let Some(tile) = that_day.surviving_tile(row_id) {
            return (Some(name), Account::PartRemoved(Box::new(tile.clone())));
        }
        let from = if on == day { begins } else { 0 };
        let reaches = end.map_or(0, |end| that_day.seconds_reached(&on, end));
        if that_day.windows.iter().any(|window| {
            matches!(window.held, Held::Unreadable) && window.start < reaches && window.end > from
        }) {
            return (Some(name), Account::Unreadable);
        }
    }
    let account = if by_owner {
        Account::RemovedByYou
    } else {
        Account::Removed
    };
    (Some(name), account)
}

/// Whether a later window of a live account's own grid, inside its span, was
/// deleted by the owner or by retention. An import run's release isn't a
/// removal (its pieces come back beside it), and a window on another grid
/// (a 300-second session inside an hour-tiled night) isn't this account's.
fn later_piece_removed(
    journal: &Path,
    stored: &StoredBody,
    start_window: &str,
    row: &BodyRow,
    days: &mut BTreeMap<(String, String), Day>,
) -> bool {
    let Some((first_start, first_end)) = window_span(start_window) else {
        return false;
    };
    let grid = grid_of(first_end - first_start);
    let Some(end) = row.end.as_deref().and_then(parse_source_time) else {
        return false;
    };
    for on in std::iter::once(stored.day.clone()).chain(later_days(&stored.day)) {
        let that_day = days
            .entry((on.clone(), stored.stream.clone()))
            .or_insert_with(|| Day {
                windows: read_windows(journal, &on, &stored.stream),
                zone: solstone_core_journal_config::placed_zone(journal, source_of(&stored.stream)),
            });
        let reaches = that_day.seconds_reached(&on, end);
        if reaches == 0 && on != stored.day {
            break;
        }
        if that_day.windows.iter().any(|window| {
            matches!(
                window.held,
                Held::Deleted {
                    released: false,
                    ..
                }
            ) && (on != stored.day || window.start > first_start)
                && window.start < reaches
                && grid_of(window.end - window.start) == grid
        }) {
            return true;
        }
    }
    false
}

/// A window length's grid: a whole-day window, an hour window and a five-minute
/// tile are three, each with its step-around (`86401`, `3601`, `301`).
fn grid_of(length: u32) -> u32 {
    match length {
        86_400.. => 86_400,
        3_600.. => 3_600,
        _ => 300,
    }
}

fn split_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(KEY_SEPARATOR)
}

fn text(record: &Map<String, Value>, path: &[&str]) -> String {
    let Some((first, rest)) = path.split_first() else {
        return String::new();
    };
    let mut value = record.get(*first);
    for step in rest {
        value = value.and_then(|value| value.get(*step));
    }
    value.and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// A body record as stored: everything the gate and grouping read, without
/// opening a window.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredBody {
    pub id: String,
    pub facet: String,
    pub day: String,
    pub activity: String,
    pub key: String,
    pub stream: String,
    pub row_id: String,
    pub stored_start: String,
    pub stored_end: String,
    /// A workout's type in the shared vocabulary, when it has one.
    pub workout_type: Option<String>,
    /// Who recorded it, as an opaque id.
    pub recorder: String,
    /// The owner's day the record arrived on, by the writer's clock.
    pub arrived: Option<String>,
    /// Owner-held fields only, by name.
    pub owner: BTreeMap<String, Value>,
    pub hidden: bool,
    /// Account keys this record is kept apart from.
    pub apart: Vec<String>,
}

impl StoredBody {
    fn instants(&self) -> Option<(i64, i64)> {
        let start = parse_source_time(&self.stored_start)?.timestamp();
        let end = parse_source_time(&self.stored_end)?.timestamp();
        (end > start).then_some((start, end))
    }
}

/// The body records stored in `facet` on `day`, hidden ones included.
///
/// # Errors
///
/// When the facet's activity file can't be read.
pub fn stored_body_records(
    journal: &Path,
    facet: &str,
    day: &str,
) -> Result<Vec<StoredBody>, ComposeError> {
    // A line that isn't a record costs only itself.
    let records = solstone_core_facets::read_activity_records_readable(journal, facet, day)
        .map_err(|error| ComposeError(error.to_string()))?;
    let superseded = solstone_core_facets::superseded_record_ids(journal, facet, day, &records);
    Ok(records
        .iter()
        .filter(|record| is_body_record(record))
        .filter(|record| {
            !record
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| superseded.contains(id))
        })
        .filter_map(|record| {
            let key = text(record, &["account", "key"]);
            let (stream, row_id) = split_key(&key)?;
            let (stream, row_id) = (stream.to_owned(), row_id.to_owned());
            Some(StoredBody {
                id: text(record, &["id"]),
                facet: facet.to_owned(),
                day: day.to_owned(),
                activity: text(record, &["activity"]),
                stream,
                row_id,
                stored_start: text(record, &["account", "start"]),
                stored_end: text(record, &["account", "end"]),
                workout_type: Some(text(record, &["account", "type"]))
                    .filter(|kind| !kind.is_empty()),
                recorder: text(record, &["account", "recorder"]),
                arrived: Some(text(record, &["account", "arrived"])).filter(|day| !day.is_empty()),
                owner: OWNER_FIELDS
                    .iter()
                    .filter(|field| activity_field_held(record, field))
                    .filter_map(|field| Some(((*field).to_owned(), record.get(*field)?.clone())))
                    .collect(),
                hidden: record
                    .get("hidden")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                apart: record
                    .get("apart")
                    .and_then(Value::as_array)
                    .map(|keys| {
                        keys.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                key,
            })
        })
        .collect())
}

/// A day's window names in one stream, with their spans, and the zone that placed them.
type Listing = (Vec<(String, u32, u32)>, Option<Tz>);

/// What composing reads, kept across records of one request.
#[derive(Default)]
pub struct Windows {
    names: BTreeMap<(String, String), Listing>,
    /// What each window holds, read once.
    held: BTreeMap<std::path::PathBuf, Held>,
    days: BTreeMap<(String, String), Day>,
}

impl Windows {
    /// The windows an account belongs to: the live window among those its start
    /// falls in that holds its row, or, when none does (it was deleted, say),
    /// every window its start falls in. Hour windows and shorter ones overlap,
    /// so the listing alone can't say which.
    pub fn account_windows(&mut self, journal: &Path, stored: &StoredBody) -> Vec<String> {
        let (names, zone) = self
            .names
            .entry((stored.day.clone(), stored.stream.clone()))
            .or_insert_with(|| {
                let dir = journal
                    .join("chronicle")
                    .join(&stored.day)
                    .join(&stored.stream);
                let mut names = fs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .filter_map(|entry| {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let (start, end) = window_span(&name)?;
                        (!name.starts_with('.')).then_some((name, start, end))
                    })
                    .collect::<Vec<_>>();
                // A record reads the copy the importer still writes to, when an
                // archive merge left two. Strava's keeps the lowest key.
                if stored.stream == STRAVA_STREAM {
                    names.sort();
                } else {
                    names.sort_by(|left, right| {
                        (copy_rank(&left.0), &left.0).cmp(&(copy_rank(&right.0), &right.0))
                    });
                }
                (
                    names,
                    solstone_core_journal_config::placed_zone(journal, source_of(&stored.stream)),
                )
            });
        let Some(seconds) = parse_source_time(&stored.stored_start)
            .and_then(|start| seconds_on(&stored.day, start, *zone))
        else {
            return Vec::new();
        };
        let covering = names
            .iter()
            .filter(|(_, start, end)| *start <= seconds && seconds < *end)
            .map(|(name, _, _)| name.clone())
            .collect::<Vec<_>>();
        let dir = journal
            .join("chronicle")
            .join(&stored.day)
            .join(&stored.stream);
        let held = &mut self.held;
        let holding = covering.iter().find(|name| {
            experience_row(
                held.entry(dir.join(name.as_str()))
                    .or_insert_with_key(|window| read_held(window, &stored.stream)),
                &stored.row_id,
            )
            .is_some()
        });
        match holding {
            Some(name) => vec![name.clone()],
            None => covering,
        }
    }

    /// Whether this reader may read every window the account belongs to.
    pub fn may_read(
        &mut self,
        journal: &Path,
        stored: &StoredBody,
        readable: Readable<'_>,
    ) -> bool {
        let windows = self.account_windows(journal, stored);
        if windows.is_empty() {
            return readable(&stored.day, &stored.stream, None);
        }
        windows
            .iter()
            .all(|window| readable(&stored.day, &stored.stream, Some(window)))
    }

    /// The account as its window says now: the row as the live window the
    /// gate checked holds it, and only that window.
    pub fn compose(&mut self, journal: &Path, stored: StoredBody) -> ComposedBody {
        let windows = self.account_windows(journal, &stored);
        let live = match windows.as_slice() {
            [name] => {
                let window = journal
                    .join("chronicle")
                    .join(&stored.day)
                    .join(&stored.stream)
                    .join(name);
                self.held
                    .get(&window)
                    .and_then(|held| experience_row(held, &stored.row_id))
                    .map(|row| (Some(name.clone()), Account::Live(Box::new(row.clone()))))
            }
            _ => None,
        };
        // A later piece of a live account deleted says so too, as a deleted
        // start does: the line keeps the times its row states.
        let live = live.map(|(name, account)| match (&name, account) {
            (Some(start), Account::Live(row))
                if later_piece_removed(journal, &stored, start, &row, &mut self.days) =>
            {
                (name, Account::PartRemoved(row))
            }
            (_, account) => (name, account),
        });
        let (window, account) = live.unwrap_or_else(|| {
            locate(
                journal,
                &stored.day,
                &stored.stream,
                &stored.row_id,
                (&stored.stored_start, &stored.stored_end),
                &mut self.days,
            )
        });
        ComposedBody {
            id: stored.id,
            facet: stored.facet,
            day: stored.day,
            activity: stored.activity,
            key: stored.key,
            stream: stored.stream,
            stored_start: stored.stored_start,
            stored_end: stored.stored_end,
            window,
            account,
            owner: stored.owner,
            hidden: stored.hidden,
            apart: stored.apart,
        }
    }
}

/// The body records in `facet` on `day` that this reader may read, composed.
/// A record whose window it may not read isn't returned at all.
///
/// # Errors
///
/// When the facet's activity file can't be read.
pub fn compose_body_records(
    journal: &Path,
    facet: &str,
    day: &str,
    readable: Readable<'_>,
    include_hidden: bool,
) -> Result<Vec<ComposedBody>, ComposeError> {
    let mut windows = Windows::default();
    let mut composed = Vec::new();
    for stored in stored_body_records(journal, facet, day)? {
        if stored.hidden && !include_hidden {
            continue;
        }
        if windows.may_read(journal, &stored, readable) {
            composed.push(windows.compose(journal, stored));
        }
    }
    Ok(composed)
}

/// Whether two accounts are one experience told twice: the same kind (and a
/// workout's same shared type), different recorders, overlapping by at least
/// 80% of the shorter and 60% of the longer on their stored instants, not kept
/// apart, and not both edited by the owner. Both are in one facet by the
/// caller's choice of records.
#[must_use]
pub fn pair(left: &StoredBody, right: &StoredBody) -> bool {
    pair_on(left, right, left.instants(), right.instants())
}

/// `pair`, on instants already read.
fn pair_on(
    left: &StoredBody,
    right: &StoredBody,
    left_at: Option<(i64, i64)>,
    right_at: Option<(i64, i64)>,
) -> bool {
    if left.activity != right.activity
        || left.recorder == right.recorder
        || left.apart.contains(&right.key)
        || right.apart.contains(&left.key)
        || (!left.owner.is_empty() && !right.owner.is_empty())
    {
        return false;
    }
    if left.activity == "workout"
        && (left.workout_type.is_none() || left.workout_type != right.workout_type)
    {
        return false;
    }
    let (Some((a_start, a_end)), Some((b_start, b_end))) = (left_at, right_at) else {
        return false;
    };
    let overlap = (a_end.min(b_end) - a_start.max(b_start)).max(0);
    let (a, b) = (a_end - a_start, b_end - b_start);
    let (shorter, longer) = (a.min(b), a.max(b));
    // Integer form of overlap >= 0.8 * shorter and >= 0.6 * longer.
    overlap * 5 >= shorter * 4 && overlap * 5 >= longer * 3
}

/// The records as groups: each a list of indices, singles included, in the
/// order of their first member. A set of accounts that pass pairwise is a group
/// only when every two of them pass, so no recorder appears twice; otherwise
/// every account in it stands alone. Nothing about membership is stored, so the
/// same records always give the same groups, in whatever order they came.
#[must_use]
pub fn group_partition(records: &[StoredBody]) -> Vec<Vec<usize>> {
    let count = records.len();
    let at = records.iter().map(StoredBody::instants).collect::<Vec<_>>();
    let mut component = (0..count).collect::<Vec<_>>();
    fn root(component: &mut [usize], index: usize) -> usize {
        let mut at = index;
        while component[at] != at {
            component[at] = component[component[at]];
            at = component[at];
        }
        at
    }
    // Two accounts pair only when they overlap, so each is weighed only against
    // those that start before it ends.
    let mut timed = (0..count)
        .filter_map(|index| at[index].map(|(start, end)| (start, end, index)))
        .collect::<Vec<_>>();
    timed.sort_unstable();
    for (position, &(_, left_end, left)) in timed.iter().enumerate() {
        for &(right_start, _, right) in &timed[position + 1..] {
            if right_start >= left_end {
                break;
            }
            if pair_on(&records[left], &records[right], at[left], at[right]) {
                let (a, b) = (root(&mut component, left), root(&mut component, right));
                component[a.max(b)] = a.min(b);
            }
        }
    }
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for index in 0..count {
        let at = root(&mut component, index);
        members.entry(at).or_default().push(index);
    }
    let mut groups = Vec::new();
    for (_, set) in members {
        let clique = set.iter().enumerate().all(|(position, &left)| {
            set[position + 1..]
                .iter()
                .all(|&right| pair_on(&records[left], &records[right], at[left], at[right]))
        });
        if set.len() > 1 && clique {
            groups.push(set);
        } else {
            groups.extend(set.into_iter().map(|index| vec![index]));
        }
    }
    groups.sort_by_key(|group| group[0]);
    groups
}

/// The member a group shows as: the one holding an owner field, else the
/// earliest stored start, ties broken by key.
#[must_use]
pub fn anchor(records: &[StoredBody], group: &[usize]) -> usize {
    if let Some(&held) = group
        .iter()
        .find(|&&index| !records[index].owner.is_empty())
    {
        return held;
    }
    *group
        .iter()
        .min_by_key(|&&index| {
            (
                records[index]
                    .instants()
                    .map_or(i64::MAX, |(start, _)| start),
                records[index].key.clone(),
            )
        })
        .expect("a group has a member")
}

/// Whether the journal holds body data at all: a body source placed windows,
/// or Strava placed tiles. A journal without it opens no body file in any reader.
#[must_use]
pub fn has_body_data(journal: &Path) -> bool {
    solstone_core_format::content::body_records::BODY_RECORD_STREAMS
        .iter()
        .chain(std::iter::once(&STRAVA_STREAM))
        .any(|stream| {
            journal
                .join(format!("imports/.{}-zone.json", source_of(stream)))
                .is_file()
        })
}

/// A group, or one account alone, as a reader shows it: under its anchor's day.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub anchor: ComposedBody,
    /// Every member, the anchor included, earliest first.
    pub members: Vec<ComposedBody>,
    /// Hidden when any member is.
    pub hidden: bool,
}

/// How many days back a group's members can be filed, and forward.
const GROUP_DAYS_BACK: i64 = 14;
const GROUP_DAYS_FORWARD: i64 = 1;

/// The days after `day` a group anchored there can have members filed on, as
/// far as a member filed on `day` can be grouped with: where a reader looks for
/// the face a member of `day` is shown under.
#[must_use]
pub fn days_after(day: &str) -> Vec<String> {
    let Ok(date) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return Vec::new();
    };
    // A member filed on `day` shows under an anchor at most this far either way.
    (-GROUP_DAYS_FORWARD..=GROUP_DAYS_BACK)
        .filter(|offset| *offset != 0)
        .filter_map(|offset| date.checked_sub_signed(chrono::Duration::days(offset)))
        .map(|date| date.format("%Y%m%d").to_string())
        .collect()
}

fn days_around(day: &str) -> Vec<String> {
    let Ok(date) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return vec![day.to_owned()];
    };
    (-GROUP_DAYS_BACK..=GROUP_DAYS_FORWARD)
        .filter_map(|offset| date.checked_add_signed(chrono::Duration::days(offset)))
        .map(|date| date.format("%Y%m%d").to_string())
        .collect()
}

/// The faces a reader shows for `facet` on `day`: groups worked out over the
/// records it may read in the days around, shown under their anchor's day
/// only. Only members whose windows this reader may read take part, so a face
/// never carries an account, an anchor or an owner field from one it can't.
///
/// # Errors
///
/// When an activity file can't be read.
pub fn body_faces(
    journal: &Path,
    facet: &str,
    day: &str,
    readable: Readable<'_>,
    include_hidden: bool,
) -> Result<Vec<Face>, ComposeError> {
    Ok(
        body_faces_range(journal, facet, day, day, Some(readable), include_hidden)?
            .remove(day)
            .unwrap_or_default(),
    )
}

/// [`body_faces`] for every anchor day from `from` to `to`, read, gated and
/// grouped once for the whole span. With no `readable`, the owner's own view,
/// which reads every window, so nothing is gated.
///
/// # Errors
///
/// When an activity file can't be read.
pub fn body_faces_range(
    journal: &Path,
    facet: &str,
    from: &str,
    to: &str,
    readable: Option<Readable<'_>>,
    include_hidden: bool,
) -> Result<BTreeMap<String, Vec<Face>>, ComposeError> {
    Ok(body_groups_range(journal, facet, from, to, readable, include_hidden)?.into_faces(journal))
}

/// One group as read, before its accounts are composed.
#[derive(Clone, Debug)]
pub struct Group {
    /// The id of the record it shows as.
    pub anchor_id: String,
    /// Hidden when any member is.
    pub hidden: bool,
    at: usize,
    order: Vec<usize>,
}

/// A facet's groups over a span of days, by the day each shows under, grouped
/// but composed only when asked: a reader that shows a few composes a few.
#[derive(Default)]
pub struct Groups {
    records: Vec<StoredBody>,
    windows: Windows,
    by_day: BTreeMap<String, Vec<Group>>,
}

impl Groups {
    /// The groups shown under `day`, in order.
    #[must_use]
    pub fn on(&self, day: &str) -> &[Group] {
        self.by_day.get(day).map_or(&[], Vec::as_slice)
    }

    /// The group as its windows say now.
    pub fn compose(&mut self, journal: &Path, group: &Group) -> Face {
        let members = group
            .order
            .iter()
            .map(|&index| self.windows.compose(journal, self.records[index].clone()))
            .collect::<Vec<_>>();
        let anchor = members
            .iter()
            .find(|member| member.id == self.records[group.at].id)
            .expect("the anchor is a member")
            .clone();
        Face {
            anchor,
            members,
            hidden: group.hidden,
        }
    }

    /// Every group composed, by day.
    #[must_use]
    pub fn into_faces(mut self, journal: &Path) -> BTreeMap<String, Vec<Face>> {
        let by_day = std::mem::take(&mut self.by_day);
        by_day
            .into_iter()
            .map(|(day, groups)| {
                let faces = groups
                    .iter()
                    .map(|group| self.compose(journal, group))
                    .collect();
                (day, faces)
            })
            .collect()
    }
}

/// [`body_faces_range`] without composing: read, gated and grouped once.
///
/// # Errors
///
/// When an activity file can't be read.
pub fn body_groups_range(
    journal: &Path,
    facet: &str,
    from: &str,
    to: &str,
    readable: Option<Readable<'_>>,
    include_hidden: bool,
) -> Result<Groups, ComposeError> {
    let mut groups = Groups {
        records: Vec::new(),
        windows: Windows::default(),
        by_day: BTreeMap::new(),
    };
    if !has_body_data(journal) {
        return Ok(groups);
    }
    let (Ok(first), Ok(last)) = (
        NaiveDate::parse_from_str(from, "%Y%m%d"),
        NaiveDate::parse_from_str(to, "%Y%m%d"),
    ) else {
        return Ok(groups);
    };
    let start = (first - chrono::Duration::days(GROUP_DAYS_BACK))
        .format("%Y%m%d")
        .to_string();
    let end = (last + chrono::Duration::days(GROUP_DAYS_FORWARD))
        .format("%Y%m%d")
        .to_string();
    let mut every = Vec::new();
    // Only the days the facet has a file for: one listing, not a look per day.
    let days = solstone_core_facets::list_activity_day_stems(journal, facet).unwrap_or_default();
    for day in days
        .iter()
        .filter(|day| day.as_str() >= start.as_str() && day.as_str() <= end.as_str())
    {
        // A day's file that can't be read at all costs only its records' part
        // in grouping: the day's own reader reports it.
        every.extend(stored_body_records(journal, facet, day).unwrap_or_default());
    }
    // Hidden is the owner's word on the group as the owner sees it: a member
    // in one with a hidden account hides for every reader, whatever else that
    // reader may read.
    let mut hides = BTreeSet::new();
    if readable.is_some() {
        for group in group_partition(&every) {
            if group.iter().any(|&index| every[index].hidden) {
                hides.extend(group.iter().map(|&index| every[index].id.clone()));
            }
        }
    }
    for stored in every {
        if readable.is_none_or(|readable| groups.windows.may_read(journal, &stored, readable)) {
            groups.records.push(stored);
        }
    }
    let records = &groups.records;
    for group in group_partition(records) {
        let at = anchor(records, &group);
        let day = records[at].day.clone();
        if day.as_str() < from || day.as_str() > to {
            continue;
        }
        let hidden = group
            .iter()
            .any(|&index| records[index].hidden || hides.contains(&records[index].id));
        if hidden && !include_hidden {
            continue;
        }
        let mut order = group;
        order.sort_by_key(|&index| {
            (
                records[index]
                    .instants()
                    .map_or(i64::MAX, |(start, _)| start),
                records[index].key.clone(),
            )
        });
        groups.by_day.entry(day).or_default().push(Group {
            anchor_id: records[at].id.clone(),
            hidden,
            at,
            order,
        });
    }
    Ok(groups)
}

/// The group `record_id` is in as the owner sees it, itself included: what a
/// hide or unhide writes. Alone, it is its own group.
///
/// # Errors
///
/// When an activity file can't be read.
pub fn owner_group(
    journal: &Path,
    facet: &str,
    day: &str,
    record_id: &str,
) -> Result<Vec<StoredBody>, ComposeError> {
    let mut records = Vec::new();
    for on in days_around(day) {
        records.extend(stored_body_records(journal, facet, &on)?);
    }
    let Some(at) = records
        .iter()
        .position(|record| record.id == record_id && record.day == day)
    else {
        return Ok(Vec::new());
    };
    let group = group_partition(&records)
        .into_iter()
        .find(|group| group.contains(&at))
        .unwrap_or_else(|| vec![at]);
    Ok(group
        .into_iter()
        .map(|index| records[index].clone())
        .collect())
}

/// The faces the owner's home shows for `day`, composed when read: from every
/// unmuted facet, whose anchor starts on `day` by the owner's clock, unhidden,
/// and with at least one account still there.
///
/// # Errors
///
/// Any read error while gathering: the caller leaves the section out.
pub fn day_faces(journal: &Path, day: &str) -> Result<Vec<Face>, ComposeError> {
    if !has_body_data(journal) {
        return Ok(Vec::new());
    }
    let date = NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|error| ComposeError(error.to_string()))?;
    let facets = solstone_core_facets::list_declared_facet_names(journal)
        .map_err(|error| ComposeError(error.to_string()))?;
    let mut muted = BTreeSet::new();
    for facet in &facets {
        if solstone_core_facets::observe_facet_declaration(journal, facet)
            .map_err(|error| ComposeError(error.to_string()))?
            .is_some_and(|declaration| declaration.muted == Some(true))
        {
            muted.insert(facet.clone());
        }
    }
    // A window refiled to a muted facet is muted too, wherever its record is filed.
    let in_muted_window = |member: &ComposedBody| {
        member.window.as_ref().is_some_and(|window| {
            solstone_core_indexer_store::classification::read_segment_facet_assignments(
                journal,
                &format!(
                    "{}/{}/{window}/{BODY_RECORDS_FILE}",
                    member.day, member.stream
                ),
            )
            .iter()
            .any(|facet| muted.contains(facet))
        })
    };
    let mut faces = Vec::new();
    for facet in &facets {
        if muted.contains(facet) {
            continue;
        }
        let mut records = Vec::new();
        for offset in -(GROUP_DAYS_BACK + 1)..=(GROUP_DAYS_FORWARD + 1) {
            let on = (date + chrono::Duration::days(offset))
                .format("%Y%m%d")
                .to_string();
            records.extend(stored_body_records(journal, facet, &on)?);
        }
        let mut windows = Windows::default();
        for group in group_partition(&records) {
            let at = anchor(&records, &group);
            let Some((start, _)) = records[at].instants() else {
                continue;
            };
            if owner_moment(journal, start).map(|(on, _)| on).as_deref() != Some(day)
                || group.iter().any(|&index| records[index].hidden)
            {
                continue;
            }
            let mut order = group.clone();
            order.sort_by_key(|&index| {
                (
                    records[index]
                        .instants()
                        .map_or(i64::MAX, |(start, _)| start),
                    records[index].key.clone(),
                )
            });
            let members = order
                .iter()
                .map(|&index| windows.compose(journal, records[index].clone()))
                .collect::<Vec<_>>();
            if members
                .iter()
                .any(|member| matches!(member.account, Account::Unreadable))
            {
                return Err(ComposeError("a body window couldn't be read".to_owned()));
            }
            if members.iter().any(in_muted_window) {
                continue;
            }
            if !members.iter().any(|member| member.still_there()) {
                continue;
            }
            let anchor = members
                .iter()
                .find(|member| member.id == records[at].id)
                .expect("the anchor is a member")
                .clone();
            faces.push(Face {
                anchor,
                members,
                hidden: false,
            });
        }
    }
    // By instant: sources write their times in different forms and offsets, so
    // their text doesn't sort as time does.
    faces.sort_by_cached_key(|face| {
        (
            parse_source_time(&face.anchor.stored_start).map(|start| start.timestamp()),
            face.anchor.stored_start.clone(),
        )
    });
    Ok(faces)
}

/// The one value a line says, beside what happened and when. Adding one for a
/// source is a deliberate choice: energy and every statistic stay out.
pub const LINE_VALUE: &str = "distance";

/// A distance as a person says it, from a source's exact value and its unit:
/// metres under a kilometre in whole metres, otherwise kilometres to one
/// decimal; yards and feet whole; and any other unit kept, to one decimal. A
/// number with no unit can't be said without guessing what it measures, so it
/// isn't, and neither is a distance that reads as nothing. The source's value
/// stays exact in `values`.
#[must_use]
pub fn line_distance(value: &str) -> Option<String> {
    let (number, unit) = value.trim().split_once(char::is_whitespace)?;
    let unit = unit.trim();
    if unit.is_empty() || unit.chars().any(char::is_whitespace) {
        return None;
    }
    let number: f64 = number
        .parse()
        .ok()
        .filter(|number: &f64| number.is_finite())?;
    let said = match unit {
        "m" if number.round() >= 1000.0 => format!("{:.1} km", number / 1000.0),
        // Units nobody says in tenths: a pool is counted in whole yards.
        "m" | "yd" | "ft" => format!("{number:.0} {unit}"),
        _ => format!("{number:.1} {unit}"),
    };
    let shown: f64 = said.split_whitespace().next()?.parse().ok()?;
    (shown > 0.0).then_some(said)
}

/// A time as the owner reads it, on their clock and with no offset: the hour
/// and minute, and the date only when it isn't the day's. The reader formats it.
fn clock_time(text: Option<&str>, zone: Tz, day: &str) -> Option<Value> {
    let local = parse_source_time(text?)?.with_timezone(&zone);
    let date = local.format("%Y%m%d").to_string();
    Some(serde_json::json!({
        "time": local.format("%H:%M").to_string(),
        "date": (date != day).then_some(date),
    }))
}

/// One group as the owner's home shows it: what, when, how far, and what
/// recorded it, as data the reader words. No assessment and no value but
/// distance. Accounts that aren't there aren't said; a group with none says
/// nothing. A name the source or this code gives is lowercase; the owner's own
/// title keeps theirs.
#[must_use]
pub fn face_line(face: &Face, zone: Tz, day: &str) -> Option<Value> {
    let there = face
        .members
        .iter()
        .filter_map(|member| match &member.account {
            Account::Live(row) => Some((row.as_ref(), false)),
            Account::PartRemoved(row) => Some((row.as_ref(), true)),
            _ => None,
        })
        .collect::<Vec<(&BodyRow, bool)>>();
    let &(first, partial) = there.first()?;
    let title = face
        .anchor
        .owner
        .iter()
        .find(|(field, _)| field.as_str() == "title")
        .and_then(|(_, value)| value.as_str())
        .map(str::trim)
        .filter(|title| !title.is_empty());
    let name = match (title, face.anchor.activity.as_str()) {
        (Some(title), _) => title.to_owned(),
        (None, "sleep") => "sleep".to_owned(),
        (None, "mindfulness") => "mindfulness".to_owned(),
        (None, _) => first.label.to_lowercase(),
    };
    let logged = there.iter().all(|(row, _)| user_entered(row));
    let mut from = Vec::new();
    if !logged {
        for (row, _) in &there {
            if let Some(by) = row.by.as_deref().map(str::trim).filter(|by| !by.is_empty())
                && !from.contains(&by)
            {
                from.push(by);
            }
        }
    }
    // The first whole account that says one: a part-removed account's value
    // was for a span the line no longer shows.
    let distance = (!partial)
        .then(|| {
            there
                .iter()
                .filter(|(_, partial)| !partial)
                .find_map(|(row, _)| row.values.get(LINE_VALUE).cloned())
        })
        .flatten()
        .and_then(|value| line_distance(&value));
    Some(serde_json::json!({
        "name": name,
        "start": clock_time(first.start.as_deref(), zone, day),
        "end": clock_time(first.end.as_deref(), zone, day),
        "distance": distance,
        "from": from,
        "logged": logged,
        "part_removed": partial,
    }))
}

/// The lines the owner's home shows for `day`, one per group, composed when
/// read and never stored.
///
/// # Errors
///
/// Any read error while gathering: the caller leaves the section out.
pub fn day_lines(journal: &Path, day: &str) -> Result<Vec<Value>, ComposeError> {
    let faces = day_faces(journal, day)?;
    if faces.is_empty() {
        return Ok(Vec::new());
    }
    let zone = solstone_core_journal_config::owner_zone(journal);
    Ok(faces
        .iter()
        .filter_map(|face| face_line(face, zone, day))
        .collect())
}

fn account_json(member: &ComposedBody) -> Value {
    let state = match &member.account {
        Account::Live(_) => "live",
        Account::PartRemoved(_) => "part_removed",
        Account::RemovedByYou => "removed_by_you",
        Account::Removed => "removed",
        Account::NotFound => "not_found",
        Account::Unreadable => "unreadable",
    };
    let mut account = Map::new();
    account.insert("id".to_owned(), Value::String(member.id.clone()));
    account.insert("day".to_owned(), Value::String(member.day.clone()));
    account.insert("state".to_owned(), Value::String(state.to_owned()));
    account.insert(
        "source".to_owned(),
        Value::String(source_of(&member.stream).to_owned()),
    );
    match member.row() {
        Some(row) => {
            account.insert("label".to_owned(), Value::String(row.label.clone()));
            account.insert("start".to_owned(), json_text(row.start.as_deref()));
            account.insert("end".to_owned(), json_text(row.end.as_deref()));
            account.insert("by".to_owned(), json_text(row.by.as_deref()));
            // A night the source assembled from stages says so, and how.
            if let Some(assembled) = &row.assembled {
                account.insert("assembled".to_owned(), Value::String(assembled.clone()));
            }
            if matches!(member.account, Account::Live(_)) {
                account.insert(
                    "values".to_owned(),
                    Value::Object(
                        row.values
                            .iter()
                            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                            .collect(),
                    ),
                );
            }
        }
        // A deleted account keeps its kind and instants, as HEAD's records do.
        None => {
            account.insert(
                "start".to_owned(),
                Value::String(member.stored_start.clone()),
            );
            account.insert("end".to_owned(), Value::String(member.stored_end.clone()));
        }
    }
    Value::Object(account)
}

fn json_text(text: Option<&str>) -> Value {
    text.map_or(Value::Null, |text| Value::String(text.to_owned()))
}

/// A face as data for a reader: the anchor's id, facet and day, the owner's
/// fields, hidden, and each account with its own composed line. Nothing is
/// summed or chosen across accounts.
#[must_use]
pub fn face_json(face: &Face) -> Value {
    let mut item = Map::new();
    item.insert("id".to_owned(), Value::String(face.anchor.id.clone()));
    item.insert("facet".to_owned(), Value::String(face.anchor.facet.clone()));
    item.insert("day".to_owned(), Value::String(face.anchor.day.clone()));
    item.insert("source".to_owned(), Value::String("body".to_owned()));
    item.insert(
        "activity".to_owned(),
        Value::String(face.anchor.activity.clone()),
    );
    item.insert("hidden".to_owned(), Value::Bool(face.hidden));
    for (field, value) in &face.anchor.owner {
        item.insert(field.clone(), value.clone());
    }
    item.insert(
        "accounts".to_owned(),
        Value::Array(face.members.iter().map(account_json).collect()),
    );
    Value::Object(item)
}

/// Whether two spans sit beside each other: they overlap by at least half of
/// either one, as a calendar entry sits beside an activity, on instants.
#[must_use]
pub fn half_cover(left: (i64, i64), right: (i64, i64)) -> bool {
    let overlap = left.1.min(right.1) - left.0.max(right.0);
    overlap > 0 && (overlap * 2 >= left.1 - left.0 || overlap * 2 >= right.1 - right.0)
}

/// Whether any account of the face is still there to show: one beside line
/// stands for what happened, so a face whose every account is gone has none.
#[must_use]
pub fn face_still_there(face: &Face) -> bool {
    face.members.iter().any(ComposedBody::still_there)
}

/// A face's span as it's shown: its first account's instants, as its window
/// says them now, else as stored. Beside lines go by this, so a night a later
/// export ended later sits beside what it now covers.
#[must_use]
pub fn shown_span(face: &Face) -> Option<(i64, i64)> {
    let first = face.members.first()?;
    let (start, end) = match first.row() {
        Some(row) => (row.start.clone()?, row.end.clone()?),
        None => (first.stored_start.clone(), first.stored_end.clone()),
    };
    let start = parse_source_time(&start)?.timestamp();
    let end = parse_source_time(&end)?.timestamp();
    (end > start).then_some((start, end))
}

/// A capture segment's span as instants: its key on the zone the device
/// reported, else on the owner's.
fn segment_instants(journal: &Path, day: &str, dir: &Path, key: &str) -> Option<(i64, i64)> {
    use chrono::{Offset, TimeZone};
    let (start, end) = window_span(key)?;
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let wall = date.and_hms_opt(0, 0, 0)? + chrono::Duration::seconds(i64::from(start));
    let owner = solstone_core_journal_config::owner_zone(journal);
    let reported = solstone_core_callosum::read_reported_zone(journal, dir).and_then(|zone| {
        solstone_core_journal_config::capture_zone_view(
            day,
            key,
            zone.tz.as_deref(),
            zone.utc_offset_seconds,
            &owner,
        )
    });
    let offset = match reported {
        Some(view) => view.utc_offset_seconds,
        None => owner
            .offset_from_local_datetime(&wall)
            .earliest()?
            .fix()
            .local_minus_utc(),
    };
    let begin = wall.and_utc().timestamp() - i64::from(offset);
    Some((begin, begin + i64::from(end - start)))
}

/// A capture activity's span as instants, from the segments it names. A key
/// that sits in no capture segment is left out.
#[must_use]
pub fn capture_instants(
    journal: &Path,
    day: &str,
    record: &Map<String, Value>,
) -> Option<(i64, i64)> {
    let stream = record
        .get("stream")
        .and_then(Value::as_str)
        .filter(|stream| !stream.is_empty());
    let day_dir = journal.join("chronicle").join(day);
    let mut span: Option<(i64, i64)> = None;
    for key in record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let dirs = match stream {
            Some(stream) if stream == solstone_core_journal_io::DEFAULT_STREAM => {
                vec![day_dir.join(key)]
            }
            Some(stream) => vec![day_dir.join(stream).join(key)],
            None => {
                let mut dirs = vec![day_dir.join(key)];
                for entry in fs::read_dir(&day_dir).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !solstone_core_format::content::body_records::BODY_RECORD_STREAMS
                        .contains(&name.as_str())
                    {
                        dirs.push(entry.path().join(key));
                    }
                }
                dirs
            }
        };
        if let Some(found) = dirs
            .iter()
            .filter(|dir| dir.is_dir())
            .find_map(|dir| segment_instants(journal, day, dir, key))
        {
            span = Some(span.map_or(found, |(start, end)| (start.min(found.0), end.max(found.1))));
        }
    }
    span
}

/// The owner's day an instant falls on, and its seconds into that day: where to
/// look up the place it was in. A night's middle is usually on the next day.
#[must_use]
pub fn owner_moment(journal: &Path, instant: i64) -> Option<(String, u32)> {
    let owner = solstone_core_journal_config::owner_zone(journal);
    let time = DateTime::from_timestamp(instant, 0)?.with_timezone(&owner);
    Some((
        time.date_naive().format("%Y%m%d").to_string(),
        time.num_seconds_from_midnight(),
    ))
}

/// The day after `day`, where a night that began on it ends.
#[must_use]
pub fn next_day(day: &str) -> Option<String> {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()?
        .succ_opt()
        .map(|date| date.format("%Y%m%d").to_string())
}

/// The actor of a separate. No route takes it back.
pub const SEPARATE_ACTOR: &str = "owner:separate";

/// Keep `record` apart from each of `others` for good: the owner's undo of a
/// group. Each side names the other in `apart`, one write per record, and
/// each write stands alone, so a stop after the first already separates them.
/// Nothing is deleted or moved.
///
/// # Errors
///
/// The first write the store refuses; writes before it stand.
pub fn separate_account(
    journal: &Path,
    record: &StoredBody,
    others: &[StoredBody],
    timestamp: &str,
) -> Result<(), ComposeError> {
    let keep_apart = |target: &StoredBody, keys: Vec<String>| {
        update_activity_record_with(
            journal,
            &target.facet,
            &target.day,
            &target.id,
            |current| {
                let mut apart = current
                    .get("apart")
                    .and_then(Value::as_array)
                    .map(|keys| {
                        keys.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_else(Vec::new);
                let before = apart.len();
                for key in keys {
                    if !apart.contains(&key) {
                        apart.push(key);
                    }
                }
                (apart.len() > before).then(|| {
                    Map::from_iter([(
                        "apart".to_owned(),
                        Value::Array(apart.into_iter().map(Value::String).collect()),
                    )])
                })
            },
            SEPARATE_ACTOR,
            "separated",
            timestamp,
        )
        .map(|_| ())
        .map_err(|error| ComposeError(error.to_string()))
    };
    keep_apart(
        record,
        others.iter().map(|other| other.key.clone()).collect(),
    )?;
    for other in others {
        keep_apart(other, vec![record.key.clone()])?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "body_records_tests.rs"]
mod tests;
