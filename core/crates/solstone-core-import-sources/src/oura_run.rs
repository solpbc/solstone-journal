// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Staged Oura pages, read into the shared writer's scan runs.
//!
//! Pages sit in a folder with an index, in the order they were fetched:
//!
//! ```text
//! <pages>/index.json                    {"pages": [{"endpoint", "chunk", "n", "file", "sha256"}, ...]}
//! <pages>/<endpoint>/<chunk>-<n>.json   the body exactly as Oura returned it
//! ```
//!
//! A heartrate page the fetch staged also names its chunk's instants, `from` and
//! `to`, and only its readings in `from <= at < to` are read. Chunks meet end to
//! start, so a reading at a boundary comes from one chunk whichever way Oura reads
//! the end it was asked for.
//!
//! This is Oura's half of the scan phase: the place and days phases, and
//! everything after them, are the writer Apple's export uses. The scan keeps one
//! version of each Oura document, the last fetched, so the placer never sees two
//! contents for one id. Before a document is staged, it is looked up where it
//! could already be stored.
//!
//! A document's live rows end up exactly its new tiling. Every other live row of
//! it, a first row in another window or a tile its new span doesn't have, is
//! staged as a `moved` mark in that row's own window, so a start that moves, an
//! end that shrinks and a tile named by an older scheme all converge alike. That
//! holds while the start stays on its owner day, by the fixed zone. A start that
//! crosses the owner's midnight isn't staged at all, so nothing is placed by a
//! span that isn't written.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path};

use chrono::{DateTime, Duration, FixedOffset, NaiveDate};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_format::content::body_records::{BodyRow, RowKind, parse_source_time};
use solstone_core_format::content::is_body_record;
use solstone_core_segment::{is_body_only_dir, owner_deleted};

use crate::apple_health::{
    HOUR_SECONDS, MAX_ROW_BYTES, StoredRowsError, WINDOW_SECONDS, WindowId, floor_to_window,
    resolve_key, row_window, stored_rows, stream_dir, tile_experience, window_id, window_seconds,
};
use crate::apple_health_run::{ScanCounts, ScanItem, Spill, placed_identity};
use crate::body_stream::BodySource;
use crate::oura::{Document, Hole, HoleReason, Shape, holes_of, read_document, read_endpoint};

/// The index a pages folder is read by.
pub const INDEX_FILE: &str = "index.json";

#[derive(Debug, Deserialize)]
struct Index {
    pages: Vec<PageEntry>,
}

/// One staged page, as the index names it.
#[derive(Debug, Deserialize)]
struct PageEntry {
    endpoint: String,
    file: String,
    sha256: String,
    /// A heartrate chunk's first instant, when the fetch staged it.
    #[serde(default)]
    from: Option<String>,
    /// The instant after its last.
    #[serde(default)]
    to: Option<String>,
}

impl PageEntry {
    /// Whether a reading at `at` is this page's to read.
    fn holds(&self, at: Option<&str>) -> bool {
        let (Some(from), Some(to)) = (self.from.as_deref(), self.to.as_deref()) else {
            return true;
        };
        let (Some(from), Some(to), Some(at)) = (
            parse_source_time(from),
            parse_source_time(to),
            at.and_then(parse_source_time),
        ) else {
            // A reading with no time is the reader's to refuse.
            return true;
        };
        from <= at && at < to
    }
}

fn io_text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_index(pages: &Path) -> Result<Vec<PageEntry>, String> {
    let path = pages.join(INDEX_FILE);
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let index: Index =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    for page in &index.pages {
        // A page is a file inside the folder, never a path out of it.
        let inside = !page.file.is_empty()
            && Path::new(&page.file)
                .components()
                .all(|part| matches!(part, Component::Normal(_)));
        if !inside {
            return Err(format!(
                "{}: a page isn't a file in its folder: {}",
                path.display(),
                page.file
            ));
        }
    }
    Ok(index.pages)
}

/// What a set of staged pages is, for carrying on a run over them: the hash of
/// every page's hash, in the order fetched.
pub fn pages_identity(pages: &Path) -> Result<String, String> {
    let entries = read_index(pages)?;
    let mut joined = String::new();
    for entry in &entries {
        joined.push_str(&entry.sha256.to_ascii_lowercase());
        joined.push('\n');
    }
    Ok(hex(joined.as_bytes()))
}

