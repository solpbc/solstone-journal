// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::Duration;
#[cfg(not(test))]
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::whole_journal_restore::{TargetUseRefused, begin_target_use_bound};
use solstone_core_journal_io::{
    HealthMarkerKind, JournalRoot, ObjectIdentity, day_dirs, day_path, health_marker_path,
};
use solstone_core_system::daily_coverage::local_now;
use solstone_core_system::process::DailyLogWriter;
use solstone_core_system_health::{FilesystemSegmentSource, scan_day};

use crate::Clock;
use crate::counts_flight::RequestRole;

/// Inline cap so a single live-day fold can finish on the request.
/// Release synthetic benchmark timings (650 days, 300 segment dirs each):
/// - Cold fill: 1881ms
/// - Warm read 1: 3ms (< 100ms)
/// - Warm read 2: 2ms
/// - Live-day refold cost: 4ms
#[cfg(not(test))]
pub const INLINE_FOLD_BUDGET: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FileStat {
    pub(crate) len: u64,
    pub(crate) mtime: (i64, u32),
    pub(crate) ctime: (i64, u32),
    pub(crate) file_id: u64,
}

#[cfg(unix)]
fn extract_file_stat(meta: &fs::Metadata) -> FileStat {
    use std::os::unix::fs::MetadataExt;
    FileStat {
        len: meta.len(),
        mtime: (meta.mtime(), meta.mtime_nsec() as u32),
        ctime: (meta.ctime(), meta.ctime_nsec() as u32),
        file_id: meta.ino(),
    }
}

#[cfg(windows)]
fn extract_file_stat(meta: &fs::Metadata) -> FileStat {
    use std::os::windows::fs::MetadataExt;
    FileStat {
        len: meta.len(),
        mtime: (meta.last_write_time() as i64, 0),
        ctime: (meta.creation_time() as i64, 0),
        // The file index is not on stable Rust; `read_marker_identity` fills
        // this with the marker's generation instead.
        file_id: 0,
    }
}

#[cfg(not(any(unix, windows)))]
fn extract_file_stat(meta: &fs::Metadata) -> FileStat {
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos()))
        .unwrap_or((0, 0));
    let ctime = meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos()))
        .unwrap_or((0, 0));
    FileStat {
        len: meta.len(),
        mtime,
        ctime,
        file_id: 0,
    }
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn segment_key_os(name: &OsStr) -> Option<&str> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        segment_key_from_bytes(name.as_bytes())
    }
    #[cfg(not(unix))]
    {
        segment_key_from_bytes(name.to_str()?.as_bytes())
    }
}

