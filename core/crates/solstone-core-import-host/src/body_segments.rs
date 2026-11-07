// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An Apple Health export, or a folder of staged Oura pages, through the ordinary
//! import lifecycle, into body windows, in bounded memory and one day at a time.
//! `docs/BODY_SEGMENTS.md` is the contract. Only the scan is each source's own;
//! placing, deciding, writing, publishing and resuming are one writer.
//!
//! No owner route reaches this yet: every owner command still saves a bundle, and
//! the only callers outside tests are `examples/apple_body_segments.rs` and
//! `examples/oura_body_segments.rs`.
//!
//! A run keeps one small state file, `imports/.<source>-run/state.json`, replaced
//! atomically. Its cursor and the day being written move in one replace, so a kill
//! anywhere leaves either a day not yet counted, which the next run redoes and
//! publishes whether or not its bytes now match, or a day done.
//!
//! An Oura run that asks Oura itself ([`import_oura`]) fetches first, into
//! `imports/.oura-run/pages/` (`oura_fetch.rs`), and reads those pages as its
//! export. Its coverage (`oura_coverage.rs`) is written here and nowhere else:
//! each hole takes back what it touches as soon as the state holds it, and what
//! the run fetched whole is added once its last day is done.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_body_ingest::oura_sync::OuraHttp;
use solstone_core_format::content::body_records::BODY_RECORDS_FILE;
use solstone_core_format::content::body_records::{BodyRow, RowKind, decode_rows};
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
    DayRows, Decision, SummaryCounts, WindowId, decide, facet_file_text, read_or_create_zone,
    settle_readings_noting, stored_windows_of_day, stream_days, summary_text,
};
use solstone_core_import_sources::apple_health_run::{
    DEFAULT_SPILL_BYTES, Days, PlaceResult, ScanCounts, place, runs_in, scan, sync_folder,
};
use solstone_core_import_sources::body_stream::{APPLE, BodySource, OURA};
use solstone_core_import_sources::oura::{Hole, HoleReason, holes_of};
use solstone_core_import_sources::oura_run::{
    ScanFindings, converging_dropped, pages_identity, scan_pages,
};
use solstone_core_import_sources::save::{self, RenderedImport, SegmentFile};
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, atomic_replace, hold_lock, remove_dir_all,
};

use crate::import_publication::{
    ImportFinish, ImportTerminalInput, finish_import_attempt_indexing,
};
use crate::oura_coverage::{Coverage, hole_place, read_coverage, update_coverage};
pub use crate::oura_fetch::{
    EndpointReport, LiveOuraTransport, OLD_LOCK_TIMEOUT, OuraAuth, OuraImportError, OuraStop,
    RecordedOura, ResultKind, USER_AGENT,
};
use crate::oura_fetch::{FetchInput, FetchRun, FetchState, fetch, plan};
use crate::registry::now_ms;

const RAW_RETENTION: &str = "discard";
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// An Apple run's own folder. Only `scan/` and `placed/` are left out of copies.
pub const RUN_DIR_REL: &str = APPLE.run_dir_rel;
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

/// What one Oura run was asked to do.
pub struct OuraSegmentsRequest<'a> {
    /// A folder of staged pages and their `index.json`.
    pub pages: &'a Path,
    /// `YYYYMMDD_HHMMSS`, as every import's id.
    pub timestamp: &'a str,
    pub facet: Option<&'a str>,
    /// How much a spill buffer holds; `DEFAULT_SPILL_BYTES` unless a test says.
    pub spill_bytes: usize,
    /// Test seam: called at each named point, and the run stops there, as if
    /// killed, when it returns true.
    pub stop_at: Option<&'a dyn Fn(&str) -> bool>,
}

impl<'a> OuraSegmentsRequest<'a> {
    #[must_use]
    pub fn new(pages: &'a Path, timestamp: &'a str, facet: Option<&'a str>) -> Self {
        Self {
            pages,
            timestamp,
            facet,
            spill_bytes: DEFAULT_SPILL_BYTES,
            stop_at: None,
        }
    }
}

/// What one Oura run did.
#[derive(Debug)]
pub struct OuraSegmentsOutcome {
    pub import_id: String,
    pub summary: String,
    pub published: u64,
    /// Nothing is left to do: every window is saved and the run state is gone.
    pub finished: bool,
    /// What Oura answered for that the journal couldn't write whole, this run's and
    /// any carried from a run it took over.
    pub holes: Vec<Hole>,
}

/// What one Oura import that asks Oura itself was asked to do.
pub struct OuraImportRequest<'a> {
    /// `YYYYMMDD_HHMMSS`, as every import's id.
    pub timestamp: &'a str,
    pub facet: Option<&'a str>,
    /// Owner days, first and last inclusive. `None` asks each endpoint from seven
    /// days before what it last covered, or the last 30 days.
    pub range: Option<(NaiveDate, NaiveDate)>,
    /// Oura, or something answering as it.
    pub transport: &'a mut dyn OuraHttp,
    pub auth: OuraAuth,
    /// How long a refresh waits for the old sync's lock.
    pub old_lock_timeout: Duration,
    /// How much a spill buffer holds; `DEFAULT_SPILL_BYTES` unless a test says.
    pub spill_bytes: usize,
    /// Test seam: called at each named point, and the run stops there, as if
    /// killed, when it returns true.
    pub stop_at: Option<&'a dyn Fn(&str) -> bool>,
    /// Test seam: the clock the run reads for today and for when a record arrived.
    pub clock: Option<&'a dyn Fn() -> DateTime<Utc>>,
}

