// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An Apple export, any size, read in bounded memory.
//!
//! Three phases, each writing sorted runs to a staging folder the caller owns:
//!
//! 1. **scan** reads the XML once. Every read row is held with its identity and
//!    content, in sorted runs by (start, a session first, identity, content).
//! 2. **place** merges those runs in time order. Rows sharing an identity are
//!    adjacent, so they are numbered by content exactly as the whole-export
//!    reference does. Nights are assembled with one open night per source. The
//!    workouts and mindfulness sessions open at the instant being placed decide
//!    which readings go on their tiles. Every final row is spilled again, sorted
//!    by its window.
//! 3. **days** merges the placed runs and hands back one day of windows at a
//!    time, for the caller to decide, write and publish.
//!
//! Memory is one spill buffer, a read buffer per run, the open nights and
//! sessions, and one day's rows. Runs are merged down until at most
//! `MAX_OPEN_RUNS` are left, so a long export can't run out of open files. Nothing
//! here writes the journal.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono_tz::Tz;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use solstone_core_body_ingest::scan_apple_export;
use solstone_core_format::content::body_records::{BodyRow, RowKind, parse_source_time};

use crate::apple_health::{
    DayRows, Held, MAX_EXPERIENCE_SECONDS, NIGHT_GAP_SECONDS, NIGHT_RULE, Reader, SLEEP_TYPE,
    SOURCE, WindowId, is_session, number_events, resolve_key, row_window, sha256_hex,
    tile_experience,
};
use crate::body_stream::BodySource;

/// How much a spill buffer holds before it is sorted and written.
pub const DEFAULT_SPILL_BYTES: usize = 64 * 1024 * 1024;

/// The most sorted runs merged at once, each holding one open file. macOS gives a
/// launched app 256, and the process holds its other files too.
const MAX_OPEN_RUNS: usize = 64;

fn io_text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Make a folder's new files survive power loss. Windows can't open a folder this
/// way, and doesn't need it to keep a file whose contents are synced.
pub fn sync_folder(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(dir)
        .and_then(|folder| folder.sync_all())
        .map_err(io_text)?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Sorted runs of one item type in one folder.
pub(crate) struct Spill<T> {
    dir: PathBuf,
    limit: usize,
    buffer: Vec<T>,
    bytes: usize,
    runs: Vec<PathBuf>,
    /// The number the next run takes. Merging removes runs, so it isn't their count.
    next: usize,
}

impl<T: Serialize + DeserializeOwned + Ord> Spill<T> {
    pub(crate) fn new(dir: &Path, limit: usize) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(io_text)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            limit,
            buffer: Vec::new(),
            bytes: 0,
            runs: Vec::new(),
            next: 0,
        })
    }

    pub(crate) fn push(&mut self, item: T, bytes: usize) -> Result<(), String> {
        self.buffer.push(item);
        self.bytes += bytes;
        if self.bytes >= self.limit {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.buffer.sort();
        let mut out = self.new_run()?;
        for item in self.buffer.drain(..) {
            out.write(&item)?;
        }
        let path = out.finish()?;
        self.bytes = 0;
        self.runs.push(path);
        Ok(())
    }

    fn new_run(&mut self) -> Result<RunWriter, String> {
        let path = self.dir.join(format!("{:06}.run", self.next));
        self.next += 1;
        let out = BufWriter::new(File::create(&path).map_err(io_text)?);
        Ok(RunWriter { path, out })
    }

    /// The last run, then merges until a reader can hold every run open at once. A
    /// phase is recorded done only after this returns, so a stop in here repeats it.
    /// Returns how many runs are left.
    pub(crate) fn finish(mut self) -> Result<u64, String> {
        self.flush()?;
        while self.runs.len() > MAX_OPEN_RUNS {
            let batch: Vec<PathBuf> = self.runs.drain(..MAX_OPEN_RUNS).collect();
            let mut out = self.new_run()?;
            let mut merge: Merge<T> = Merge::open(&batch)?;
            while let Some(item) = merge.next_item()? {
                out.write(&item)?;
            }
            // Closed before they're removed.
            drop(merge);
            let path = out.finish()?;
            for run in &batch {
                fs::remove_file(run).map_err(io_text)?;
            }
            self.runs.push(path);
        }
        sync_folder(&self.dir)?;
        Ok(self.runs.len() as u64)
    }
}

