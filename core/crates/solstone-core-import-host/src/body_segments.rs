// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An Apple Health export through the ordinary import lifecycle, into body
//! windows, in bounded memory and one day at a time. `docs/BODY_SEGMENTS.md` is
//! the contract.
//!
//! No owner route reaches this yet: every owner command still saves a bundle, and
//! this function's one caller outside tests is `examples/apple_body_segments.rs`.
//!
//! A run keeps one small state file, `imports/.apple_health-run/state.json`,
//! replaced atomically. Its cursor and the day being written move in one replace,
//! so a kill anywhere leaves either a day not yet counted, which the next run
//! redoes and publishes whether or not its bytes now match, or a day done.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_format::content::body_records::BODY_RECORDS_FILE;
use solstone_core_import::RegistrySource;
use solstone_core_import::metadata::{
    AttemptRead, AttemptState, hold_import_lock, read_attempt_facts, read_provenance,
    record_unconfirmed_attempt_unlocked,
};
use solstone_core_import::publish::{
    CreatedSegment, DayMarkerStatus, NativePublicationOperations, PublicationInput,
    PublicationStatus, PublishError, SegmentBindingOutcome, publish_with_operations,
    read_publication_record,
};
use solstone_core_import::text::TextCreated;
use solstone_core_import_sources::apple_health::{
    DayRows, Decision, LOCK_REL, STREAM, SummaryCounts, WindowId, decide, facet_file_text,
    read_or_create_zone, settle_readings, stored_windows_of_day, stream_days, summary_text,
};
use solstone_core_import_sources::apple_health_run::{
    DEFAULT_SPILL_BYTES, Days, PlaceResult, ScanCounts, place, runs_in, scan, sync_folder,
};
use solstone_core_import_sources::save::{self, RenderedImport, SegmentFile};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, atomic_replace, hold_lock, remove_dir_all,
};

use crate::import_publication::{
    ImportFinish, ImportTerminalInput, finish_import_attempt_indexing,
};
use crate::registry::now_ms;

const RAW_RETENTION: &str = "discard";
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// The run's own folder. Only `scan/` and `placed/` are left out of copies.
pub const RUN_DIR_REL: &str = "imports/.apple_health-run";
const STATE_FILE: &str = "state.json";
/// 3: staging records how many runs it holds, and places rows without a note of
/// whether another source's session covers them.
const STATE_SCHEMA: u32 = 3;
const PROGRESS_FILE: &str = "body-progress.json";
/// So many windows that can't be published, in a row and none bound between them,
/// is the stream's fault rather than theirs.
const MAX_FAILED_IN_A_ROW: u32 = 64;
const LISTING_FILE: &str = "body-touched.jsonl";
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// What one run was asked to do.
pub struct AppleSegmentsRequest<'a> {
    pub source: &'a Path,
    /// `YYYYMMDD_HHMMSS`, as every import's id.
    pub timestamp: &'a str,
    pub facet: Option<&'a str>,
    /// How much a spill buffer holds; `DEFAULT_SPILL_BYTES` unless a test says.
    pub spill_bytes: usize,
    /// Test seam: called at each named point, and the run stops there, as if
    /// killed, when it returns true.
    pub stop_at: Option<&'a dyn Fn(&str) -> bool>,
}

impl<'a> AppleSegmentsRequest<'a> {
    #[must_use]
    pub fn new(source: &'a Path, timestamp: &'a str, facet: Option<&'a str>) -> Self {
        Self {
            source,
            timestamp,
            facet,
            spill_bytes: DEFAULT_SPILL_BYTES,
            stop_at: None,
        }
    }
}

/// What one run did.
#[derive(Debug)]
pub struct AppleSegmentsOutcome {
    pub import_id: String,
    pub summary: String,
    pub published: u64,
    /// Nothing is left to do: every window is saved and the run state is gone.
    pub finished: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    #[default]
    Scan,
    Place,
    Write,
    /// Republishing every window in the stream, day by day, because the state
    /// couldn't be trusted.
    Repair,
    /// Finished, except windows that couldn't be published; the next run, with
    /// any export, retries them first.
    Retry,
}