impl<'a> OuraImportRequest<'a> {
    #[must_use]
    pub fn new(
        timestamp: &'a str,
        facet: Option<&'a str>,
        transport: &'a mut dyn OuraHttp,
    ) -> Self {
        Self {
            timestamp,
            facet,
            range: None,
            transport,
            auth: OuraAuth::Journal,
            old_lock_timeout: OLD_LOCK_TIMEOUT,
            spill_bytes: DEFAULT_SPILL_BYTES,
            stop_at: None,
            clock: None,
        }
    }
}

/// What one Oura import that asked Oura itself did.
#[derive(Debug)]
pub struct OuraImportOutcome {
    pub import_id: String,
    pub summary: String,
    pub published: u64,
    /// Nothing is left to do: every window is saved and the run state is gone.
    pub finished: bool,
    /// How each endpoint ended, in the order Oura was asked.
    pub results: Vec<EndpointReport>,
    /// What this run, and any it took over, couldn't write whole.
    pub holes: Vec<Hole>,
    /// The journal's coverage as this run left it.
    pub coverage: Coverage,
}

/// What a run reads: the only part of a request that is each source's own.
#[derive(Clone, Copy)]
enum Input<'a> {
    /// An Apple Health export, a zip or a folder.
    Apple(&'a Path),
    /// A folder of staged Oura pages.
    Oura(&'a Path),
    /// Oura itself, fetched into the run's own staging first.
    Fetch(&'a FetchInput<'a>),
}

impl Input<'_> {
    /// What the import record is bound to, when the input is there to hash.
    fn path(&self) -> Option<&Path> {
        match self {
            Self::Apple(path) | Self::Oura(path) => Some(path),
            Self::Fetch(_) => None,
        }
    }
}

/// One run's request, whichever source it reads.
struct RunRequest<'a> {
    input: Input<'a>,
    timestamp: &'a str,
    facet: Option<&'a str>,
    spill_bytes: usize,
    stop_at: Option<&'a dyn Fn(&str) -> bool>,
    clock: Option<&'a dyn Fn() -> DateTime<Utc>>,
}

/// What one run did, whichever source it read.
struct RunOutcome {
    import_id: String,
    summary: String,
    published: u64,
    finished: bool,
    holes: Vec<Hole>,
    results: Vec<EndpointReport>,
}

/// How a run ended short.
enum RunError {
    /// One of the stops an Oura fetch makes for the whole run.
    Stop(OuraStop),
    Failed(String),
}

impl RunError {
    fn text(self) -> String {
        match self {
            Self::Stop(stop) => stop.words().to_owned(),
            Self::Failed(error) => error,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    /// Asking Oura for its pages, into the run's staging.
    Fetch,
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
    /// What this day couldn't write whole, so a redone day reports the same.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    holes: BTreeSet<Hole>,
    /// Per window that had rows become `moved`: its key and how many.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    moved: Vec<(String, u64)>,
    /// Converging documents with a window here that couldn't be read as rows.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    unfinished: BTreeSet<String>,
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
    /// What a source's scan found besides its counts. Apple's finds nothing here.
    #[serde(default, skip_serializing_if = "ScanFindings::is_empty")]
    findings: ScanFindings,
    place: PlaceResult,
    /// The last day written; the next run continues after it.
    done_through: Option<String>,
    totals: Totals,
    failed: BTreeSet<Failed>,
    /// What the days written so far couldn't write whole, and what a run this one
    /// took over couldn't.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    holes: BTreeSet<Hole>,
    pending: Option<Pending>,
    listing_len: u64,
    /// The import whose run this state is: kept apart from the current one
    /// while a repair or takeover runs under a newer attempt.
    notes: Vec<String>,
    /// What an Oura run that asks Oura itself asked for, and how each endpoint
    /// ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetch: Option<FetchState>,
    /// Whether each converging document whose start day has been reached was
    /// dropped, because the owner deleted a window it moves from or to. Decided
    /// once, before that day's first file, and kept past that day, so a redone day
    /// and every later one do as it said.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    drops: BTreeMap<String, bool>,
    /// Converging documents a window of which couldn't be read as rows, so the
    /// move isn't done.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    unfinished: BTreeSet<String>,
}

enum Read {
    Absent,
    Trusted(Box<RunState>),
    Untrusted {
        /// The import whose run it was, when the state can still say.
        import_id: Option<String>,
        /// Its holes, when the state still parses and only its staging is gone.
        holes: Option<BTreeSet<Hole>>,
    },
}

const UNPARSED: Read = Read::Untrusted {
    import_id: None,
    holes: None,
};

fn run_dir(journal: &Path, source: &BodySource) -> PathBuf {
    journal.join(source.run_dir_rel)
}

fn window_dir(journal: &Path, source: &BodySource, (day, key): &WindowId) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join(source.stream)
        .join(key)
}

fn read_state(journal: &Path, source: &BodySource) -> Read {
    let path = run_dir(journal, source).join(STATE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A run folder without its state is a run that didn't finish.
            return if run_dir(journal, source).exists() {
                UNPARSED
            } else {
                Read::Absent
            };
        }
        Err(_) => return UNPARSED,
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
                Phase::Fetch | Phase::Scan | Phase::Repair | Phase::Retry => None,
            };
            let mut whole = needs.is_none_or(|(folder, runs)| {
                runs_in(&run_dir(journal, source).join(folder))
                    .is_ok_and(|found| found.len() as u64 == runs)
            });
            // A fetched run about to scan needs the very pages it fetched.
            if state.phase == Phase::Scan && state.fetch.is_some() {
                whole &= pages_identity(&run_dir(journal, source).join("pages"))
                    .is_ok_and(|identity| identity == state.source_hash);
            }
            if whole {
                Read::Trusted(Box::new(state))
            } else {
                Read::Untrusted {
                    holes: Some(holes_in(&state)),
                    import_id: Some(state.import_id),
                }
            }
        }
        _ => Read::Untrusted {
            import_id: import_id(),
            holes: None,
        },
    }
}