/// One sorted run being written.
struct RunWriter {
    path: PathBuf,
    out: BufWriter<File>,
}

impl RunWriter {
    fn write(&mut self, item: &impl Serialize) -> Result<(), String> {
        serde_json::to_writer(&mut self.out, item).map_err(io_text)?;
        self.out.write_all(b"\n").map_err(io_text)
    }

    fn finish(self) -> Result<PathBuf, String> {
        let file = self.out.into_inner().map_err(io_text)?;
        file.sync_all().map_err(io_text)?;
        Ok(self.path)
    }
}

/// The runs in a staging folder, in the order they were written.
pub fn runs_in(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut runs: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(io_text)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "run"))
        .collect();
    runs.sort();
    Ok(runs)
}

/// Every item of every run, in order.
struct Merge<T> {
    readers: Vec<io::Lines<BufReader<File>>>,
    heap: BinaryHeap<Reverse<(T, usize)>>,
}

impl<T: DeserializeOwned + Ord> Merge<T> {
    fn open(runs: &[PathBuf]) -> Result<Self, String> {
        let mut merge = Self {
            readers: Vec::with_capacity(runs.len()),
            heap: BinaryHeap::new(),
        };
        for (index, run) in runs.iter().enumerate() {
            let file = File::open(run).map_err(io_text)?;
            merge
                .readers
                .push(BufReader::with_capacity(64 * 1024, file).lines());
            merge.advance(index)?;
        }
        Ok(merge)
    }

    fn advance(&mut self, index: usize) -> Result<(), String> {
        if let Some(line) = self.readers[index].next() {
            let line = line.map_err(io_text)?;
            let item: T = serde_json::from_str(&line).map_err(io_text)?;
            self.heap.push(Reverse((item, index)));
        }
        Ok(())
    }

    fn next_item(&mut self) -> Result<Option<T>, String> {
        let Some(Reverse((item, index))) = self.heap.pop() else {
            return Ok(None);
        };
        self.advance(index)?;
        Ok(Some(item))
    }
}

/// A read row before it has an id: (start ms, rank, identity, content, its
/// events). A session ranks first, so one starting on the same millisecond as a
/// reading it covers is open when the reading is placed. The events are the
/// tie-break, so two workouts identical but for their events collapse to the one
/// whose events sort last, in any export order.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ScanItem(
    pub(crate) i64,
    pub(crate) u8,
    pub(crate) String,
    pub(crate) String,
    pub(crate) Vec<String>,
);

/// One distinct content of an identity, and its events.
type Content = (String, Vec<String>);

/// A final row and the window it sits in, before any step around older content.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct PlacedItem(String, String, String);

/// What a scan counted. Names and counts, never a value.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanCounts {
    pub read: BTreeMap<String, u64>,
    pub not_read: BTreeMap<String, u64>,
    pub refused: BTreeMap<String, u64>,
    pub rows: u64,
    /// The sorted runs left in staging, which a resume checks are all still there.
    #[serde(default)]
    pub runs: u64,
}

