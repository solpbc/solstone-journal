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
//! This is Oura's half of the scan phase: the place and days phases, and
//! everything after them, are the writer Apple's export uses. The scan keeps one
//! version of each Oura document, the last fetched, so the placer never sees two
//! contents for one id. Before a document is staged, it is looked up where it
//! could already be stored. A revision that would move or shorten what is stored
//! isn't staged at all, so nothing is placed by a span that isn't written.

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
    resolve_key, stored_rows, stream_dir, window_id, window_seconds, windows_of,
};
use crate::apple_health_run::{ScanCounts, ScanItem, Spill};
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

/// What the scan found besides its counts. Kept in the run's state with the scan,
/// so a resumed run reports what the scan it carries on from found.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFindings {
    /// Revisions that would move or shorten what is stored, kept as first written.
    #[serde(default)]
    pub deferred: u64,
    /// Documents whose stored experience the owner deleted.
    #[serde(default)]
    pub stayed_deleted: u64,
    /// Documents held back, by endpoint, because a window they could already be in
    /// couldn't be read as rows.
    #[serde(default)]
    pub held: BTreeMap<String, u64>,
    #[serde(default)]
    pub holes: BTreeSet<Hole>,
    /// Plain sentences for the summary.
    #[serde(default)]
    pub notes: Vec<String>,
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

/// What a day's stream holds that the lookup asks about: ids, windows and spans,
/// never row contents. An experience's tiles are the windows of its stored span,
/// so they needn't be held.
#[derive(Default)]
struct DayIndex {
    /// Experience and logged rows by id.
    firsts: HashMap<String, Vec<Placed>>,
    /// The owner deleted a body window here.
    deleted: bool,
    /// A window here can't be read as rows.
    unparseable: bool,
    /// Body records filed on this day, by account key: the account's stored
    /// start and its activity.
    records: HashMap<String, (String, String)>,
}

/// What the lookup decided for one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Stage,
    /// Kept as first written: staging it would move or shorten what is stored.
    Defer,
    /// A window it could already be in can't be read, so nothing is added on a guess.
    Hold,
    /// The owner deleted its stored experience.
    StayedDeleted,
}

struct Lookup<'a> {
    journal: &'a Path,
    source: &'a BodySource,
    zone: Tz,
    days: HashMap<String, DayIndex>,
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
                index.deleted |= is_body_only_dir(self.source.stream, &dir);
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
            for row in rows.into_iter().filter(|row| {
                row.source == self.source.source
                    && matches!(row.kind, RowKind::Experience | RowKind::Logged)
            }) {
                index.firsts.entry(row.id).or_default().push(Placed {
                    key: key.clone(),
                    start: row.start,
                    end: row.end,
                });
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

    fn judge(
        &mut self,
        id: &str,
        row: &BodyRow,
        oura_day: Option<NaiveDate>,
    ) -> Result<Verdict, String> {
        let Some(start) = row.start.as_deref().and_then(parse_source_time) else {
            return Ok(Verdict::Stage);
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
        let index = |day: &String| self.days.get(day).expect("loaded above");
        if days.iter().any(|day| index(day).unparseable) {
            return Ok(Verdict::Hold);
        }
        let length = window_seconds(row, false);
        let placed = self.placements(&days, id);
        match placed.as_slice() {
            [] => {}
            [(day, stored)] => {
                let first = window_id(floor_to_window(start, self.zone, length), length);
                let key = resolve_key(self.journal, self.source, &first.0, &first.1);
                // A start in another window, or a window of another length, is a move.
                if first.0 != *day || key != stored.key {
                    return Ok(Verdict::Defer);
                }
                if row.kind != RowKind::Experience {
                    return Ok(Verdict::Stage);
                }
                // An earlier end may not leave a stored tile outside the new span.
                // A stored span that can't be read can't say, so it's kept.
                let tiled: BTreeSet<WindowId> = windows_of(start, end, self.zone, length)
                    .into_iter()
                    .collect();
                let inside = match (
                    stored.start.as_deref().and_then(parse_source_time),
                    stored.end.as_deref().and_then(parse_source_time),
                ) {
                    (Some(from), Some(to)) => windows_of(from, to, self.zone, length)
                        .into_iter()
                        .all(|window| tiled.contains(&window)),
                    _ => false,
                };
                return Ok(if inside {
                    Verdict::Stage
                } else {
                    Verdict::Defer
                });
            }
            // A half-written move, or one beyond the lookup's reach.
            _ => return Ok(Verdict::Defer),
        }
        if row.kind == RowKind::Experience {
            let account = format!("{}\u{1f}{id}", self.source.stream);
            for day in &days {
                if let Some((start, activity)) = index(day).records.get(&account)
                    && self.record_window_deleted(start, activity)?
                {
                    return Ok(Verdict::StayedDeleted);
                }
            }
        }
        // Not stored live anywhere it could be. Where the owner deleted a body
        // window that could have held it, it's left as it is rather than brought
        // back.
        if days.iter().any(|day| index(day).deleted) {
            return Ok(Verdict::Defer);
        }
        Ok(Verdict::Stage)
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

fn readable_day(day: &str) -> String {
    match (day.get(..4), day.get(4..6), day.get(6..8)) {
        (Some(year), Some(month), Some(date)) => format!("{year}-{month}-{date}"),
        _ => day.to_owned(),
    }
}

/// Read staged pages into sorted runs under `dir`. `progress` hears the row count
/// now and then. A window the lookup can't read at all stops the scan, which the
/// next run with the same pages does again.
pub fn scan_pages(
    journal: &Path,
    source: &BodySource,
    zone: Tz,
    pages: &Path,
    dir: &Path,
    spill_bytes: usize,
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
    let mut lookup = Lookup {
        journal,
        source,
        zone,
        days: HashMap::new(),
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
            let verdict = if matches!(
                shape,
                Shape::Sleep | Shape::Workout | Shape::Session | Shape::Tag
            ) {
                let id = crate::oura::row_id(&read.identity);
                lookup.judge(&id, &read.row, read.oura_day)?
            } else {
                Verdict::Stage
            };
            match verdict {
                Verdict::Stage => {}
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
            }
            let start_ms = read
                .row
                .begins()
                .and_then(parse_source_time)
                .map_or(0, |time| time.timestamp_millis());
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
    Ok((counts, findings))
}