/// A page's bytes, refused when they aren't what its index recorded.
fn page_bytes(pages: &Path, entry: &PageEntry) -> Result<Vec<u8>, String> {
    let path = pages.join(&entry.file);
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if hex(&bytes) != entry.sha256.to_ascii_lowercase() {
        return Err(format!(
            "{}: the staged page doesn't match the hash its index records",
            path.display()
        ));
    }
    Ok(bytes)
}

/// A page's documents, or `None` when it isn't a page.
fn page_items(bytes: &[u8]) -> Option<Vec<Value>> {
    match serde_json::from_slice::<Value>(bytes).ok()? {
        Value::Object(mut page) => match page.remove("data")? {
            Value::Array(items) => Some(items),
            _ => None,
        },
        _ => None,
    }
}

/// A document whose stored rows this run converges to its new tiling: what the
/// writer needs to decide, at its start day, whether the owner deleted where it
/// moves from or to, and to say when it is done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Converging {
    /// The owner day its start is on, before and after: the day written first.
    pub day: String,
    /// The windows its live first rows were in when the scan read them.
    pub firsts: Vec<WindowId>,
    /// Its new first window, before any step around older content.
    pub first: WindowId,
    /// Whether Oura changed its times or its window's length: no first row was
    /// already in its new first window with its new span.
    pub revised: bool,
    /// What it is a hole in until it is done.
    pub holes: BTreeSet<Hole>,
}

/// Separates an endpoint from its days in [`ScanFindings::unsure`]'s keys.
pub const UNSURE_SEPARATOR: char = '\u{1f}';

/// What the scan found besides its counts. Kept in the run's state with the scan,
/// so a resumed run reports what the scan it carries on from found.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFindings {
    /// Revisions kept as first written: a start that crosses the owner's midnight,
    /// or a placement or record on another day.
    #[serde(default)]
    pub deferred: u64,
    /// Documents whose stored experience the owner deleted.
    #[serde(default)]
    pub stayed_deleted: u64,
    /// Documents Oura moved further than a day, kept where the journal first
    /// wrote them.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub moved_far: u64,
    /// Documents held back, by endpoint, because a window they could already be in
    /// couldn't be read as rows.
    #[serde(default)]
    pub held: BTreeMap<String, u64>,
    #[serde(default)]
    pub holes: BTreeSet<Hole>,
    /// Documents with no record, not added because the owner deleted part of the
    /// days they could have been on: keyed by the endpoint and those days, joined
    /// by [`UNSURE_SEPARATOR`], how many.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unsure: BTreeMap<String, u64>,
    /// Oura nights in this import that overlap another of its nights. Oura gives a
    /// night one id, so two that overlap can be one night revised under a new id.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub overlapping_nights: u64,
    /// Plain sentences for the summary.
    #[serde(default)]
    pub notes: Vec<String>,
    /// Documents this run converges, by row id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub converging: BTreeMap<String, Converging>,
}