fn segment_key_from_bytes(bytes: &[u8]) -> Option<&str> {
    let mut index = 0;
    while index + 8 <= bytes.len() {
        let word_before = index == 0 || !is_word_byte(bytes[index - 1]);
        if !word_before || !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        if !bytes[index..index + 6]
            .iter()
            .all(|byte| byte.is_ascii_digit())
            || bytes[index + 6] != b'_'
        {
            index += 1;
            continue;
        }
        let mut end = index + 7;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > index + 7
            && (end == bytes.len() || bytes[end] == b'_' || !is_word_byte(bytes[end]))
        {
            return std::str::from_utf8(&bytes[index..end]).ok();
        }
        index = end;
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkerStat {
    Absent,
    StatError(io::ErrorKind),
    Stat(FileStat),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MarkerIdentity {
    Absent,
    StatError(io::ErrorKind),
    Present { stat: FileStat, malformed: bool },
}

impl MarkerIdentity {
    pub(crate) fn stat_only(&self) -> MarkerStat {
        match self {
            Self::Absent => MarkerStat::Absent,
            Self::StatError(kind) => MarkerStat::StatError(*kind),
            Self::Present { stat, .. } => MarkerStat::Stat(*stat),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_malformed(&self) -> bool {
        matches!(
            self,
            Self::Present {
                malformed: true,
                ..
            }
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FileDigestEntry {
    DayEntry(String),
    SegmentDir {
        stream: String,
        segment_dir: String,
    },
    Present {
        stream: String,
        segment_dir: String,
        file_name: String,
        stat: FileStat,
    },
    StatError {
        path: PathBuf,
        kind: io::ErrorKind,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TreeDigest {
    pub(crate) entries: BTreeSet<FileDigestEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DayIdentity {
    StatOnly(MarkerIdentity),
    LiveTree {
        marker: MarkerIdentity,
        tree: TreeDigest,
    },
}

impl DayIdentity {
    pub(crate) fn marker(&self) -> &MarkerIdentity {
        match self {
            Self::StatOnly(m) | Self::LiveTree { marker: m, .. } => m,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DayFoldState {
    Computed(u64),
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Backoff {
    pub(crate) consecutive_failures: u32,
    pub(crate) next_retry: Option<DateTime<Utc>>,
}

impl Backoff {
    pub(crate) fn record_failure(&mut self, now: DateTime<Utc>) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let shift = self.consecutive_failures.saturating_sub(1).min(5);
        let delay_secs = (30u64 << shift).min(900);
        self.next_retry = Some(now + chrono::Duration::seconds(delay_secs as i64));
    }

    pub(crate) fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.next_retry = None;
    }

    pub(crate) fn can_retry(&self, now: DateTime<Utc>) -> bool {
        self.next_retry.is_none_or(|retry| now >= retry)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DayRow {
    pub(crate) identity: DayIdentity,
    pub(crate) state: DayFoldState,
    #[allow(dead_code)]
    pub(crate) fold_generation: u64,
    pub(crate) in_failed_episode: bool,
    pub(crate) first_seen_pending: Option<DateTime<Utc>>,
    pub(crate) backoff: Backoff,
}

pub(crate) struct JournalInventory {
    pub(crate) rows: BTreeMap<String, DayRow>,
    pub(crate) last_full_pass_finish: Option<DateTime<Utc>>,
    pub(crate) last_classified_date: Option<chrono::NaiveDate>,
    pub(crate) active_streams: usize,
    /// Indicates whether a full pass (either initial fill or hourly claim) is currently in flight.
    pub(crate) in_full_pass: bool,
    /// Per-day condvar map to guarantee single-flight folding per day, paired with request generation.
    pub(crate) in_flight: HashMap<String, Arc<(Mutex<bool>, Condvar, u64)>>,
    pub(crate) pending_first_seen: HashMap<String, DateTime<Utc>>,
}

impl JournalInventory {
    fn new() -> Self {
        Self {
            rows: BTreeMap::new(),
            last_full_pass_finish: None,
            last_classified_date: None,
            active_streams: 0,
            in_full_pass: false,
            in_flight: HashMap::new(),
            pending_first_seen: HashMap::new(),
        }
    }
}

static INVENTORIES: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<Mutex<JournalInventory>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static GENERATION_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_generation() -> u64 {
    GENERATION_COUNTER.fetch_add(1, Ordering::SeqCst)
}

struct WorkerPassRequest {
    need_pass: bool,
    full_pass: bool,
}

struct WorkerControl {
    canonical_path: PathBuf,
    request: Mutex<WorkerPassRequest>,
    cv: Condvar,
    stop: AtomicBool,
    clock: Mutex<Clock>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

static WORKERS: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<WorkerControl>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get_or_create_inventory(
    root: &Arc<JournalRoot>,
    identity: ObjectIdentity,
) -> Arc<Mutex<JournalInventory>> {
    let canonical = root.canonical_path();
    let mut reg = INVENTORIES.lock().expect("inventories lock");
    let mut workers = WORKERS.lock().expect("workers lock");

    if let Some(existing_worker) = workers.get(&identity)
        && existing_worker.canonical_path != canonical
    {
        existing_worker.stop.store(true, Ordering::SeqCst);
        existing_worker.cv.notify_all();
        workers.remove(&identity);
        reg.remove(&identity);
    }

    let inv_arc = reg
        .entry(identity)
        .or_insert_with(|| Arc::new(Mutex::new(JournalInventory::new())))
        .clone();

    if let std::collections::hash_map::Entry::Vacant(entry) = workers.entry(identity) {
        let worker = Arc::new(WorkerControl {
            canonical_path: canonical.to_path_buf(),
            request: Mutex::new(WorkerPassRequest {
                need_pass: false,
                full_pass: false,
            }),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            clock: Mutex::new(Clock::system()),
            handle: Mutex::new(None),
        });
        entry.insert(worker.clone());

        let root_c = root.clone();
        let inv_arc_c = inv_arc.clone();
        let worker_c = worker.clone();
        let handle = std::thread::spawn(move || {
            run_worker_loop(root_c, inv_arc_c, worker_c);
        });
        *worker.handle.lock().expect("worker handle lock") = Some(handle);
    }

    inv_arc
}

/// The inventory already serving this root, if any. Background work that
/// must never bring a retired root back to life looks it up here instead of
/// creating one.
fn existing_inventory(identity: ObjectIdentity) -> Option<Arc<Mutex<JournalInventory>>> {
    let reg = INVENTORIES.lock().expect("inventories lock");
    let workers = WORKERS.lock().expect("workers lock");
    if !workers.contains_key(&identity) {
        return None;
    }
    reg.get(&identity).cloned()
}

/// Every listed day is settled for this pass.
/// A Failed row is settled at any generation: it stays reported in `failed`
/// and retries on its own backoff, so a pass can stamp while that day waits.
/// A Computed row must have been folded in this pass.
fn listed_days_refolded(inv: &JournalInventory, listed_days: &[String], req_gen: u64) -> bool {
    !listed_days.is_empty()
        && listed_days.iter().all(|day| {
            inv.rows.get(day).is_some_and(|row| match row.state {
                DayFoldState::Failed(_) => true,
                DayFoldState::Computed(_) => row.fold_generation >= req_gen,
            })
        })
}

/// Stat `chronicle/` before listing. NotFound is an empty journal. Any other
/// stat or read error must not look like an empty listing.
fn list_chronicle_days(journal: &Path) -> Result<HashMap<String, PathBuf>, String> {
    let chronicle = journal.join("chronicle");
    match fs::metadata(&chronicle) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(err) => Err(format!("{}: {err}", chronicle.display())),
        Ok(_) => day_dirs(journal).map_err(|err| err.to_string()),
    }
}

fn run_worker_loop(
    root: Arc<JournalRoot>,
    inv_arc: Arc<Mutex<JournalInventory>>,
    worker: Arc<WorkerControl>,
) {
    let identity = root.identity();
    let journal_path = root.canonical_path().to_path_buf();

    loop {
        let is_full_pass = {
            let mut req = worker.request.lock().unwrap();
            while !req.need_pass && !worker.stop.load(Ordering::SeqCst) {
                req = worker.cv.wait(req).unwrap();
            }
            if worker.stop.load(Ordering::SeqCst) {
                break;
            }
            let is_full = req.full_pass;
            req.need_pass = false;
            req.full_pass = false;
            is_full
        };

        #[cfg(test)]
        test_support::wait_hold_background_fill(identity);

        let clock = worker.clock.lock().unwrap().clone();
        let now = clock.now();
        let req_gen = next_generation();

        let listed_days_map = match list_chronicle_days(&journal_path) {
            Ok(m) => m,
            Err(_) => {
                let mut inv = inv_arc.lock().expect("inventory lock");
                inv.in_full_pass = false;
                continue;
            }
        };

        let dropped_days: Vec<String> = {
            let mut inv = inv_arc.lock().expect("inventory lock");
            let stale: Vec<String> = inv
                .rows
                .keys()
                .filter(|d| !listed_days_map.contains_key(*d))
                .cloned()
                .collect();
            for d in &stale {
                inv.rows.remove(d);
                inv.pending_first_seen.remove(d);
            }
            stale
        };
        for d in dropped_days {
            emit_notification(&journal_path, identity, "day", &d);
        }

        let mut listed_days: Vec<String> = listed_days_map.into_keys().collect();
        listed_days.sort_by(|a, b| b.cmp(a));

        let (_today_date, _today_str, _yesterday_str, live_set) =
            compute_live_set(&journal_path, &listed_days, now);

        for day in &listed_days {
            if worker.stop.load(Ordering::SeqCst) {
                break;
            }
            let need_fold = {
                let inv = inv_arc.lock().expect("inventory lock");
                match inv.rows.get(day) {
                    None => true,
                    Some(row) => {
                        if is_full_pass {
                            if matches!(row.state, DayFoldState::Failed(_)) {
                                row.backoff.can_retry(now)
                            } else {
                                row.fold_generation < req_gen
                            }
                        } else {
                            let is_live = live_set.contains(day);
                            let current_id = read_day_identity(
                                &journal_path,
                                day,
                                is_live,
                                Some(row.identity.marker()),
                                identity,
                            );
                            if current_id != row.identity {
                                true
                            } else if matches!(row.state, DayFoldState::Failed(_)) {
                                row.backoff.can_retry(now)
                            } else {
                                false
                            }
                        }
                    }
                }
            };

            if need_fold {
                let is_live = live_set.contains(day);
                let _ = fold_single_day(
                    &root,
                    &journal_path,
                    day,
                    is_live,
                    now,
                    req_gen,
                    &inv_arc,
                    false,
                );
            }
            #[cfg(any(test, feature = "test-hooks"))]
            lifecycle_hooks::run_worker_between_days(identity);
        }

        if is_full_pass {
            let finish_now = clock.now();
            let mut inv = inv_arc.lock().expect("inventory lock");
            if listed_days_refolded(&inv, &listed_days, req_gen) {
                inv.last_full_pass_finish = Some(finish_now);
            }
            inv.in_full_pass = false;
        }

        if worker.stop.load(Ordering::SeqCst) {
            break;
        }

        #[cfg(test)]
        test_support::inc_worker_passes(identity);
    }
}

/// Retire one root's calendar: stop its background worker and every dashboard
/// watcher, wait for those threads to finish, and forget its rows. Nothing it
/// owned can take the journal again afterwards; a later request on the same
/// root starts a fresh calendar.
pub fn drop_inventory(identity: ObjectIdentity) {
    // One retirement per journal at a time. A second one, for example a stop
    // retried after the first caller gave up waiting, waits until the first
    // has joined everything, then retires whatever was opened since.
    let _turn = RetireTurn::take(identity);
    let worker = {
        let mut reg = INVENTORIES.lock().expect("inventories lock");
        reg.remove(&identity);
        let mut workers = WORKERS.lock().expect("workers lock");
        workers.remove(&identity)
    };
    let watcher = WATCHERS.lock().expect("watchers lock").remove(&identity);

    if let Some(w) = &worker {
        w.stop.store(true, Ordering::SeqCst);
        // Take the request lock so a worker between its stop check and its
        // wait cannot miss this wake.
        drop(w.request.lock().expect("worker request lock"));
        w.cv.notify_all();
    }
    if let Some(w) = &watcher {
        w.active.store(false, Ordering::SeqCst);
        w.generation.fetch_add(1, Ordering::SeqCst);
        drop(w.mu.lock().expect("watcher lock"));
        w.cv.notify_all();
    }

    let current = std::thread::current().id();
    if let Some(w) = worker {
        let handle = w.handle.lock().expect("worker handle lock").take();
        if let Some(handle) = handle
            && handle.thread().id() != current
        {
            #[cfg(any(test, feature = "test-hooks"))]
            lifecycle_hooks::note_retire_event(identity, RetireEvent::Joining);
            let _ = handle.join();
        }
    }
    if let Some(w) = watcher {
        let handles = std::mem::take(&mut *w.handles.lock().expect("watcher handles lock"));
        for handle in handles {
            if handle.thread().id() != current {
                let _ = handle.join();
            }
        }
    }
}

struct RetireDone {
    done: Mutex<bool>,
    cv: Condvar,
}

static RETIRING: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<RetireDone>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// This thread's turn to retire one journal's calendar. Dropping it, even on
/// a panic, lets the next retirement of that journal proceed.
struct RetireTurn {
    identity: ObjectIdentity,
    mine: Arc<RetireDone>,
}

impl RetireTurn {
    fn take(identity: ObjectIdentity) -> Self {
        loop {
            let earlier = {
                let mut retiring = RETIRING.lock().expect("retiring lock");
                match retiring.get(&identity) {
                    Some(earlier) => Arc::clone(earlier),
                    None => {
                        let mine = Arc::new(RetireDone {
                            done: Mutex::new(false),
                            cv: Condvar::new(),
                        });
                        retiring.insert(identity, Arc::clone(&mine));
                        return Self { identity, mine };
                    }
                }
            };
            #[cfg(any(test, feature = "test-hooks"))]
            lifecycle_hooks::note_retire_event(identity, RetireEvent::Waiting);
            let mut done = earlier.done.lock().expect("retire done lock");
            while !*done {
                done = earlier.cv.wait(done).expect("retire done lock");
            }
        }
    }
}

impl Drop for RetireTurn {
    fn drop(&mut self) {
        {
            let mut retiring = RETIRING.lock().unwrap_or_else(|p| p.into_inner());
            if retiring
                .get(&self.identity)
                .is_some_and(|current| Arc::ptr_eq(current, &self.mine))
            {
                retiring.remove(&self.identity);
            }
        }
        *self.mine.done.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.mine.cv.notify_all();
    }
}

/// Retire the calendar of the journal an embedding owner retained, by that
/// root's own identity, so a removed or replaced directory at the same path
/// can neither hide the old worker nor be retired in its place.
pub fn retire_calendar_root(root: &JournalRoot) {
    drop_inventory(root.identity());
}

/// Retire the calendar of the journal at `journal`. Called by the journal's
/// stop and restore drains while the target is held, after admitted use has
/// ended, so a background day fold cannot take the journal again once the
/// hold is released.
pub fn retire_calendar(journal: &Path) {
    // A drain that holds only the path opens it here; an owner that kept its
    // root retires by that root's identity with `retire_calendar_root`.
    let Ok(root) = JournalRoot::open(journal) else {
        return;
    };
    drop_inventory(root.identity());
}

pub(crate) fn notify_inventory_worker(identity: ObjectIdentity) {
    let workers = WORKERS.lock().expect("workers lock");
    if let Some(worker) = workers.get(&identity) {
        worker.cv.notify_one();
    }
}

pub fn reset_calendar_while_held(journal: &Path) {
    let Ok(root) = JournalRoot::open(journal) else {
        return;
    };
    let identity = root.identity();
    drop_inventory(identity);
    emit_notification(root.canonical_path(), identity, "system", "calendar");
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn restart_wipe(identity: ObjectIdentity) {
    let reg = INVENTORIES.lock().expect("inventories lock");
    if let Some(inv_arc) = reg.get(&identity) {
        let mut inv = inv_arc.lock().expect("inventory lock");
        inv.rows.clear();
        inv.last_full_pass_finish = None;
        inv.pending_first_seen.clear();
    }
}

struct WatcherControl {
    active: AtomicBool,
    generation: AtomicU64,
    cv: Condvar,
    mu: Mutex<()>,
    clock: Mutex<Clock>,
    handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

static WATCHERS: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<WatcherControl>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn note_dashboard_stream(journal: &Path, connected: bool) {
    let Ok(root) = JournalRoot::open(journal) else {
        return;
    };
    let root = Arc::new(root);
    let identity = root.identity();
    // A stream closing during a stop must not bring the calendar back.
    let inv_arc = if connected {
        get_or_create_inventory(&root, identity)
    } else {
        match existing_inventory(identity) {
            Some(inv_arc) => inv_arc,
            None => return,
        }
    };
    let (transition_to_nonzero, transition_to_zero) = {
        let mut inv = inv_arc.lock().expect("inventory lock");
        let prev = inv.active_streams;
        if connected {
            inv.active_streams = inv.active_streams.saturating_add(1);
            (prev == 0 && inv.active_streams > 0, false)
        } else {
            inv.active_streams = inv.active_streams.saturating_sub(1);
            (false, prev > 0 && inv.active_streams == 0)
        }
    };

    if transition_to_nonzero {
        let mut watchers = WATCHERS.lock().expect("watchers lock");
        let ctrl = watchers.entry(identity).or_insert_with(|| {
            Arc::new(WatcherControl {
                active: AtomicBool::new(true),
                generation: AtomicU64::new(0),
                cv: Condvar::new(),
                mu: Mutex::new(()),
                clock: Mutex::new(Clock::system()),
                handles: Mutex::new(Vec::new()),
            })
        });
        ctrl.active.store(true, Ordering::SeqCst);
        let my_generation = ctrl.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let ctrl_c = ctrl.clone();
        let root_c = root.clone();
        let mut handles = ctrl.handles.lock().expect("watcher handles lock");
        handles.retain(|h| !h.is_finished());
        handles.push(std::thread::spawn(move || {
            while ctrl_c.active.load(Ordering::SeqCst)
                && ctrl_c.generation.load(Ordering::SeqCst) == my_generation
            {
                let guard = ctrl_c.mu.lock().unwrap();
                // Re-check under the lock a retirement takes before it wakes us.
                if !ctrl_c.active.load(Ordering::SeqCst)
                    || ctrl_c.generation.load(Ordering::SeqCst) != my_generation
                {
                    break;
                }
                let _ = ctrl_c
                    .cv
                    .wait_timeout(guard, Duration::from_secs(10))
                    .unwrap();
                if !ctrl_c.active.load(Ordering::SeqCst)
                    || ctrl_c.generation.load(Ordering::SeqCst) != my_generation
                {
                    break;
                }
                let clock = ctrl_c.clock.lock().unwrap().clone();
                tick_journal(&root_c, &clock);
            }
        }));
    } else if transition_to_zero {
        let watchers = WATCHERS.lock().expect("watchers lock");
        if let Some(ctrl) = watchers.get(&identity) {
            ctrl.active.store(false, Ordering::SeqCst);
            ctrl.generation.fetch_add(1, Ordering::SeqCst);
            ctrl.cv.notify_all();
        }
    }
}

fn tick_journal(root: &Arc<JournalRoot>, clock: &Clock) {
    let now = clock.now();
    let identity = root.identity();
    // A tick serves a live calendar only. A retired one stays retired until a
    // request opens it again.
    if existing_inventory(identity).is_none() {
        return;
    }
    let journal_path = root.canonical_path().to_path_buf();
    let _ = read_inventory_inner_with(
        Some(root.clone()),
        &journal_path,
        identity,
        now,
        clock,
        RequestRole::Index,
        false,
    );
}

fn emit_notification(journal_root: &Path, _identity: ObjectIdentity, kind: &str, key: &str) {
    #[cfg(test)]
    test_support::record_notification(_identity, kind, key);

    // No existence check: on Windows the bus is a named pipe and nothing
    // lives at this path, and an absent bus fails the send at once anyway.
    let sock_path = journal_root.join("health").join("callosum.sock");
    let payload = serde_json::json!({
        "tract": "change",
        "event": "changed",
        "kind": kind,
        "key": key,
    });
    let line = format!("{}\n", payload);
    let sender = solstone_core_callosum::CallosumOneShotSender::new(
        sock_path,
        std::time::Duration::from_millis(500),
    );
    let _ = sender.send_line_confirmed(&line);
}

fn read_marker_identity(
    journal: &Path,
    day: &str,
    stored_marker: Option<&MarkerIdentity>,
    _identity: ObjectIdentity,
) -> MarkerIdentity {
    #[cfg(test)]
    test_support::inc_marker_stats(_identity);

    let path = health_marker_path(journal, day, HealthMarkerKind::Stream);
    match fs::metadata(&path) {
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                MarkerIdentity::Absent
            } else {
                MarkerIdentity::StatError(err.kind())
            }
        }
        Ok(meta) => present_marker_identity(&path, &meta, stored_marker),
    }
}

/// An empty marker is the older writers' shape: the next bump replaces it like any
/// other, so its day needs no tree on every read. This parses the text itself because
/// `read_health_marker` sets a malformed file aside, and a read writes nothing.
fn marker_text_is_malformed(content: &str) -> bool {
    !content.is_empty()
        && serde_json::from_str::<solstone_core_journal_io::HealthMarker>(content).is_err()
}

#[cfg(unix)]
fn present_marker_identity(
    path: &Path,
    meta: &fs::Metadata,
    stored_marker: Option<&MarkerIdentity>,
) -> MarkerIdentity {
    let stat = extract_file_stat(meta);
    if let Some(MarkerIdentity::Present {
        stat: stored_stat,
        malformed,
    }) = stored_marker
        && stored_stat == &stat
    {
        return MarkerIdentity::Present {
            stat,
            malformed: *malformed,
        };
    }
    let malformed =
        fs::read_to_string(path).is_ok_and(|content| marker_text_is_malformed(&content));
    MarkerIdentity::Present { stat, malformed }
}

// Without an inode, two bumps inside one clock tick can leave the marker's
// size and times unchanged, so its generation stands in for the file id.
#[cfg(not(unix))]
fn present_marker_identity(
    path: &Path,
    meta: &fs::Metadata,
    _stored_marker: Option<&MarkerIdentity>,
) -> MarkerIdentity {
    let marker = fs::read_to_string(path)
        .ok()
        .filter(|content| !content.is_empty())
        .map(|content| serde_json::from_str::<solstone_core_journal_io::HealthMarker>(&content));
    let mut stat = extract_file_stat(meta);
    if let Some(Ok(marker)) = &marker {
        stat.file_id = marker.generation;
    }
    let malformed = matches!(marker, Some(Err(_)));
    MarkerIdentity::Present { stat, malformed }
}

fn read_tree_digest(journal: &Path, day: &str, _identity: ObjectIdentity) -> TreeDigest {
    #[cfg(test)]
    test_support::inc_tree_descents(_identity);

    let Ok(day_dir) = day_path(journal, day, false) else {
        return TreeDigest::default();
    };
    if !day_dir.is_dir() {
        return TreeDigest::default();
    }
    let mut entries = BTreeSet::new();
    let read_dir = match fs::read_dir(&day_dir) {
        Ok(rd) => rd,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                entries.insert(FileDigestEntry::StatError {
                    path: day_dir,
                    kind: err.kind(),
                });
            }
            return TreeDigest { entries };
        }
    };

    let mut segment_dirs = Vec::new();
    for entry in read_dir.flatten() {
        let file_name = entry.file_name();
        if file_name == OsStr::new("health") {
            continue;
        }
        let file_name_str = file_name.to_string_lossy().to_string();
        entries.insert(FileDigestEntry::DayEntry(file_name_str.clone()));

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        if let Some(_key) = segment_key_os(&file_name) {
            entries.insert(FileDigestEntry::SegmentDir {
                stream: String::new(),
                segment_dir: file_name_str.clone(),
            });
            segment_dirs.push((String::new(), file_name_str.clone(), path));
            continue;
        }

        let stream_name = file_name_str;
        match fs::read_dir(&path) {
            Ok(stream_entries) => {
                for sub_entry in stream_entries.flatten() {
                    let sub_name = sub_entry.file_name();
                    let sub_path = sub_entry.path();
                    if !sub_path.is_dir() {
                        continue;
                    }
                    if let Some(_key) = segment_key_os(&sub_name) {
                        let sub_name_str = sub_name.to_string_lossy().to_string();
                        entries.insert(FileDigestEntry::SegmentDir {
                            stream: stream_name.clone(),
                            segment_dir: sub_name_str.clone(),
                        });
                        segment_dirs.push((stream_name.clone(), sub_name_str, sub_path));
                    }
                }
            }
            Err(err) => {
                if err.kind() != io::ErrorKind::NotFound {
                    entries.insert(FileDigestEntry::StatError {
                        path,
                        kind: err.kind(),
                    });
                }
            }
        }
    }

    for (stream, segment_dir, seg_path) in segment_dirs {
        let file_entries = match fs::read_dir(&seg_path) {
            Ok(fe) => fe,
            Err(err) => {
                if err.kind() != io::ErrorKind::NotFound {
                    entries.insert(FileDigestEntry::StatError {
                        path: seg_path,
                        kind: err.kind(),
                    });
                }
                continue;
            }
        };

        for f_entry in file_entries.flatten() {
            let f_name = f_entry.file_name();
            if f_name == OsStr::new("talents") {
                continue;
            }
            let f_path = f_entry.path();

            #[cfg(test)]
            test_support::run_before_file_stat(_identity, &f_path);

            match fs::metadata(&f_path) {
                Ok(meta) => {
                    if meta.is_file() {
                        let stat = extract_file_stat(&meta);
                        let file_name = f_name.to_string_lossy().to_string();
                        entries.insert(FileDigestEntry::Present {
                            stream: stream.clone(),
                            segment_dir: segment_dir.clone(),
                            file_name,
                            stat,
                        });
                    }
                }
                Err(err) => {
                    if err.kind() != io::ErrorKind::NotFound {
                        entries.insert(FileDigestEntry::StatError {
                            path: f_path,
                            kind: err.kind(),
                        });
                    }
                }
            }
        }
    }

    TreeDigest { entries }
}

fn read_day_identity(
    journal: &Path,
    day: &str,
    is_live: bool,
    stored_marker: Option<&MarkerIdentity>,
    identity: ObjectIdentity,
) -> DayIdentity {
    let marker = read_marker_identity(journal, day, stored_marker, identity);
    let is_malformed = matches!(
        marker,
        MarkerIdentity::Present {
            malformed: true,
            ..
        }
    );
    if is_live || is_malformed {
        let tree = read_tree_digest(journal, day, identity);
        DayIdentity::LiveTree { marker, tree }
    } else {
        DayIdentity::StatOnly(marker)
    }
}

fn compute_live_set(
    journal: &Path,
    listed_days: &[String],
    now: DateTime<Utc>,
) -> (chrono::NaiveDate, String, String, BTreeSet<String>) {
    let local = local_now(journal, now);
    let today_date = local.date();
    let today_str = today_date.format("%Y%m%d").to_string();
    let yesterday_date = today_date - chrono::Days::new(1);
    let yesterday_str = yesterday_date.format("%Y%m%d").to_string();

    let mut live_set = BTreeSet::new();
    live_set.insert(today_str.clone());
    live_set.insert(yesterday_str.clone());
    for day in listed_days {
        if day > &today_str {
            live_set.insert(day.clone());
        }
    }
    (today_date, today_str, yesterday_str, live_set)
}

fn compute_day_count(
    journal: &Path,
    day: &str,
    now: DateTime<Utc>,
    _identity: ObjectIdentity,
) -> Result<u64, String> {
    #[cfg(test)]
    if let Some(err) = test_support::injected_compute_error(_identity, day) {
        return Err(err);
    }

    let (audio, screen, segments) =
        scan_day(&FilesystemSegmentSource, journal, day, now).map_err(|e| e.to_string())?;
    let mut segments = segments
        .into_iter()
        .map(crate::day::from_native)
        .collect::<Vec<_>>();
    crate::attach::normalize_markdown_only_segments(journal, day, &mut segments);
    let count = audio.len() as u64
        + screen.len() as u64
        + segments
            .iter()
            .filter(|s| s.types.iter().any(|kind| kind == "browser"))
            .count() as u64;
    Ok(count)
}

enum FoldAborted {
    TargetUseRefused,
}

#[allow(clippy::too_many_arguments)]
fn fold_single_day(
    root: &Arc<JournalRoot>,
    journal: &Path,
    day: &str,
    is_live: bool,
    now: DateTime<Utc>,
    req_gen: u64,
    inv_arc: &Arc<Mutex<JournalInventory>>,
    mut retry_earlier_failure: bool,
) -> Result<DayIdentity, FoldAborted> {
    loop {
        let claimed = {
            let mut inv = inv_arc.lock().expect("inventory lock");
            if let Some(entry) = inv.in_flight.get(day) {
                Err(entry.clone())
            } else {
                let entry = Arc::new((Mutex::new(false), Condvar::new(), req_gen));
                inv.in_flight.insert(day.to_string(), entry.clone());
                Ok(entry)
            }
        };

        let entry = match claimed {
            Ok(entry) => {
                return fold_single_day_exec(
                    root, journal, day, is_live, now, req_gen, inv_arc, entry,
                );
            }
            Err(entry) => entry,
        };

        let (lock, cv, starter_req_gen) = &*entry;
        let starter_req_gen = *starter_req_gen;
        {
            #[cfg(test)]
            test_support::inc_waiter_counter(root.identity());
            let mut done = lock.lock().unwrap();
            while !*done {
                done = cv.wait(done).unwrap();
            }
            drop(done);
            #[cfg(test)]
            test_support::dec_waiter_counter(root.identity());
        }

        let stored_marker = {
            let inv = inv_arc.lock().expect("inventory lock");
            inv.rows.get(day).map(|row| row.identity.marker().clone())
        };
        let disk_identity = read_day_identity(
            journal,
            day,
            is_live,
            stored_marker.as_ref(),
            root.identity(),
        );
        let (matches_stored, is_failed) = {
            let inv = inv_arc.lock().expect("inventory lock");
            match inv.rows.get(day) {
                Some(row) => (
                    row.identity == disk_identity,
                    matches!(row.state, DayFoldState::Failed(_)),
                ),
                None => (false, false),
            }
        };

        // One retry, and only for the month route: a fold that started before
        // this request arrived does not count as its retry of a failed day.
        // The retry claims again at the top of this loop.
        if retry_earlier_failure && is_failed && starter_req_gen < req_gen {
            retry_earlier_failure = false;
            continue;
        }
        if matches_stored {
            return Ok(disk_identity);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fold_single_day_exec(
    root: &Arc<JournalRoot>,
    journal: &Path,
    day: &str,
    is_live: bool,
    now: DateTime<Utc>,
    req_gen: u64,
    inv_arc: &Arc<Mutex<JournalInventory>>,
    claimed: Arc<(Mutex<bool>, Condvar, u64)>,
) -> Result<DayIdentity, FoldAborted> {
    struct InFlightGuard {
        day: String,
        entry: Arc<(Mutex<bool>, Condvar, u64)>,
        inv_arc: Arc<Mutex<JournalInventory>>,
    }

    impl Drop for InFlightGuard {
        fn drop(&mut self) {
            let mut inv = self.inv_arc.lock().expect("inventory lock");
            if inv
                .in_flight
                .get(&self.day)
                .is_some_and(|current| Arc::ptr_eq(current, &self.entry))
            {
                inv.in_flight.remove(&self.day);
            }
            let (lock, cv, _) = &*self.entry;
            let mut done = lock.lock().unwrap();
            *done = true;
            cv.notify_all();
        }
    }

    let in_flight_guard = InFlightGuard {
        day: day.to_string(),
        entry: claimed,
        inv_arc: inv_arc.clone(),
    };

    let admission = match begin_target_use_bound(Arc::clone(root)) {
        Ok(guard) => guard,
        Err(TargetUseRefused) => {
            drop(in_flight_guard);
            return Err(FoldAborted::TargetUseRefused);
        }
    };

    let stored_marker = {
        let inv = inv_arc.lock().expect("inventory lock");
        inv.rows.get(day).map(|row| row.identity.marker().clone())
    };

    let identity_pre = read_day_identity(
        journal,
        day,
        is_live,
        stored_marker.as_ref(),
        root.identity(),
    );

    #[cfg(any(test, feature = "test-hooks"))]
    identity_barrier::run(root.identity(), day);

    #[cfg(any(test, feature = "test-hooks"))]
    lifecycle_hooks::note_fold(root.identity());

    #[cfg(test)]
    test_support::inc_compute_starts(root.identity());

    let compute_outcome = compute_day_count(journal, day, now, root.identity());

    #[cfg(test)]
    test_support::run_barrier_after_compute(root.identity(), day);

    let emit_day = {
        let mut inv = inv_arc.lock().expect("inventory lock");
        let prev_row = inv.rows.get(day).cloned();
        let mut emit = false;

        let new_row = match compute_outcome {
            Ok(count) => {
                inv.pending_first_seen.remove(day);
                if !is_live
                    && let Some(prev) = prev_row.as_ref()
                    && let DayFoldState::Computed(old_count) = prev.state
                    && old_count != count
                    && prev.identity.marker().stat_only() == identity_pre.marker().stat_only()
                    && let Ok(mut writer) =
                        DailyLogWriter::new(journal, "calendar", "inventory", Some(day.to_string()))
                {
                    writer.write(&format!("chronicle day {day}: missed producer\n"));
                }

                match &prev_row {
                    None => emit = inv.last_full_pass_finish.is_some(),
                    Some(prev) => match prev.state {
                        DayFoldState::Computed(old) => {
                            if old != count {
                                emit = true;
                            }
                        }
                        DayFoldState::Failed(_) => emit = true,
                    },
                }

                let mut backoff = prev_row.map(|r| r.backoff).unwrap_or_default();
                backoff.record_success();

                DayRow {
                    identity: identity_pre.clone(),
                    state: DayFoldState::Computed(count),
                    fold_generation: req_gen,
                    in_failed_episode: false,
                    first_seen_pending: None,
                    backoff,
                }
            }
            Err(err) => {
                let in_failed_episode = prev_row.as_ref().is_some_and(|row| row.in_failed_episode);
                if !in_failed_episode
                    && let Ok(mut writer) =
                        DailyLogWriter::new(journal, "calendar", "inventory", Some(day.to_string()))
                {
                    writer.write(&format!("chronicle day {day}: count failed\n"));
                }

                match &prev_row {
                    None => emit = inv.last_full_pass_finish.is_some(),
                    Some(prev) => {
                        if matches!(prev.state, DayFoldState::Computed(_)) {
                            emit = true;
                        }
                    }
                }

                let mut backoff = prev_row.as_ref().map(|r| r.backoff).unwrap_or_default();
                backoff.record_failure(now);

                let first_seen = prev_row
                    .as_ref()
                    .and_then(|r| r.first_seen_pending)
                    .or_else(|| inv.pending_first_seen.get(day).copied())
                    .or(Some(now));
                if let Some(fs) = first_seen {
                    inv.pending_first_seen.entry(day.to_string()).or_insert(fs);
                }

                DayRow {
                    identity: identity_pre.clone(),
                    state: DayFoldState::Failed(err),
                    fold_generation: req_gen,
                    in_failed_episode: true,
                    first_seen_pending: first_seen,
                    backoff,
                }
            }
        };

        inv.rows.insert(day.to_string(), new_row);
        emit
    };

    drop(in_flight_guard);
    drop(admission);

    if emit_day {
        emit_notification(journal, root.identity(), "day", day);
    }

    Ok(identity_pre)
}

pub(crate) fn read_inventory_admitted(
    root: Arc<JournalRoot>,
    now: DateTime<Utc>,
    clock: &Clock,
    role: RequestRole,
) -> (Arc<BTreeMap<String, u64>>, Value) {
    let identity = root.identity();
    let journal_path = root.canonical_path().to_path_buf();
    read_inventory_inner(Some(root), &journal_path, identity, now, clock, role)
}

pub(crate) fn read_inventory_unadmitted(
    fallback: &Path,
    now: DateTime<Utc>,
    clock: &Clock,
    role: RequestRole,
) -> (Arc<BTreeMap<String, u64>>, Value) {
    let Ok(root) = JournalRoot::open(fallback) else {
        return (
            Arc::new(BTreeMap::new()),
            json!({"complete": false, "behind": 0, "failed": 0, "oldest": now.to_rfc3339()}),
        );
    };
    let root = Arc::new(root);
    let identity = root.identity();
    let journal_path = root.canonical_path().to_path_buf();
    read_inventory_inner(Some(root), &journal_path, identity, now, clock, role)
}

fn read_inventory_inner(
    root_opt: Option<Arc<JournalRoot>>,
    journal_path: &Path,
    identity: ObjectIdentity,
    now: DateTime<Utc>,
    clock: &Clock,
    role: RequestRole,
) -> (Arc<BTreeMap<String, u64>>, Value) {
    read_inventory_inner_with(root_opt, journal_path, identity, now, clock, role, true)
}

#[allow(clippy::too_many_arguments)]
fn read_inventory_inner_with(
    root_opt: Option<Arc<JournalRoot>>,
    journal_path: &Path,
    identity: ObjectIdentity,
    now: DateTime<Utc>,
    clock: &Clock,
    role: RequestRole,
    create: bool,
) -> (Arc<BTreeMap<String, u64>>, Value) {
    let Some(root) = root_opt else {
        return (
            Arc::new(BTreeMap::new()),
            json!({"complete": false, "behind": 0, "failed": 0, "oldest": now.to_rfc3339()}),
        );
    };

    let req_gen = next_generation();
    let inv_arc = if create {
        get_or_create_inventory(&root, identity)
    } else {
        match existing_inventory(identity) {
            Some(inv_arc) => inv_arc,
            None => {
                return (
                    Arc::new(BTreeMap::new()),
                    json!({"complete": false, "behind": 0, "failed": 0, "oldest": now.to_rfc3339()}),
                );
            }
        }
    };

    // Update worker clock
    {
        let workers = WORKERS.lock().expect("workers lock");
        if let Some(w) = workers.get(&identity) {
            *w.clock.lock().unwrap() = clock.clone();
        }
    }
    {
        let watchers = WATCHERS.lock().expect("watchers lock");
        if let Some(ctrl) = watchers.get(&identity) {
            *ctrl.clock.lock().unwrap() = clock.clone();
        }
    }

    #[cfg(test)]
    test_support::inc_chronicle_lists(identity);

    let listed_days_map = match list_chronicle_days(journal_path) {
        Ok(m) => m,
        Err(_) => {
            let mut inv = inv_arc.lock().expect("inventory lock");
            inv.in_full_pass = false;
            return (
                Arc::new(BTreeMap::new()),
                json!({"complete": false, "behind": 0, "failed": 0, "oldest": now.to_rfc3339()}),
            );
        }
    };

    let dropped_days: Vec<String> = {
        let mut inv = inv_arc.lock().expect("inventory lock");
        let stale: Vec<String> = inv
            .rows
            .keys()
            .filter(|d| !listed_days_map.contains_key(*d))
            .cloned()
            .collect();
        for d in &stale {
            inv.rows.remove(d);
            inv.pending_first_seen.remove(d);
        }
        stale
    };
    for d in dropped_days {
        emit_notification(journal_path, identity, "day", &d);
    }

    let mut listed_days: Vec<String> = listed_days_map.into_keys().collect();
    listed_days.sort();

    let (today_date, _today_str, _yesterday_str, live_set) =
        compute_live_set(journal_path, &listed_days, now);

    // Rollover: fold previous today and yesterday first; store last_classified_date only after both Ok
    let rollover_info = {
        let inv = inv_arc.lock().expect("inventory lock");
        match inv.last_classified_date {
            Some(last_date) if today_date > last_date => {
                let prev_today = last_date.format("%Y%m%d").to_string();
                let prev_yesterday = (last_date - chrono::Days::new(1))
                    .format("%Y%m%d")
                    .to_string();
                Some((prev_today, prev_yesterday))
            }
            Some(_) => None,
            None => None,
        }
    };

    if let Some((prev_today, prev_yesterday)) = rollover_info {
        let mut both_ok = true;
        if listed_days.contains(&prev_today) {
            let res = fold_single_day(
                &root,
                journal_path,
                &prev_today,
                true,
                now,
                req_gen,
                &inv_arc,
                false,
            );
            if res.is_err() {
                both_ok = false;
            }
        }
        if listed_days.contains(&prev_yesterday) {
            let res = fold_single_day(
                &root,
                journal_path,
                &prev_yesterday,
                true,
                now,
                req_gen,
                &inv_arc,
                false,
            );
            if res.is_err() {
                both_ok = false;
            }
        }
        if both_ok {
            let mut inv = inv_arc.lock().expect("inventory lock");
            inv.last_classified_date = Some(today_date);
        }
    } else {
        let mut inv = inv_arc.lock().expect("inventory lock");
        if inv.last_classified_date.is_none() {
            inv.last_classified_date = Some(today_date);
        }
    }

    let run_full_pass = {
        let mut inv = inv_arc.lock().expect("inventory lock");
        if !inv.in_full_pass {
            let should_start = match inv.last_full_pass_finish {
                None => true,
                Some(last_finish) => (now - last_finish).num_seconds() > 3600,
            };
            if should_start {
                inv.in_full_pass = true;
                true
            } else {
                false
            }
        } else {
            false
        }
    };

    // Stat each day ONCE and determine pending days
    let mut day_identities: HashMap<String, DayIdentity> = HashMap::new();
    let mut pending_days = Vec::new();
    {
        let mut inv = inv_arc.lock().expect("inventory lock");
        for day in &listed_days {
            let is_live = live_set.contains(day);
            let (stored_marker, stored_identity, is_failed, can_retry) = match inv.rows.get(day) {
                Some(row) => (
                    Some(row.identity.marker().clone()),
                    Some(row.identity.clone()),
                    matches!(row.state, DayFoldState::Failed(_)),
                    row.backoff.can_retry(now),
                ),
                None => (None, None, false, false),
            };
            let current_id =
                read_day_identity(journal_path, day, is_live, stored_marker.as_ref(), identity);
            day_identities.insert(day.clone(), current_id.clone());

            match stored_identity {
                None => {
                    inv.pending_first_seen.entry(day.clone()).or_insert(now);
                    pending_days.push(day.clone());
                }
                Some(row_id) => {
                    if run_full_pass {
                        pending_days.push(day.clone());
                    } else if current_id != row_id {
                        inv.pending_first_seen.entry(day.clone()).or_insert(now);
                        pending_days.push(day.clone());
                    } else if is_failed {
                        inv.pending_first_seen.entry(day.clone()).or_insert(now);
                        if can_retry {
                            pending_days.push(day.clone());
                        }
                    } else {
                        inv.pending_first_seen.remove(day);
                    }
                }
            }
        }
    }

    // Newest pending first
    pending_days.sort_by(|a, b| b.cmp(a));

    #[cfg(not(test))]
    let start = Instant::now();
    let mut budget_spent = false;

    // Unit tests choose a fold count instead of racing the host's scheduling.
    #[cfg(test)]
    let fold_limit = test_support::inline_fold_limit(identity);
    #[cfg(test)]
    let mut fold_count = 0;

    let mut remaining_pending_days: Vec<String> = Vec::new();

    if role == RequestRole::Index {
        for (i, day) in pending_days.iter().enumerate() {
            #[cfg(test)]
            let inline_budget_spent = fold_limit.is_some_and(|limit| fold_count >= limit);
            #[cfg(not(test))]
            let inline_budget_spent = start.elapsed() >= INLINE_FOLD_BUDGET;

            if inline_budget_spent {
                budget_spent = true;
                remaining_pending_days.extend(pending_days[i..].iter().cloned());
                break;
            }
            let is_live = live_set.contains(day);
            if let Ok(new_id) = fold_single_day(
                &root,
                journal_path,
                day,
                is_live,
                now,
                req_gen,
                &inv_arc,
                false,
            ) {
                day_identities.insert(day.clone(), new_id);
            }
            #[cfg(test)]
            {
                fold_count += 1;
            }
        }
    } else {
        remaining_pending_days = pending_days.clone();
    }

    if !remaining_pending_days.is_empty() {
        let workers = WORKERS.lock().expect("workers lock");
        if let Some(w) = workers.get(&identity) {
            let mut req = w.request.lock().unwrap();
            req.need_pass = true;
            if run_full_pass {
                req.full_pass = true;
            }
            // The month route folds its own days after this read returns.
            // Waking now lets that pass claim the day before the route stores
            // the count it read.
            if role != RequestRole::MonthStats {
                w.cv.notify_one();
            }
        }
    }

    if role == RequestRole::Index && run_full_pass && !budget_spent {
        let mut inv = inv_arc.lock().expect("inventory lock");
        if listed_days_refolded(&inv, &listed_days, req_gen) {
            let finish_now = clock.now();
            inv.last_full_pass_finish = Some(finish_now);
        }
        inv.in_full_pass = false;
    }

    // Build counts snapshot and freshness using stored day_identities (no extra stats)
    let inv = inv_arc.lock().expect("inventory lock");
    let mut counts = BTreeMap::new();
    let mut behind = 0;
    let mut failed = 0;
    let mut oldest: Option<DateTime<Utc>> = None;

    for day in &listed_days {
        let current_id = day_identities.get(day);
        match inv.rows.get(day) {
            None => {
                behind += 1;
                let t = inv.pending_first_seen.get(day).copied().unwrap_or(now);
                oldest = Some(oldest.map_or(t, |o| o.min(t)));
            }
            Some(row) => match &row.state {
                DayFoldState::Computed(count) => {
                    if *count > 0 {
                        counts.insert(day.clone(), *count);
                    }
                    if current_id != Some(&row.identity) {
                        behind += 1;
                        let t = inv
                            .pending_first_seen
                            .get(day)
                            .copied()
                            .or(row.first_seen_pending)
                            .unwrap_or(now);
                        oldest = Some(oldest.map_or(t, |o| o.min(t)));
                    }
                }
                DayFoldState::Failed(_) => {
                    failed += 1;
                    let t = inv
                        .pending_first_seen
                        .get(day)
                        .copied()
                        .or(row.first_seen_pending)
                        .unwrap_or(now);
                    oldest = Some(oldest.map_or(t, |o| o.min(t)));
                }
            },
        }
    }

    let freshness = if behind == 0 && failed == 0 {
        json!({"complete": true})
    } else {
        json!({
            "complete": false,
            "behind": behind,
            "failed": failed,
            "oldest": oldest.unwrap_or(now).to_rfc3339()
        })
    };

    (Arc::new(counts), freshness)
}

pub(crate) fn fold_month_days(
    root_opt: Option<Arc<JournalRoot>>,
    journal_path: &Path,
    month: &str,
    clock: &Clock,
) -> Result<BTreeMap<String, u64>, String> {
    let now = clock.now();
    let root = match root_opt {
        Some(r) => r,
        None => {
            let r = JournalRoot::open(journal_path).map_err(|e| e.to_string())?;
            Arc::new(r)
        }
    };
    let identity = root.identity();
    let inv_arc = get_or_create_inventory(&root, identity);
    let req_gen = next_generation();

    let listed_days_map = list_chronicle_days(journal_path)?;

    let dropped_days: Vec<String> = {
        let mut inv = inv_arc.lock().expect("inventory lock");
        let stale: Vec<String> = inv
            .rows
            .keys()
            .filter(|d| !listed_days_map.contains_key(*d))
            .cloned()
            .collect();
        for d in &stale {
            inv.rows.remove(d);
            inv.pending_first_seen.remove(d);
        }
        stale
    };
    for d in dropped_days {
        emit_notification(journal_path, identity, "day", &d);
    }

    let mut month_days: Vec<String> = listed_days_map
        .into_keys()
        .filter(|d| d.starts_with(month))
        .collect();
    month_days.sort();

    let (_today_date, _today_str, _yesterday_str, live_set) =
        compute_live_set(journal_path, &month_days, now);

    let mut day_identities: HashMap<String, DayIdentity> = HashMap::new();

    for day in &month_days {
        let is_live = live_set.contains(day);
        let stored_marker = {
            let inv = inv_arc.lock().expect("inventory lock");
            inv.rows.get(day).map(|r| r.identity.marker().clone())
        };
        let current_id =
            read_day_identity(journal_path, day, is_live, stored_marker.as_ref(), identity);
        let need_fold = {
            let inv = inv_arc.lock().expect("inventory lock");
            match inv.rows.get(day) {
                None => true,
                Some(row) => {
                    matches!(row.state, DayFoldState::Failed(_)) || current_id != row.identity
                }
            }
        };
        if need_fold {
            match fold_single_day(
                &root,
                journal_path,
                day,
                is_live,
                now,
                req_gen,
                &inv_arc,
                true,
            ) {
                Ok(new_id) => {
                    day_identities.insert(day.clone(), new_id);
                }
                Err(FoldAborted::TargetUseRefused) => {
                    return Err(format!("chronicle day {day}: target use refused"));
                }
            }
        } else {
            day_identities.insert(day.clone(), current_id);
        }
    }

    let inv = inv_arc.lock().expect("inventory lock");
    let mut month_counts = BTreeMap::new();
    for day in &month_days {
        let expected_id = day_identities.get(day);
        match inv.rows.get(day) {
            None => return Err(format!("chronicle day {day}: missing count")),
            Some(row) => {
                if Some(&row.identity) != expected_id {
                    return Err(format!("chronicle day {day}: identity mismatch"));
                }
                match &row.state {
                    DayFoldState::Computed(count) => {
                        if *count > 0 {
                            month_counts.insert(day.clone(), *count);
                        }
                    }
                    DayFoldState::Failed(err) => {
                        return Err(err.clone());
                    }
                }
            }
        }
    }

    Ok(month_counts)
}

#[cfg(any(test, feature = "test-hooks"))]
mod lifecycle_hooks {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, LazyLock, Mutex};

    use solstone_core_journal_io::ObjectIdentity;

    /// What a retirement did, in order, for one journal.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum RetireEvent {
        /// It is waiting for the background worker to finish.
        Joining,
        /// It is waiting for an earlier retirement of the same journal.
        Waiting,
        /// The caller saw it return.
        Returned,
    }

    /// The background worker parked after one day's fold.
    pub struct BetweenDays {
        /// (arrived, held)
        state: Mutex<(bool, bool)>,
        cv: Condvar,
    }

    impl BetweenDays {
        pub fn wait_arrived(&self) {
            let mut state = self.state.lock().unwrap();
            while !state.0 {
                state = self.cv.wait(state).unwrap();
            }
        }
        pub fn release(&self) {
            self.state.lock().unwrap().1 = false;
            self.cv.notify_all();
        }
    }

    static BETWEEN_DAYS: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<BetweenDays>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    type RetireLog = (Mutex<HashMap<ObjectIdentity, Vec<RetireEvent>>>, Condvar);

    static RETIRE_EVENTS: LazyLock<RetireLog> =
        LazyLock::new(|| (Mutex::new(HashMap::new()), Condvar::new()));

    static FOLDS: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<AtomicUsize>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    /// Park the background worker after its next day, until released.
    pub fn hold_worker_between_days(identity: ObjectIdentity) -> Arc<BetweenDays> {
        let hold = Arc::new(BetweenDays {
            state: Mutex::new((false, true)),
            cv: Condvar::new(),
        });
        BETWEEN_DAYS.lock().unwrap().insert(identity, hold.clone());
        hold
    }

    pub(crate) fn run_worker_between_days(identity: ObjectIdentity) {
        let hold = BETWEEN_DAYS.lock().unwrap().remove(&identity);
        if let Some(h) = hold {
            let mut state = h.state.lock().unwrap();
            state.0 = true;
            h.cv.notify_all();
            while state.1 {
                state = h.cv.wait(state).unwrap();
            }
        }
    }

    /// Forget every hook and record kept for this journal identity. Tests call
    /// it first: a new temporary journal can reuse an earlier one's identity.
    pub fn clear(identity: ObjectIdentity) {
        if let Some(h) = BETWEEN_DAYS.lock().unwrap().remove(&identity) {
            h.release();
        }
        RETIRE_EVENTS.0.lock().unwrap().remove(&identity);
        FOLDS.lock().unwrap().remove(&identity);
    }

    pub fn note_retire_event(identity: ObjectIdentity, event: RetireEvent) {
        let (lock, cv) = &*RETIRE_EVENTS;
        lock.lock()
            .unwrap()
            .entry(identity)
            .or_default()
            .push(event);
        cv.notify_all();
    }

    /// Block until `n` retirement events are recorded; return them all.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn wait_retire_events(identity: ObjectIdentity, n: usize) -> Vec<RetireEvent> {
        let (lock, cv) = &*RETIRE_EVENTS;
        let mut events = lock.lock().unwrap();
        while events.get(&identity).map_or(0, Vec::len) < n {
            events = cv.wait(events).unwrap();
        }
        events.get(&identity).cloned().unwrap_or_default()
    }

    /// Block until a retirement event is recorded; return every event so far.
    pub fn wait_retire_event(identity: ObjectIdentity) -> Vec<RetireEvent> {
        let (lock, cv) = &*RETIRE_EVENTS;
        let mut events = lock.lock().unwrap();
        while events.get(&identity).is_none_or(|e| e.is_empty()) {
            events = cv.wait(events).unwrap();
        }
        events.get(&identity).cloned().unwrap_or_default()
    }

    pub(crate) fn note_fold(identity: ObjectIdentity) {
        FOLDS
            .lock()
            .unwrap()
            .entry(identity)
            .or_default()
            .fetch_add(1, Ordering::SeqCst);
    }

    /// Day folds that started computing for this journal.
    #[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
    pub fn folds(identity: ObjectIdentity) -> usize {
        FOLDS
            .lock()
            .unwrap()
            .get(&identity)
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(0)
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub use lifecycle_hooks::RetireEvent;

/// Test hooks for the calendar's background lifecycle.
#[cfg(feature = "test-hooks")]
pub mod calendar_lifecycle_hooks {
    pub use super::lifecycle_hooks::{
        BetweenDays, RetireEvent, clear, folds, hold_worker_between_days, note_retire_event,
        wait_retire_event, wait_retire_events,
    };
}

#[cfg(any(test, feature = "test-hooks"))]
mod identity_barrier {
    use std::collections::HashMap;
    use std::sync::{Arc, Condvar, LazyLock, Mutex};

    use solstone_core_journal_io::ObjectIdentity;

    struct BarrierEntry {
        hook: Arc<dyn Fn(&str) -> bool + Send + Sync>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    static BARRIERS: LazyLock<Mutex<HashMap<ObjectIdentity, BarrierEntry>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    pub(crate) fn install(
        identity: ObjectIdentity,
        hook: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) {
        BARRIERS.lock().unwrap().insert(
            identity,
            BarrierEntry {
                hook: Arc::new(hook),
                gate: Arc::new((Mutex::new(false), Condvar::new())),
            },
        );
    }

    pub(crate) fn run(identity: ObjectIdentity, day: &str) {
        let entry = BARRIERS
            .lock()
            .unwrap()
            .get(&identity)
            .map(|entry| (entry.hook.clone(), entry.gate.clone()));
        if let Some((hook, gate)) = entry
            && hook(day)
        {
            let (lock, cv) = &*gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cv.wait(released).unwrap();
            }
            *released = false;
        }
    }

    pub(crate) fn release(identity: ObjectIdentity) {
        let barriers = BARRIERS.lock().unwrap();
        if let Some(entry) = barriers.get(&identity) {
            let (lock, cv) = &*entry.gate;
            let mut released = lock.lock().unwrap();
            *released = true;
            cv.notify_all();
        }
    }

    // The integration target builds this module without cfg(test). The lib
    // tests are the caller; that build still needs the function.
    #[allow(dead_code)]
    pub(crate) fn clear(identity: ObjectIdentity) {
        let entry = BARRIERS.lock().unwrap().remove(&identity);
        if let Some(entry) = entry {
            let (lock, cv) = &*entry.gate;
            let mut released = lock.lock().unwrap();
            *released = true;
            cv.notify_all();
        }
    }
}

#[cfg(feature = "test-hooks")]
pub fn install_calendar_identity_barrier(
    identity: ObjectIdentity,
    hook: impl Fn(&str) -> bool + Send + Sync + 'static,
) {
    identity_barrier::install(identity, hook);
}

#[cfg(feature = "test-hooks")]
pub fn release_calendar_identity_barrier(identity: ObjectIdentity) {
    identity_barrier::release(identity);
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod test_support {
    use solstone_core_journal_io::ObjectIdentity;
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, LazyLock, Mutex};

    #[derive(Default)]
    pub(crate) struct SeamCounters {
        pub(crate) chronicle_lists: AtomicUsize,
        pub(crate) marker_stats: AtomicUsize,
        pub(crate) tree_descents: AtomicUsize,
        pub(crate) compute_starts: AtomicUsize,
    }

    static SEAM_COUNTERS: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<SeamCounters>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn get_seam_counters(identity: ObjectIdentity) -> Arc<SeamCounters> {
        let mut map = SEAM_COUNTERS.lock().unwrap();
        map.entry(identity).or_default().clone()
    }

    pub(crate) fn inc_chronicle_lists(identity: ObjectIdentity) {
        get_seam_counters(identity)
            .chronicle_lists
            .fetch_add(1, Ordering::SeqCst);
    }
    pub(crate) fn inc_marker_stats(identity: ObjectIdentity) {
        get_seam_counters(identity)
            .marker_stats
            .fetch_add(1, Ordering::SeqCst);
    }
    pub(crate) fn inc_tree_descents(identity: ObjectIdentity) {
        get_seam_counters(identity)
            .tree_descents
            .fetch_add(1, Ordering::SeqCst);
    }
    pub(crate) fn inc_compute_starts(identity: ObjectIdentity) {
        get_seam_counters(identity)
            .compute_starts
            .fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn chronicle_lists(identity: ObjectIdentity) -> usize {
        get_seam_counters(identity)
            .chronicle_lists
            .load(Ordering::SeqCst)
    }
    pub(crate) fn marker_stats(identity: ObjectIdentity) -> usize {
        get_seam_counters(identity)
            .marker_stats
            .load(Ordering::SeqCst)
    }
    pub(crate) fn tree_descents(identity: ObjectIdentity) -> usize {
        get_seam_counters(identity)
            .tree_descents
            .load(Ordering::SeqCst)
    }
    pub(crate) fn compute_starts(identity: ObjectIdentity) -> usize {
        get_seam_counters(identity)
            .compute_starts
            .load(Ordering::SeqCst)
    }

    pub(crate) fn reset_seam_counters(identity: ObjectIdentity) {
        let mut map = SEAM_COUNTERS.lock().unwrap();
        map.remove(&identity);
    }

    type Counter<T> = Arc<(Mutex<T>, Condvar)>;

    static WAITER_COUNTERS: LazyLock<Mutex<HashMap<ObjectIdentity, Counter<usize>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn get_waiter_counter(identity: ObjectIdentity) -> Arc<(Mutex<usize>, Condvar)> {
        let mut map = WAITER_COUNTERS.lock().unwrap();
        map.entry(identity)
            .or_insert_with(|| Arc::new((Mutex::new(0), Condvar::new())))
            .clone()
    }

    pub(crate) fn inc_waiter_counter(identity: ObjectIdentity) {
        let entry = get_waiter_counter(identity);
        let (lock, cv) = &*entry;
        let mut count = lock.lock().unwrap();
        *count += 1;
        cv.notify_all();
    }
    pub(crate) fn dec_waiter_counter(identity: ObjectIdentity) {
        let entry = get_waiter_counter(identity);
        let (lock, cv) = &*entry;
        let mut count = lock.lock().unwrap();
        *count = count.saturating_sub(1);
        cv.notify_all();
    }
    pub(crate) fn waiter_counter(identity: ObjectIdentity) -> usize {
        let entry = get_waiter_counter(identity);
        let (lock, _) = &*entry;
        *lock.lock().unwrap()
    }

    pub(crate) fn wait_waiter_count(
        identity: ObjectIdentity,
        pred: impl Fn(usize) -> bool,
        timeout: std::time::Duration,
    ) -> usize {
        let entry = get_waiter_counter(identity);
        let (lock, cv) = &*entry;
        let mut count = lock.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if pred(*count) {
                return *count;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return *count;
            }
            let (guard, wait) = cv.wait_timeout(count, deadline - now).unwrap();
            count = guard;
            if wait.timed_out() && !pred(*count) {
                return *count;
            }
        }
    }

    static WORKER_PASSES: LazyLock<Mutex<HashMap<ObjectIdentity, Counter<u64>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn get_worker_passes(identity: ObjectIdentity) -> Arc<(Mutex<u64>, Condvar)> {
        let mut map = WORKER_PASSES.lock().unwrap();
        map.entry(identity)
            .or_insert_with(|| Arc::new((Mutex::new(0), Condvar::new())))
            .clone()
    }

    pub(crate) fn inc_worker_passes(identity: ObjectIdentity) {
        let entry = get_worker_passes(identity);
        let (lock, cv) = &*entry;
        let mut count = lock.lock().unwrap();
        *count += 1;
        cv.notify_all();
    }

    pub(crate) fn snapshot_worker_passes(identity: ObjectIdentity) -> u64 {
        let entry = get_worker_passes(identity);
        let (lock, _) = &*entry;
        *lock.lock().unwrap()
    }

    pub(crate) fn wait_worker_passes_after(identity: ObjectIdentity, snapshot: u64) {
        let entry = get_worker_passes(identity);
        let (lock, cv) = &*entry;
        let mut count = lock.lock().unwrap();
        while *count <= snapshot {
            count = cv.wait(count).unwrap();
        }
    }

    type BarrierHook = Arc<dyn Fn(&str) -> bool + Send + Sync>;

    pub(crate) struct BarrierEntry {
        pub(crate) hook: BarrierHook,
        pub(crate) gate: Arc<(Mutex<bool>, Condvar)>,
    }

    pub(crate) struct HoldBarrier {
        pub(crate) held: Mutex<bool>,
        pub(crate) cv: Condvar,
    }

    type FileStatHook = Arc<dyn Fn(&Path) + Send + Sync>;

    #[derive(Default)]
    struct BarrierState {
        after_compute: HashMap<ObjectIdentity, BarrierEntry>,
        injected_compute_errors: HashMap<(ObjectIdentity, String), String>,
        hold_background_fills: HashMap<ObjectIdentity, Arc<HoldBarrier>>,
        inline_fold_limits: HashMap<ObjectIdentity, usize>,
        before_file_stats: HashMap<ObjectIdentity, FileStatHook>,
        notifications: HashMap<ObjectIdentity, Vec<(String, String)>>,
    }

    static BARRIERS: LazyLock<Mutex<BarrierState>> =
        LazyLock::new(|| Mutex::new(BarrierState::default()));

    pub(crate) fn install_barrier_after_identity_read(
        identity: ObjectIdentity,
        hook: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) {
        super::identity_barrier::install(identity, hook);
    }

    pub(crate) fn install_barrier_after_compute(
        identity: ObjectIdentity,
        hook: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) {
        BARRIERS.lock().unwrap().after_compute.insert(
            identity,
            BarrierEntry {
                hook: Arc::new(hook),
                gate: Arc::new((Mutex::new(false), Condvar::new())),
            },
        );
    }

    pub(crate) fn run_barrier_after_compute(identity: ObjectIdentity, day: &str) {
        let entry = BARRIERS
            .lock()
            .unwrap()
            .after_compute
            .get(&identity)
            .map(|e| (e.hook.clone(), e.gate.clone()));
        if let Some((hook, gate)) = entry
            && hook(day)
        {
            let (lock, cv) = &*gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cv.wait(released).unwrap();
            }
            *released = false;
        }
    }

    pub(crate) fn release_identity_barrier(identity: ObjectIdentity) {
        super::identity_barrier::release(identity);
    }

    pub(crate) fn release_compute_barrier(identity: ObjectIdentity) {
        let b = BARRIERS.lock().unwrap();
        if let Some(entry) = b.after_compute.get(&identity) {
            let (lock, cv) = &*entry.gate;
            let mut released = lock.lock().unwrap();
            *released = true;
            cv.notify_all();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn release_barrier(identity: ObjectIdentity) {
        release_identity_barrier(identity);
        release_compute_barrier(identity);
    }

    pub(crate) fn set_injected_compute_error(
        identity: ObjectIdentity,
        day: &str,
        error: Option<&str>,
    ) {
        let mut b = BARRIERS.lock().unwrap();
        if let Some(err) = error {
            b.injected_compute_errors
                .insert((identity, day.to_string()), err.to_string());
        } else {
            b.injected_compute_errors
                .remove(&(identity, day.to_string()));
        }
    }

    pub(crate) fn injected_compute_error(identity: ObjectIdentity, day: &str) -> Option<String> {
        BARRIERS
            .lock()
            .unwrap()
            .injected_compute_errors
            .get(&(identity, day.to_string()))
            .cloned()
    }

    pub(crate) fn limit_inline_folds(identity: ObjectIdentity, n: usize) {
        BARRIERS
            .lock()
            .unwrap()
            .inline_fold_limits
            .insert(identity, n);
    }

    pub(crate) fn inline_fold_limit(identity: ObjectIdentity) -> Option<usize> {
        BARRIERS
            .lock()
            .unwrap()
            .inline_fold_limits
            .get(&identity)
            .copied()
    }

    pub(crate) fn install_before_file_stat(
        identity: ObjectIdentity,
        hook: impl Fn(&Path) + Send + Sync + 'static,
    ) {
        BARRIERS
            .lock()
            .unwrap()
            .before_file_stats
            .insert(identity, Arc::new(hook));
    }

    pub(crate) fn run_before_file_stat(identity: ObjectIdentity, path: &Path) {
        let hook = BARRIERS
            .lock()
            .unwrap()
            .before_file_stats
            .get(&identity)
            .cloned();
        if let Some(h) = hook {
            h(path);
        }
    }

    pub(crate) fn record_notification(identity: ObjectIdentity, kind: &str, key: &str) {
        let mut b = BARRIERS.lock().unwrap();
        b.notifications
            .entry(identity)
            .or_default()
            .push((kind.to_string(), key.to_string()));
    }

    pub(crate) fn recorded_notifications(identity: ObjectIdentity) -> Vec<(String, String)> {
        BARRIERS
            .lock()
            .unwrap()
            .notifications
            .get(&identity)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn clear_recorded_notifications(identity: ObjectIdentity) {
        BARRIERS.lock().unwrap().notifications.remove(&identity);
    }

    pub(crate) fn hold_background_fill(identity: ObjectIdentity) -> Arc<HoldBarrier> {
        let hold = Arc::new(HoldBarrier {
            held: Mutex::new(true),
            cv: Condvar::new(),
        });
        BARRIERS
            .lock()
            .unwrap()
            .hold_background_fills
            .insert(identity, hold.clone());
        hold
    }

    pub(crate) fn release_background_fill(identity: ObjectIdentity) {
        let hold = BARRIERS
            .lock()
            .unwrap()
            .hold_background_fills
            .remove(&identity);
        if let Some(h) = hold {
            *h.held.lock().unwrap() = false;
            h.cv.notify_all();
        }
    }

    pub(crate) fn wait_hold_background_fill(identity: ObjectIdentity) {
        let hold = BARRIERS
            .lock()
            .unwrap()
            .hold_background_fills
            .get(&identity)
            .cloned();
        if let Some(h) = hold {
            let mut guard = h.held.lock().unwrap();
            while *guard {
                guard = h.cv.wait(guard).unwrap();
            }
        }
    }

    pub(crate) fn reset(identity: ObjectIdentity) {
        super::identity_barrier::clear(identity);
        let mut b = BARRIERS.lock().unwrap();
        if let Some(entry) = b.after_compute.remove(&identity) {
            let (lock, cv) = &*entry.gate;
            let mut released = lock.lock().unwrap();
            *released = true;
            cv.notify_all();
        }
        b.injected_compute_errors
            .retain(|(id, _), _| id != &identity);
        if let Some(h) = b.hold_background_fills.remove(&identity) {
            *h.held.lock().unwrap() = false;
            h.cv.notify_all();
        }
        b.inline_fold_limits.remove(&identity);
        b.before_file_stats.remove(&identity);
        super::lifecycle_hooks::clear(identity);
        b.notifications.remove(&identity);
        drop(b);
        reset_seam_counters(identity);
        let mut w = WAITER_COUNTERS.lock().unwrap();
        w.remove(&identity);
        let mut wp = WORKER_PASSES.lock().unwrap();
        wp.remove(&identity);
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default, clippy::too_many_arguments)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use chrono::{DateTime, TimeZone, Utc};
    use serde_json::{Value, json};
    use solstone_core_journal_io::{
        HealthMarkerKind, JournalRoot, ObjectIdentity, day_dirs, health_marker_path,
    };
    use tower::ServiceExt;

    use super::*;
    use crate::Clock;

    struct TestResetGuard(ObjectIdentity);
    impl Drop for TestResetGuard {
        fn drop(&mut self) {
            test_support::release_identity_barrier(self.0);
            test_support::release_compute_barrier(self.0);
            test_support::release_background_fill(self.0);
            crate::counts_flight::test_support::reset(self.0);
            drop_inventory(self.0);
            test_support::reset(self.0);
        }
    }

    fn copy_dir_all(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let ty = entry.file_type().unwrap();
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir_all(&src_path, &dst_path);
            } else if ty.is_file() {
                fs::copy(&src_path, &dst_path).unwrap();
            }
        }
    }

    fn delete_stats_json(dir: &Path) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    delete_stats_json(&p);
                } else if entry.file_name() == "stats.json" {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }

    fn setup_fixture_journal() -> (tempfile::TempDir, Arc<JournalRoot>) {
        let temp = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/journal");
        copy_dir_all(&fixture, temp.path());
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        (temp, root)
    }

    fn write_browser_segment(journal: &Path, day: &str, key: &str) {
        let seg = journal
            .join("chronicle")
            .join(day)
            .join("workstation.browser")
            .join(key);
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("stream.json"),
            b"{\"stream\":\"workstation.browser\",\"seq\":1}\n",
        )
        .unwrap();
        fs::write(
            seg.join("browser_page.jsonl"),
            b"{\"t\":\"segment_start\",\"ts\":1783006230000,\"rel\":30000,\"site\":\"example.com\",\"url\":\"https://example.com\",\"title\":\"Test\",\"adapter\":\"docs\",\"ctx\":\"doc\",\"n\":1,\"blocks\":[]}\n",
        )
        .unwrap();
    }

    fn setup_minimal_journal() -> (tempfile::TempDir, Arc<JournalRoot>) {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"},"convey":{"password_hash":"","secret":"sec","trust_localhost":true}}"#,
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        (temp, root)
    }

    fn write_lying_stats(journal: &Path, day: &str, sentinel: u64) {
        let mut scan = solstone_core_journal_stats_cli::DayScan::default();
        scan.schema_version = solstone_core_journal_stats_cli::SCHEMA_VERSION;
        scan.stats.transcript_ranges = sentinel;
        let stats_path = journal.join("chronicle").join(day).join("stats.json");
        solstone_core_journal_io::write_json(
            &stats_path,
            &scan,
            solstone_core_journal_io::JsonWriteOptions::default(),
        )
        .unwrap();
        let future = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() + std::time::Duration::from_secs(100),
        );
        filetime::set_file_mtime(&stats_path, future).unwrap();
    }

    fn stats_free_walk(journal: &Path, now: DateTime<Utc>) -> BTreeMap<String, u64> {
        let mut aside = Vec::new();
        if let Ok(entries) = fs::read_dir(journal.join("chronicle")) {
            for entry in entries.flatten() {
                let stats_path = entry.path().join("stats.json");
                let aside_path = entry.path().join("stats.json.aside");
                if stats_path.is_file() {
                    fs::rename(&stats_path, &aside_path).unwrap();
                    aside.push((stats_path, aside_path));
                }
            }
        }
        let walk = crate::calendar::counts_by_walk(journal, now).unwrap();
        for (stats_path, aside_path) in aside {
            fs::rename(&aside_path, &stats_path).unwrap();
        }
        walk
    }

    fn month_sum(walk: &BTreeMap<String, u64>, month: &str) -> u64 {
        walk.iter()
            .filter(|(day, _)| day.starts_with(month))
            .map(|(_, count)| *count)
            .sum()
    }

    async fn index_month(app: &axum::Router, root: &Arc<JournalRoot>, month: &str) -> u64 {
        index_body(app, root).await["months"][month]
            .as_u64()
            .unwrap_or(0)
    }

    fn write_analyzed_audio(dir: &Path, text: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("mic_audio.jsonl"),
            format!(
                "{{\"_solstone_processing\":{{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}}}\n{{\"start\":\"10:00:00\",\"end\":\"10:00:01\",\"text\":\"{text}\"}}\n"
            ),
        )
        .unwrap();
    }

    async fn index_body(app: &axum::Router, root: &Arc<JournalRoot>) -> Value {
        let req = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    fn assert_next_read_sees_change(body: &Value, month: &str, stale: u64, fresh: u64) {
        let complete = body["freshness"]["complete"] == true;
        let shown = body["months"][month].as_u64().unwrap_or(0);
        assert!(
            !(complete && shown == stale),
            "next read reported the pre-write count {stale} with complete:true"
        );
        if complete {
            assert_eq!(shown, fresh);
        }
    }

    fn full_pass_claimed(identity: ObjectIdentity) -> bool {
        let reg = INVENTORIES.lock().expect("inventories lock");
        reg.get(&identity)
            .expect("inventory")
            .lock()
            .expect("inventory lock")
            .in_full_pass
    }

    /// Wait until no full pass is claimed. The worker clears the claim before
    /// it counts the pass, so a count past the snapshot means it was cleared.
    fn wait_full_pass_released(identity: ObjectIdentity) {
        loop {
            let snap = test_support::snapshot_worker_passes(identity);
            if !full_pass_claimed(identity) {
                return;
            }
            test_support::wait_worker_passes_after(identity, snap);
        }
    }

    async fn assert_unbumped_then_hourly(
        app: &axum::Router,
        root: &Arc<JournalRoot>,
        journal: &Path,
        now_ref: &Arc<Mutex<DateTime<Utc>>>,
        identity: ObjectIdentity,
        month: &str,
        day: &str,
        old: u64,
    ) -> u64 {
        *now_ref.lock().unwrap() += chrono::Duration::minutes(30);
        let hidden = index_month(app, root, month).await;
        assert_eq!(
            hidden, old,
            "unbumped change on {day} stays hidden inside the hour"
        );
        *now_ref.lock().unwrap() += chrono::Duration::seconds(3700);
        test_support::clear_recorded_notifications(identity);
        let body = index_body(app, root).await;
        let walk = crate::calendar::counts_by_walk(journal, *now_ref.lock().unwrap()).unwrap();
        let new = month_sum(&walk, month);
        assert_ne!(old, new, "change on {day} must move the count");
        assert_eq!(body["months"][month].as_u64(), Some(new));
        assert_eq!(
            test_support::recorded_notifications(identity),
            vec![("day".to_string(), day.to_string())]
        );
        new
    }

    #[tokio::test]
    async fn test_oracle_equivalence() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        delete_stats_json(temp.path());

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let now_ref = Arc::new(Mutex::new(now));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();

        let app = crate::router(temp.path().to_path_buf(), clock.clone(), || {
            Response::new(Body::empty())
        });
        let req = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes1 = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let mut body1: Value = serde_json::from_slice(&bytes1).unwrap();

        let freshness = body1.as_object_mut().unwrap().remove("freshness").unwrap();
        assert_eq!(freshness, json!({"complete": true}));

        let mut expected_index = crate::calendar::date_nav_index(&walk, Value::Null);
        expected_index.as_object_mut().unwrap().remove("freshness");
        assert_eq!(body1, expected_index);

        let mut month_bytes_map = BTreeMap::new();
        let months: Vec<String> = body1["months"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for month in &months {
            let req = Request::get(format!("/app/transcripts/api/stats/{month}"))
                .extension(root.clone())
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let month_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let month_val: Value = serde_json::from_slice(&month_bytes).unwrap();

            let expected_month: BTreeMap<String, u64> = walk
                .iter()
                .filter(|(day, count)| day.starts_with(month) && **count > 0)
                .map(|(d, c)| (d.clone(), *c))
                .collect();
            assert_eq!(month_val, serde_json::to_value(&expected_month).unwrap());
            month_bytes_map.insert(month.clone(), month_bytes);
        }

        // Second check: copy whose stats.json files are rewritten to prepare count with newer mtime
        let (temp2, root2) = setup_fixture_journal();
        let identity2 = root2.identity();
        let _guard2 = TestResetGuard(identity2);

        let future_time = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() + std::time::Duration::from_secs(100),
        );
        for (day, count) in &walk {
            let mut scan = solstone_core_journal_stats_cli::DayScan::default();
            scan.schema_version = solstone_core_journal_stats_cli::SCHEMA_VERSION;
            scan.stats.transcript_ranges = count.saturating_add(1_000_000);
            assert_ne!(scan.stats.transcript_ranges, *count);
            let dir = solstone_core_journal_io::day_path(temp2.path(), day, false).unwrap();
            let stats_path = dir.join("stats.json");
            solstone_core_journal_io::write_json(
                &stats_path,
                &scan,
                solstone_core_journal_io::JsonWriteOptions::default(),
            )
            .unwrap();
            filetime::set_file_mtime(&stats_path, future_time).unwrap();
        }

        let app2 = crate::router(temp2.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });
        let req2 = Request::get("/app/transcripts/api/index")
            .extension(root2.clone())
            .body(Body::empty())
            .unwrap();
        let resp2 = app2.clone().oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let bytes2 = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
        let mut body2: Value = serde_json::from_slice(&bytes2).unwrap();
        body2.as_object_mut().unwrap().remove("freshness");
        assert_eq!(body2, body1);

        for month in &months {
            let req = Request::get(format!("/app/transcripts/api/stats/{month}"))
                .extension(root2.clone())
                .body(Body::empty())
                .unwrap();
            let resp = app2.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let m2_bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            assert_eq!(m2_bytes, month_bytes_map[month]);
        }

        // One fixture-day change at a time, with no marker bump. Each stays
        // hidden until the hourly refold, and each refold moves the count.
        // Newest fixture day is folded first in a full pass, before the inline budget.
        let day = walk.keys().next_back().cloned().expect("fixture day");
        let month = &day[..6];
        let before = index_month(&app, &root, month).await;
        write_browser_segment(temp.path(), &day, "235000_60");
        let hidden_first = index_month(&app, &root, month).await;
        assert_eq!(hidden_first, before);
        let walked_first = month_sum(
            &crate::calendar::counts_by_walk(temp.path(), now).unwrap(),
            month,
        );
        assert_ne!(before, walked_first);
        *now_ref.lock().unwrap() += chrono::Duration::seconds(3700);
        let after_first = index_month(&app, &root, month).await;
        assert_ne!(after_first, before);
        assert_eq!(
            after_first,
            month_sum(
                &crate::calendar::counts_by_walk(temp.path(), *now_ref.lock().unwrap()).unwrap(),
                month
            )
        );

        write_browser_segment(temp.path(), &day, "235100_60");
        let hidden_second = index_month(&app, &root, month).await;
        assert_eq!(hidden_second, after_first);
        let walked_second_hidden = month_sum(
            &crate::calendar::counts_by_walk(temp.path(), *now_ref.lock().unwrap()).unwrap(),
            month,
        );
        assert_ne!(after_first, walked_second_hidden);
        *now_ref.lock().unwrap() += chrono::Duration::seconds(3700);
        let after_second = index_month(&app, &root, month).await;
        assert_ne!(after_second, after_first);
        assert_eq!(
            after_second,
            month_sum(
                &crate::calendar::counts_by_walk(temp.path(), *now_ref.lock().unwrap()).unwrap(),
                month
            )
        );
    }

    #[tokio::test]
    async fn test_warm_seam() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock.clone(), || {
            Response::new(Body::empty())
        });

        let req = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        test_support::reset_seam_counters(identity);

        let req2 = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp2 = app.clone().oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);

        assert_eq!(
            test_support::chronicle_lists(identity),
            1,
            "exactly one chronicle list"
        );
        assert_eq!(
            test_support::compute_starts(identity),
            0,
            "zero compute starts for old days"
        );
        let listed_days_map = day_dirs(temp.path()).unwrap();
        let listed_days: Vec<String> = listed_days_map.into_keys().collect();
        assert_eq!(
            test_support::marker_stats(identity),
            listed_days.len(),
            "one marker stat per listed day"
        );

        let (_today_date, _today_str, _yesterday_str, live_set) =
            compute_live_set(temp.path(), &listed_days, now);
        let live_days_count = listed_days.iter().filter(|d| live_set.contains(*d)).count();
        // A malformed stream marker is read as a live tree. The fixture day whose
        // marker is empty is not one: the next bump replaces it.
        let marker_text = |day: &String| {
            fs::read_to_string(health_marker_path(
                temp.path(),
                day,
                HealthMarkerKind::Stream,
            ))
            .ok()
        };
        assert!(
            listed_days
                .iter()
                .any(|day| marker_text(day).is_some_and(|text| text.is_empty())),
            "the fixture carries an empty marker"
        );
        let malformed_days = listed_days
            .iter()
            .filter(|day| marker_text(day).is_some_and(|text| marker_text_is_malformed(&text)))
            .count();
        assert_eq!(
            test_support::tree_descents(identity),
            live_days_count + malformed_days,
            "tree descent equals live days and malformed markers"
        );
    }

    // The write lands after compute. A mutant that reads identity after that
    // stores the new marker with the old count, and the next read would report
    // that count with complete:true.
    #[tokio::test]
    async fn test_barrier_hide_stale() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let target_day = "20260306";
        let target_path = temp.path().join("chronicle").join(target_day);
        let stale_walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let stale_sum = month_sum(&stale_walk, "202603");
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let held_tx = Arc::new(Mutex::new(Some(held_tx)));
        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();
        let target_day_s = target_day.to_string();

        test_support::install_barrier_after_compute(identity, move |day| {
            if day == target_day_s && !fired_c.swap(true, Ordering::SeqCst) {
                if let Some(tx) = held_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                true
            } else {
                false
            }
        });

        let target_path_c = target_path.clone();
        let temp_path = temp.path().to_path_buf();
        let target_day_c = target_day.to_string();
        std::thread::spawn(move || {
            held_rx.recv().unwrap();
            let seg = target_path_c.join("field").join("120000_60");
            fs::create_dir_all(&seg).unwrap();
            fs::write(
                seg.join("mic_audio.jsonl"),
                b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"12:00:00\",\"end\":\"12:00:01\",\"text\":\"new\"}\n",
            )
            .unwrap();
            solstone_core_journal_io::bump_stream_marker(&temp_path, &target_day_c).unwrap();
            test_support::release_compute_barrier(identity);
        });

        let req = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["freshness"], json!({"complete": true}));
        assert_eq!(body["months"]["202603"], stale_sum);

        let fresh_walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let fresh_sum = month_sum(&fresh_walk, "202603");
        assert_ne!(fresh_sum, stale_sum);
        let next = index_body(&app, &root).await;
        assert_next_read_sees_change(&next, "202603", stale_sum, fresh_sum);
    }

    #[tokio::test]
    async fn test_barrier_today_raw_media() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["identity"]["timezone"] = json!("UTC");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });
        let today = "20260801";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(today)
                .join("field/100000_60"),
            "today",
        );
        let seeded = index_body(&app, &root).await;
        assert_eq!(seeded["freshness"]["complete"], true);
        let before = seeded["months"]["202608"].as_u64().unwrap();

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let held_tx = Arc::new(Mutex::new(Some(held_tx)));
        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();
        let today_s = today.to_string();
        test_support::install_barrier_after_compute(identity, move |day| {
            if day == today_s && !fired_c.swap(true, Ordering::SeqCst) {
                if let Some(tx) = held_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                true
            } else {
                false
            }
        });

        let today_path = temp.path().join("chronicle").join(today);
        let note = today_path.join("field/100000_60/note.txt");
        fs::write(&note, b"tree").unwrap();
        std::thread::spawn(move || {
            held_rx.recv().unwrap();
            let raw = today_path.join("field/180000_60");
            fs::create_dir_all(&raw).unwrap();
            fs::write(raw.join("mic.flac"), b"raw today").unwrap();
            test_support::release_compute_barrier(identity);
        });

        let body = index_body(&app, &root).await;
        assert_eq!(body["months"]["202608"], before);
        let walk = stats_free_walk(temp.path(), now);
        let expected = month_sum(&walk, "202608");
        assert_ne!(expected, before);
        let next = index_body(&app, &root).await;
        assert_next_read_sees_change(&next, "202608", before, expected);
    }

    #[tokio::test]
    async fn test_barrier_hourly_refold() {
        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let day = "20260101";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(day)
                .join("field/100000_60"),
            "seed",
        );
        let now_ref = Arc::new(Mutex::new(
            Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap(),
        ));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });
        let seeded = index_body(&app, &root).await;
        assert_eq!(seeded["freshness"]["complete"], true);
        let before = seeded["months"]["202601"].as_u64().unwrap();

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let held_tx = Arc::new(Mutex::new(Some(held_tx)));
        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();
        let day_s = day.to_string();
        test_support::install_barrier_after_compute(identity, move |day_name| {
            if day_name == day_s && !fired_c.swap(true, Ordering::SeqCst) {
                if let Some(tx) = held_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                true
            } else {
                false
            }
        });

        // The hour starts the fold. The browser write and the marker bump land
        // after compute, so this read stores the pre-write count.
        *now_ref.lock().unwrap() += chrono::Duration::seconds(3700);
        let journal = temp.path().to_path_buf();
        let day_c = day.to_string();
        std::thread::spawn(move || {
            held_rx.recv().unwrap();
            write_browser_segment(&journal, day, "235000_60");
            solstone_core_journal_io::bump_stream_marker(&journal, &day_c).unwrap();
            test_support::release_compute_barrier(identity);
        });

        let body = index_body(&app, &root).await;
        assert_eq!(body["months"]["202601"], before);
        let walk = stats_free_walk(temp.path(), *now_ref.lock().unwrap());
        let expected = month_sum(&walk, "202601");
        assert_ne!(expected, before);
        let next = index_body(&app, &root).await;
        assert_next_read_sees_change(&next, "202601", before, expected);
    }

    // A mutant that treats a fold started before arrival as current would return the pre-bump count.
    #[tokio::test]
    async fn test_barrier_month_twin() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock.clone(), || {
            Response::new(Body::empty())
        });

        // Seed with a complete read
        let req = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let initial_body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(initial_body["freshness"], json!({"complete": true}));
        let pre_bump_sum = initial_body["months"]["202603"].as_u64().unwrap();

        let target_day = "20260307";
        let inv_arc = get_or_create_inventory(&root, identity);
        let pre_bump_count = inv_arc
            .lock()
            .unwrap()
            .rows
            .get(target_day)
            .and_then(|r| match r.state {
                DayFoldState::Computed(c) => Some(c),
                _ => None,
            })
            .unwrap();

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let held_tx = Arc::new(Mutex::new(Some(held_tx)));
        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();
        let target_day_s = target_day.to_string();

        test_support::install_barrier_after_compute(identity, move |day| {
            if day == target_day_s && !fired_c.swap(true, Ordering::SeqCst) {
                if let Some(tx) = held_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                true
            } else {
                false
            }
        });

        // Hold direct fold_single_day of an old day AFTER COMPUTE
        let root_c = root.clone();
        let temp_c = temp.path().to_path_buf();
        let target_day_c = target_day.to_string();
        let inv_arc_c = inv_arc.clone();
        let fold_thread = std::thread::spawn(move || {
            let _ = fold_single_day(
                &root_c,
                &temp_c,
                &target_day_c,
                false,
                now,
                next_generation(),
                &inv_arc_c,
                false,
            );
        });
        held_rx.recv().unwrap();

        // While held, add a segment and bump
        let target_path = temp.path().join("chronicle").join(target_day);
        let seg = target_path.join("field").join("130000_60");
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("mic_audio.jsonl"),
            b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"13:00:00\",\"end\":\"13:00:01\",\"text\":\"hi\"}\n",
        )
        .unwrap();
        solstone_core_journal_io::bump_stream_marker(temp.path(), target_day).unwrap();

        // Start index and stats/{month} on other tasks
        let app_index = app.clone();
        let root_index = root.clone();
        let index_task = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/index")
                .extension(root_index)
                .body(Body::empty())
                .unwrap();
            let resp = app_index.oneshot(req).await.unwrap();
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            body
        });

        let app_stats = app.clone();
        let root_stats = root.clone();
        let stats_task = tokio::spawn(async move {
            let req = Request::get("/app/transcripts/api/stats/202603")
                .extension(root_stats)
                .body(Body::empty())
                .unwrap();
            let resp = app_stats.oneshot(req).await.unwrap();
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            body
        });

        // Spin on the waiter counter until it is at least 1 (yield, do not sleep)
        while test_support::waiter_counter(identity) < 1 {
            tokio::task::yield_now().await;
        }
        test_support::release_compute_barrier(identity);

        fold_thread.join().unwrap();
        let index_body = index_task.await.unwrap();
        let stats_body = stats_task.await.unwrap();

        if index_body["freshness"]["complete"] == true {
            let month_sum = index_body["months"]["202603"].as_u64().unwrap();
            assert_ne!(
                month_sum, pre_bump_sum,
                "index complete must not report pre-bump count"
            );
        }

        let walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let expected_month: BTreeMap<String, u64> = walk
            .iter()
            .filter(|(day, count)| day.starts_with("202603") && **count > 0)
            .map(|(d, c)| (d.clone(), *c))
            .collect();
        assert_eq!(stats_body, serde_json::to_value(expected_month).unwrap());
        let month_day_count = stats_body[target_day].as_u64().unwrap();
        assert_ne!(
            month_day_count, pre_bump_count,
            "month body must not be pre-bump count"
        );
    }

    #[tokio::test]
    async fn test_barrier_month_capture() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);
        // Only the request folds this day, so a worker cannot repair a stale
        // identity before the second read checks it.
        let _hold = test_support::hold_background_fill(identity);

        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["identity"]["timezone"] = json!("UTC");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 7, 3, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let target_day = "20260703";
        let target_path = temp.path().join("chronicle").join(target_day);
        let pre_walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let pre_target_count = *pre_walk.get(target_day).unwrap_or(&0);

        let target_path_c = target_path.clone();
        let temp_path = temp.path().to_path_buf();
        let target_day_c = target_day.to_string();

        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();

        test_support::install_barrier_after_compute(identity, move |day| {
            if day == target_day_c && !fired_c.swap(true, Ordering::SeqCst) {
                let seg = target_path_c.join("field").join("140000_60");
                fs::create_dir_all(&seg).unwrap();
                fs::write(
                    seg.join("mic_audio.jsonl"),
                    b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"14:00:00\",\"end\":\"14:00:01\",\"text\":\"after\"}\n",
                )
                .unwrap();
                solstone_core_journal_io::bump_stream_marker(&temp_path, &target_day_c).unwrap();
            }
            false
        });

        let req1 = Request::get("/app/transcripts/api/stats/202607")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        let bytes1 = to_bytes(resp1.into_body(), usize::MAX).await.unwrap();
        let body1: Value = serde_json::from_slice(&bytes1).unwrap();

        assert!(fired.load(Ordering::SeqCst));
        let count1 = body1[target_day].as_u64().unwrap();
        assert_eq!(count1, pre_target_count);

        let post_walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let post_target_count = *post_walk.get(target_day).unwrap_or(&0);
        assert_ne!(post_target_count, pre_target_count);

        let req2 = Request::get("/app/transcripts/api/stats/202607")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp2 = app.clone().oneshot(req2).await.unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let bytes2 = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
        let body2: Value = serde_json::from_slice(&bytes2).unwrap();

        let count2 = body2[target_day].as_u64().unwrap();
        assert_eq!(count2, post_target_count);
    }

    #[tokio::test]
    async fn test_unbumped_older_day_correction() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now_ref = Arc::new(Mutex::new(
            Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap(),
        ));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock.clone(), || {
            Response::new(Body::empty())
        });

        let seeded = index_body(&app, &root).await;
        assert_eq!(seeded["freshness"]["complete"], true);
        let mut shown = seeded["months"]["202605"].as_u64().unwrap();

        // Three unbumped edits, one at a time, on segments that already exist.
        // 094000_300 is the only audio in its quarter-hour. 091000_300 takes the
        // browser file. 090000_300 is screen-only, so a raw adds an audio slot.
        let day = "20260520";
        let day_path = temp.path().join("chronicle").join(day);
        let removed = day_path.join("default/094000_300");
        assert!(
            removed.join("audio.jsonl").is_file(),
            "removal target must be a real segment"
        );
        fs::remove_dir_all(&removed).unwrap();
        shown = assert_unbumped_then_hourly(
            &app,
            &root,
            temp.path(),
            &now_ref,
            identity,
            "202605",
            day,
            shown,
        )
        .await;

        let browser_seg = day_path.join("default/091000_300");
        assert!(
            browser_seg.is_dir(),
            "browser edit must use an existing segment"
        );
        fs::write(
            browser_seg.join("browser_page.jsonl"),
            b"{\"t\":\"segment_start\",\"title\":\"Test\",\"text\":\"page\"}\n",
        )
        .unwrap();
        shown = assert_unbumped_then_hourly(
            &app,
            &root,
            temp.path(),
            &now_ref,
            identity,
            "202605",
            day,
            shown,
        )
        .await;

        let raw_seg = day_path.join("default/090000_300");
        assert!(raw_seg.is_dir(), "raw edit must use an existing segment");
        fs::write(raw_seg.join("mic.flac"), b"raw audio bytes").unwrap();
        shown = assert_unbumped_then_hourly(
            &app,
            &root,
            temp.path(),
            &now_ref,
            identity,
            "202605",
            day,
            shown,
        )
        .await;

        let kept = index_body(&app, &root).await;
        assert_eq!(kept["months"]["202605"].as_u64(), Some(shown));

        let mut found_missed_producer = false;
        if let Ok(entries) = fs::read_dir(temp.path().join("chronicle")) {
            for day_entry in entries.flatten() {
                let health_dir = day_entry.path().join("health");
                if let Ok(h_entries) = fs::read_dir(health_dir) {
                    for h_file in h_entries.flatten() {
                        if let Ok(content) = fs::read_to_string(h_file.path())
                            && content.contains("chronicle day 20260520: missed producer")
                        {
                            found_missed_producer = true;
                        }
                    }
                }
            }
        }
        assert!(found_missed_producer, "must write missed producer line");

        test_support::clear_recorded_notifications(identity);
        restart_wipe(identity);
        assert!(
            test_support::recorded_notifications(identity).is_empty(),
            "restart_wipe records nothing"
        );

        let after_wipe = index_body(&app, &root).await;
        assert_eq!(after_wipe["months"]["202605"].as_u64(), Some(shown));
    }

    #[tokio::test]
    async fn test_live_today_yesterday_edits() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["identity"]["timezone"] = json!("UTC");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let today_path = temp.path().join("chronicle/20260801");
        fs::create_dir_all(&today_path).unwrap();
        let yesterday_path = temp.path().join("chronicle/20260731");
        fs::create_dir_all(&yesterday_path).unwrap();

        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Unbumped edits on today and yesterday:
        // Direct-layout segment on today
        let direct_seg = today_path.join("160000_60");
        fs::create_dir_all(&direct_seg).unwrap();
        fs::write(
            direct_seg.join("mic_audio.jsonl"),
            b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"16:00:00\",\"end\":\"16:00:01\",\"text\":\"direct\"}\n",
        )
        .unwrap();

        // Raw file inside a named segment on yesterday
        let named_seg = yesterday_path.join("field/170000_60");
        fs::create_dir_all(&named_seg).unwrap();
        fs::write(
            named_seg.join("mic_audio.jsonl"),
            b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"17:00:00\",\"end\":\"17:00:01\",\"text\":\"named\"}\n",
        )
        .unwrap();
        fs::write(named_seg.join("mic.raw"), b"raw bytes").unwrap();

        // A real fresh cache that lies. The inventory must not read it.
        let sentinel = 1_000_000;
        write_lying_stats(temp.path(), "20260801", sentinel);
        write_lying_stats(temp.path(), "20260731", sentinel);

        let resp = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let b: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(b["freshness"]["complete"], true);

        let walk = stats_free_walk(temp.path(), now);
        let today_month = month_sum(&walk, "202608");
        let yesterday_month = month_sum(&walk, "202607");
        assert_eq!(b["months"]["202608"], today_month);
        assert_eq!(b["months"]["202607"], yesterday_month);
        assert_ne!(today_month, sentinel);
        assert!(today_month > 0, "count must have changed");
        assert!(
            walk.get("20260731").copied().unwrap_or(0) > 0,
            "yesterday count changed"
        );

        // The next read keeps the count the edit read stored.
        let again = index_body(&app, &root).await;
        assert_eq!(again["months"]["202608"], today_month);
        assert_eq!(again["months"]["202607"], yesterday_month);
        assert_eq!(again["freshness"]["complete"], true);

        // Today raw-media variant: a raw file with no transcript, then the read after that.
        let raw_only = today_path.join("field/180000_60");
        fs::create_dir_all(&raw_only).unwrap();
        fs::write(raw_only.join("mic.flac"), b"raw today bytes").unwrap();
        let with_raw = index_body(&app, &root).await;
        let raw_month = with_raw["months"]["202608"].as_u64().unwrap();
        assert_ne!(raw_month, today_month);
        let after_raw = index_body(&app, &root).await;
        assert_eq!(after_raw["months"]["202608"], raw_month);
        assert_eq!(after_raw["freshness"]["complete"], true);
    }

    #[tokio::test]
    async fn test_asia_tokyo_midnight_rollover() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["identity"]["timezone"] = json!("Asia/Tokyo");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let t0 = Utc.with_ymd_and_hms(2026, 3, 1, 14, 30, 0).unwrap();
        let now_ref = Arc::new(Mutex::new(t0));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let day1 = temp.path().join("chronicle/20260301");
        fs::create_dir_all(&day1).unwrap();
        let seg1 = day1.join("field/233000_60");
        fs::create_dir_all(&seg1).unwrap();
        fs::write(seg1.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"23:30:00\",\"end\":\"23:30:01\",\"text\":\"t1\"}\n").unwrap();

        let day0 = temp.path().join("chronicle/20260228");
        fs::create_dir_all(&day0).unwrap();
        let seg0 = day0.join("field/233000_60");
        fs::create_dir_all(&seg0).unwrap();
        fs::write(seg0.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"23:30:00\",\"end\":\"23:30:01\",\"text\":\"t0\"}\n").unwrap();

        let resp1 = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let b1: Value =
            serde_json::from_slice(&to_bytes(resp1.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(b1["freshness"]["complete"], true);

        // After that read, unbumped edits on both days, no read in between
        let seg1_new = day1.join("field/234500_60");
        fs::create_dir_all(&seg1_new).unwrap();
        fs::write(seg1_new.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"23:45:00\",\"end\":\"23:45:01\",\"text\":\"t1_edit\"}\n").unwrap();

        let seg0_new = day0.join("field/234500_60");
        fs::create_dir_all(&seg0_new).unwrap();
        fs::write(seg0_new.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"23:45:00\",\"end\":\"23:45:01\",\"text\":\"t0_edit\"}\n").unwrap();

        // Advance to 2026-03-01T15:30:00Z (00:30 on 2026-03-02 in Tokyo)
        *now_ref.lock().unwrap() = Utc.with_ymd_and_hms(2026, 3, 1, 15, 30, 0).unwrap();

        let resp2 = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let b2: Value =
            serde_json::from_slice(&to_bytes(resp2.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(b2["freshness"]["complete"], true);

        let walk = crate::calendar::counts_by_walk(temp.path(), *now_ref.lock().unwrap()).unwrap();
        let expected_03: u64 = walk
            .iter()
            .filter(|(d, _)| d.starts_with("202603"))
            .map(|(_, c)| *c)
            .sum();
        let expected_02: u64 = walk
            .iter()
            .filter(|(d, _)| d.starts_with("202602"))
            .map(|(_, c)| *c)
            .sum();
        assert_eq!(b2["months"]["202603"], expected_03);
        assert_eq!(b2["months"]["202602"], expected_02);
    }

    #[tokio::test]
    async fn test_held_fill_pending_count() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock.clone(), || {
            Response::new(Body::empty())
        });

        let listed_days_map = day_dirs(temp.path()).unwrap();
        let total_days = listed_days_map.len();

        test_support::limit_inline_folds(identity, 1);
        let _hold = test_support::hold_background_fill(identity);

        let req = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["freshness"]["complete"], false);
        let behind = body["freshness"]["behind"].as_u64().unwrap();
        assert_eq!(behind as usize, total_days - 1);

        let snap = test_support::snapshot_worker_passes(identity);
        test_support::release_background_fill(identity);
        test_support::wait_worker_passes_after(identity, snap);

        // After release, both routes match and index is complete
        let req = Request::get("/app/transcripts/api/index")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let b: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            b["freshness"]["complete"], true,
            "index must become complete"
        );

        test_support::clear_recorded_notifications(identity);
        reset_calendar_while_held(temp.path());
        assert_eq!(
            test_support::recorded_notifications(identity),
            vec![("system".to_string(), "calendar".to_string())]
        );

        test_support::clear_recorded_notifications(identity);
        assert!(test_support::recorded_notifications(identity).is_empty());
        restart_wipe(identity);
        assert!(test_support::recorded_notifications(identity).is_empty());
    }

    #[tokio::test]
    async fn test_churn_invariance() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["identity"]["timezone"] = json!("UTC");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let today_path = temp.path().join("chronicle/20260801");
        fs::create_dir_all(&today_path).unwrap();
        let seg_path = today_path.join("field/100000_60");
        fs::create_dir_all(&seg_path).unwrap();
        fs::write(seg_path.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"10:00:00\",\"end\":\"10:00:01\",\"text\":\"test\"}\n").unwrap();

        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // A new line under today chronicle/<day>/health/
        let today_health = today_path.join("health");
        fs::create_dir_all(&today_health).unwrap();
        fs::write(today_health.join("churn.log"), b"churn line\n").unwrap();

        // And a new file in a today segment talents/
        let talents_dir = seg_path.join("talents");
        fs::create_dir_all(&talents_dir).unwrap();
        fs::write(talents_dir.join("talent.txt"), b"churn talent\n").unwrap();

        test_support::reset_seam_counters(identity);
        let resp = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let b: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(b["freshness"]["complete"], true);
        assert_eq!(
            test_support::compute_starts(identity),
            0,
            "no compute triggered by health or talents churn"
        );
    }

    // Windows file names are UTF-16, so this fixture cannot exist there.
    #[cfg(unix)]
    #[tokio::test]
    async fn test_error_non_utf8_segment() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        // Warm up the journal so other fixture days are already computed
        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let target_day = "20260309";
        let stream_dir = temp.path().join("chronicle").join(target_day).join("field");
        fs::create_dir_all(&stream_dir).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let mut bad_name = b"120000_60".to_vec();
            bad_name.push(0xFF);
            let bad_os = std::ffi::OsStr::from_bytes(&bad_name);
            let bad_seg = stream_dir.join(bad_os);
            // APFS rejects this fixture before the inventory can read it.
            match fs::create_dir_all(&bad_seg) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(nix::libc::EILSEQ) => return,
                Err(error) => panic!("create non-UTF-8 segment: {error}"),
            }
        }

        solstone_core_journal_io::bump_stream_marker(temp.path(), target_day).unwrap();

        test_support::reset_seam_counters(identity);
        for _ in 0..3 {
            let resp = app
                .clone()
                .oneshot(
                    Request::get("/app/transcripts/api/index")
                        .extension(root.clone())
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
        assert_eq!(
            test_support::compute_starts(identity),
            1,
            "only 1 compute start due to backoff"
        );

        let resp_m = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/stats/202603")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_m.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let m_bytes = to_bytes(resp_m.into_body(), usize::MAX).await.unwrap();
        let m_body: Value = serde_json::from_slice(&m_bytes).unwrap();
        assert_eq!(m_body["reason_code"], "internal_error");

        let resp_other = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/stats/202601")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_other.status(), StatusCode::OK);

        let mut found_health_line = false;
        if let Ok(entries) = fs::read_dir(temp.path().join("chronicle/20260309/health")) {
            for h_file in entries.flatten() {
                if let Ok(content) = fs::read_to_string(h_file.path())
                    && content.contains("chronicle day 20260309: count failed")
                {
                    assert!(
                        !content.contains(temp.path().to_str().unwrap()),
                        "health line must not contain path"
                    );
                    found_health_line = true;
                }
            }
        }
        assert!(found_health_line, "must log count failed health line");
    }

    #[tokio::test]
    async fn test_error_injected_compute_clears_inside_backoff() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let target_day = "20260310";
        test_support::set_injected_compute_error(identity, target_day, Some("temporary error"));

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Clear error without advancing the clock
        test_support::set_injected_compute_error(identity, target_day, None);

        test_support::reset_seam_counters(identity);
        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            test_support::compute_starts(identity),
            0,
            "index does not retry inside backoff"
        );

        let resp_m = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/stats/202603")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_m.status(), StatusCode::OK);
        let walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let b: Value =
            serde_json::from_slice(&to_bytes(resp_m.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(b[target_day], walk[target_day]);
    }

    #[tokio::test]
    async fn test_error_moving_identity_failing_day() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["identity"]["timezone"] = json!("UTC");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let target_day = "20260801";
        let target_path = temp.path().join("chronicle").join(target_day);
        fs::create_dir_all(&target_path).unwrap();
        test_support::set_injected_compute_error(identity, target_day, Some("persistent error"));

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        fn count_health_lines(journal: &Path, target_day: &str) -> usize {
            let mut count = 0;
            let health_dir = journal.join("chronicle").join(target_day).join("health");
            if let Ok(entries) = fs::read_dir(health_dir) {
                for h in entries.flatten() {
                    if let Ok(content) = fs::read_to_string(h.path()) {
                        for line in content.lines() {
                            if line.contains(&format!("chronicle day {target_day}: count failed")) {
                                count += 1;
                            }
                        }
                    }
                }
            }
            count
        }

        assert_eq!(
            count_health_lines(temp.path(), target_day),
            1,
            "one health line for episode"
        );

        // Identity moves while still failed
        let seg = target_path.join("170000_60");
        fs::create_dir_all(&seg).unwrap();
        fs::write(seg.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"17:00:00\",\"end\":\"17:00:01\",\"text\":\"moving\"}\n").unwrap();

        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            count_health_lines(temp.path(), target_day),
            1,
            "identity move while failed does not add line"
        );

        // A stored count ends the episode
        test_support::set_injected_compute_error(identity, target_day, None);
        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/stats/202608")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(count_health_lines(temp.path(), target_day), 1);

        // A later failure adds one more line
        test_support::set_injected_compute_error(identity, target_day, Some("second failure"));
        let seg2 = target_path.join("180000_60");
        fs::create_dir_all(&seg2).unwrap();
        fs::write(seg2.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"18:00:00\",\"end\":\"18:00:01\",\"text\":\"moving2\"}\n").unwrap();

        let _ = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            count_health_lines(temp.path(), target_day),
            2,
            "later failure adds one more line"
        );
    }

    #[tokio::test]
    async fn test_error_enotdir_health_marker() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let day_dir = temp.path().join("chronicle/20260327");
        let health_dir = day_dir.join("health");
        let _ = fs::remove_dir_all(&health_dir);
        fs::write(&health_dir, b"regular file instead of directory").unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let resp = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let b: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(b["freshness"]["complete"], true);
    }

    #[tokio::test]
    async fn test_error_vanishing_file() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let config_path = temp.path().join("config/journal.json");
        let mut config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
        config["identity"]["timezone"] = json!("UTC");
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let seg_dir = temp.path().join("chronicle/20260801/field/100000_60");
        fs::create_dir_all(&seg_dir).unwrap();
        let vanish_file = seg_dir.join("vanishing.jsonl");
        fs::write(&vanish_file, b"vanishing content").unwrap();

        test_support::install_before_file_stat(identity, move |p| {
            if p.ends_with("vanishing.jsonl") {
                let _ = fs::remove_file(p);
            }
        });

        let resp = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let b: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(b["freshness"]["complete"], true);
    }

    #[tokio::test]
    async fn empty_stream_marker_day_follows_bumps_and_the_hourly_pass() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let marker = temp.path().join("chronicle/20260416/health/stream.updated");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"").unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let month_count = |now: DateTime<Utc>| {
            let app = crate::router(temp.path().to_path_buf(), Clock::fixed(now), || {
                Response::new(Body::empty())
            });
            let root = root.clone();
            async move {
                let resp = app
                    .oneshot(
                        Request::get("/app/transcripts/api/stats/202604")
                            .extension(root)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                let body: Value =
                    serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap())
                        .unwrap();
                body["20260416"].as_u64().unwrap_or(0)
            }
        };
        let walked = |now: DateTime<Utc>| {
            crate::calendar::counts_by_walk(temp.path(), now)
                .unwrap()
                .get("20260416")
                .copied()
                .unwrap_or(0)
        };
        assert_eq!(month_count(now).await, walked(now));

        // A change that bumps is counted on the next read.
        let new_seg = temp.path().join("chronicle/20260416/field/190000_60");
        fs::create_dir_all(&new_seg).unwrap();
        fs::write(
            new_seg.join("mic_audio.jsonl"),
            b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"19:00:00\",\"end\":\"19:00:01\",\"text\":\"unbumped\"}\n",
        )
        .unwrap();
        solstone_core_journal_io::bump_stream_marker(temp.path(), "20260416").unwrap();
        let bumped = month_count(now).await;
        assert_eq!(bumped, walked(now));

        // Back to an empty marker: one that never bumps waits for the hourly pass,
        // like any other older day, rather than walking the tree on every read.
        fs::write(&marker, b"").unwrap();
        assert_eq!(month_count(now).await, bumped);
        let later_seg = temp.path().join("chronicle/20260416/field/210000_60");
        fs::create_dir_all(&later_seg).unwrap();
        fs::write(
            later_seg.join("mic_audio.jsonl"),
            b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"19:00:00\",\"end\":\"19:00:01\",\"text\":\"unbumped\"}\n",
        )
        .unwrap();
        assert_ne!(walked(now), bumped);
        assert_eq!(month_count(now).await, bumped);
        // The pass starts on an index read after the hour. A pass the first
        // read claimed may still be running on the worker; an index read
        // never starts a second one, so let that one finish first.
        wait_full_pass_released(identity);
        let later = now + chrono::Duration::hours(2);
        let index = crate::router(temp.path().to_path_buf(), Clock::fixed(later), || {
            Response::new(Body::empty())
        })
        .oneshot(
            Request::get("/app/transcripts/api/index")
                .extension(root.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(month_count(later).await, walked(later));

        // Nothing was set aside: reading never writes.
        let health = marker.parent().unwrap();
        assert!(
            fs::read_dir(health)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().contains("wedged"))
        );
    }

    #[tokio::test]
    async fn test_malformed_stream_updated() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let marker = temp.path().join("chronicle/20260416/health/stream.updated");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"{ not json at all }").unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let resp = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let b: Value =
            serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(b["freshness"]["complete"], true);

        let walk_before = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let before_count = walk_before.get("20260416").copied().unwrap_or(0);

        // Add an unbumped change on this old day with a malformed marker
        let new_seg = temp.path().join("chronicle/20260416/field/190000_60");
        fs::create_dir_all(&new_seg).unwrap();
        fs::write(
            new_seg.join("mic_audio.jsonl"),
            b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"19:00:00\",\"end\":\"19:00:01\",\"text\":\"unbumped\"}\n",
        )
        .unwrap();

        let resp2 = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let b2: Value =
            serde_json::from_slice(&to_bytes(resp2.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(b2["freshness"]["complete"], true);

        let walk_after = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let after_count = walk_after.get("20260416").copied().unwrap_or(0);
        assert_ne!(before_count, after_count);

        let req_stats = Request::get("/app/transcripts/api/stats/202604")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp_stats = app.clone().oneshot(req_stats).await.unwrap();
        assert_eq!(resp_stats.status(), StatusCode::OK);
        let stats_val: Value =
            serde_json::from_slice(&to_bytes(resp_stats.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(stats_val["20260416"], json!(after_count));
    }

    fn full_pass_finish(identity: ObjectIdentity) -> Option<DateTime<Utc>> {
        let reg = INVENTORIES.lock().expect("inventories lock");
        let inv = reg
            .get(&identity)
            .expect("inventory")
            .lock()
            .expect("inventory lock");
        inv.last_full_pass_finish
    }

    #[tokio::test]
    async fn test_month_stats_does_not_stamp_without_folding() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now_ref = Arc::new(Mutex::new(
            Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap(),
        ));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let first = index_body(&app, &root).await;
        assert_eq!(first["freshness"]["complete"], true);
        let before = first["months"]["202601"].as_u64().unwrap();
        let stamped = full_pass_finish(identity).expect("initial pass stamped");

        *now_ref.lock().unwrap() += chrono::Duration::hours(2);
        write_browser_segment(temp.path(), "20260101", "234000_60");

        let _hold = test_support::hold_background_fill(identity);
        let month = index_month_stats(&app, &root, "202603").await;
        assert!(month.is_object());
        assert_eq!(full_pass_finish(identity), Some(stamped));

        let snap = test_support::snapshot_worker_passes(identity);
        test_support::release_background_fill(identity);
        test_support::wait_worker_passes_after(identity, snap);

        let after = index_body(&app, &root).await;
        assert_ne!(after["months"]["202601"].as_u64().unwrap(), before);
        assert!(full_pass_finish(identity).unwrap() > stamped);
    }

    #[tokio::test]
    async fn test_digest_uses_raw_segment_dir_names() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"},"convey":{"password_hash":"","secret":"sec","trust_localhost":true}}"#,
        )
        .unwrap();
        let root = JournalRoot::open(temp.path()).unwrap();
        let day = temp.path().join("chronicle/20200101");
        fs::create_dir_all(day.join("120000_60")).unwrap();
        fs::create_dir_all(day.join("120000_60_extra")).unwrap();
        fs::write(day.join("120000_60/note.txt"), b"a").unwrap();
        fs::write(day.join("120000_60_extra/note.txt"), b"a").unwrap();

        let digest = read_tree_digest(temp.path(), "20200101", root.identity());
        let segment_dirs: Vec<&str> = digest
            .entries
            .iter()
            .filter_map(|entry| match entry {
                FileDigestEntry::SegmentDir {
                    stream,
                    segment_dir,
                } if stream.is_empty() => Some(segment_dir.as_str()),
                _ => None,
            })
            .collect();
        assert!(segment_dirs.contains(&"120000_60"));
        assert!(
            segment_dirs.contains(&"120000_60_extra"),
            "raw directory name must stay in the digest: {segment_dirs:?}"
        );
        assert!(digest.entries.iter().any(|entry| matches!(
            entry,
            FileDigestEntry::Present { segment_dir, .. } if segment_dir == "120000_60_extra"
        )));
    }

    #[tokio::test]
    async fn test_ac4_markers_placement_and_clock() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"},"convey":{"password_hash":"","secret":"sec","trust_localhost":true}}"#,
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let t0 = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let now_ref = Arc::new(Mutex::new(t0));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let today = temp.path().join("chronicle/20260801");
        write_analyzed_audio(&today.join("field/100000_60"), "analyzed");
        let raw = today.join("field/140000_60");
        fs::create_dir_all(&raw).unwrap();
        fs::write(raw.join("mic.flac"), b"raw").unwrap();
        fs::write(raw.join(".analyzing_audio"), b"{}\n").unwrap();
        let marker_time =
            filetime::FileTime::from_unix_time(t0.timestamp(), t0.timestamp_subsec_nanos());
        filetime::set_file_mtime(raw.join(".analyzing_audio"), marker_time).unwrap();

        let baseline = index_body(&app, &root).await;
        assert_eq!(baseline["freshness"]["complete"], true);
        let count = baseline["months"]["202608"].as_u64().unwrap();
        assert!(count > 0);

        fs::write(today.join("field/100000_60/.analyzing_audio"), b"{}\n").unwrap();
        filetime::set_file_mtime(today.join("field/100000_60/.analyzing_audio"), marker_time)
            .unwrap();
        assert_eq!(index_body(&app, &root).await["months"]["202608"], count);

        let placed = today.join("field/100000_60");
        fs::write(placed.join("p0000_0123456789abcdef.flac"), b"raw-part").unwrap();
        fs::write(
            placed.join("p0000_0123456789abcdef.jsonl"),
            b"{\"note\":\"no terminal proof\"}\n",
        )
        .unwrap();
        fs::write(
            placed.join("placement.json"),
            br#"{"version":1,"parts":[{"stored_name":"p0000_0123456789abcdef.flac","original_occurrence":"rec","origin_ms":0,"offset_ms":0,"length_ms":1000}]}"#,
        )
        .unwrap();
        assert_eq!(index_body(&app, &root).await["months"]["202608"], count);

        let marker_only = today.join("field/130000_60");
        fs::create_dir_all(&marker_only).unwrap();
        fs::write(
            marker_only.join("stream.json"),
            b"{\"stream\":\"field\",\"seq\":1}\n",
        )
        .unwrap();
        assert_eq!(index_body(&app, &root).await["months"]["202608"], count);

        // Past analyzing-stale (1800s), modality-input age (12h), and the 3-day sense age.
        *now_ref.lock().unwrap() = t0 + chrono::Duration::days(4);
        let aged = index_body(&app, &root).await;
        assert_eq!(aged["freshness"]["complete"], true);
        assert_eq!(aged["months"]["202608"], count);
        let next = index_body(&app, &root).await;
        assert_eq!(next["months"]["202608"], count);
    }

    #[tokio::test]
    async fn test_ac9_one_day_envelope_per_count_change() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"},"convey":{"password_hash":"","secret":"sec","trust_localhost":true}}"#,
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let day = "20260101";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(day)
                .join("field/100000_60"),
            "seed",
        );
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let first = index_body(&app, &root).await;
        assert_eq!(first["freshness"]["complete"], true);
        let before = first["months"]["202601"].as_u64().unwrap();
        assert!(
            test_support::recorded_notifications(identity).is_empty(),
            "cold fill publishes no day envelope"
        );

        solstone_core_journal_io::bump_stream_marker(temp.path(), day).unwrap();
        test_support::clear_recorded_notifications(identity);
        let unchanged = index_body(&app, &root).await;
        assert_eq!(unchanged["months"]["202601"], before);
        assert!(
            test_support::recorded_notifications(identity).is_empty(),
            "unchanged fold publishes no day envelope"
        );

        write_browser_segment(temp.path(), day, "150000_60");
        solstone_core_journal_io::bump_stream_marker(temp.path(), day).unwrap();
        test_support::clear_recorded_notifications(identity);
        let changed = index_body(&app, &root).await;
        assert_ne!(changed["months"]["202601"].as_u64().unwrap(), before);
        assert_eq!(
            test_support::recorded_notifications(identity),
            vec![("day".to_string(), day.to_string())]
        );

        test_support::clear_recorded_notifications(identity);
        let next = index_body(&app, &root).await;
        assert_eq!(next["months"]["202601"], changed["months"]["202601"]);
        assert!(test_support::recorded_notifications(identity).is_empty());
    }

    #[tokio::test]
    async fn test_failed_day_does_not_block_hourly_refold() {
        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let good_day = "20260101";
        let bad_day = "20260102";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(good_day)
                .join("field/100000_60"),
            "good",
        );
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(bad_day)
                .join("field/120000_60"),
            "failed",
        );
        test_support::set_injected_compute_error(identity, bad_day, Some("persistent error"));

        let t0 = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let now_ref = Arc::new(Mutex::new(t0));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let seeded = index_body(&app, &root).await;
        assert_eq!(seeded["freshness"]["complete"], false);
        assert_eq!(seeded["freshness"]["failed"].as_u64(), Some(1));
        let before = seeded["months"]["202601"].as_u64().unwrap();
        assert_eq!(
            before,
            crate::calendar::day_range_count(temp.path(), good_day, t0).unwrap()
        );
        let stamped = full_pass_finish(identity).expect("failed day still settles the pass");

        write_browser_segment(temp.path(), good_day, "235000_60");
        *now_ref.lock().unwrap() = t0 + chrono::Duration::seconds(1800);
        let hidden = index_body(&app, &root).await;
        assert_eq!(hidden["months"]["202601"].as_u64(), Some(before));
        assert_eq!(hidden["freshness"]["failed"].as_u64(), Some(1));
        assert_eq!(full_pass_finish(identity), Some(stamped));

        *now_ref.lock().unwrap() = t0 + chrono::Duration::seconds(3700);
        let corrected = index_body(&app, &root).await;
        let expected =
            crate::calendar::day_range_count(temp.path(), good_day, *now_ref.lock().unwrap())
                .unwrap();
        assert_ne!(expected, before);
        assert_eq!(corrected["months"]["202601"].as_u64(), Some(expected));
        assert_eq!(corrected["freshness"]["complete"], false);
        assert_eq!(corrected["freshness"]["failed"].as_u64(), Some(1));
        assert!(full_pass_finish(identity).unwrap() > stamped);

        let next = index_body(&app, &root).await;
        assert_eq!(next["months"]["202601"].as_u64(), Some(expected));
        assert_eq!(next["freshness"]["failed"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn test_worker_full_pass_stamps_while_failed_day_in_backoff() {
        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let failed_day = "20260201";
        let good_day = "20260101";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(failed_day)
                .join("field/100000_60"),
            "failed",
        );
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(good_day)
                .join("field/100000_60"),
            "good",
        );
        test_support::set_injected_compute_error(identity, failed_day, Some("injected"));
        test_support::limit_inline_folds(identity, 1);

        let t0 = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let now_ref = Arc::new(Mutex::new(t0));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let snap = test_support::snapshot_worker_passes(identity);
        let _seeded = index_body(&app, &root).await;
        test_support::wait_worker_passes_after(identity, snap);
        let stamped = full_pass_finish(identity)
            .expect("worker pass stamps while the newest day is in backoff");
        assert_eq!(stamped, t0);
        let passes = test_support::snapshot_worker_passes(identity);
        assert_eq!(passes, snap + 1);
        assert!(!full_pass_claimed(identity));

        *now_ref.lock().unwrap() = t0 + chrono::Duration::minutes(10);
        let _inside_hour = index_body(&app, &root).await;
        assert_eq!(full_pass_finish(identity), Some(stamped));
        assert_eq!(test_support::snapshot_worker_passes(identity), passes);
        assert!(!full_pass_claimed(identity));

        let new_day = "20260202";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(new_day)
                .join("field/100000_60"),
            "arrived",
        );
        test_support::clear_recorded_notifications(identity);
        let _arrived = index_body(&app, &root).await;
        assert_eq!(
            test_support::recorded_notifications(identity),
            vec![("day".to_string(), new_day.to_string())]
        );
        assert_eq!(full_pass_finish(identity), Some(stamped));
        assert_eq!(test_support::snapshot_worker_passes(identity), passes);
    }

    fn calendar_registered(identity: ObjectIdentity) -> (bool, bool, bool) {
        (
            INVENTORIES.lock().unwrap().contains_key(&identity),
            WORKERS.lock().unwrap().contains_key(&identity),
            WATCHERS.lock().unwrap().contains_key(&identity),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_stop_between_days_retires_and_joins_the_calendar() {
        use solstone_core_journal_io::whole_journal_restore::{
            hold_target_for_restore, release_target_for_restore, wait_for_target_use_to_end,
        };

        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);
        for day in ["20260101", "20260102", "20260103"] {
            write_analyzed_audio(
                &temp
                    .path()
                    .join("chronicle")
                    .join(day)
                    .join("field/100000_60"),
                day,
            );
        }
        // The read folds the newest day; the worker takes the other two and
        // parks after the first of them.
        test_support::limit_inline_folds(identity, 1);
        let parked = lifecycle_hooks::hold_worker_between_days(identity);

        let t0 = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::new(move || t0);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });
        let _first = index_body(&app, &root).await;
        note_dashboard_stream(temp.path(), true);
        parked.wait_arrived();
        let folds_when_parked = test_support::compute_starts(identity);
        assert_eq!(calendar_registered(identity), (true, true, true));

        // Stop: hold, drain, retire, release. Retirement must wait for the
        // parked worker rather than leave it running.
        let journal = temp.path().to_path_buf();
        let stopping = std::thread::spawn(move || {
            hold_target_for_restore(&journal);
            wait_for_target_use_to_end(&journal);
            retire_calendar(&journal);
            lifecycle_hooks::note_retire_event(identity, RetireEvent::Returned);
            release_target_for_restore(&journal);
        });
        // While the worker is parked, retirement can only be waiting for it.
        // Returning first would leave the worker free to take the journal
        // after the hold is released.
        let first = lifecycle_hooks::wait_retire_event(identity);
        assert_eq!(
            first,
            vec![RetireEvent::Joining],
            "retirement must wait for the parked worker before it returns"
        );
        parked.release();
        stopping.join().expect("stop sequence");

        // Nothing it owned runs again, and nothing brings it back on its own.
        assert_eq!(calendar_registered(identity), (false, false, false));
        assert_eq!(test_support::compute_starts(identity), folds_when_parked);
        notify_inventory_worker(identity);
        tick_journal(&root, &Clock::new(move || t0 + chrono::Duration::hours(2)));
        note_dashboard_stream(temp.path(), false);
        assert_eq!(calendar_registered(identity), (false, false, false));
        assert_eq!(test_support::compute_starts(identity), folds_when_parked);

        // A request on the same root opens a fresh calendar that counts every day.
        test_support::limit_inline_folds(identity, 10);
        let reopened = index_body(&app, &root).await;
        assert_eq!(reopened["freshness"], json!({"complete": true}));
        let months = reopened["months"]["202601"]
            .as_u64()
            .expect("202601 months");
        let walk: u64 = ["20260101", "20260102", "20260103"]
            .iter()
            .map(|d| crate::calendar::day_range_count(temp.path(), d, t0).unwrap())
            .sum();
        assert_eq!(months, walk);
        assert_eq!(calendar_registered(identity), (true, true, false));
    }

    #[tokio::test]
    async fn test_skipped_refold_releases_full_pass_claim() {
        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let good_day = "20260102";
        let failed_day = "20260101";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(good_day)
                .join("field/100000_60"),
            "good",
        );
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(failed_day)
                .join("field/100000_60"),
            "failed",
        );
        test_support::set_injected_compute_error(identity, failed_day, Some("injected"));

        let t0 = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let now_ref = Arc::new(Mutex::new(t0));
        let now_c = now_ref.clone();
        let clock = Clock::new(move || *now_c.lock().unwrap());
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        let seeded = index_body(&app, &root).await;
        assert_eq!(seeded["freshness"]["failed"].as_u64(), Some(1));
        let before = seeded["months"]["202601"].as_u64().unwrap();
        let stamped = full_pass_finish(identity).expect("seed pass stamps");
        assert_eq!(stamped, t0);

        let at_hour = t0 + chrono::Duration::seconds(7200);
        *now_ref.lock().unwrap() = at_hour;

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();
        let good_day_s = good_day.to_string();
        test_support::install_barrier_after_compute(identity, move |day| {
            if day == good_day_s && !fired_c.swap(true, Ordering::SeqCst) {
                let _ = held_tx.send(());
                true
            } else {
                false
            }
        });

        let fold_root = root.clone();
        let fold_journal = temp.path().to_path_buf();
        let fold_inv = get_or_create_inventory(&root, identity);
        let fold_day = good_day.to_string();
        let fold_thread = std::thread::spawn(move || {
            assert!(
                fold_single_day(
                    &fold_root,
                    &fold_journal,
                    &fold_day,
                    false,
                    at_hour,
                    next_generation(),
                    &fold_inv,
                    false,
                )
                .is_ok(),
                "direct fold aborted"
            );
        });
        held_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("direct fold did not park after compute");

        let app_c = app.clone();
        let root_c = root.clone();
        let index_task = tokio::spawn(async move { index_body(&app_c, &root_c).await });
        // This runtime is current-thread. Waiting here would stall the index read.
        let identity_wait = identity;
        let waiting = tokio::task::spawn_blocking(move || {
            test_support::wait_waiter_count(
                identity_wait,
                |count| count >= 1,
                Duration::from_secs(5),
            )
        })
        .await
        .unwrap();
        assert!(
            waiting >= 1,
            "inline full pass did not wait on the in-flight day: {waiting}"
        );
        test_support::release_compute_barrier(identity);
        fold_thread.join().unwrap();
        let skipped = index_task.await.unwrap();

        assert_eq!(skipped["months"]["202601"].as_u64(), Some(before));
        assert_eq!(full_pass_finish(identity), Some(stamped));
        assert!(
            !full_pass_claimed(identity),
            "skipped full pass must release the claim"
        );

        write_browser_segment(temp.path(), good_day, "235000_60");
        *now_ref.lock().unwrap() = at_hour + chrono::Duration::seconds(3700);
        let corrected = index_body(&app, &root).await;
        let expected =
            crate::calendar::day_range_count(temp.path(), good_day, *now_ref.lock().unwrap())
                .unwrap();
        assert_ne!(expected, before);
        assert_eq!(corrected["months"]["202601"].as_u64(), Some(expected));
        assert!(full_pass_finish(identity).unwrap() > stamped);
    }

    #[test]
    fn test_two_waiters_one_further_fold() {
        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let day = "20260101";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(day)
                .join("field/100000_60"),
            "seed",
        );
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let inv_arc = get_or_create_inventory(&root, identity);

        let hooks = Arc::new(AtomicUsize::new(0));
        let (parked_tx, parked_rx) = std::sync::mpsc::sync_channel(8);
        let hooks_c = hooks.clone();
        let day_s = day.to_string();
        test_support::install_barrier_after_identity_read(identity, move |name| {
            if name == day_s {
                let n = hooks_c.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = parked_tx.send(n);
                true
            } else {
                false
            }
        });

        let owner_root = root.clone();
        let owner_journal = temp.path().to_path_buf();
        let owner_inv = inv_arc.clone();
        let owner_day = day.to_string();
        let owner = std::thread::spawn(move || {
            assert!(
                fold_single_day(
                    &owner_root,
                    &owner_journal,
                    &owner_day,
                    false,
                    now,
                    next_generation(),
                    &owner_inv,
                    false,
                )
                .is_ok(),
                "owner fold aborted"
            );
        });
        assert_eq!(
            parked_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            1,
            "first fold parks after its identity read"
        );

        let spawn_waiter = || {
            let waiter_root = root.clone();
            let waiter_journal = temp.path().to_path_buf();
            let waiter_inv = inv_arc.clone();
            let waiter_day = day.to_string();
            std::thread::spawn(move || {
                assert!(
                    fold_single_day(
                        &waiter_root,
                        &waiter_journal,
                        &waiter_day,
                        false,
                        now,
                        next_generation(),
                        &waiter_inv,
                        false,
                    )
                    .is_ok(),
                    "waiter fold aborted"
                );
            })
        };
        let waiter_a = spawn_waiter();
        let waiter_b = spawn_waiter();

        let parked =
            test_support::wait_waiter_count(identity, |count| count >= 2, Duration::from_secs(5));
        assert!(parked >= 2, "waiters did not park: {parked}");

        write_browser_segment(temp.path(), day, "235000_60");
        solstone_core_journal_io::bump_stream_marker(temp.path(), day).unwrap();
        test_support::release_identity_barrier(identity);

        let second = parked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(second, 2, "more than one further fold started: {second}");
        let waiting =
            test_support::wait_waiter_count(identity, |count| count == 1, Duration::from_secs(5));
        assert_eq!(
            waiting, 1,
            "second fold did not single-flight: hooks={second} waiters={waiting}"
        );

        test_support::release_identity_barrier(identity);
        owner.join().unwrap();
        waiter_a.join().unwrap();
        waiter_b.join().unwrap();
        assert_eq!(hooks.load(Ordering::SeqCst), 2);
        assert_eq!(test_support::compute_starts(identity), 2);
    }

    #[tokio::test]
    async fn test_warm_inventory_publishes_new_day_envelope() {
        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let first_day = "20260101";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(first_day)
                .join("field/100000_60"),
            "seed",
        );
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let app = crate::router(temp.path().to_path_buf(), Clock::fixed(now), || {
            Response::new(Body::empty())
        });

        let seeded = index_body(&app, &root).await;
        assert_eq!(seeded["freshness"]["complete"], true);
        assert!(full_pass_finish(identity).is_some());
        assert!(
            test_support::recorded_notifications(identity).is_empty(),
            "cold fill publishes no day envelope"
        );

        let new_day = "20260102";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(new_day)
                .join("field/100000_60"),
            "arrived",
        );
        test_support::clear_recorded_notifications(identity);
        let body = index_body(&app, &root).await;
        assert!(
            body["months"]["202601"].as_u64().unwrap()
                > seeded["months"]["202601"].as_u64().unwrap()
        );
        assert_eq!(
            test_support::recorded_notifications(identity),
            vec![("day".to_string(), new_day.to_string())]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_chronicle_stat_error_is_not_an_empty_listing() {
        use std::os::unix::fs::symlink;

        let (temp, root) = setup_minimal_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let day = "20260101";
        write_analyzed_audio(
            &temp
                .path()
                .join("chronicle")
                .join(day)
                .join("field/100000_60"),
            "seed",
        );
        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let app = crate::router(temp.path().to_path_buf(), Clock::fixed(now), || {
            Response::new(Body::empty())
        });
        let seeded = index_body(&app, &root).await;
        assert_eq!(seeded["freshness"]["complete"], true);
        let real_count = seeded["months"]["202601"].as_u64().unwrap();
        let poisoned = 424_242;
        assert_ne!(real_count, poisoned);
        {
            let reg = INVENTORIES.lock().expect("inventories lock");
            let mut inv = reg
                .get(&identity)
                .expect("inventory")
                .lock()
                .expect("inventory lock");
            inv.rows.get_mut(day).expect("day row").state = DayFoldState::Computed(poisoned);
        }
        let stamped = full_pass_finish(identity);

        struct RestoreChronicle {
            journal: std::path::PathBuf,
            armed: bool,
        }
        impl Drop for RestoreChronicle {
            fn drop(&mut self) {
                if !self.armed {
                    return;
                }
                let chronicle = self.journal.join("chronicle");
                let bak = self.journal.join("chronicle.bak");
                let _ = fs::remove_file(&chronicle);
                if bak.exists() {
                    let _ = fs::rename(&bak, &chronicle);
                }
            }
        }
        let chronicle = temp.path().join("chronicle");
        let bak = temp.path().join("chronicle.bak");
        let mut restore = RestoreChronicle {
            journal: temp.path().to_path_buf(),
            armed: false,
        };
        fs::rename(&chronicle, &bak).unwrap();
        symlink("chronicle", &chronicle).unwrap();
        restore.armed = true;

        let broken = index_body(&app, &root).await;
        assert_eq!(broken["freshness"]["complete"], false);
        assert_eq!(full_pass_finish(identity), stamped);

        let month = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/stats/202601")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(month.status(), StatusCode::OK);

        fs::remove_file(&chronicle).unwrap();
        fs::rename(&bak, &chronicle).unwrap();
        restore.armed = false;

        let after = index_body(&app, &root).await;
        assert_eq!(
            after["months"]["202601"].as_u64(),
            Some(poisoned),
            "a chronicle stat error must not delete rows"
        );
    }

    async fn index_month_stats(app: &axum::Router, root: &Arc<JournalRoot>, month: &str) -> Value {
        let req = Request::get(format!("/app/transcripts/api/stats/{month}"))
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap()
    }

    // Offload does not change the count because raw removal leaves the jsonl, so modality state remains Analyzed.
    #[tokio::test]
    async fn test_parity_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let config_path = temp.path().join("config/journal.json");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let config = json!({
            "identity": { "timezone": "UTC" },
            "convey": { "password_hash": "", "secret": "sec", "trust_localhost": true }
        });
        fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();

        // Small journal, at least four days, one older than yesterday, with audio, screen, and browser.
        let days = ["20260101", "20260102", "20260702", "20260703"];
        for d in days {
            let day_dir = temp.path().join("chronicle").join(d);
            fs::create_dir_all(&day_dir).unwrap();
        }

        // 20260101: audio
        let a_seg = temp.path().join("chronicle/20260101/field/100000_60");
        fs::create_dir_all(&a_seg).unwrap();
        fs::write(a_seg.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"10:00:00\",\"end\":\"10:00:01\",\"text\":\"a\"}\n").unwrap();
        let retain_seg = temp.path().join("chronicle/20260101/field/120000_60");
        fs::create_dir_all(&retain_seg).unwrap();
        fs::write(retain_seg.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"12:00:00\",\"end\":\"12:00:01\",\"text\":\"retain\"}\n").unwrap();

        // 20260102: screen
        let s_seg = temp.path().join("chronicle/20260102/field/110000_60");
        fs::create_dir_all(&s_seg).unwrap();
        fs::write(s_seg.join("screen.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"timestamp\":60,\"analysis\":{\"visible\":\"browser\"}}\n").unwrap();

        // 20260702: browser
        let b_seg = temp
            .path()
            .join("chronicle/20260702/workstation.browser/100000_60");
        fs::create_dir_all(&b_seg).unwrap();
        fs::write(
            b_seg.join("stream.json"),
            b"{\"stream\":\"workstation.browser\",\"seq\":1}\n",
        )
        .unwrap();
        fs::write(b_seg.join("browser_page.jsonl"), b"{\"t\":\"segment_start\",\"ts\":1783006230000,\"rel\":30000,\"site\":\"example.com\",\"url\":\"https://example.com\",\"title\":\"T\",\"adapter\":\"docs\",\"ctx\":\"doc\",\"n\":1,\"blocks\":[]}\n").unwrap();

        // 20260703: audio
        let a2_seg = temp.path().join("chronicle/20260703/field/100000_60");
        fs::create_dir_all(&a2_seg).unwrap();
        fs::write(a2_seg.join("mic_audio.jsonl"), b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"10:00:00\",\"end\":\"10:00:01\",\"text\":\"a2\"}\n").unwrap();

        let now = Utc.with_ymd_and_hms(2026, 7, 3, 18, 0, 0).unwrap();
        let walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();

        let future_time = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() + std::time::Duration::from_secs(100),
        );
        for d in days {
            let dir = temp.path().join("chronicle").join(d);
            let mut scan = solstone_core_journal_stats_cli::DayScan::default();
            scan.schema_version = solstone_core_journal_stats_cli::SCHEMA_VERSION;
            scan.stats.transcript_ranges =
                walk.get(d).copied().unwrap_or(0).saturating_add(1_000_000);
            let stats_path = dir.join("stats.json");
            solstone_core_journal_io::write_json(
                &stats_path,
                &scan,
                solstone_core_journal_io::JsonWriteOptions::default(),
            )
            .unwrap();
            filetime::set_file_mtime(&stats_path, future_time).unwrap();
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(
                temp.path().join("chronicle/20260101/stats.json"),
                fs::Permissions::from_mode(0o000),
            );
        }

        let clock = Clock::fixed(now);
        let app = crate::router_with_delete_window(
            temp.path().to_path_buf(),
            clock.clone(),
            || Response::new(Body::empty()),
            Duration::ZERO,
        );

        async fn assert_matches(
            app: &axum::Router,
            root: &Arc<JournalRoot>,
            temp: &Path,
            now: DateTime<Utc>,
        ) -> BTreeMap<String, u64> {
            let mut aside_paths = Vec::new();
            if let Ok(entries) = fs::read_dir(temp.join("chronicle")) {
                for entry in entries.flatten() {
                    let stats_path = entry.path().join("stats.json");
                    let aside_path = entry.path().join("stats.json.aside");
                    if stats_path.is_file() {
                        let _ = fs::rename(&stats_path, &aside_path);
                        aside_paths.push((stats_path, aside_path));
                    }
                }
            }
            let walk = crate::calendar::counts_by_walk(temp, now).unwrap();
            for (stats_path, aside_path) in aside_paths {
                let _ = fs::rename(&aside_path, &stats_path);
            }
            let req = Request::get("/app/transcripts/api/index")
                .extension(root.clone())
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["freshness"], json!({"complete": true}));

            for month in body["months"].as_object().unwrap().keys() {
                let req_m = Request::get(format!("/app/transcripts/api/stats/{month}"))
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap();
                let resp_m = app.clone().oneshot(req_m).await.unwrap();
                assert_eq!(resp_m.status(), StatusCode::OK);
                let m_bytes = to_bytes(resp_m.into_body(), usize::MAX).await.unwrap();
                let m_val: Value = serde_json::from_slice(&m_bytes).unwrap();
                let expected: BTreeMap<String, u64> = walk
                    .iter()
                    .filter(|(d, c)| d.starts_with(month) && **c > 0)
                    .map(|(d, c)| (d.clone(), *c))
                    .collect();
                assert_eq!(m_val, serde_json::to_value(expected).unwrap());
            }
            walk
        }

        // An offload raw restore does not change day_range_count (a raw file removed and written back left the count unchanged), so this fixture does not run offload.

        let initial_walk = assert_matches(&app, &root, temp.path(), now).await;
        assert!(initial_walk.get("20260702").copied().unwrap_or(0) > 0);
        assert!(initial_walk.get("20260703").copied().unwrap_or(0) > 0);

        // Event 1: a new raw-only segment and a browser segment on an old day
        let before_1 = initial_walk.get("20260102").copied().unwrap_or(0);
        let raw_seg = temp.path().join("chronicle/20260102/field/140000_60");
        fs::create_dir_all(&raw_seg).unwrap();
        fs::write(raw_seg.join("mic.flac"), b"raw audio bytes").unwrap();

        let new_b_seg = temp
            .path()
            .join("chronicle/20260102/workstation.browser/150000_60");
        fs::create_dir_all(&new_b_seg).unwrap();
        fs::write(
            new_b_seg.join("stream.json"),
            b"{\"stream\":\"workstation.browser\",\"seq\":2}\n",
        )
        .unwrap();
        fs::write(new_b_seg.join("browser_page.jsonl"), b"{\"t\":\"segment_start\",\"ts\":1783006240000,\"rel\":40000,\"site\":\"example.com\",\"url\":\"https://example.com\",\"title\":\"T2\",\"adapter\":\"docs\",\"ctx\":\"doc\",\"n\":1,\"blocks\":[]}\n").unwrap();
        let mut scan1 = solstone_core_journal_stats_cli::DayScan::default();
        scan1.schema_version = solstone_core_journal_stats_cli::SCHEMA_VERSION;
        scan1.stats.transcript_ranges = 999_999;
        let stats_path1 = temp.path().join("chronicle/20260102/stats.json");
        solstone_core_journal_io::write_json(
            &stats_path1,
            &scan1,
            solstone_core_journal_io::JsonWriteOptions::default(),
        )
        .unwrap();
        filetime::set_file_mtime(&stats_path1, future_time).unwrap();
        solstone_core_journal_io::bump_stream_marker(temp.path(), "20260102").unwrap();
        let walk1 = assert_matches(&app, &root, temp.path(), now).await;
        let after_1 = walk1.get("20260102").copied().unwrap_or(0);
        assert_ne!(before_1, after_1);

        // Event 2: segment delete through DELETE /app/transcripts/api/segment/{day}/{stream}/{segment_key}
        let before_2 = walk1.get("20260102").copied().unwrap_or(0);
        let del_req = Request::builder()
            .method("DELETE")
            .uri("/app/transcripts/api/segment/20260102/workstation.browser/150000_60")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let del_resp = app.clone().oneshot(del_req).await.unwrap();
        assert!(del_resp.status().is_success());
        let walk2 = assert_matches(&app, &root, temp.path(), now).await;
        let after_2 = walk2.get("20260102").copied().unwrap_or(0);
        assert_ne!(before_2, after_2);

        // Event 3: retention removal through solstone_core_retention::door::remove_segments
        let before_3 = walk2.get("20260101").copied().unwrap_or(0);
        let target = solstone_core_retention::Target {
            day: "20260101".to_owned(),
            stream: "field".to_owned(),
            dir: "120000_60".to_owned(),
        };
        let _ = solstone_core_retention::door::remove_segments(
            temp.path(),
            std::slice::from_ref(&target),
            "2026-07-02T22:00:00Z",
            solstone_core_retention::RemovalReason::OwnerSegmentDelete,
            "sha256:abc",
        );
        let walk3 = assert_matches(&app, &root, temp.path(), now).await;
        let after_3 = walk3.get("20260101").copied().unwrap_or(0);
        assert_ne!(before_3, after_3);

        // Event 4: solstone_core_segment::relocate_segment across two days
        let before_4_src = walk3.get("20260101").copied().unwrap_or(0);
        let before_4_dst = walk3.get("20260102").copied().unwrap_or(0);
        // Relocation admits the canonical journal path, which on Windows is
        // the verbatim form; join component by component so it stays valid.
        #[cfg(windows)]
        let journal = fs::canonicalize(temp.path()).unwrap();
        #[cfg(not(windows))]
        let journal = temp.path().to_path_buf();
        let src_dir = journal
            .join("chronicle")
            .join("20260101")
            .join("field")
            .join("100000_60");
        let dst_dir = journal
            .join("chronicle")
            .join("20260102")
            .join("field")
            .join("100000_60");
        let source = solstone_core_segment::RelocationEnd {
            day: "20260101".to_string(),
            segment: "100000_60".to_string(),
            path: src_dir.clone(),
            disk_rel: "chronicle/20260101/field/100000_60".to_string(),
            parent_rel: "chronicle/20260101/field".to_string(),
        };
        let destination = solstone_core_segment::RelocationEnd {
            day: "20260102".to_string(),
            segment: "100000_60".to_string(),
            path: dst_dir.clone(),
            disk_rel: "chronicle/20260102/field/100000_60".to_string(),
            parent_rel: "chronicle/20260102/field".to_string(),
        };
        let relocation = solstone_core_segment::Relocation {
            journal: &journal,
            source: &source,
            destination: &destination,
            successor: None,
            stream: "field",
            sequence: 1,
            locks: solstone_core_segment::LockOptions::default(),
        };
        let outcome = solstone_core_segment::relocate_segment(&relocation).unwrap();
        for day in &outcome.mutated_days {
            solstone_core_journal_io::bump_stream_marker(temp.path(), day).unwrap();
        }
        let walk4 = assert_matches(&app, &root, temp.path(), now).await;
        let after_4_src = walk4.get("20260101").copied().unwrap_or(0);
        let after_4_dst = walk4.get("20260102").copied().unwrap_or(0);
        assert_ne!(before_4_src, after_4_src);
        assert_ne!(before_4_dst, after_4_dst);

        // Restart leaves the count unchanged. Restore ordering is the integration
        // test, where only one copy of this crate is linked.
        test_support::clear_recorded_notifications(identity);
        restart_wipe(identity);
        assert!(test_support::recorded_notifications(identity).is_empty());
        let walk_restart = assert_matches(&app, &root, temp.path(), now).await;
        assert_eq!(walk4, walk_restart);
        assert!(walk_restart.get("20260702").copied().unwrap_or(0) > 0);
        assert!(walk_restart.get("20260703").copied().unwrap_or(0) > 0);

        // Host import: process_transcript in-process
        let sample_txt = temp.path().join("import_sample.txt");
        fs::write(
            &sample_txt,
            b"10:00:00 Speaker 1: Hello from import transcript.\n",
        )
        .unwrap();
        let day_dir = temp.path().join("chronicle/20260102");
        let outcome = solstone_core_import::process_transcript(
            &sample_txt,
            &day_dir,
            "10:00:00",
            "imp_001",
            "imported",
            None,
            None,
            Some(now),
        );
        for created in outcome.created() {
            solstone_core_journal_io::bump_stream_marker(temp.path(), &created.day).unwrap();
        }
        let walk_import = assert_matches(&app, &root, temp.path(), now).await;
        let created_day = &outcome.created()[0].day;
        let before_import = walk_restart.get(created_day.as_str()).copied().unwrap_or(0);
        let after_import = walk_import.get(created_day.as_str()).copied().unwrap_or(0);
        assert_ne!(before_import, after_import);

        // Archive merge: merge_journal_archive publishes one new counted segment
        let before_merge = walk_import.get("20260102").copied().unwrap_or(0);
        let archive_zip_path = temp.path().join("archive_merge.zip");
        {
            use std::io::Write;
            use zip::CompressionMethod;
            use zip::ZipWriter;
            use zip::write::SimpleFileOptions;

            let file = fs::File::create(&archive_zip_path).unwrap();
            let mut zip = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            zip.start_file(
                "chronicle/20260102/field/170000_60/mic_audio.jsonl",
                options,
            )
            .unwrap();
            zip.write_all(b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"17:00:00\",\"end\":\"17:00:01\",\"text\":\"merged\"}\n").unwrap();
            zip.finish().unwrap();
        }

        let archive_work = temp.path().join("archive-work");
        fs::create_dir_all(&archive_work).unwrap();
        let merge_options = solstone_core_import_sources::archive::ArchiveMergeOptions {
            working_root: archive_work,
            ..solstone_core_import_sources::archive::ArchiveMergeOptions::default()
        };
        solstone_core_import_sources::archive::merge_journal_archive(
            &archive_zip_path,
            temp.path(),
            &merge_options,
            None,
        )
        .unwrap();

        let walk_merge = assert_matches(&app, &root, temp.path(), now).await;
        let after_merge = walk_merge.get("20260102").copied().unwrap_or(0);
        assert_ne!(before_merge, after_merge);
    }

    #[cfg(feature = "full-tests")]
    #[tokio::test]
    async fn test_full_events_notification_forwarding() {
        use axum::body::HttpBody;
        use std::pin::Pin;
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let socket = temp.path().join("health").join("callosum.sock");
        #[cfg(windows)]
        let _namespace =
            solstone_core_callosum::test_fixture::WindowsPipeNamespace::register(&socket);
        let bus = solstone_core_callosum::CallosumSocketServer::bind(&socket)
            .await
            .unwrap();

        let convey_router = solstone_core_convey_shell::router(temp.path().to_path_buf());
        let response = convey_router
            .oneshot(Request::get("/sse/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();

        // Bounded wait for SSE client to connect to Callosum bus
        tokio::time::timeout(Duration::from_secs(5), async {
            while bus.client_count() == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("SSE client connected to callosum bus");

        let day = "20260306";
        // Send from a blocking thread, as production does: the send waits for
        // the bus to confirm, and this runtime thread is what serves the bus.
        for (kind, key) in [("day", day), ("system", "calendar")] {
            let journal = temp.path().to_path_buf();
            tokio::task::spawn_blocking(move || emit_notification(&journal, identity, kind, key))
                .await
                .unwrap();
        }

        let mut received = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let frame_opt = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
                let Some(Ok(frame)) = frame_opt else { break };
                if let Some(bytes) = frame.data_ref() {
                    let text = String::from_utf8_lossy(bytes);
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ")
                            && let Ok(mut val) = serde_json::from_str::<Value>(data)
                            && val.get("tract") == Some(&json!("change"))
                        {
                            if let Some(obj) = val.as_object_mut() {
                                obj.remove("ts");
                            }
                            received.push(val);
                            if received.len() >= 2 {
                                return;
                            }
                        }
                    }
                }
            }
        })
        .await;

        assert_eq!(received.len(), 2, "received: {:?}", received);
        assert_eq!(
            received[0],
            json!({"tract":"change","event":"changed","kind":"day","key":day})
        );
        assert_eq!(
            received[1],
            json!({"tract":"change","event":"changed","kind":"system","key":"calendar"})
        );
    }
}