/// One window or day that couldn't be published.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
struct Failed {
    day: String,
    /// `None` when the day's body marker didn't move.
    key: Option<String>,
    stage: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Totals {
    written: u64,
    unchanged: u64,
    stayed_deleted: u64,
    /// Readings not added because a place they could have been was deleted, or
    /// couldn't be read.
    #[serde(default)]
    held_back: u64,
    unreadable: u64,
    kept_facet: u64,
    refiled: u64,
    published: u64,
    entries: u64,
    days: BTreeSet<String>,
    /// Activity records created for experiences.
    #[serde(default)]
    activities: u64,
    /// Experiences in a window with no facet, which get no activity record.
    #[serde(default)]
    activities_no_facet: u64,
    /// Activity records brought to their account's new times, type or recorder.
    #[serde(default)]
    activities_refreshed: u64,
    /// Facets whose activity file refused a record; the next run tries again.
    #[serde(default)]
    activities_refused: BTreeSet<String>,
}

impl Totals {
    fn add(&mut self, other: &Self) {
        self.written += other.written;
        self.unchanged += other.unchanged;
        self.stayed_deleted += other.stayed_deleted;
        self.held_back += other.held_back;
        self.unreadable += other.unreadable;
        self.kept_facet += other.kept_facet;
        self.refiled += other.refiled;
        self.published += other.published;
        self.activities += other.activities;
        self.activities_no_facet += other.activities_no_facet;
        self.activities_refreshed += other.activities_refreshed;
        self.activities_refused
            .extend(other.activities_refused.iter().cloned());
        self.entries += other.entries;
        self.days.extend(other.days.iter().cloned());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct PendingWindow {
    key: String,
    write: bool,
    facet: bool,
    publish: bool,
}

/// The day being written: its decisions, made before any of its files.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Pending {
    day: String,
    windows: Vec<PendingWindow>,
    counts: Totals,
    /// Per window: key, rows new, updated and unchanged.
    listing: Vec<(String, u64, u64, u64)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct RunState {
    schema: u32,
    phase: Phase,
    source_hash: String,
    /// A folder's hash is names and sizes, which another folder can match, so
    /// carrying on needs its export's own bytes. Empty for a zip, whose hash has them.
    #[serde(default)]
    export_hash: String,
    import_id: String,
    facet: Option<String>,
    scan: ScanCounts,
    place: PlaceResult,
    /// The last day written; the next run continues after it.
    done_through: Option<String>,
    totals: Totals,
    failed: BTreeSet<Failed>,
    pending: Option<Pending>,
    listing_len: u64,
    /// The import whose run this state is: kept apart from the current one
    /// while a repair or takeover runs under a newer attempt.
    notes: Vec<String>,
}

enum Read {
    Absent,
    Trusted(Box<RunState>),
    /// Carrying the import whose run it was, when the state can still say.
    Untrusted(Option<String>),
}

fn run_dir(journal: &Path) -> PathBuf {
    journal.join(RUN_DIR_REL)
}

fn window_dir(journal: &Path, (day, key): &WindowId) -> PathBuf {
    journal.join("chronicle").join(day).join(STREAM).join(key)
}

fn read_state(journal: &Path) -> Read {
    let path = run_dir(journal).join(STATE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A run folder without its state is a run that didn't finish.
            return if run_dir(journal).exists() {
                Read::Untrusted(None)
            } else {
                Read::Absent
            };
        }
        Err(_) => return Read::Untrusted(None),
    };
    let import_id = || {
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|state| state.get("import_id")?.as_str().map(str::to_owned))
    };
    match serde_json::from_slice::<RunState>(&bytes) {
        Ok(state) if state.schema == STATE_SCHEMA => {
            // A phase that needs staging the folder no longer holds in full (a
            // restore copies the state and leaves staging out, and power loss can
            // lose a run) can't be trusted.
            let needs = match state.phase {
                Phase::Place => Some(("scan", state.scan.runs)),
                Phase::Write => Some(("placed", state.place.runs)),
                Phase::Scan | Phase::Repair | Phase::Retry => None,
            };
            let whole = needs.is_none_or(|(folder, runs)| {
                runs_in(&run_dir(journal).join(folder))
                    .is_ok_and(|found| found.len() as u64 == runs)
            });
            if whole {
                Read::Trusted(Box::new(state))
            } else {
                Read::Untrusted(Some(state.import_id))
            }
        }
        _ => Read::Untrusted(import_id()),
    }
}

fn save_state(journal: &Path, state: &RunState) -> Result<(), String> {
    let bytes = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    fs::create_dir_all(run_dir(journal)).map_err(|error| error.to_string())?;
    atomic_replace(run_dir(journal).join(STATE_FILE), &bytes, PRIVATE)
        .map_err(|error| error.to_string())
}

/// How a run stopped short.
enum Stop {
    /// An injected stop point: the disk is as a kill there would leave it.
    Injected(String),
    /// The owner asked the import to stop.
    Stopped,
    /// Another attempt owns this import now.
    Superseded,
    Failed(String),
}

impl From<String> for Stop {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

struct Run<'a> {
    journal: &'a Path,
    request: &'a AppleSegmentsRequest<'a>,
    import_id: String,
    generation: u64,
    /// Windows that couldn't be published since one last was.
    failed_in_a_row: Cell<u32>,
}

impl Run<'_> {
    fn point(&self, name: &str) -> Result<(), Stop> {
        if self.request.stop_at.is_some_and(|stop| stop(name)) {
            return Err(Stop::Injected(name.to_owned()));
        }
        if !solstone_core_import::work_continues() {
            return Err(Stop::Stopped);
        }
        Ok(())
    }

    fn import_dir(&self) -> PathBuf {
        self.journal.join("imports").join(&self.import_id)
    }

    fn still_this_attempt(&self) -> bool {
        read_provenance(self.journal, &self.import_id)
            .ok()
            .flatten()
            .and_then(|metadata| match read_attempt_facts(&metadata) {
                AttemptRead::Present(facts) => Some(facts),
                AttemptRead::Absent | AttemptRead::Malformed => None,
            })
            .is_some_and(|facts| facts.generation == self.generation)
    }

    fn created(&self, window: &WindowId) -> TextCreated {
        TextCreated {
            day: window.0.clone(),
            segment: window.1.clone(),
            stream: STREAM.to_owned(),
            hints: save::stream_hints(RegistrySource::AppleHealth),
            path: window_dir(self.journal, window).join(BODY_RECORDS_FILE),
        }
    }