/// Read an export into sorted runs under `dir`. `progress` hears the row count
/// now and then.
pub fn scan(
    source: &Path,
    dir: &Path,
    spill_bytes: usize,
    progress: &mut dyn FnMut(u64),
) -> Result<ScanCounts, String> {
    let mut reader = Reader::default();
    let mut spill = Spill::new(dir, spill_bytes)?;
    let mut rows = 0_u64;
    let mut failure = None;
    scan_apple_export(source, &mut |item| {
        if failure.is_some() {
            return;
        }
        reader.visit(item);
        for Held {
            identity,
            start_ms,
            rank,
            content,
            events,
        } in reader.held.drain(..)
        {
            let events: Vec<String> = events
                .iter()
                .map(|event| serde_json::to_string(event).expect("a body row serializes"))
                .collect();
            let bytes =
                identity.len() + content.len() + events.iter().map(String::len).sum::<usize>() + 48;
            if let Err(error) =
                spill.push(ScanItem(start_ms, rank, identity, content, events), bytes)
            {
                failure = Some(error);
                return;
            }
            rows += 1;
            if rows.is_multiple_of(100_000) {
                progress(rows);
            }
        }
    })
    .map_err(|error| error.to_string())?;
    if let Some(error) = failure {
        return Err(error);
    }
    let runs = spill.finish()?;
    progress(rows);
    let export = reader.export;
    Ok(ScanCounts {
        read: export.read,
        not_read: export.not_read,
        refused: export.refused,
        rows,
        runs,
    })
}

struct OpenNight {
    id: String,
    by: Option<String>,
    /// The earliest stage: (start s, id, its start as the source wrote it).
    first: (i64, String, String),
    /// The stage that ends last, latest in (start, id) order among equals.
    last: (i64, i64, String, String),
    reach: i64,
}

/// What placing found that the write phase needs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceResult {
    /// Nights refused as too long, whose stages must not land.
    pub refused_nights: BTreeSet<String>,
    /// Refusals placing added, by stage.
    pub refused: BTreeMap<String, u64>,
    pub rows: u64,
    /// Every day a placed row sits on.
    pub days: BTreeSet<String>,
    /// The sorted runs left in staging, which a resume checks are all still there.
    #[serde(default)]
    pub runs: u64,
}

struct Placer<'a> {
    journal: &'a Path,
    source: &'a BodySource,
    zone: Tz,
    out: Spill<PlacedItem>,
    nights: BTreeMap<String, OpenNight>,
    /// Workouts and mindfulness sessions not yet ended at the instant being
    /// placed: (source's `by`, start ms, end ms as tiled).
    sessions: Vec<(Option<String>, i64, i64)>,
    result: PlaceResult,
    progress: &'a mut dyn FnMut(u64),
}

fn seconds(text: Option<&str>) -> i64 {
    text.and_then(parse_source_time)
        .map(|time| time.timestamp())
        .unwrap_or_default()
}