fn save_state(journal: &Path, source: &BodySource, state: &RunState) -> Result<(), String> {
    let bytes = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    fs::create_dir_all(run_dir(journal, source)).map_err(|error| error.to_string())?;
    atomic_replace(run_dir(journal, source).join(STATE_FILE), &bytes, PRIVATE)
        .map_err(|error| error.to_string())
}

/// How a run stopped short.
pub(crate) enum Stop {
    /// An injected stop point: the disk is as a kill there would leave it.
    Injected(String),
    /// The owner asked the import to stop.
    Stopped,
    /// Another attempt owns this import now.
    Superseded,
    Failed(String),
    /// An Oura fetch stopped the whole run.
    Oura(OuraStop),
}

impl From<String> for Stop {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

struct Run<'a, 'r> {
    journal: &'a Path,
    source: &'a BodySource,
    request: &'a RunRequest<'r>,
    import_id: String,
    generation: u64,
    /// Windows that couldn't be published since one last was.
    failed_in_a_row: Cell<u32>,
}

impl Run<'_, '_> {
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

    fn now(&self) -> DateTime<Utc> {
        self.request.clock.map_or_else(Utc::now, |clock| clock())
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
            stream: self.source.stream.to_owned(),
            hints: save::stream_hints(self.source.registry),
            path: window_dir(self.journal, self.source, window).join(BODY_RECORDS_FILE),
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
            importer: self.source.registry.name(),
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
                window_dir(self.journal, self.source, window)
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
                errored.path
                    == window_dir(self.journal, self.source, window).join(BODY_RECORDS_FILE)
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
                    let dir = window_dir(self.journal, self.source, &window);
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
            let dir = window_dir(
                self.journal,
                self.source,
                &(day.to_owned(), window.key.clone()),
            );
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
    ///
    /// A day is written in two passes, so a document moving within it is never
    /// live nowhere: first every window that gains a new row, keeping the old rows
    /// it holds, then every window where a document's old first row becomes
    /// `moved`. A stop between them leaves it live twice, which the next read of it
    /// heals.
    fn write_day(
        &self,
        state: &mut RunState,
        day: String,
        windows: DayRows,
        (held_back, mut holes): (u64, BTreeSet<Hole>),
    ) -> Result<(), Stop> {
        let facet = self.request.facet;
        let redo = state.pending.take().filter(|pending| pending.day == day);
        // Before any of the day's files: a document moving from or into a window
        // the owner has deleted since the scan is dropped, here and on every later
        // day. A redone day keeps what was decided.
        if redo.is_none() {
            for (id, converging) in &state.findings.converging {
                if converging.day == day && !state.drops.contains_key(id) {
                    let dropped = converging_dropped(self.journal, self.source, converging)?;
                    state.drops.insert(id.clone(), dropped);
                }
            }
        }
        let dropped: BTreeSet<&str> = state
            .drops
            .iter()
            .filter(|(_, dropped)| **dropped)
            .map(|(id, _)| id.as_str())
            .collect();
        let converging = &state.findings.converging;
        // The converging document a row is of, when it is one's.
        let document = |row: &BodyRow| -> Option<String> {
            [Some(row.id.as_str()), row.of.as_deref()]
                .into_iter()
                .flatten()
                .find(|id| converging.contains_key(*id))
                .map(str::to_owned)
        };
        let first_mark = |row: &BodyRow| {
            row.kind == RowKind::Moved && row.of.as_deref() == Some(row.id.as_str())
        };
        let mut decided: Vec<(WindowId, Decision, bool)> = Vec::with_capacity(windows.len());
        // The first pass's decision for a window the second pass writes too.
        let mut early: BTreeMap<WindowId, Decision> = BTreeMap::new();
        let mut unfinished = BTreeSet::new();
        for (window, mut rows) in windows {
            if !dropped.is_empty() {
                rows.retain(|_, row| document(row).is_none_or(|id| !dropped.contains(id.as_str())));
                if rows.is_empty() {
                    continue;
                }
            }
            // A window left alone as unreadable is a hole for what its rows say.
            let unwritten = holes_of(rows.values(), HoleReason::Unreadable);
            let touched: BTreeSet<String> = rows.values().filter_map(document).collect();
            let marks_first = rows.values().any(first_mark);
            if marks_first {
                let before: BTreeMap<String, BodyRow> = rows
                    .iter()
                    .filter(|(_, row)| !first_mark(row))
                    .map(|(id, row)| (id.clone(), row.clone()))
                    .collect();
                if !before.is_empty() {
                    let decision = decide(
                        self.journal,
                        self.source,
                        &window,
                        before,
                        facet,
                        Some(&self.import_id),
                    )?;
                    early.insert(window.clone(), decision);
                }
            }
            let decision = decide(
                self.journal,
                self.source,
                &window,
                rows,
                facet,
                Some(&self.import_id),
            )?;
            if decision.unreadable {
                holes.extend(unwritten);
                unfinished.extend(touched);
            }
            decided.push((window, decision, marks_first));
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
                let mut moved = Vec::new();
                let mut recorded = Vec::new();
                for (window, decision, _) in &decided {
                    if decision.absent {
                        continue;
                    }
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
                    if decision.moved_rows > 0 {
                        moved.push((window.1.clone(), decision.moved_rows));
                    }
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
                    holes,
                    moved,
                    unfinished,
                }
            }
        };
        state.pending = Some(pending.clone());
        save_state(self.journal, self.source, state)?;
        self.point("pending")?;
        // A window holding records takes its new facet first, so new rows never sit
        // under its old one. A new window takes it after its records.
        if let Some(facet) = facet {
            self.write_facets(&day, &pending.windows, facet)?;
        }
        // Each window as each pass has it: a window gaining a first row's mark is
        // written in the first pass with its other rows, if it has any.
        let mut passes: [Vec<(WindowId, Decision)>; 2] = [Vec::new(), Vec::new()];
        for (window, decision, marks_first) in decided {
            if marks_first {
                if let Some(before) = early.remove(&window) {
                    passes[0].push((window.clone(), before));
                }
                passes[1].push((window, decision));
            } else {
                passes[0].push((window, decision));
            }
        }
        let mut written = 0;
        for (pass, writes) in passes.into_iter().enumerate() {
            if pass == 1 && !writes.is_empty() {
                self.point("marking")?;
            }
            for (window, decision) in writes {
                let Some(contents) = decision.contents else {
                    continue;
                };
                self.write_window(&window, (contents, decision.units))?;
                written += 1;
                if written == 1 {
                    self.point("written")?;
                }
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
            sync_day_folders(self.journal, self.source, &day)?;
        }
        self.append_listing(state, &pending)?;
        state.totals.add(&pending.counts);
        let records = activity_totals(self.journal, self.source, &day, self.now());
        if !records.activities_refused.is_empty() {
            state
                .holes
                .extend(record_holes(self.journal, self.source, &day));
        }
        state.totals.add(&records);
        state.holes.extend(pending.holes.iter().cloned());
        state.unfinished.extend(pending.unfinished.iter().cloned());
        state.failed.extend(failed);
        state.done_through = Some(day.clone());
        state.pending = None;
        save_state(self.journal, self.source, state)?;
        take_back(self.journal, self.source, &holes_in(state))?;
        self.write_progress(state);
        self.stop_if_stuck(&day)
    }

    /// Write one window's rows, as the ordinary save writes a file.
    fn write_window(
        &self,
        window: &WindowId,
        (contents, units): (String, u64),
    ) -> Result<(), Stop> {
        let rendered = RenderedImport {
            skipped: 0,
            source: self.source.registry,
            entries: units,
            files: vec![SegmentFile {
                day: window.0.clone(),
                segment: window.1.clone(),
                name: BODY_RECORDS_FILE.to_owned(),
                units,
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
        Ok(())
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
        let mut entry = serde_json::json!({
            "day": pending.day,
            "windows": pending.listing,
        });
        // Rows that became moved, apart from the rows a window gained or changed.
        if !pending.moved.is_empty() {
            entry["moved"] = serde_json::json!(pending.moved);
        }
        let mut line = serde_json::to_string(&entry).map_err(|error| error.to_string())?;
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
            save_state(self.journal, self.source, state)?;
        }
        if let Some(note) = reconcile_stream(self.journal, self.source)? {
            state.notes.push(note);
        }
        for day in stream_days(self.journal, self.source)? {
            if state.done_through.as_ref().is_some_and(|done| day <= *done) {
                continue;
            }
            let windows = stored_windows_of_day(self.journal, self.source, &day)?;
            let failed = self.publish_day(&windows)?;
            state.failed.extend(failed);
            state.done_through = Some(day.clone());
            save_state(self.journal, self.source, state)?;
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
fn sync_day_folders(journal: &Path, source: &BodySource, day: &str) -> Result<(), String> {
    let chronicle = journal.join("chronicle");
    let day_dir = chronicle.join(day);
    if !day_dir.join(source.stream).is_dir() {
        return Ok(());
    }
    sync_folder(&day_dir.join(source.stream))?;
    sync_folder(&day_dir)?;
    sync_folder(&chronicle)
}

/// Move the stream's record up to its markers' tail, as the streams command does,
/// and say so if two windows already claim one place in the stream.
fn reconcile_stream(journal: &Path, source: &BodySource) -> Result<Option<String>, String> {
    let mut tail: Option<(u64, String, String)> = None;
    let mut seen: BTreeMap<u64, u32> = BTreeMap::new();
    for day in stream_days(journal, source)? {
        for window in stored_windows_of_day(journal, source, &day)? {
            let Ok(bytes) = fs::read(window_dir(journal, source, &window).join("stream.json"))
            else {
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
            source.stream,
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
fn activity_totals(journal: &Path, source: &BodySource, day: &str, now: DateTime<Utc>) -> Totals {
    let counts = crate::body_activity::write_records_at(journal, source, day, now);
    Totals {
        activities: counts.written,
        activities_no_facet: counts.no_facet,
        activities_refreshed: counts.refreshed,
        activities_refused: counts.refused.into_iter().map(|(facet, _)| facet).collect(),
        ..Totals::default()
    }
}

/// The holes of a day whose activity records couldn't all be written: every
/// experience its stream holds. Only Oura's rows give any.
fn record_holes(journal: &Path, source: &BodySource, day: &str) -> BTreeSet<Hole> {
    let mut holes = BTreeSet::new();
    let Ok(windows) = stored_windows_of_day(journal, source, day) else {
        return holes;
    };
    for window in windows {
        let Ok(text) =
            fs::read_to_string(window_dir(journal, source, &window).join(BODY_RECORDS_FILE))
        else {
            continue;
        };
        let Ok(rows) = decode_rows(&text) else {
            continue;
        };
        let experiences = rows
            .iter()
            .filter(|row: &&BodyRow| row.kind == RowKind::Experience);
        holes.extend(holes_of(experiences, HoleReason::RecordRefused));
    }
    holes
}

/// Take back the coverage a run's holes touch, as soon as the state holds them.
/// Only Oura's rows give holes, and doing it again changes nothing.
fn take_back(journal: &Path, source: &BodySource, holes: &BTreeSet<Hole>) -> Result<(), String> {
    if source.source != OURA.source || holes.is_empty() {
        return Ok(());
    }
    update_coverage(journal, |coverage| coverage.take_back(holes))
}

/// The summary's list of holes.
fn holes_section(holes: &BTreeSet<Hole>) -> String {
    if holes.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "## Not written in full\n\nOura answered for these, and the journal couldn't write all of it.\n\n",
    );
    for hole in holes {
        text.push_str(&format!(
            "- {}, {}: {}\n",
            hole.endpoint,
            hole_place(&hole.endpoint, &hole.at),
            hole.reason.words()
        ));
    }
    text.push('\n');
    text
}

/// Clear the `converging` hole of every document this run wrote everywhere it was
/// and goes, or dropped, once every converging document sharing that hole is done.
/// A hole carried from a run this one took over isn't its scan's, so it stays.
fn settle_converging(state: &mut RunState) {
    let open: BTreeSet<Hole> = state
        .findings
        .converging
        .iter()
        .filter(|(id, _)| state.unfinished.contains(*id))
        .flat_map(|(_, converging)| converging.holes.iter().cloned())
        .collect();
    state
        .findings
        .holes
        .retain(|hole| hole.reason != HoleReason::Converging || open.contains(hole));
}

/// Converging documents the owner's deletes dropped.
fn dropped_count(state: &RunState) -> u64 {
    state.drops.values().filter(|dropped| **dropped).count() as u64
}

/// Every hole a state holds, its scan's included.
fn holes_in(state: &RunState) -> BTreeSet<Hole> {
    let mut holes = state.holes.clone();
    holes.extend(state.findings.holes.iter().cloned());
    if let Some(pending) = &state.pending {
        holes.extend(pending.holes.iter().cloned());
    }
    holes
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
        entries_stayed_deleted: state.findings.stayed_deleted + dropped_count(state),
        held_back: state.totals.held_back,
        unreadable: state.totals.unreadable,
        kept_facet: state.totals.kept_facet,
        refiled: state.totals.refiled,
        activities: state.totals.activities,
        activities_no_facet: state.totals.activities_no_facet,
        activities_refreshed: state.totals.activities_refreshed,
        activities_refused: state.totals.activities_refused.iter().cloned().collect(),
        deferred: state.findings.deferred,
        overlapping_nights: state.findings.overlapping_nights,
        unsure: state
            .findings
            .unsure
            .iter()
            .filter_map(|(key, count)| {
                let (endpoint, days) =
                    key.split_once(solstone_core_import_sources::oura_run::UNSURE_SEPARATOR)?;
                let days = days.split(',').map(str::to_owned).collect();
                Some((endpoint.to_owned(), days, *count))
            })
            .collect(),
        moved: state
            .findings
            .converging
            .iter()
            .filter(|(id, converging)| {
                converging.revised
                    && !state.drops.get(*id).copied().unwrap_or(false)
                    && !state.unfinished.contains(*id)
            })
            .count() as u64,
        held: state.findings.held.clone(),
        not_saved: state
            .failed
            .iter()
            .map(|failed| match &failed.key {
                Some(key) => format!("{} {key}: {}", failed.day, failed.stage),
                None => format!("{}: {}", failed.day, failed.stage),
            })
            .collect(),
        notes: state
            .findings
            .notes
            .iter()
            .chain(&state.notes)
            .cloned()
            .collect(),
    }
}

/// Close an earlier import whose run another export replaced, as unconfirmed.
fn close_superseded(journal: &Path, source: &BodySource, state: &RunState, zone: chrono_tz::Tz) {
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
    let summary = summary_text(source, &summary_of(state), state.facet.as_deref(), zone);
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
    let run = RunRequest {
        input: Input::Apple(request.source),
        timestamp: request.timestamp,
        facet: request.facet,
        spill_bytes: request.spill_bytes,
        stop_at: request.stop_at,
        clock: None,
    };
    run_segments(journal, &APPLE, &run, &|| {
        let source_hash = solstone_core_import::hash_source(request.source)
            .map_err(|error| error.to_string())?
            .into_inner();
        let export_hash = match solstone_core_body_ingest::apple_folder_export(request.source)
            .map_err(|error| refusal_text(&error.to_string()))?
        {
            Some(export) => solstone_core_import::hash_source(&export)
                .map_err(|error| error.to_string())?
                .into_inner(),
            None => String::new(),
        };
        Ok((source_hash, export_hash))
    })
    .map(|outcome| AppleSegmentsOutcome {
        import_id: outcome.import_id,
        summary: outcome.summary,
        published: outcome.published,
        finished: outcome.finished,
    })
    .map_err(RunError::text)
}

/// Publish a folder of staged Oura pages as body windows, on the writer Apple's
/// exports use. Runs under Oura's own lock. The same pages carry on a run that
/// stopped, under the import it began as.
pub fn run_oura_segments(
    journal: &Path,
    request: &OuraSegmentsRequest<'_>,
) -> Result<OuraSegmentsOutcome, String> {
    let run = RunRequest {
        input: Input::Oura(request.pages),
        timestamp: request.timestamp,
        facet: request.facet,
        spill_bytes: request.spill_bytes,
        stop_at: request.stop_at,
        clock: None,
    };
    // The pages' own hashes say what they hold, so nothing else needs checking.
    run_segments(journal, &OURA, &run, &|| {
        Ok((pages_identity(request.pages)?, String::new()))
    })
    .map(|outcome| OuraSegmentsOutcome {
        import_id: outcome.import_id,
        summary: outcome.summary,
        published: outcome.published,
        finished: outcome.finished,
        holes: outcome.holes,
    })
    .map_err(RunError::text)
}

/// Ask Oura for a range of days, with the journal's own tokens or through any
/// transport that answers as Oura does, and publish what it answers as body
/// windows, on the writer Apple's exports use. Runs under Oura's own lock, and
/// takes the old sync's lock only around a token refresh. A run that stopped
/// carries on when asked again with the same range, or with none.
pub fn import_oura(
    journal: &Path,
    request: OuraImportRequest<'_>,
) -> Result<OuraImportOutcome, OuraImportError> {
    let fetch = FetchInput {
        range: request.range,
        transport: std::cell::RefCell::new(request.transport),
        auth: request.auth,
        old_lock_timeout: request.old_lock_timeout,
    };
    let run = RunRequest {
        input: Input::Fetch(&fetch),
        timestamp: request.timestamp,
        facet: request.facet,
        spill_bytes: request.spill_bytes,
        stop_at: request.stop_at,
        clock: request.clock,
    };
    // A fetched run is known by what it asked; its pages' hash is set once fetched.
    let outcome = run_segments(journal, &OURA, &run, &|| Ok((String::new(), String::new())))
        .map_err(|error| match error {
            RunError::Stop(stop) => OuraImportError::Stop(stop),
            RunError::Failed(error) => OuraImportError::Failed(error),
        })?;
    let coverage = read_coverage(journal).map_err(OuraImportError::Failed)?;
    Ok(OuraImportOutcome {
        import_id: outcome.import_id,
        summary: outcome.summary,
        published: outcome.published,
        finished: outcome.finished,
        results: outcome.results,
        holes: outcome.holes,
        coverage,
    })
}

/// One run of any source, under its lock. `identity` says what the input is,
/// so the same input carries on an earlier run.
fn run_segments(
    journal: &Path,
    source: &BodySource,
    request: &RunRequest<'_>,
    identity: &dyn Fn() -> Result<(String, String), String>,
) -> Result<RunOutcome, RunError> {
    run_segments_text(journal, source, request, identity).map_err(|error| match error {
        Ok(stop) => RunError::Stop(stop),
        Err(text) => RunError::Failed(text),
    })
}

fn run_segments_text(
    journal: &Path,
    source: &BodySource,
    request: &RunRequest<'_>,
    identity: &dyn Fn() -> Result<(String, String), String>,
) -> Result<RunOutcome, Result<OuraStop, String>> {
    if let Some(facet) = request.facet {
        crate::cli_argv::require_enabled_facet(facet, journal).map_err(Err)?;
    }
    fs::create_dir_all(journal.join("imports")).map_err(|error| Err(error.to_string()))?;
    let _lock = hold_lock(
        journal.join(source.lock_rel),
        LockOptions {
            timeout: LOCK_TIMEOUT,
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| Err(source.lock_unavailable.to_owned()))?;
    let zone = read_or_create_zone(journal, source).map_err(Err)?;
    // Before anything touches an earlier run: an export that can't be read this
    // moment says nothing about whether that run should carry on.
    let (source_hash, export_hash) = identity().map_err(Err)?;

    // The run state is read before any binding, so the same export resumes under
    // the import it began as, whatever timestamp this request carries.
    let previous = read_state(journal, source);
    let resumable = match &previous {
        Read::Trusted(state)
            if state.facet.as_deref() == request.facet
                && matches!(
                    state.phase,
                    Phase::Fetch | Phase::Scan | Phase::Place | Phase::Write
                )
                && match request.input {
                    // A fetch carries on the range it saved.
                    Input::Fetch(fetch) => state
                        .fetch
                        .as_ref()
                        .is_some_and(|saved| saved.resumed_by(fetch.range)),
                    Input::Apple(_) | Input::Oura(_) => {
                        state.fetch.is_none()
                            && state.source_hash == source_hash
                            && state.export_hash == export_hash
                    }
                } =>
        {
            Some(state.import_id.clone())
        }
        _ => None,
    };
    let import_id = match resumable.clone() {
        Some(id) => id,
        None => {
            let requested = solstone_core_import::validate_timestamp(request.timestamp)
                .map_err(|e| Err(e.to_string()))?;
            solstone_core_import::bind_import_record_where(
                journal,
                &requested,
                request.input.path(),
                &|existing| !holds_a_run(journal, existing),
            )
            .map_err(|error| Err(error.to_string()))?
            .import_id
            .as_str()
            .to_owned()
        }
    };
    if solstone_core_import::refuse_if_live_running(journal, &import_id, source.stream).is_some() {
        return Err(Err("this import is already running".to_owned()));
    }
    let generation = solstone_core_import::admit_running_attempt(
        journal,
        &import_id,
        now_ms(),
        Some(source.registry.name()),
    )
    .map_err(|error| Err(error.to_string()))?
    .generation;
    let run = Run {
        journal,
        source,
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
            source.registry.name(),
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
            Err(Err(format!("stopped at {point}")))
        }
        Err(Stop::Superseded) => Err(Err("this import was superseded".to_owned())),
        Err(Stop::Stopped) => {
            let _ = finish(ImportTerminalInput::Failed(&[]), &[]);
            Err(Err("import stopped".to_owned()))
        }
        Err(Stop::Failed(error)) => {
            let _ = finish(ImportTerminalInput::Failed(&[]), &[]);
            Err(Err(refusal_text(&error)))
        }
        Err(Stop::Oura(stop)) => {
            let _ = finish(ImportTerminalInput::Failed(&[]), &[]);
            Err(Ok(stop))
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
    run: &Run<'_, '_>,
    previous: Read,
    resume: bool,
    (source_hash, export_hash): (&str, &str),
    zone: chrono_tz::Tz,
) -> Result<RunOutcome, Stop> {
    let journal = run.journal;
    let source = run.source;
    let request = run.request;
    let dir = run_dir(journal, source);
    let run_dir_rel = source.run_dir_rel;
    let fresh = |failed: BTreeSet<Failed>, notes: Vec<String>, holes: BTreeSet<Hole>| RunState {
        schema: STATE_SCHEMA,
        phase: Phase::Scan,
        source_hash: source_hash.to_owned(),
        export_hash: export_hash.to_owned(),
        import_id: run.import_id.clone(),
        facet: request.facet.map(str::to_owned),
        failed,
        holes,
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
            fresh(carried, std::mem::take(&mut old.notes), holes_in(&old))
        }
        Read::Trusted(mut old) => {
            // What the old run couldn't write whole stays a hole: this run won't
            // redo its days.
            let mut holes = holes_in(&old);
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
                sync_day_folders(journal, source, &pending.day)?;
                // That day's rows name the old import, so its listing has to, whichever
                // of the windows it had got to. A window it hadn't is listed too.
                let import = journal.join("imports").join(&old.import_id);
                if import.is_dir() {
                    run.append_listing(&mut old, &pending)?;
                    old.totals.add(&pending.counts);
                }
                // A record its run didn't get to; the totals are this run's.
                if !activity_totals(journal, source, &pending.day, run.now())
                    .activities_refused
                    .is_empty()
                {
                    holes.extend(record_holes(journal, source, &pending.day));
                }
            }
            run.retry_failed(&mut old.failed)?;
            if old.phase != Phase::Retry && old.import_id != run.import_id {
                close_superseded(journal, source, &old, zone);
            }
            fresh(std::mem::take(&mut old.failed), Vec::new(), holes)
        }
        Read::Untrusted {
            import_id: old,
            holes,
        } => {
            if let Some(old) = old.filter(|old| *old != run.import_id) {
                close_attempt(journal, &old);
            }
            // A state that still parses carries its holes. One that doesn't can't
            // say what its run would have written whole, so every Oura claim goes,
            // which only ever claims too little.
            let holes = match holes {
                Some(holes) => holes,
                None => {
                    if source.source == OURA.source {
                        update_coverage(journal, |coverage| {
                            coverage.take_back_all();
                            Ok(())
                        })?;
                    }
                    BTreeSet::new()
                }
            };
            let mut repairing = fresh(BTreeSet::new(), Vec::new(), BTreeSet::new());
            run.repair(&mut repairing)?;
            fresh(
                std::mem::take(&mut repairing.failed),
                repairing.notes,
                holes,
            )
        }
        Read::Absent => fresh(BTreeSet::new(), Vec::new(), BTreeSet::new()),
    };
    // A new fetched run works out its range now, after anything above took claims
    // back, and starts from no staged pages.
    if let Input::Fetch(input) = request.input
        && !resume
    {
        let today = run.now().with_timezone(&zone).date_naive();
        state.fetch = Some(plan(input.range, &read_coverage(journal)?, today, zone)?);
        state.phase = Phase::Fetch;
        let _ = remove_dir_all(journal, &format!("{run_dir_rel}/pages"));
    }
    if state.phase == Phase::Fetch {
        save_state(journal, source, &state)?;
        let Input::Fetch(input) = request.input else {
            return Err(Stop::Failed(
                "a fetch can only carry on from Oura itself".to_owned(),
            ));
        };
        let mut fetching = state.fetch.take().unwrap_or_default();
        let point = |name: &str| run.point(name);
        let ended = fetch(
            &FetchRun {
                journal,
                pages: dir.join("pages"),
                zone,
                point: &point,
            },
            &mut fetching,
            input,
            &mut |fetching| {
                state.fetch = Some(fetching.clone());
                save_state(journal, source, &state)
            },
        );
        state.fetch = Some(fetching);
        ended?;
        state.source_hash = pages_identity(&dir.join("pages"))?;
        state.phase = Phase::Scan;
        save_state(journal, source, &state)?;
        run.point("fetched")?;
    }
    if state.phase == Phase::Scan {
        let _ = remove_dir_all(journal, &format!("{run_dir_rel}/scan"));
        let _ = remove_dir_all(journal, &format!("{run_dir_rel}/placed"));
        save_state(journal, source, &state)?;
        run.retry_failed(&mut state.failed)?;
        let progress = run.import_dir().join(PROGRESS_FILE);
        let mut progress = |rows| {
            let line =
                serde_json::json!({"phase": Phase::Scan, "rows_scanned": rows, "at_ms": now_ms()});
            let _ = atomic_replace(&progress, line.to_string().as_bytes(), PRIVATE);
        };
        match request.input {
            Input::Apple(export) => {
                state.scan = scan(
                    export,
                    &dir.join("scan"),
                    request.spill_bytes,
                    &mut progress,
                )?;
            }
            Input::Oura(_) | Input::Fetch(_) => {
                let pages = match request.input {
                    Input::Oura(pages) => pages.to_path_buf(),
                    _ => dir.join("pages"),
                };
                (state.scan, state.findings) = scan_pages(
                    journal,
                    source,
                    zone,
                    &pages,
                    &dir.join("scan"),
                    request.spill_bytes,
                    &mut progress,
                )?;
            }
        }
        state.phase = Phase::Place;
        save_state(journal, source, &state)?;
        take_back(journal, source, &holes_in(&state))?;
        run.write_progress(&state);
        run.point("scanned")?;
    }
    if state.phase == Phase::Place {
        let _ = remove_dir_all(journal, &format!("{run_dir_rel}/placed"));
        state.place = place(
            journal,
            source,
            zone,
            &dir.join("scan"),
            &dir.join("placed"),
            request.spill_bytes,
            &mut |_| {},
        )?;
        state.phase = Phase::Write;
        save_state(journal, source, &state)?;
        let _ = remove_dir_all(journal, &format!("{run_dir_rel}/scan"));
        run.write_progress(&state);
        run.point("placed")?;
    }
    let refused = state.place.refused_nights.clone();
    let mut days = Days::open(
        journal,
        source,
        &dir.join("placed"),
        &refused,
        state.done_through.clone(),
    )?;
    while let Some((day, windows)) = days.next_day()? {
        let (windows, held_back, unreadable) =
            settle_readings_noting(journal, source, &day, zone, windows)?;
        let holes = holes_of(&unreadable, HoleReason::ReadingHeldBack);
        run.write_day(&mut state, day, windows, (held_back, holes))?;
    }
    // Close the placed runs before anything removes them.
    drop(days);
    run.retry_failed(&mut state.failed)?;
    run.point("written-all")?;

    // Coverage: what this run fetched whole is added now that its last day is
    // done, whether or not every window published, and before the run folder or
    // its state goes. Its holes stay out of it. A move it finished is no hole.
    settle_converging(&mut state);
    if source.source == OURA.source {
        let holes = holes_in(&state);
        update_coverage(journal, |coverage| match &state.fetch {
            Some(fetched) => coverage.add(&fetched.claims(), &holes),
            None => coverage.take_back(&holes),
        })?;
        run.point("covered")?;
    }

    // Finish: the summary, the manifest, and one terminal attempt that lists
    // only the windows still failing, so the import reads as partly failed
    // until they're saved, and any endpoint Oura refused or that failed.
    let mut summary = summary_text(source, &summary_of(&state), request.facet, zone);
    if let Some(fetched) = &state.fetch {
        summary.push_str(&fetched.summary_section(zone));
    }
    if source.source == OURA.source {
        summary.push_str(&holes_section(&holes_in(&state)));
    }
    let import_dir = run.import_dir();
    let summary_path = import_dir.join("summary.md");
    atomic_replace(&summary_path, summary.as_bytes(), PRIVATE)
        .map_err(|error| error.to_string())?;
    let days: Vec<String> = state.totals.days.iter().cloned().collect();
    solstone_core_import::write_manifest(&solstone_core_import::ManifestWriteRequest {
        journal_root: journal,
        import_id: &run.import_id,
        source_type: source.registry.name(),
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
            None => stored_windows_of_day(journal, source, &failed.day)?
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
    let not_read = state
        .fetch
        .as_ref()
        .map(FetchState::not_read)
        .unwrap_or_default();
    let terminal = if not_read.is_empty() {
        ImportTerminalInput::Success(&failing)
    } else {
        ImportTerminalInput::Partial(&failing, &not_read)
    };
    match finish_import_attempt_indexing(
        journal,
        &run.import_id,
        run.generation,
        source.registry.name(),
        terminal,
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
        remove_dir_all(journal, run_dir_rel).map_err(|error| error.to_string())?;
    } else {
        let _ = remove_dir_all(journal, &format!("{run_dir_rel}/placed"));
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
        save_state(journal, source, &retry)?;
        run.point("retry-saved")?;
    }
    run.write_progress(&state);
    Ok(RunOutcome {
        import_id: run.import_id.clone(),
        summary,
        published: state.totals.published,
        finished,
        holes: holes_in(&state).into_iter().collect(),
        results: state
            .fetch
            .as_ref()
            .map(|fetched| fetched.reports(zone))
            .unwrap_or_default(),
    })
}

#[cfg(test)]
#[path = "body_segments_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "body_segments_oura_tests.rs"]
mod oura_tests;