    /// Publish windows in one call and read what came back.
    fn publish_once(
        &self,
        windows: &[WindowId],
    ) -> Result<solstone_core_import::publish::PublicationRecord, Stop> {
        let created: Vec<TextCreated> = windows.iter().map(|window| self.created(window)).collect();
        let segments: Vec<CreatedSegment> =
            created.iter().map(TextCreated::created_segment).collect();
        let files: Vec<PathBuf> = created.iter().map(|created| created.path.clone()).collect();
        let may_write = || self.still_this_attempt();
        let input = PublicationInput {
            journal: self.journal,
            import_dir: None,
            import_id: &self.import_id,
            importer: RegistrySource::AppleHealth.name(),
            revision: None,
            segments: &segments,
            files_created: &files,
            index_only: &[],
            may_write_record: Some(&may_write),
        };
        match publish_with_operations(input, &NativePublicationOperations) {
            Ok(record) => Ok(record),
            Err(PublishError::NotAdmitted) if !solstone_core_import::work_continues() => {
                Err(Stop::Stopped)
            }
            Err(PublishError::NotAdmitted | PublishError::Superseded) => Err(Stop::Superseded),
            Err(error) => Err(Stop::Failed(error.to_string())),
        }
    }

    /// Publish one day's windows. A failure of one window is recorded and the
    /// rest go on; a failure the whole stream would repeat ends the day.
    fn publish_day(&self, windows: &[WindowId]) -> Result<Vec<Failed>, Stop> {
        // Binding a window that holds no records would make it not body-only.
        let windows: Vec<WindowId> = windows
            .iter()
            .filter(|window| {
                window_dir(self.journal, window)
                    .join(BODY_RECORDS_FILE)
                    .is_file()
            })
            .cloned()
            .collect();
        let mut failed = Vec::new();
        if windows.is_empty() {
            return Ok(failed);
        }
        let record = self.publish_once(&windows)?;
        if record.status == PublicationStatus::Success {
            self.failed_in_a_row.set(0);
            return Ok(failed);
        }
        let mut first = true;
        let mut retry = Vec::new();
        for (window, outcome) in windows.iter().zip(&record.segments) {
            match &outcome.outcome {
                SegmentBindingOutcome::Bound { .. } => self.failed_in_a_row.set(0),
                SegmentBindingOutcome::OwnerDeleted => {}
                SegmentBindingOutcome::FailedAtMarkerWrite { error } => {
                    return Err(Stop::Failed(format!(
                        "{}/{}: the stream's marker couldn't be written: {error}",
                        window.0, window.1
                    )));
                }
                SegmentBindingOutcome::FailedAtAdvance { .. } if first => {
                    first = false;
                    self.failed_in_a_row.set(self.failed_in_a_row.get() + 1);
                    failed.push(Failed {
                        day: window.0.clone(),
                        key: Some(window.1.clone()),
                        stage: "bind".to_owned(),
                    });
                }
                SegmentBindingOutcome::FailedAtAdvance { .. } => retry.push(window.clone()),
            }
        }
        for errored in &record.indexing.errored {
            if let Some(window) = windows.iter().find(|window| {
                errored.path == window_dir(self.journal, window).join(BODY_RECORDS_FILE)
            }) {
                failed.push(Failed {
                    day: window.0.clone(),
                    key: Some(window.1.clone()),
                    stage: "search".to_owned(),
                });
            }
        }
        for marker in &record.day_markers {
            if matches!(marker.outcome, DayMarkerStatus::Failed { .. }) {
                failed.push(Failed {
                    day: marker.day.clone(),
                    key: None,
                    stage: "day marker".to_owned(),
                });
            }
        }
        // The windows the stream didn't attempt after the first failure, one at a
        // time, since any number of them can be at fault.
        for window in &retry {
            let one = self.publish_once(std::slice::from_ref(window))?;
            if one
                .segments
                .iter()
                .any(|outcome| matches!(outcome.outcome, SegmentBindingOutcome::Bound { .. }))
            {
                self.failed_in_a_row.set(0);
            } else {
                self.failed_in_a_row.set(self.failed_in_a_row.get() + 1);
            }
            if one.status != PublicationStatus::Success {
                failed.push(Failed {
                    day: window.0.clone(),
                    key: Some(window.1.clone()),
                    stage: "bind".to_owned(),
                });
            }
        }
        Ok(failed)
    }

    /// So many windows failing in a row, across days, with none between them bound,
    /// is the stream's fault, and going on would only repeat it for every window
    /// left. Asked once a day is recorded, so the next run starts on the day after
    /// and every run gets at least a day further.
    fn stop_if_stuck(&self, day: &str) -> Result<(), Stop> {
        let count = self.failed_in_a_row.get();
        if count >= MAX_FAILED_IN_A_ROW {
            return Err(Stop::Failed(format!(
                "the import stopped after {}: {count} segments in a row couldn't be finished; import the same file again to carry on from there",
                readable_day(day)
            )));
        }
        Ok(())
    }

    /// Retry what an earlier run couldn't publish. What still fails stays.
    fn retry_failed(&self, failed: &mut BTreeSet<Failed>) -> Result<(), Stop> {
        let mut still = BTreeSet::new();
        for entry in std::mem::take(failed) {
            match &entry.key {
                None => {
                    if solstone_core_journal_io::bump_body_marker(self.journal, &entry.day).is_err()
                    {
                        still.insert(entry);
                    }
                }
                Some(key) => {
                    let window = (entry.day.clone(), key.clone());
                    let dir = window_dir(self.journal, &window);
                    if !dir.join(BODY_RECORDS_FILE).is_file()
                        || solstone_core_segment::owner_deleted(&dir).unwrap_or(false)
                    {
                        continue;
                    }
                    let record = self.publish_once(std::slice::from_ref(&window))?;
                    if record.status != PublicationStatus::Success {
                        still.insert(entry);
                    }
                }
            }
        }
        *failed = still;
        Ok(())
    }