impl ScanFindings {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// An Oura experience's or tag's row in a stored window.
struct Placed {
    key: String,
    start: Option<String>,
    end: Option<String>,
}

/// What a day's stream holds that the lookup asks about: ids, windows, `of` and
/// spans, never row contents.
#[derive(Default)]
struct DayIndex {
    /// Experience and logged rows by id.
    firsts: HashMap<String, Vec<Placed>>,
    /// Live tiles by the id of their experience: each one's window and id.
    tiles: HashMap<String, Vec<(String, String)>>,
    /// A window here can't be read as rows.
    unparseable: bool,
    /// Body records filed on this day, by account key: the account's stored
    /// start and its activity.
    records: HashMap<String, (String, String)>,
}

/// The rows a document converges: each live row its new tiling doesn't have, by
/// window and id, and what the writer needs to know of it.
#[derive(Debug)]
struct Plan {
    marks: Vec<(WindowId, String)>,
    converging: Converging,
}

/// What the lookup decided for one document.
#[derive(Debug)]
enum Verdict {
    /// Staged, with the marks that converge what is stored to it, if any.
    Stage(Option<Box<Plan>>),
    /// Kept as first written: its start crosses the owner's midnight.
    Defer,
    /// A window it could already be in can't be read, so nothing is added on a guess.
    Hold,
    /// The owner deleted its stored experience, or where it would go.
    StayedDeleted,
    /// Kept as first written: Oura moved it further than a day, and the journal
    /// still holds it on the day it first wrote it.
    MovedFar,
}

/// A span's two ends as instants, for telling a revised time from a rendering.
fn instants(start: Option<&str>, end: Option<&str>) -> (Option<i64>, Option<i64>) {
    let at = |text: Option<&str>| {
        text.and_then(parse_source_time)
            .map(|time| time.timestamp_millis())
    };
    (at(start), at(end))
}

struct Lookup<'a> {
    journal: &'a Path,
    source: &'a BodySource,
    zone: Tz,
    days: HashMap<String, DayIndex>,
    /// A re-import's (an explicit range): what the owner deleted is no reason to
    /// leave a document out. An ordinary sync's lookup keeps it out.
    comes_back: bool,
    /// An ordinary sync's record of what Oura runs brought in: a document in it
    /// that the journal no longer holds anywhere it could be is one the owner
    /// deleted, and stays out with its tiles, a revision of it too.
    fetched: Option<&'a solstone_core_import::oura_fetched::Fetched>,
}

fn day_key(date: NaiveDate) -> String {
    date.format("%Y%m%d").to_string()
}