impl Placer<'_> {
    fn emit(&mut self, window: WindowId, row: &BodyRow) -> Result<(), String> {
        let text = serde_json::to_string(row).expect("a body row serializes");
        let bytes = window.0.len() + window.1.len() + text.len() + 16;
        if !self.result.days.contains(&window.0) {
            self.result.days.insert(window.0.clone());
        }
        self.out.push(PlacedItem(window.0, window.1, text), bytes)?;
        self.result.rows += 1;
        if self.result.rows.is_multiple_of(100_000) {
            (self.progress)(self.result.rows);
        }
        Ok(())
    }

    /// Whether a session from the reading's own source covers it. Sessions end out
    /// of the set at the reading's own instant, which only grows in merge order.
    fn covering(&mut self, row: &BodyRow) -> bool {
        if row.kind != RowKind::Reading || row.date.is_some() {
            return false;
        }
        let Some(at) = row
            .begins()
            .and_then(parse_source_time)
            .map(|time| time.timestamp_millis())
        else {
            return false;
        };
        self.sessions.retain(|(_, _, end)| *end > at);
        self.sessions
            .iter()
            .any(|(by, start, _)| *start <= at && *by == row.by)
    }

    fn row(&mut self, row: &BodyRow) -> Result<(), String> {
        let covered = self.covering(row);
        let window = row_window(row, self.zone, covered)?;
        self.emit(window, row)
    }

    fn experience(&mut self, experience: &BodyRow) -> Result<(), String> {
        let tiles = tile_experience(self.journal, self.source, experience, self.zone)?;
        if is_session(experience) {
            let millis = |text: Option<&str>| {
                text.and_then(parse_source_time)
                    .map(|time| time.timestamp_millis())
            };
            // The span its tiles are written to, a stored later end included.
            if let (Some(start), Some(end)) = (
                millis(experience.start.as_deref()),
                millis(tiles[0].1.end.as_deref()),
            ) {
                self.sessions.push((experience.by.clone(), start, end));
            }
        }
        for (window, row) in tiles {
            self.emit(window, &row)?;
        }
        Ok(())
    }

    fn stage(&mut self, mut row: BodyRow) -> Result<(), String> {
        let by = row.by.clone().unwrap_or_default();
        let start = seconds(row.start.as_deref());
        let end = seconds(row.end.as_deref());
        if self
            .nights
            .get(&by)
            .is_some_and(|night| start - night.reach > NIGHT_GAP_SECONDS)
        {
            let night = self.nights.remove(&by).expect("checked above");
            self.close(night)?;
        }
        let night = self.nights.entry(by.clone()).or_insert_with(|| OpenNight {
            id: sha256_hex(&format!(
                "{SOURCE}\u{1f}{SLEEP_TYPE}\u{1f}{by}\u{1f}{start}"
            )),
            by: row.by.clone(),
            first: (start, row.id.clone(), row.start.clone().unwrap_or_default()),
            last: (i64::MIN, i64::MIN, String::new(), String::new()),
            reach: i64::MIN,
        });
        if (start, row.id.as_str()) < (night.first.0, night.first.1.as_str()) {
            night.first = (start, row.id.clone(), row.start.clone().unwrap_or_default());
        }
        let candidate = (
            end,
            start,
            row.id.clone(),
            row.end.clone().unwrap_or_default(),
        );
        if candidate > night.last {
            night.last = candidate;
        }
        night.reach = night.reach.max(end);
        row.of = Some(night.id.clone());
        self.row(&row)
    }

    fn close(&mut self, night: OpenNight) -> Result<(), String> {
        if night.last.0 - night.first.0 > MAX_EXPERIENCE_SECONDS {
            *self
                .result
                .refused
                .entry("span_too_long".to_owned())
                .or_default() += 1;
            self.result.refused_nights.insert(night.id);
            return Ok(());
        }
        let mut row = BodyRow::new(
            RowKind::Experience,
            night.id,
            SOURCE,
            SLEEP_TYPE,
            "night".to_owned(),
        );
        row.experience = Some("sleep".to_owned());
        row.assembled = Some(NIGHT_RULE.to_owned());
        row.by = night.by;
        row.start = Some(night.first.2);
        row.end = Some(night.last.3);
        self.experience(&row)
    }

    /// One identity's rows, numbered by content.
    fn group(&mut self, identity: &str, contents: Vec<Content>) -> Result<(), String> {
        for (occurrence, (content, events)) in contents.into_iter().enumerate() {
            let mut row: BodyRow = serde_json::from_str(&content).map_err(io_text)?;
            row.id = sha256_hex(&format!("{identity}\u{1f}{occurrence}"));
            match row.kind {
                RowKind::Stage => self.stage(row)?,
                RowKind::Experience => {
                    let events = events
                        .iter()
                        .map(|event| serde_json::from_str(event).map_err(io_text))
                        .collect::<Result<Vec<BodyRow>, String>>()?;
                    for event in number_events(&row.id, events) {
                        self.row(&event)?;
                    }
                    self.experience(&row)?;
                }
                _ => self.row(&row)?,
            }
        }
        Ok(())
    }
}