    fn write_progress(&self, state: &RunState) {
        let progress = serde_json::json!({
            "phase": state.phase,
            "rows_scanned": state.scan.rows,
            "rows_placed": state.place.rows,
            "days_total": state.place.days.len(),
            "days_done": state.done_through.as_ref().map_or(0, |done| {
                state.place.days.range(..=done.clone()).count()
            }),
            "windows": {
                "written": state.totals.written,
                "unchanged": state.totals.unchanged,
                "stayed_deleted": state.totals.stayed_deleted,
                "held_back": state.totals.held_back,
                "unreadable": state.totals.unreadable,
                "not_saved": state.failed.len(),
            },
            "published": state.totals.published,
            "at_ms": now_ms(),
        });
        let _ = atomic_replace(
            self.import_dir().join(PROGRESS_FILE),
            progress.to_string().as_bytes(),
            PRIVATE,
        );
    }

    /// Give windows their facet, where they hold records: a folder holding only a
    /// facet would stop being body-only.
    fn write_facets(
        &self,
        day: &str,
        windows: &[PendingWindow],
        facet: &str,
    ) -> Result<(), String> {
        for window in windows.iter().filter(|window| window.facet) {
            let dir = window_dir(self.journal, &(day.to_owned(), window.key.clone()));
            if !dir.join(BODY_RECORDS_FILE).is_file() {
                continue;
            }
            let path = dir.join("talents/facets.json");
            fs::create_dir_all(path.parent().expect("facets file has a parent"))
                .map_err(|error| error.to_string())?;
            atomic_replace(&path, facet_file_text(facet).as_bytes(), PRIVATE)
                .map_err(|error| error.to_string())?;
            sync_folder(&dir)?;
        }
        Ok(())
    }

    /// Decide, write and publish one day, its readings already settled. `redo` is
    /// the same day's recorded decisions, from a run that stopped partway through
    /// it, whose counts stand in for this attempt's.
    fn write_day(
        &self,
        state: &mut RunState,
        day: String,
        windows: DayRows,
        held_back: u64,
    ) -> Result<(), Stop> {
        let facet = self.request.facet;
        let redo = state.pending.take().filter(|pending| pending.day == day);
        let mut decided: Vec<(WindowId, Decision)> = Vec::with_capacity(windows.len());
        for (window, rows) in windows {
            let decision = decide(self.journal, &window, rows, facet, Some(&self.import_id))?;
            decided.push((window, decision));
        }
        let pending = match redo {
            // The recorded decisions stand: a window written last time now
            // reads as unchanged, and must still be published and counted.
            Some(recorded) => recorded,
            None => {
                let mut counts = Totals {
                    held_back,
                    ..Totals::default()
                };
                let mut listing = Vec::new();
                let mut recorded = Vec::new();
                for (window, decision) in &decided {
                    counts.stayed_deleted += u64::from(decision.stayed_deleted);
                    counts.unreadable += u64::from(decision.unreadable);
                    if decision.stayed_deleted || decision.unreadable {
                        continue;
                    }
                    counts.kept_facet += u64::from(decision.kept_facet);
                    counts.refiled += u64::from(decision.refiled);
                    counts.unchanged += u64::from(decision.unchanged);
                    if decision.contents.is_some() {
                        counts.written += 1;
                        counts.entries += decision.units;
                    }
                    if decision.publish {
                        counts.published += 1;
                        counts.days.insert(day.clone());
                    }
                    listing.push((
                        window.1.clone(),
                        decision.new_rows,
                        decision.updated_rows,
                        decision.unchanged_rows,
                    ));
                    recorded.push(PendingWindow {
                        key: window.1.clone(),
                        write: decision.contents.is_some(),
                        facet: decision.facet_changed,
                        publish: decision.publish,
                    });
                }
                Pending {
                    day: day.clone(),
                    windows: recorded,
                    counts,
                    listing,
                }
            }
        };
        state.pending = Some(pending.clone());
        save_state(self.journal, state)?;
        self.point("pending")?;
        // A window holding records takes its new facet first, so new rows never sit
        // under its old one. A new window takes it after its records.
        if let Some(facet) = facet {
            self.write_facets(&day, &pending.windows, facet)?;
        }
        let mut written = 0;
        for (window, decision) in decided {
            let Some(contents) = decision.contents else {
                continue;
            };
            let rendered = RenderedImport {
                skipped: 0,
                source: RegistrySource::AppleHealth,
                entries: decision.units,
                files: vec![SegmentFile {
                    day: window.0.clone(),
                    segment: window.1.clone(),
                    name: BODY_RECORDS_FILE.to_owned(),
                    units: decision.units,
                    contents,
                    calendar: false,
                }],
                items: Vec::new(),
                summary: String::new(),
                calendar_counts: Default::default(),
            };
            let outcome = save::write_rendered(self.journal, None, &rendered);
            if let Some(error) = outcome.error {
                if !solstone_core_import::work_continues() {
                    return Err(Stop::Stopped);
                }
                return Err(Stop::Failed(format!(
                    "{}: {}",
                    error.path.display(),
                    error.detail
                )));
            }
            written += 1;
            if written == 1 {
                self.point("written")?;
            }
        }
        if let Some(facet) = facet {
            self.write_facets(&day, &pending.windows, facet)?;
        }

        let publish: Vec<WindowId> = pending
            .windows
            .iter()
            .filter(|window| window.publish)
            .map(|window| (day.clone(), window.key.clone()))
            .collect();
        let failed = self.publish_day(&publish)?;
        self.point("published")?;

        // A window's own folder is synced as its file lands. What holds that folder
        // has to last too, before the day counts as done and its staging can go.
        if pending.windows.iter().any(|window| window.write) {
            sync_day_folders(self.journal, &day)?;
        }
        self.append_listing(state, &pending)?;
        state.totals.add(&pending.counts);
        state.totals.add(&activity_totals(self.journal, &day));
        state.failed.extend(failed);
        state.done_through = Some(day.clone());
        state.pending = None;
        save_state(self.journal, state)?;
        self.write_progress(state);
        self.stop_if_stuck(&day)
    }