impl Lookup<'_> {
    /// Oura's own date and the fixed-zone day of the new start, each with the days
    /// either side: Oura's dates are the wearer's, and placement is the fixed zone's.
    fn days_for(&self, oura_day: Option<NaiveDate>, start: DateTime<FixedOffset>) -> Vec<String> {
        let mut days = BTreeSet::new();
        let local = start.with_timezone(&self.zone).date_naive();
        for center in oura_day.into_iter().chain([local]) {
            for offset in -1..=1 {
                days.insert(day_key(center + Duration::days(offset)));
            }
        }
        days.into_iter().collect()
    }

    fn load(&mut self, day: &str) -> Result<(), String> {
        if self.days.contains_key(day) {
            return Ok(());
        }
        let mut index = DayIndex::default();
        let stream = stream_dir(self.journal, self.source, day);
        let entries = match fs::read_dir(&stream) {
            Ok(entries) => Some(entries),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) || (stream.exists() && !stream.is_dir()) =>
            {
                None
            }
            Err(error) => return Err(format!("{}: {error}", stream.display())),
        };
        for entry in entries.into_iter().flatten() {
            let entry = entry.map_err(io_text)?;
            let Some(key) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let dir = entry.path();
            if key.starts_with('.') || !dir.is_dir() {
                continue;
            }
            if owner_deleted(&dir).map_err(io_text)? {
                continue;
            }
            let rows = match stored_rows(self.journal, self.source, &(day.to_owned(), key.clone()))
            {
                Ok(Some((_, rows))) => rows,
                Ok(None) => continue,
                Err(StoredRowsError::NotRows) => {
                    index.unparseable = true;
                    continue;
                }
                Err(StoredRowsError::Read(error)) => return Err(error),
            };
            for row in rows
                .into_iter()
                .filter(|row| row.source == self.source.source)
            {
                match (row.kind, row.of) {
                    (RowKind::Experience | RowKind::Logged, _) => {
                        index.firsts.entry(row.id).or_default().push(Placed {
                            key: key.clone(),
                            start: row.start,
                            end: row.end,
                        });
                    }
                    (RowKind::ExperienceTile, Some(of)) => {
                        index
                            .tiles
                            .entry(of)
                            .or_default()
                            .push((key.clone(), row.id));
                    }
                    _ => {}
                }
            }
        }
        index.records = self.records_on(day)?;
        self.days.insert(day.to_owned(), index);
        Ok(())
    }

    /// The body records of this stream filed on `day`, in any facet. A line that
    /// can't be read names nothing; a file that can't be read at all stops the run,
    /// since it could hold the record that says the owner deleted something.
    fn records_on(&self, day: &str) -> Result<HashMap<String, (String, String)>, String> {
        let mut records = HashMap::new();
        let Ok(facets) = fs::read_dir(self.journal.join("facets")) else {
            return Ok(records);
        };
        let prefix = format!("{}\u{1f}", self.source.stream);
        for facet in facets.flatten() {
            // A facet is a folder; a plain file beside them holds no records.
            if !facet.path().is_dir() {
                continue;
            }
            let path = facet.path().join("activities").join(format!("{day}.jsonl"));
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("{}: {error}", path.display())),
            };
            for line in bytes.split(|&byte| byte == b'\n') {
                let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(line) else {
                    continue;
                };
                if !is_body_record(&record) {
                    continue;
                }
                let account = record.get("account");
                let field = |name: &str| {
                    account
                        .and_then(|account| account.get(name))
                        .and_then(Value::as_str)
                };
                if let (Some(key), Some(start)) = (field("key"), field("start"))
                    && key.starts_with(&prefix)
                {
                    let activity = record
                        .get("activity")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    records.insert(key.to_owned(), (start.to_owned(), activity.to_owned()));
                }
            }
        }
        Ok(records)
    }

    /// Whether the window a record's stored start is in, or the one beside older
    /// content there, is a body window the owner deleted.
    fn record_window_deleted(&self, start: &str, activity: &str) -> Result<bool, String> {
        let Some(start) = parse_source_time(start) else {
            return Ok(false);
        };
        let length = if activity == "sleep" {
            HOUR_SECONDS
        } else {
            WINDOW_SECONDS
        };
        self.window_deleted(start, length)
    }

    /// Whether the owner deleted the body window a row starting at `start`, on
    /// windows of `length`, sits in, or the one stepped around older content.
    fn window_deleted(
        &self,
        start: chrono::DateTime<chrono::FixedOffset>,
        length: i64,
    ) -> Result<bool, String> {
        if self.comes_back {
            return Ok(false);
        }
        let (day, key) = window_id(floor_to_window(start, self.zone, length), length);
        let clock = key.split_once('_').map_or(key.as_str(), |(clock, _)| clock);
        let stream = stream_dir(self.journal, self.source, &day);
        for key in [key.clone(), format!("{clock}_{}", length + 1)] {
            let dir = stream.join(key);
            if dir.is_dir()
                && owner_deleted(&dir).map_err(io_text)?
                && is_body_only_dir(self.source.stream, &dir)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Where a document with this row id is stored live, on these days.
    fn placements<'b>(&'b self, days: &'b [String], id: &str) -> Vec<(&'b str, &'b Placed)> {
        days.iter()
            .filter_map(|day| Some((day.as_str(), self.days.get(day)?.firsts.get(id)?)))
            .flat_map(|(day, placed)| placed.iter().map(move |placed| (day, placed)))
            .collect()
    }

    /// The owner days a span is on, by the fixed zone: no more than an
    /// experience can hold.
    fn span_days(&self, from: DateTime<FixedOffset>, to: DateTime<FixedOffset>) -> Vec<String> {
        let first = from.with_timezone(&self.zone).date_naive();
        let last = to.with_timezone(&self.zone).date_naive();
        let mut days = Vec::new();
        let mut day = first;
        while day <= last && days.len() <= 15 {
            days.push(day_key(day));
            day += Duration::days(1);
        }
        days
    }

    /// A document's new tiling: each of its rows' windows, before any step around
    /// older content, and ids.
    fn tiling(&self, id: &str, row: &BodyRow) -> Result<Vec<(WindowId, String)>, String> {
        let mut first = row.clone();
        first.id = id.to_owned();
        if row.kind == RowKind::Experience {
            return Ok(
                tile_experience(self.journal, self.source, &first, self.zone)?
                    .into_iter()
                    .map(|(window, row)| (window, row.id))
                    .collect(),
            );
        }
        Ok(vec![(row_window(&first, self.zone, false)?, first.id)])
    }

    fn judge(
        &mut self,
        id: &str,
        row: &BodyRow,
        oura_day: Option<NaiveDate>,
    ) -> Result<Verdict, String> {
        let Some(start) = row.start.as_deref().and_then(parse_source_time) else {
            return Ok(Verdict::Stage(None));
        };
        let end = row
            .end
            .as_deref()
            .and_then(parse_source_time)
            .unwrap_or(start);
        let days = self.days_for(oura_day, start);
        for day in &days {
            self.load(day)?;
        }
        // Live first rows: rows whose id is the document's. Tiles on other days, a
        // night's next morning say, aren't placements.
        let firsts: Vec<(String, Placed)> = self
            .placements(&days, id)
            .into_iter()
            .map(|(day, placed)| {
                (
                    day.to_owned(),
                    Placed {
                        key: placed.key.clone(),
                        start: placed.start.clone(),
                        end: placed.end.clone(),
                    },
                )
            })
            .collect();
        // Every day of each stored span holds its tiles, so the lookup reaches them
        // all and can't see them whole if one can't be read. The new span's days
        // may hold tiles an earlier run left there.
        let mut watched: BTreeSet<String> = days.iter().cloned().collect();
        for (_, placed) in &firsts {
            if let (Some(from), Some(to)) = (
                placed.start.as_deref().and_then(parse_source_time),
                placed.end.as_deref().and_then(parse_source_time),
            ) {
                watched.extend(self.span_days(from, to));
            }
        }
        let mut reach = watched.clone();
        reach.extend(self.span_days(start, end));
        for day in &reach {
            self.load(day)?;
        }
        let index = |day: &String| self.days.get(day).expect("loaded above");
        if watched.iter().any(|day| index(day).unparseable) {
            return Ok(Verdict::Hold);
        }
        let length = window_seconds(row, false);
        let new_day = day_key(start.with_timezone(&self.zone).date_naive());
        let account = format!("{}\u{1f}{id}", self.source.stream);
        if firsts.is_empty() {
            if let Some(fetched) = self.fetched
                && fetched.contains(id)
            {
                // Oura can move a document further than the days looked at here;
                // then the journal still holds it where an Oura run first wrote it.
                for day in fetched.days_of(id) {
                    if days.contains(&day) {
                        continue;
                    }
                    self.load(&day)?;
                    let far = self.days.get(&day).expect("loaded above");
                    if far.unparseable {
                        return Ok(Verdict::Hold);
                    }
                    if far.firsts.contains_key(id) {
                        return Ok(Verdict::MovedFar);
                    }
                }
                return Ok(Verdict::StayedDeleted);
            }
            // Its own window is one the owner deleted: it stays deleted, as any new
            // row does there.
            if self.window_deleted(start, length)? {
                return Ok(Verdict::StayedDeleted);
            }
            if row.kind == RowKind::Experience {
                for day in &days {
                    if let Some((start, activity)) = index(day).records.get(&account)
                        && self.record_window_deleted(start, activity)?
                    {
                        return Ok(Verdict::StayedDeleted);
                    }
                }
            }
        } else {
            // A start that crosses the owner's midnight, or a place or a record on
            // another day, is kept as first written.
            if firsts.iter().any(|(day, _)| *day != new_day) {
                return Ok(Verdict::Defer);
            }
            if row.kind == RowKind::Experience
                && days
                    .iter()
                    .any(|day| *day != new_day && index(day).records.contains_key(&account))
            {
                return Ok(Verdict::Defer);
            }
            // Moving into a first window the owner deleted: it stays deleted, and
            // nothing of it is staged anywhere.
            let logical_first = self.tiling(id, row)?[0].0.clone();
            let live_sibling = firsts.iter().any(|(day, placed)| {
                day == &logical_first.0
                    && solstone_core_format::content::body_records::same_grid(
                        &placed.key,
                        &logical_first.1,
                    )
                    && solstone_core_format::content::body_records::copy_rank(&placed.key) > 1
            });
            if !live_sibling && self.window_deleted(start, length)? {
                return Ok(Verdict::StayedDeleted);
            }
        }
        let mut live: BTreeSet<(WindowId, String)> = firsts
            .iter()
            .map(|(day, placed)| ((day.clone(), placed.key.clone()), id.to_owned()))
            .collect();
        for day in &reach {
            for (key, tile) in index(day).tiles.get(id).into_iter().flatten() {
                live.insert(((day.clone(), key.clone()), tile.clone()));
            }
        }
        // Nothing of it is stored: there is nothing to converge.
        if live.is_empty() {
            return Ok(Verdict::Stage(None));
        }
        let tiling = self.tiling(id, row)?;
        let resolve = |(day, key): &WindowId, row_id: &str| {
            (
                day.clone(),
                live.iter()
                    .filter(|((old_day, old_key), id)| {
                        old_day == day
                            && id == row_id
                            && solstone_core_format::content::body_records::same_grid(old_key, key)
                    })
                    .min_by_key(|((_, key), _)| {
                        (
                            solstone_core_format::content::body_records::copy_rank(key),
                            key,
                        )
                    })
                    .map_or_else(
                        || resolve_key(self.journal, self.source, day, key),
                        |((_, key), _)| key.clone(),
                    ),
            )
        };
        let tiled: BTreeSet<(WindowId, String)> = tiling
            .iter()
            .map(|(window, tile)| (resolve(window, tile), tile.clone()))
            .collect();
        // The trigger is the difference: a move, a changed span, or tiles named by
        // an older scheme.
        let marks: Vec<(WindowId, String)> = live.difference(&tiled).cloned().collect();
        if marks.is_empty() {
            return Ok(Verdict::Stage(None));
        }
        let first = tiling[0].0.clone();
        let placed_first = resolve(&first, id);
        let now = instants(row.start.as_deref(), row.end.as_deref());
        // A revision Oura made, rather than a copy left by a stopped run or tiles
        // named by an older scheme: no first row is already where and when it is.
        let revised = !firsts.is_empty()
            && !firsts.iter().any(|(day, placed)| {
                (day.clone(), placed.key.clone()) == placed_first
                    && instants(placed.start.as_deref(), placed.end.as_deref()) == now
            });
        Ok(Verdict::Stage(Some(Box::new(Plan {
            marks,
            converging: Converging {
                day: new_day,
                firsts: firsts
                    .into_iter()
                    .map(|(day, placed)| (day, placed.key))
                    .collect(),
                first: if solstone_core_format::content::body_records::copy_rank(&placed_first.1)
                    > 1
                {
                    placed_first
                } else {
                    first
                },
                revised,
                holes: holes_of([row], HoleReason::Converging),
            },
        }))))
    }

    /// The owner day of a night stored live under `id`, when there is one.
    fn stored_day(
        &mut self,
        id: &str,
        oura_day: Option<NaiveDate>,
        start: Option<&str>,
    ) -> Result<Option<String>, String> {
        let mut days: BTreeSet<String> = BTreeSet::new();
        if let Some(start) = start.and_then(parse_source_time) {
            days.extend(self.days_for(oura_day, start));
        } else if let Some(day) = oura_day {
            for offset in -1..=1 {
                days.insert(day_key(day + Duration::days(offset)));
            }
        }
        let days: Vec<String> = days.into_iter().collect();
        for day in &days {
            self.load(day)?;
        }
        Ok(self
            .placements(&days, id)
            .first()
            .map(|(day, _)| (*day).to_owned()))
    }
}