/// Merge a scan's runs into final rows, spilled by window under `dir`.
pub fn place(
    journal: &Path,
    source: &BodySource,
    zone: Tz,
    scan_dir: &Path,
    dir: &Path,
    spill_bytes: usize,
    progress: &mut dyn FnMut(u64),
) -> Result<PlaceResult, String> {
    let runs = runs_in(scan_dir)?;
    let mut merge: Merge<ScanItem> = Merge::open(&runs)?;
    let mut placer = Placer {
        journal,
        source,
        zone,
        out: Spill::new(dir, spill_bytes)?,
        nights: BTreeMap::new(),
        sessions: Vec::new(),
        result: PlaceResult::default(),
        progress,
    };
    // One identity at a time: its distinct contents, a repeated content keeping
    // the item that sorts last, which is the one with the last events.
    let mut current: Option<(String, Vec<Content>)> = None;
    while let Some(ScanItem(_, _, identity, content, events)) = merge.next_item()? {
        match current.as_mut() {
            Some((held, contents)) if *held == identity => match contents.last_mut() {
                Some(last) if last.0 == content => last.1 = events,
                _ => contents.push((content, events)),
            },
            _ => {
                if let Some((held, contents)) = current.take() {
                    placer.group(&held, contents)?;
                }
                current = Some((identity, vec![(content, events)]));
            }
        }
    }
    if let Some((held, contents)) = current.take() {
        placer.group(&held, contents)?;
    }
    let nights: Vec<OpenNight> = std::mem::take(&mut placer.nights).into_values().collect();
    for night in nights {
        placer.close(night)?;
    }
    // The scan's runs are done with, so finishing the placed ones can use their files.
    drop(merge);
    let Placer {
        out, mut result, ..
    } = placer;
    result.runs = out.finish()?;
    Ok(result)
}

/// One day's windows, each with this run's rows for it by id.
pub type DayWindows = (String, DayRows);

/// The placed rows, one day at a time, for days after `after`. Each window's key has
/// already stepped around older content, and a refused night's stages are gone.
pub struct Days<'a> {
    journal: &'a Path,
    source: &'a BodySource,
    merge: Merge<PlacedItem>,
    refused_nights: &'a BTreeSet<String>,
    peeked: Option<PlacedItem>,
    after: Option<String>,
}

impl<'a> Days<'a> {
    pub fn open(
        journal: &'a Path,
        source: &'a BodySource,
        dir: &Path,
        refused_nights: &'a BTreeSet<String>,
        after: Option<String>,
    ) -> Result<Self, String> {
        let runs = runs_in(dir)?;
        Ok(Self {
            journal,
            source,
            merge: Merge::open(&runs)?,
            refused_nights,
            peeked: None,
            after,
        })
    }

    fn pull(&mut self) -> Result<Option<PlacedItem>, String> {
        if let Some(item) = self.peeked.take() {
            return Ok(Some(item));
        }
        self.merge.next_item()
    }

    /// The next day, or `None` at the end.
    pub fn next_day(&mut self) -> Result<Option<DayWindows>, String> {
        let mut day: Option<String> = None;
        let mut windows: BTreeMap<String, BTreeMap<String, BodyRow>> = BTreeMap::new();
        while let Some(item) = self.pull()? {
            if self
                .after
                .as_deref()
                .is_some_and(|after| item.0.as_str() <= after)
            {
                continue;
            }
            match &day {
                Some(current) if *current != item.0 => {
                    self.peeked = Some(item);
                    break;
                }
                None => day = Some(item.0.clone()),
                _ => {}
            }
            let row: BodyRow = serde_json::from_str(&item.2).map_err(io_text)?;
            if row.kind == RowKind::Stage
                && row
                    .of
                    .as_ref()
                    .is_some_and(|of| self.refused_nights.contains(of))
            {
                continue;
            }
            windows
                .entry(item.1)
                .or_default()
                .insert(row.id.clone(), row);
        }
        let Some(day) = day else {
            return Ok(None);
        };
        let mut resolved: BTreeMap<String, BTreeMap<String, BodyRow>> = BTreeMap::new();
        for (key, rows) in windows {
            resolved
                .entry(resolve_key(self.journal, self.source, &day, &key))
                .or_default()
                .extend(rows);
        }
        let windows = resolved
            .into_iter()
            .map(|(key, rows)| ((day.clone(), key), rows))
            .collect();
        Ok(Some((day, windows)))
    }
}

#[cfg(test)]
#[path = "apple_health_run_tests.rs"]
mod tests;