    /// One line per day of what this run touched, kept under the import whose run
    /// the state is, which a takeover isn't.
    fn append_listing(&self, state: &mut RunState, pending: &Pending) -> Result<(), String> {
        let path = self
            .journal
            .join("imports")
            .join(&state.import_id)
            .join(LISTING_FILE);
        // Not append mode: Windows refuses to cut a file opened that way.
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| error.to_string())?;
        // A run that stopped after appending and before its checkpoint wrote this
        // day already: cut back to the length the state knows.
        file.set_len(state.listing_len)
            .map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(state.listing_len))
            .map_err(|error| error.to_string())?;
        let mut line = serde_json::to_string(&serde_json::json!({
            "day": pending.day,
            "windows": pending.listing,
        }))
        .map_err(|error| error.to_string())?;
        line.push('\n');
        file.write_all(line.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        state.listing_len += line.len() as u64;
        Ok(())
    }

    /// Republish every window in the stream, day by day, after first bringing the
    /// stream's record level with its markers.
    fn repair(&self, state: &mut RunState) -> Result<(), Stop> {
        if state.phase != Phase::Repair {
            state.phase = Phase::Repair;
            state.done_through = None;
            save_state(self.journal, state)?;
        }
        if let Some(note) = reconcile_stream(self.journal)? {
            state.notes.push(note);
        }
        for day in stream_days(self.journal)? {
            if state.done_through.as_ref().is_some_and(|done| day <= *done) {
                continue;
            }
            let windows = stored_windows_of_day(self.journal, &day)?;
            let failed = self.publish_day(&windows)?;
            state.failed.extend(failed);
            state.done_through = Some(day.clone());
            save_state(self.journal, state)?;
            self.point("repaired")?;
            self.stop_if_stuck(&day)?;
        }
        state.notes.push(
            "The last import's progress couldn't be read, so every window already saved was published again."
                .to_owned(),
        );
        Ok(())
    }
}

/// A day as a person reads a date.
fn readable_day(day: &str) -> String {
    match (day.get(..4), day.get(4..6), day.get(6..8)) {
        (Some(year), Some(month), Some(date)) => format!("{year}-{month}-{date}"),
        _ => day.to_owned(),
    }
}

/// Make a day's new folders last: the windows in the stream, the stream in the day,
/// and the day in the chronicle. A day that got no window has none to keep.
fn sync_day_folders(journal: &Path, day: &str) -> Result<(), String> {
    let chronicle = journal.join("chronicle");
    let day_dir = chronicle.join(day);
    if !day_dir.join(STREAM).is_dir() {
        return Ok(());
    }
    sync_folder(&day_dir.join(STREAM))?;
    sync_folder(&day_dir)?;
    sync_folder(&chronicle)
}

/// Move the stream's record up to its markers' tail, as the streams command does,
/// and say so if two windows already claim one place in the stream.
fn reconcile_stream(journal: &Path) -> Result<Option<String>, String> {
    let mut tail: Option<(u64, String, String)> = None;
    let mut seen: BTreeMap<u64, u32> = BTreeMap::new();
    for day in stream_days(journal)? {
        for window in stored_windows_of_day(journal, &day)? {
            let Ok(bytes) = fs::read(window_dir(journal, &window).join("stream.json")) else {
                continue;
            };
            let Some(seq) = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|marker| marker.get("seq").and_then(serde_json::Value::as_u64))
            else {
                continue;
            };
            *seen.entry(seq).or_default() += 1;
            if tail.as_ref().is_none_or(|(best, ..)| seq > *best) {
                tail = Some((seq, window.0.clone(), window.1.clone()));
            }
        }
    }
    if let Some((max_seq, last_day, last_segment)) = &tail {
        let _ = solstone_core_segment::repair_stream_tail_from_markers(
            journal,
            STREAM,
            &solstone_core_segment::MarkerTail {
                last_day,
                last_segment,
                max_seq: *max_seq,
            },
            LockOptions::default(),
        );
    }
    let shared = seen.values().filter(|count| **count > 1).count();
    Ok((shared > 0).then(|| {
        format!("{shared} places in this source's sequence are claimed by more than one window; they were left as they are.")
    }))
}