/// Whether the owner deleted a window a converging document moves from or into,
/// since the scan read it: its live first rows' windows, or its new first window,
/// or the one beside older content there. Then nothing of the move is written.
pub fn converging_dropped(
    journal: &Path,
    source: &BodySource,
    converging: &Converging,
) -> Result<bool, String> {
    let deleted = |day: &str, key: &str| -> Result<bool, String> {
        let dir = stream_dir(journal, source, day).join(key);
        Ok(dir.is_dir()
            && owner_deleted(&dir).map_err(io_text)?
            && is_body_only_dir(source.stream, &dir))
    };
    for (day, key) in &converging.firsts {
        if deleted(day, key)? {
            return Ok(true);
        }
    }
    let (day, key) = &converging.first;
    if solstone_core_format::content::body_records::copy_rank(key) > 1 {
        return deleted(day, key);
    }
    let beside = key
        .split_once('_')
        .and_then(|(clock, length)| Some(format!("{clock}_{}", length.parse::<u64>().ok()? + 1)));
    for key in std::iter::once(key.clone()).chain(beside) {
        if deleted(day, &key)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn readable_day(day: &str) -> String {
    match (day.get(..4), day.get(4..6), day.get(6..8)) {
        (Some(year), Some(month), Some(date)) => format!("{year}-{month}-{date}"),
        _ => day.to_owned(),
    }
}

/// Read staged pages into sorted runs under `dir`. `progress` hears the row count
/// now and then. A window the lookup can't read at all stops the scan, which the
/// next run with the same pages does again. `fetched` is an ordinary sync's
/// record of what it brought in, and keeps out what the owner deleted; a
/// re-import passes none, and what the owner deleted comes back.
#[expect(
    clippy::too_many_arguments,
    reason = "the scan of one run's pages; each argument is a fact about the run"
)]
pub fn scan_pages(
    journal: &Path,
    source: &BodySource,
    zone: Tz,
    pages: &Path,
    dir: &Path,
    spill_bytes: usize,
    fetched: Option<&solstone_core_import::oura_fetched::Fetched>,
    progress: &mut dyn FnMut(u64),
) -> Result<(ScanCounts, ScanFindings), String> {
    let entries = read_index(pages)?;
    // The last-fetched place of each document, so only that version is read.
    let mut last: HashMap<(String, String), (usize, usize)> = HashMap::new();
    for (page, entry) in entries.iter().enumerate() {
        if matches!(
            read_endpoint(&entry.endpoint),
            None | Some(Shape::Heartrate)
        ) {
            continue;
        }
        let Some(items) = page_items(&page_bytes(pages, entry)?) else {
            continue;
        };
        for (item, document) in items.iter().enumerate() {
            if let Some(id) = document.get("id").and_then(Value::as_str) {
                last.insert((entry.endpoint.clone(), id.to_owned()), (page, item));
            }
        }
    }

    let mut counts = ScanCounts::default();
    let mut findings = ScanFindings::default();
    let mut spill = Spill::new(dir, spill_bytes)?;
    let mut nights: Vec<(i64, i64)> = Vec::new();
    let mut lookup = Lookup {
        journal,
        source,
        zone,
        days: HashMap::new(),
        comes_back: fetched.is_none(),
        fetched,
    };
    for (page, entry) in entries.iter().enumerate() {
        let endpoint = entry.endpoint.as_str();
        let bytes = page_bytes(pages, entry)?;
        let Some(shape) = read_endpoint(endpoint) else {
            let items = page_items(&bytes).map_or(1, |items| items.len() as u64);
            *counts.not_read.entry(endpoint.to_owned()).or_default() += items;
            continue;
        };
        let Some(items) = page_items(&bytes) else {
            *counts
                .refused
                .entry(format!("{endpoint}: page"))
                .or_default() += 1;
            continue;
        };
        for (item, document) in items.iter().enumerate() {
            if shape != Shape::Heartrate
                && let Some(id) = document.get("id").and_then(Value::as_str)
                && last.get(&(endpoint.to_owned(), id.to_owned())) != Some(&(page, item))
            {
                continue;
            }
            if shape == Shape::Heartrate
                && !entry.holds(document.get("timestamp").and_then(Value::as_str))
            {
                continue;
            }
            let read = match read_document(endpoint, document) {
                Document::Row(read) => read,
                Document::NotRead(name) => {
                    *counts.not_read.entry(name).or_default() += 1;
                    continue;
                }
                Document::Refused(stage) => {
                    *counts.refused.entry(stage).or_default() += 1;
                    continue;
                }
                Document::NotANight {
                    id,
                    kind,
                    oura_day,
                    start,
                } => {
                    *counts.not_read.entry(format!("sleep {kind}")).or_default() += 1;
                    if let Some(day) = lookup.stored_day(&id, oura_day, start.as_deref())? {
                        let marks = if kind == "deleted" {
                            "marks it deleted"
                        } else {
                            "marks it as not sleep"
                        };
                        findings.notes.push(format!(
                            "The sleep stored for {} stays as first written, though Oura now {marks}.",
                            readable_day(&day)
                        ));
                    }
                    continue;
                }
            };
            let content = serde_json::to_string(&read.row).expect("a body row serializes");
            if content.len() > MAX_ROW_BYTES {
                *counts
                    .refused
                    .entry(format!("{endpoint}: row_too_large"))
                    .or_default() += 1;
                continue;
            }
            *counts.read.entry(endpoint.to_owned()).or_default() += 1;
            let id = crate::oura::row_id(&read.identity);
            let verdict = if matches!(
                shape,
                Shape::Sleep | Shape::Workout | Shape::Session | Shape::Tag
            ) {
                lookup.judge(&id, &read.row, read.oura_day)?
            } else {
                Verdict::Stage(None)
            };
            let plan = match verdict {
                Verdict::Stage(plan) => plan,
                Verdict::Defer => {
                    findings.deferred += 1;
                    findings
                        .holes
                        .extend(holes_of([&read.row], HoleReason::Deferred));
                    continue;
                }
                Verdict::Hold => {
                    *findings.held.entry(endpoint.to_owned()).or_default() += 1;
                    findings
                        .holes
                        .extend(holes_of([&read.row], HoleReason::HeldBack));
                    continue;
                }
                Verdict::StayedDeleted => {
                    findings.stayed_deleted += 1;
                    continue;
                }
                Verdict::MovedFar => {
                    findings.moved_far += 1;
                    findings
                        .holes
                        .extend(holes_of([&read.row], HoleReason::MovedFar));
                    continue;
                }
            };
            if let Some(plan) = plan {
                // Each mark goes in its row's own window, under that row's id, and
                // replaces it there or nothing.
                for (window, mark) in &plan.marks {
                    let mut row = BodyRow::new(
                        RowKind::Moved,
                        mark.clone(),
                        source.source,
                        "",
                        String::new(),
                    );
                    row.of = Some(id.clone());
                    let content = serde_json::to_string(&row).expect("a body row serializes");
                    let identity = placed_identity(window, mark);
                    let bytes = identity.len() + content.len() + 48;
                    spill.push(ScanItem(0, 2, identity, content, Vec::new()), bytes)?;
                }
                let Plan { converging, .. } = *plan;
                findings.holes.extend(converging.holes.iter().cloned());
                findings.converging.insert(id, converging);
            }
            let start_ms = read
                .row
                .begins()
                .and_then(parse_source_time)
                .map_or(0, |time| time.timestamp_millis());
            if read.row.kind == RowKind::Experience
                && read.row.experience.as_deref() == Some("sleep")
                && let Some(end) = read.row.end.as_deref().and_then(parse_source_time)
            {
                nights.push((start_ms, end.timestamp_millis()));
            }
            let rank = u8::from(read.row.kind != RowKind::Experience);
            let bytes = read.identity.len() + content.len() + 48;
            spill.push(
                ScanItem(start_ms, rank, read.identity, content, Vec::new()),
                bytes,
            )?;
            counts.rows += 1;
            if counts.rows.is_multiple_of(100_000) {
                progress(counts.rows);
            }
        }
    }
    counts.runs = spill.finish()?;
    progress(counts.rows);
    findings.overlapping_nights = overlapping(&mut nights);
    Ok((counts, findings))
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// How many of these spans overlap another of them.
fn overlapping(spans: &mut [(i64, i64)]) -> u64 {
    spans.sort_unstable();
    let mut overlapping = vec![false; spans.len()];
    // The span, among those before, that reaches furthest.
    let mut reach: Option<(usize, i64)> = None;
    for (index, &(start, end)) in spans.iter().enumerate() {
        if let Some((before, until)) = reach
            && start < until
        {
            overlapping[index] = true;
            overlapping[before] = true;
        }
        if reach.is_none_or(|(_, until)| end > until) {
            reach = Some((index, end));
        }
    }
    overlapping.iter().filter(|&&overlaps| overlaps).count() as u64
}

#[cfg(test)]
mod overlap_tests {
    use super::overlapping;

    #[test]
    fn spans_that_overlap_any_other_are_counted_once_each() {
        assert_eq!(overlapping(&mut []), 0);
        assert_eq!(overlapping(&mut [(0, 10), (10, 20)]), 0);
        assert_eq!(overlapping(&mut [(0, 10), (5, 20)]), 2);
        // A long span covering two short ones that don't touch each other.
        assert_eq!(overlapping(&mut [(0, 100), (10, 20), (30, 40)]), 3);
        assert_eq!(overlapping(&mut [(30, 40), (0, 5), (35, 50)]), 2);
    }
}