/// Create the activity records of a day's experiences, as totals.
fn activity_totals(journal: &Path, day: &str) -> Totals {
    let counts = crate::body_activity::write_records(journal, day);
    Totals {
        activities: counts.written,
        activities_no_facet: counts.no_facet,
        activities_refreshed: counts.refreshed,
        activities_refused: counts.refused.into_iter().map(|(facet, _)| facet).collect(),
        ..Totals::default()
    }
}

fn summary_of(state: &RunState) -> SummaryCounts {
    let mut refused = state.scan.refused.clone();
    for (stage, count) in &state.place.refused {
        *refused.entry(stage.clone()).or_default() += count;
    }
    SummaryCounts {
        read: state.scan.read.clone(),
        not_read: state.scan.not_read.clone(),
        refused,
        written: state.totals.written,
        unchanged: state.totals.unchanged,
        stayed_deleted: state.totals.stayed_deleted,
        held_back: state.totals.held_back,
        unreadable: state.totals.unreadable,
        kept_facet: state.totals.kept_facet,
        refiled: state.totals.refiled,
        activities: state.totals.activities,
        activities_no_facet: state.totals.activities_no_facet,
        activities_refreshed: state.totals.activities_refreshed,
        activities_refused: state.totals.activities_refused.iter().cloned().collect(),
        not_saved: state
            .failed
            .iter()
            .map(|failed| match &failed.key {
                Some(key) => format!("{} {key}: {}", failed.day, failed.stage),
                None => format!("{}: {}", failed.day, failed.stage),
            })
            .collect(),
        notes: state.notes.clone(),
    }
}

/// Close an earlier import whose run another export replaced, as unconfirmed.
fn close_superseded(journal: &Path, state: &RunState, zone: chrono_tz::Tz) {
    let Ok(_lock) = hold_import_lock(journal, &state.import_id) else {
        return;
    };
    // It finished, and only its staging was left when it stopped.
    if let Ok(Some(metadata)) = read_provenance(journal, &state.import_id)
        && let AttemptRead::Present(facts) = read_attempt_facts(&metadata)
        && facts.state == AttemptState::Completed
        && facts.failure_reason.is_none()
    {
        return;
    }
    let summary = summary_text(&summary_of(state), state.facet.as_deref(), zone);
    let _ = atomic_replace(
        journal
            .join("imports")
            .join(&state.import_id)
            .join("summary.md"),
        format!("{summary}This import was replaced by a later one before it finished.\n")
            .as_bytes(),
        PRIVATE,
    );
    close_attempt_unlocked(journal, &state.import_id);
}

/// An earlier run's import still reading as running, when this run is taking over
/// from it, reads as unconfirmed instead.
fn close_attempt(journal: &Path, import_id: &str) {
    if let Ok(_lock) = hold_import_lock(journal, import_id) {
        close_attempt_unlocked(journal, import_id);
    }
}

fn close_attempt_unlocked(journal: &Path, import_id: &str) {
    if let Ok(Some(metadata)) = read_provenance(journal, import_id)
        && let AttemptRead::Present(facts) = read_attempt_facts(&metadata)
        && facts.state == AttemptState::Running
    {
        let _ = record_unconfirmed_attempt_unlocked(
            journal,
            import_id,
            facts.generation,
            now_ms(),
            Some("replaced by a later import before it finished".to_owned()),
        );
    }
}

/// Whether an earlier run left its record in this import, which a run that isn't
/// resuming it must not overwrite.
fn holds_a_run(journal: &Path, import_id: &str) -> bool {
    let dir = journal.join("imports").join(import_id);
    [LISTING_FILE, PROGRESS_FILE, "summary.md", "manifest.json"]
        .iter()
        .any(|name| dir.join(name).exists())
}

/// Publish an Apple export as body windows. Runs under the source's lock.
pub fn run_apple_health_segments(
    journal: &Path,
    request: &AppleSegmentsRequest<'_>,
) -> Result<AppleSegmentsOutcome, String> {
    if let Some(facet) = request.facet {
        crate::cli_argv::require_enabled_facet(facet, journal)?;
    }
    fs::create_dir_all(journal.join("imports")).map_err(|error| error.to_string())?;
    let _lock = hold_lock(
        journal.join(LOCK_REL),
        LockOptions {
            timeout: LOCK_TIMEOUT,
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| "apple_health_lock_unavailable".to_owned())?;
    let zone = read_or_create_zone(journal)?;
    let source_hash = solstone_core_import::hash_source(request.source)
        .map_err(|error| error.to_string())?
        .into_inner();
    // Before anything touches an earlier run: an export that can't be read this
    // moment says nothing about whether that run should carry on.
    let export_hash = match solstone_core_body_ingest::apple_folder_export(request.source)
        .map_err(|error| refusal_text(&error.to_string()))?
    {
        Some(export) => solstone_core_import::hash_source(&export)
            .map_err(|error| error.to_string())?
            .into_inner(),
        None => String::new(),
    };

    // The run state is read before any binding, so the same export resumes under
    // the import it began as, whatever timestamp this request carries.
    let previous = read_state(journal);
    let resumable = match &previous {
        Read::Trusted(state)
            if state.source_hash == source_hash
                && state.export_hash == export_hash
                && state.facet.as_deref() == request.facet
                && matches!(state.phase, Phase::Scan | Phase::Place | Phase::Write) =>
        {
            Some(state.import_id.clone())
        }
        _ => None,
    };
    let import_id = match resumable.clone() {
        Some(id) => id,
        None => {
            let requested = solstone_core_import::validate_timestamp(request.timestamp)
                .map_err(|e| e.to_string())?;
            solstone_core_import::bind_import_record_where(
                journal,
                &requested,
                Some(request.source),
                &|existing| !holds_a_run(journal, existing),
            )
            .map_err(|error| error.to_string())?
            .import_id
            .as_str()
            .to_owned()
        }
    };
    if solstone_core_import::refuse_if_live_running(journal, &import_id, STREAM).is_some() {
        return Err("this import is already running".to_owned());
    }
    let generation = solstone_core_import::admit_running_attempt(
        journal,
        &import_id,
        now_ms(),
        Some("apple_health"),
    )
    .map_err(|error| error.to_string())?
    .generation;
    let run = Run {
        journal,
        request,
        import_id: import_id.clone(),
        generation,
        failed_in_a_row: Cell::new(0),
    };
    let finish = |input, index_only: &[PathBuf]| {
        finish_import_attempt_indexing(
            journal,
            &import_id,
            generation,
            RegistrySource::AppleHealth.name(),
            input,
            index_only,
        )
    };
    let resume = resumable.is_some();
    let outcome = run_phases(&run, previous, resume, (&source_hash, &export_hash), zone);
    match outcome {
        Ok(outcome) => Ok(outcome),
        Err(Stop::Injected(point)) => {
            // As a process that died there would: its attempt lock goes with it.
            solstone_core_import::metadata::release_attempt(journal, &import_id);
            Err(format!("stopped at {point}"))
        }
        Err(Stop::Superseded) => Err("this import was superseded".to_owned()),
        Err(Stop::Stopped) => {
            let _ = finish(ImportTerminalInput::Failed(&[]), &[]);
            Err("import stopped".to_owned())
        }
        Err(Stop::Failed(error)) => {
            let _ = finish(ImportTerminalInput::Failed(&[]), &[]);
            Err(refusal_text(&error))
        }
    }
}

/// What a run's failure says, in plain words where the export is why.
fn refusal_text(error: &str) -> String {
    if error.contains("export_size_limit") {
        format!(
            "this export is larger than {} GiB, the most one import reads",
            solstone_core_body_ingest::apple_scan_byte_bound() / (1024 * 1024 * 1024)
        )
    } else if error.contains(solstone_core_body_ingest::EXPORT_INCOMPLETE) {
        solstone_core_body_ingest::EXPORT_INCOMPLETE_TEXT.to_owned()
    } else {
        error.to_owned()
    }
}

fn run_phases(
    run: &Run<'_>,
    previous: Read,
    resume: bool,
    (source_hash, export_hash): (&str, &str),
    zone: chrono_tz::Tz,
) -> Result<AppleSegmentsOutcome, Stop> {
    let journal = run.journal;
    let request = run.request;
    let dir = run_dir(journal);
    let fresh = |failed: BTreeSet<Failed>, notes: Vec<String>| RunState {
        schema: STATE_SCHEMA,
        phase: Phase::Scan,
        source_hash: source_hash.to_owned(),
        export_hash: export_hash.to_owned(),
        import_id: run.import_id.clone(),
        facet: request.facet.map(str::to_owned),
        failed,
        notes,
        ..RunState::default()
    };
    let mut state = match previous {
        Read::Trusted(state) if resume => *state,
        Read::Trusted(mut old) if old.phase == Phase::Repair => {
            if old.import_id != run.import_id {
                close_attempt(journal, &old.import_id);
            }
            run.repair(&mut old)?;
            let carried = std::mem::take(&mut old.failed);
            fresh(carried, std::mem::take(&mut old.notes))
        }
        Read::Trusted(mut old) => {
            // Another export, or a finished run's leftovers: everything the old
            // run wrote is bound and queued before its staging goes.
            // Its half-done day's windows are on disk as far as they got; what it
            // owed them is publication, which is all this does.
            if let Some(pending) = old.pending.take() {
                if let Some(facet) = &old.facet {
                    run.write_facets(&pending.day, &pending.windows, facet)?;
                }
                let windows: Vec<WindowId> = pending
                    .windows
                    .iter()
                    .filter(|window| window.publish)
                    .map(|window| (pending.day.clone(), window.key.clone()))
                    .collect();
                let failed = run.publish_day(&windows)?;
                old.failed.extend(failed);
                sync_day_folders(journal, &pending.day)?;
                // That day's rows name the old import, so its listing has to, whichever
                // of the windows it had got to. A window it hadn't is listed too.
                let import = journal.join("imports").join(&old.import_id);
                if import.is_dir() {
                    run.append_listing(&mut old, &pending)?;
                    old.totals.add(&pending.counts);
                }
                // A record its run didn't get to; the totals are this run's.
                let _ = activity_totals(journal, &pending.day);
            }
            run.retry_failed(&mut old.failed)?;
            if old.phase != Phase::Retry && old.import_id != run.import_id {
                close_superseded(journal, &old, zone);
            }
            fresh(std::mem::take(&mut old.failed), Vec::new())
        }
        Read::Untrusted(old) => {
            if let Some(old) = old.filter(|old| *old != run.import_id) {
                close_attempt(journal, &old);
            }
            let mut repairing = fresh(BTreeSet::new(), Vec::new());
            run.repair(&mut repairing)?;
            fresh(std::mem::take(&mut repairing.failed), repairing.notes)
        }
        Read::Absent => fresh(BTreeSet::new(), Vec::new()),
    };
    if state.phase == Phase::Scan {
        let _ = remove_dir_all(journal, &format!("{RUN_DIR_REL}/scan"));
        let _ = remove_dir_all(journal, &format!("{RUN_DIR_REL}/placed"));
        save_state(journal, &state)?;
        run.retry_failed(&mut state.failed)?;
        let progress = run.import_dir().join(PROGRESS_FILE);
        state.scan = scan(
            request.source,
            &dir.join("scan"),
            request.spill_bytes,
            &mut |rows| {
                let line = serde_json::json!({"phase": Phase::Scan, "rows_scanned": rows, "at_ms": now_ms()});
                let _ = atomic_replace(&progress, line.to_string().as_bytes(), PRIVATE);
            },
        )?;
        state.phase = Phase::Place;
        save_state(journal, &state)?;
        run.write_progress(&state);
        run.point("scanned")?;
    }
    if state.phase == Phase::Place {
        let _ = remove_dir_all(journal, &format!("{RUN_DIR_REL}/placed"));
        state.place = place(
            journal,
            zone,
            &dir.join("scan"),
            &dir.join("placed"),
            request.spill_bytes,
            &mut |_| {},
        )?;
        state.phase = Phase::Write;
        save_state(journal, &state)?;
        let _ = remove_dir_all(journal, &format!("{RUN_DIR_REL}/scan"));
        run.write_progress(&state);
        run.point("placed")?;
    }
    let refused = state.place.refused_nights.clone();
    let mut days = Days::open(
        journal,
        &dir.join("placed"),
        &refused,
        state.done_through.clone(),
    )?;
    while let Some((day, windows)) = days.next_day()? {
        let (windows, held_back) = settle_readings(journal, &day, zone, windows)?;
        run.write_day(&mut state, day, windows, held_back)?;
    }
    // Close the placed runs before anything removes them.
    drop(days);
    run.retry_failed(&mut state.failed)?;
    run.point("written-all")?;

    // Finish: the summary, the manifest, and one terminal attempt that lists
    // only the windows still failing, so the import reads as partly failed
    // until they're saved.
    let summary = summary_text(&summary_of(&state), request.facet, zone);
    let import_dir = run.import_dir();
    let summary_path = import_dir.join("summary.md");
    atomic_replace(&summary_path, summary.as_bytes(), PRIVATE)
        .map_err(|error| error.to_string())?;
    let days: Vec<String> = state.totals.days.iter().cloned().collect();
    solstone_core_import::write_manifest(&solstone_core_import::ManifestWriteRequest {
        journal_root: journal,
        import_id: &run.import_id,
        source_type: "apple_health",
        source_hash: &solstone_core_import::SourceHash::new(source_hash.to_owned()),
        entry_count: state.totals.entries,
        days_affected: &days,
        files_created: &[],
        imported_via: "native",
        link_id: None,
        observer_handle: None,
        raw_retention: Some(RAW_RETENTION),
    })
    .map_err(|error| error.to_string())?;
    // A day whose marker didn't move is listed by one of its windows, so the
    // marker is tried again and the import reads as partly failed meanwhile.
    let mut failing: Vec<TextCreated> = Vec::new();
    for failed in &state.failed {
        let window = match &failed.key {
            Some(key) => Some((failed.day.clone(), key.clone())),
            None => stored_windows_of_day(journal, &failed.day)?
                .into_iter()
                .next(),
        };
        if let Some(window) = window
            && !failing
                .iter()
                .any(|created| created.day == window.0 && created.segment == window.1)
        {
            failing.push(run.created(&window));
        }
    }
    let index_only = [summary_path];
    match finish_import_attempt_indexing(
        journal,
        &run.import_id,
        run.generation,
        RegistrySource::AppleHealth.name(),
        ImportTerminalInput::Success(&failing),
        &index_only,
    ) {
        Ok(ImportFinish::Applied) => {}
        Ok(ImportFinish::Stale) => return Err(Stop::Superseded),
        Err(error) => return Err(Stop::Failed(error.to_string())),
    }
    run.point("finished")?;
    let record = read_publication_record(&import_dir).ok().flatten();
    if let Some(record) = &record
        && record.status == PublicationStatus::Success
    {
        // Every listed window bound, was queued, and moved its day's marker.
        state.failed.clear();
    }
    let completed = solstone_core_import::project_import_result(journal, &run.import_id).status
        == solstone_core_import::ProjectionStatus::Success
        && record.is_some_and(|record| record.status == PublicationStatus::Success);
    let finished = completed && state.failed.is_empty();
    if finished {
        remove_dir_all(journal, RUN_DIR_REL).map_err(|error| error.to_string())?;
    } else {
        let _ = remove_dir_all(journal, &format!("{RUN_DIR_REL}/placed"));
        let retry = RunState {
            schema: STATE_SCHEMA,
            phase: Phase::Retry,
            source_hash: state.source_hash.clone(),
            export_hash: state.export_hash.clone(),
            import_id: state.import_id.clone(),
            facet: state.facet.clone(),
            failed: state.failed.clone(),
            ..RunState::default()
        };
        save_state(journal, &retry)?;
    }
    run.write_progress(&state);
    Ok(AppleSegmentsOutcome {
        import_id: run.import_id.clone(),
        summary,
        published: state.totals.published,
        finished,
    })
}

#[cfg(test)]
#[path = "body_segments_tests.rs"]
mod tests;
