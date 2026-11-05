// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

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
        file_id: meta.file_index(),
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
        segment_key: String,
    },
    Present {
        stream: String,
        segment_key: String,
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
        self.next_retry.map_or(true, |retry| now >= retry)
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
    /// Per-day condvar map to guarantee single-flight folding per day.
    pub(crate) in_flight: HashMap<String, Arc<(Mutex<bool>, Condvar)>>,
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

    if let Some(existing_worker) = workers.get(&identity) {
        if existing_worker.canonical_path != canonical {
            existing_worker.stop.store(true, Ordering::SeqCst);
            existing_worker.cv.notify_all();
            workers.remove(&identity);
            reg.remove(&identity);
        }
    }

    let inv_arc = reg
        .entry(identity)
        .or_insert_with(|| Arc::new(Mutex::new(JournalInventory::new())))
        .clone();

    if !workers.contains_key(&identity) {
        let worker = Arc::new(WorkerControl {
            canonical_path: canonical.to_path_buf(),
            request: Mutex::new(WorkerPassRequest {
                need_pass: false,
                full_pass: false,
            }),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            clock: Mutex::new(Clock::system()),
        });
        workers.insert(identity, worker.clone());

        let root_c = root.clone();
        let inv_arc_c = inv_arc.clone();
        std::thread::spawn(move || {
            run_worker_loop(root_c, inv_arc_c, worker);
        });
    }

    inv_arc
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

        let listed_days_map = day_dirs(&journal_path).unwrap_or_default();
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
                let _ = fold_single_day(&root, &journal_path, day, is_live, now, req_gen, &inv_arc);
            }
        }

        if is_full_pass {
            let finish_now = clock.now();
            let mut inv = inv_arc.lock().expect("inventory lock");
            let all_attempted = listed_days.iter().all(|d| inv.rows.contains_key(d));
            if all_attempted {
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

#[allow(dead_code)]
pub fn drop_inventory(identity: ObjectIdentity) {
    let mut reg = INVENTORIES.lock().expect("inventories lock");
    reg.remove(&identity);
    let mut workers = WORKERS.lock().expect("workers lock");
    if let Some(w) = workers.remove(&identity) {
        w.stop.store(true, Ordering::SeqCst);
        w.cv.notify_all();
    }
    let mut watchers = WATCHERS.lock().expect("watchers lock");
    if let Some(w) = watchers.remove(&identity) {
        w.active.store(false, Ordering::SeqCst);
        w.cv.notify_all();
    }
}

pub fn reset_calendar_while_held(journal: &Path) {
    let Ok(root) = JournalRoot::open(journal) else {
        return;
    };
    let identity = root.identity();
    let reg = INVENTORIES.lock().expect("inventories lock");
    if let Some(inv_arc) = reg.get(&identity) {
        let mut inv = inv_arc.lock().expect("inventory lock");
        inv.rows.clear();
        inv.last_full_pass_finish = None;
        inv.pending_first_seen.clear();
    }
    drop(reg);
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
    cv: Condvar,
    mu: Mutex<()>,
    clock: Mutex<Clock>,
}

static WATCHERS: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<WatcherControl>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn note_dashboard_stream(journal: &Path, connected: bool) {
    let Ok(root) = JournalRoot::open(journal) else {
        return;
    };
    let root = Arc::new(root);
    let identity = root.identity();
    let inv_arc = get_or_create_inventory(&root, identity);
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
                cv: Condvar::new(),
                mu: Mutex::new(()),
                clock: Mutex::new(Clock::system()),
            })
        });
        ctrl.active.store(true, Ordering::SeqCst);
        let ctrl_c = ctrl.clone();
        let root_c = root.clone();
        std::thread::spawn(move || {
            while ctrl_c.active.load(Ordering::SeqCst) {
                let guard = ctrl_c.mu.lock().unwrap();
                let _ = ctrl_c
                    .cv
                    .wait_timeout(guard, Duration::from_secs(10))
                    .unwrap();
                if !ctrl_c.active.load(Ordering::SeqCst) {
                    break;
                }
                let clock = ctrl_c.clock.lock().unwrap().clone();
                tick_journal(&root_c, &clock);
            }
        });
    } else if transition_to_zero {
        let watchers = WATCHERS.lock().expect("watchers lock");
        if let Some(ctrl) = watchers.get(&identity) {
            ctrl.active.store(false, Ordering::SeqCst);
            ctrl.cv.notify_all();
        }
    }
}

#[allow(dead_code)]
pub fn tick(journal: &Path, clock: &Clock) {
    if let Ok(root) = JournalRoot::open(journal) {
        let root = Arc::new(root);
        let identity = root.identity();
        let watchers = WATCHERS.lock().expect("watchers lock");
        if let Some(ctrl) = watchers.get(&identity) {
            *ctrl.clock.lock().unwrap() = clock.clone();
        }
        tick_journal(&root, clock);
    }
}

fn tick_journal(root: &Arc<JournalRoot>, clock: &Clock) {
    let now = clock.now();
    let identity = root.identity();
    let journal_path = root.canonical_path().to_path_buf();
    let _ = read_inventory_inner(
        Some(root.clone()),
        &journal_path,
        identity,
        now,
        clock,
        RequestRole::Index,
    );
}

fn emit_notification(journal_root: &Path, _identity: ObjectIdentity, kind: &str, key: &str) {
    test_support::record_notification(_identity, kind, key);

    let sock_path = journal_root.join("health/callosum.sock");
    if !sock_path.exists() {
        return;
    }
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
        Ok(meta) => {
            let stat = extract_file_stat(&meta);
            if let Some(MarkerIdentity::Present {
                stat: stored_stat,
                malformed,
            }) = stored_marker
            {
                if stored_stat == &stat {
                    return MarkerIdentity::Present {
                        stat,
                        malformed: *malformed,
                    };
                }
            }
            let malformed = match fs::read_to_string(&path) {
                Ok(content) => {
                    serde_json::from_str::<solstone_core_journal_io::HealthMarker>(&content)
                        .is_err()
                }
                Err(_) => false,
            };
            MarkerIdentity::Present { stat, malformed }
        }
    }
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

        if let Some(key) = segment_key_os(&file_name) {
            entries.insert(FileDigestEntry::SegmentDir {
                stream: String::new(),
                segment_key: key.to_string(),
            });
            segment_dirs.push((String::new(), key.to_string(), path));
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
                    if let Some(key) = segment_key_os(&sub_name) {
                        entries.insert(FileDigestEntry::SegmentDir {
                            stream: stream_name.clone(),
                            segment_key: key.to_string(),
                        });
                        segment_dirs.push((stream_name.clone(), key.to_string(), sub_path));
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

    for (stream, segment_key, seg_path) in segment_dirs {
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
                            segment_key: segment_key.clone(),
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
    if is_live {
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

fn fold_single_day(
    root: &Arc<JournalRoot>,
    journal: &Path,
    day: &str,
    is_live: bool,
    now: DateTime<Utc>,
    req_gen: u64,
    inv_arc: &Arc<Mutex<JournalInventory>>,
) -> Result<DayIdentity, FoldAborted> {
    let in_flight_entry = {
        let mut inv = inv_arc.lock().expect("inventory lock");
        if let Some(entry) = inv.in_flight.get(day) {
            Some(entry.clone())
        } else {
            let entry = Arc::new((Mutex::new(false), Condvar::new()));
            inv.in_flight.insert(day.to_string(), entry);
            None
        }
    };

    if let Some(entry) = in_flight_entry {
        let (lock, cv) = &*entry;
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
            inv.rows.get(day).map(|r| r.identity.marker().clone())
        };
        let disk_identity = read_day_identity(
            journal,
            day,
            is_live,
            stored_marker.as_ref(),
            root.identity(),
        );
        let matches_stored = {
            let inv = inv_arc.lock().expect("inventory lock");
            inv.rows
                .get(day)
                .map(|r| r.identity == disk_identity)
                .unwrap_or(false)
        };
        if matches_stored {
            return Ok(disk_identity);
        } else {
            return fold_single_day(root, journal, day, is_live, now, req_gen, inv_arc);
        }
    }

    struct InFlightGuard {
        day: String,
        inv_arc: Arc<Mutex<JournalInventory>>,
    }

    impl Drop for InFlightGuard {
        fn drop(&mut self) {
            let mut inv = self.inv_arc.lock().expect("inventory lock");
            if let Some(entry) = inv.in_flight.remove(&self.day) {
                let (lock, cv) = &*entry;
                let mut done = lock.lock().unwrap();
                *done = true;
                cv.notify_all();
            }
        }
    }

    let _in_flight_guard = InFlightGuard {
        day: day.to_string(),
        inv_arc: inv_arc.clone(),
    };

    let admission = match begin_target_use_bound(Arc::clone(root)) {
        Ok(guard) => guard,
        Err(TargetUseRefused) => return Err(FoldAborted::TargetUseRefused),
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

    #[cfg(test)]
    test_support::run_barrier_after_identity_read(root.identity(), day);

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
                if let Some(ref prev) = prev_row {
                    if let DayFoldState::Computed(old_count) = prev.state {
                        if old_count != count
                            && prev.identity.marker().stat_only()
                                == identity_pre.marker().stat_only()
                        {
                            if let Ok(mut writer) = DailyLogWriter::new(
                                journal,
                                "calendar",
                                "inventory",
                                Some(day.to_string()),
                            ) {
                                writer.write(&format!("chronicle day {day}: missed producer\n"));
                            }
                            emit = true;
                        }
                    }
                }

                match &prev_row {
                    None => emit = true,
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
                let in_failed_episode = prev_row.as_ref().map_or(false, |r| r.in_failed_episode);
                if !in_failed_episode {
                    if let Ok(mut writer) =
                        DailyLogWriter::new(journal, "calendar", "inventory", Some(day.to_string()))
                    {
                        writer.write(&format!("chronicle day {day}: count failed\n"));
                    }
                }

                match &prev_row {
                    None => emit = true,
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
    let Some(root) = root_opt else {
        return (
            Arc::new(BTreeMap::new()),
            json!({"complete": false, "behind": 0, "failed": 0, "oldest": now.to_rfc3339()}),
        );
    };

    let req_gen = next_generation();
    let inv_arc = get_or_create_inventory(&root, identity);

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

    let listed_days_map = day_dirs(journal_path).unwrap_or_default();
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

    let start = Instant::now();
    let mut budget_spent = false;

    #[cfg(test)]
    let fold_limit = test_support::inline_fold_limit(identity);
    #[cfg(test)]
    let mut fold_count = 0;

    if role == RequestRole::Index {
        for day in &pending_days {
            #[cfg(test)]
            if let Some(limit) = fold_limit {
                if fold_count >= limit {
                    budget_spent = true;
                    let workers = WORKERS.lock().expect("workers lock");
                    if let Some(w) = workers.get(&identity) {
                        let mut req = w.request.lock().unwrap();
                        req.need_pass = true;
                        if run_full_pass {
                            req.full_pass = true;
                        }
                        w.cv.notify_one();
                    }
                    break;
                }
            }

            if start.elapsed() >= INLINE_FOLD_BUDGET {
                budget_spent = true;
                let workers = WORKERS.lock().expect("workers lock");
                if let Some(w) = workers.get(&identity) {
                    let mut req = w.request.lock().unwrap();
                    req.need_pass = true;
                    if run_full_pass {
                        req.full_pass = true;
                    }
                    w.cv.notify_one();
                }
                break;
            }
            let is_live = live_set.contains(day);
            if let Ok(new_id) =
                fold_single_day(&root, journal_path, day, is_live, now, req_gen, &inv_arc)
            {
                day_identities.insert(day.clone(), new_id);
            }
            #[cfg(test)]
            {
                fold_count += 1;
            }
        }
    }

    if run_full_pass && !budget_spent {
        let mut inv = inv_arc.lock().expect("inventory lock");
        let all_attempted = listed_days.iter().all(|d| inv.rows.contains_key(d));
        if all_attempted {
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
) -> Result<(), String> {
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

    let listed_days_map = day_dirs(journal_path).unwrap_or_default();
    let mut month_days: Vec<String> = listed_days_map
        .into_keys()
        .filter(|d| d.starts_with(month))
        .collect();
    month_days.sort();

    let (_today_date, _today_str, _yesterday_str, live_set) =
        compute_live_set(journal_path, &month_days, now);

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
            let _ = fold_single_day(&root, journal_path, day, is_live, now, req_gen, &inv_arc);
        }
    }

    let inv = inv_arc.lock().expect("inventory lock");
    for day in &month_days {
        match inv.rows.get(day) {
            None => return Err(format!("chronicle day {day}: missing count")),
            Some(row) => {
                if matches!(row.state, DayFoldState::Failed(_)) {
                    return Err(format!("chronicle day {day}: count failed"));
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn get_month_counts(journal_path: &Path, month: &str) -> BTreeMap<String, u64> {
    let Ok(root) = JournalRoot::open(journal_path) else {
        return BTreeMap::new();
    };
    let root = Arc::new(root);
    let identity = root.identity();
    let inv_arc = get_or_create_inventory(&root, identity);
    let inv = inv_arc.lock().expect("inventory lock");
    let mut map = BTreeMap::new();
    for (day, row) in &inv.rows {
        if day.starts_with(month) {
            if let DayFoldState::Computed(count) = row.state {
                if count > 0 {
                    map.insert(day.clone(), count);
                }
            }
        }
    }
    map
}

#[allow(dead_code)]
pub(crate) mod test_support {
    use solstone_core_journal_io::ObjectIdentity;
    use std::collections::HashMap;
    use std::fs;
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

    static WAITER_COUNTERS: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<AtomicUsize>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    fn get_waiter_counter(identity: ObjectIdentity) -> Arc<AtomicUsize> {
        let mut map = WAITER_COUNTERS.lock().unwrap();
        map.entry(identity).or_default().clone()
    }

    pub(crate) fn inc_waiter_counter(identity: ObjectIdentity) {
        get_waiter_counter(identity).fetch_add(1, Ordering::SeqCst);
    }
    pub(crate) fn dec_waiter_counter(identity: ObjectIdentity) {
        get_waiter_counter(identity).fetch_sub(1, Ordering::SeqCst);
    }
    pub(crate) fn waiter_counter(identity: ObjectIdentity) -> usize {
        get_waiter_counter(identity).load(Ordering::SeqCst)
    }

    static WORKER_PASSES: LazyLock<Mutex<HashMap<ObjectIdentity, Arc<(Mutex<u64>, Condvar)>>>> =
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
        after_identity_read: HashMap<ObjectIdentity, BarrierEntry>,
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
        BARRIERS.lock().unwrap().after_identity_read.insert(
            identity,
            BarrierEntry {
                hook: Arc::new(hook),
                gate: Arc::new((Mutex::new(false), Condvar::new())),
            },
        );
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

    pub(crate) fn run_barrier_after_identity_read(identity: ObjectIdentity, day: &str) {
        let entry = BARRIERS
            .lock()
            .unwrap()
            .after_identity_read
            .get(&identity)
            .map(|e| (e.hook.clone(), e.gate.clone()));
        if let Some((hook, gate)) = entry {
            if hook(day) {
                let (lock, cv) = &*gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = cv.wait(released).unwrap();
                }
                *released = false;
            }
        }
    }

    pub(crate) fn run_barrier_after_compute(identity: ObjectIdentity, day: &str) {
        let entry = BARRIERS
            .lock()
            .unwrap()
            .after_compute
            .get(&identity)
            .map(|e| (e.hook.clone(), e.gate.clone()));
        if let Some((hook, gate)) = entry {
            if hook(day) {
                let (lock, cv) = &*gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = cv.wait(released).unwrap();
                }
                *released = false;
            }
        }
    }

    pub(crate) fn release_identity_barrier(identity: ObjectIdentity) {
        let b = BARRIERS.lock().unwrap();
        if let Some(entry) = b.after_identity_read.get(&identity) {
            let (lock, cv) = &*entry.gate;
            let mut released = lock.lock().unwrap();
            *released = true;
            cv.notify_all();
        }
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

    fn notification_file(identity: ObjectIdentity) -> std::path::PathBuf {
        let obs = identity.observation_id().replace(':', "_");
        std::env::temp_dir().join(format!("solstone_test_notifs_{obs}"))
    }

    pub(crate) fn record_notification(identity: ObjectIdentity, kind: &str, key: &str) {
        use std::io::Write;
        let mut b = BARRIERS.lock().unwrap();
        b.notifications
            .entry(identity)
            .or_default()
            .push((kind.to_string(), key.to_string()));
        drop(b);
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(notification_file(identity))
        {
            let _ = writeln!(file, "{kind}\t{key}");
        }
    }

    pub(crate) fn recorded_notifications(identity: ObjectIdentity) -> Vec<(String, String)> {
        if let Ok(content) = fs::read_to_string(notification_file(identity)) {
            let mut list = Vec::new();
            for line in content.lines() {
                if let Some((kind, key)) = line.split_once('\t') {
                    list.push((kind.to_string(), key.to_string()));
                }
            }
            if !list.is_empty() {
                return list;
            }
        }
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
        let _ = fs::remove_file(notification_file(identity));
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
        let mut b = BARRIERS.lock().unwrap();
        if let Some(entry) = b.after_identity_read.remove(&identity) {
            let (lock, cv) = &*entry.gate;
            let mut released = lock.lock().unwrap();
            *released = true;
            cv.notify_all();
        }
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
        b.notifications.remove(&identity);
        drop(b);
        let _ = fs::remove_file(notification_file(identity));
        reset_seam_counters(identity);
        let mut w = WAITER_COUNTERS.lock().unwrap();
        w.remove(&identity);
        let mut wp = WORKER_PASSES.lock().unwrap();
        wp.remove(&identity);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::response::Response;
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
    use solstone_core_journal_io::{JournalRoot, ObjectIdentity, day_dirs};
    use tower::ServiceExt;

    use super::*;
    use crate::Clock;

    struct TestResetGuard(ObjectIdentity);
    impl Drop for TestResetGuard {
        fn drop(&mut self) {
            test_support::release_identity_barrier(self.0);
            test_support::release_compute_barrier(self.0);
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

    #[tokio::test]
    async fn test_oracle_equivalence() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        delete_stats_json(temp.path());

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
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
            scan.stats.transcript_ranges = *count;
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
        assert_eq!(
            test_support::tree_descents(identity),
            live_days_count,
            "tree descent equals live days"
        );
    }

    // A mutant that reads the marker after compute would store the new stat with the old count and this assertion would fail.
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
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let held_tx = Arc::new(Mutex::new(Some(held_tx)));
        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();
        let target_day_s = target_day.to_string();

        test_support::install_barrier_after_identity_read(identity, move |day| {
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
            test_support::release_identity_barrier(identity);
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
        let walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        let expected_sum: u64 = walk
            .iter()
            .filter(|(d, _)| d.starts_with("202603"))
            .map(|(_, c)| *c)
            .sum();
        assert_eq!(body["months"]["202603"], expected_sum);
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

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let held_tx = Arc::new(Mutex::new(Some(held_tx)));
        let fired = Arc::new(AtomicBool::new(false));
        let fired_c = fired.clone();
        let identity_c = identity;

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
                if let Some(tx) = held_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                true
            } else {
                false
            }
        });

        std::thread::spawn(move || {
            held_rx.recv().unwrap();
            test_support::release_compute_barrier(identity_c);
        });

        let req1 = Request::get("/app/transcripts/api/stats/202607")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let resp1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        let bytes1 = to_bytes(resp1.into_body(), usize::MAX).await.unwrap();
        let body1: Value = serde_json::from_slice(&bytes1).unwrap();

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

    // A reset before the wait would leave C_old current, because the marker is still absent.
    #[tokio::test]
    async fn test_barrier_restore_ordering() {
        let (temp, root) = setup_fixture_journal();
        let identity = root.identity();
        let _guard = TestResetGuard(identity);

        let now = Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap();
        let clock = Clock::fixed(now);
        let app = crate::router(temp.path().to_path_buf(), clock, || {
            Response::new(Body::empty())
        });

        // Old day with no stream.updated
        let target_day = "20260101";
        let marker = health_marker_path(temp.path(), target_day, HealthMarkerKind::Stream);
        let _ = fs::remove_file(&marker);

        let _hooks = solstone_core_convey_shell::recovery_drain::register_recovery_hooks(
            "test-barrier-restore-ordering",
            root.clone(),
            |_| Ok(()),
            |_| Ok(()),
        );

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

        let inv_arc = get_or_create_inventory(&root, identity);
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
            );
        });
        held_rx.recv().unwrap();

        // While held, change files to C_new
        let seg = temp.path().join("chronicle/20260101/field/180000_60");
        fs::create_dir_all(&seg).unwrap();
        fs::write(
            seg.join("mic_audio.jsonl"),
            b"{\"_solstone_processing\":{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\"}}\n{\"start\":\"18:00:00\",\"end\":\"18:00:01\",\"text\":\"c_new\"}\n",
        )
        .unwrap();

        struct WaitHookGuard;
        impl Drop for WaitHookGuard {
            fn drop(&mut self) {
                solstone_core_journal_io::whole_journal_restore::install_target_use_wait_hook(None);
            }
        }
        let _hook_guard = WaitHookGuard;

        let (wait_tx, wait_rx) = std::sync::mpsc::channel();
        let wait_tx = Arc::new(Mutex::new(Some(wait_tx)));
        solstone_core_journal_io::whole_journal_restore::install_target_use_wait_hook(Some(
            Arc::new(move |_| {
                if let Some(tx) = wait_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }),
        ));

        let root_drain = root.clone();
        let pause_thread = std::thread::spawn(move || {
            solstone_core_convey_shell::recovery_drain::pause_admitted_work(root_drain)
        });

        wait_rx.recv().unwrap();
        test_support::release_compute_barrier(identity);

        fold_thread.join().unwrap();
        let paused = pause_thread.join().unwrap().unwrap();
        paused.resume(&|| false).unwrap();

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
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["freshness"]["complete"], true);

        let walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        assert_eq!(
            body["months"]["202601"],
            walk.get("20260101").copied().unwrap_or(0)
        );
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
        let old_month_count = b1["months"]["202601"].as_u64().unwrap();

        let target_day = "20260101";
        let day_path = temp.path().join("chronicle").join(target_day);
        let marker = health_marker_path(temp.path(), target_day, HealthMarkerKind::Stream);
        let _ = fs::remove_file(&marker);

        // Three edits on a day older than yesterday, no bump:
        // 1. Remove an existing segment
        let existing_seg = day_path.join("field/100000_60");
        let _ = fs::remove_dir_all(&existing_seg);

        // 2. Add a browser file in a segment dir
        let browser_seg = day_path.join("workstation.browser/100000_60");
        fs::create_dir_all(&browser_seg).unwrap();
        fs::write(
            browser_seg.join("stream.json"),
            b"{\"stream\":\"workstation.browser\",\"seq\":1}\n",
        )
        .unwrap();
        fs::write(
            browser_seg.join("browser_page.jsonl"),
            b"{\"t\":\"segment_start\",\"ts\":1783006230000,\"rel\":30000,\"site\":\"example.com\",\"url\":\"https://example.com\",\"title\":\"Test\",\"adapter\":\"docs\",\"ctx\":\"doc\",\"n\":1,\"blocks\":[]}\n",
        )
        .unwrap();

        // 3. Add a raw-only segment directory
        let raw_seg = day_path.join("field/190000_60");
        fs::create_dir_all(&raw_seg).unwrap();
        fs::write(raw_seg.join("mic.raw"), b"raw audio bytes").unwrap();

        // A read 30 minutes later keeps the old count
        *now_ref.lock().unwrap() += chrono::Duration::minutes(30);
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
        assert_eq!(b2["months"]["202601"], old_month_count);

        // Advance the injected clock to more than 3600 seconds past last full-pass finish and tick/read
        *now_ref.lock().unwrap() += chrono::Duration::seconds(3700);
        test_support::clear_recorded_notifications(identity);

        let resp3 = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let b3: Value =
            serde_json::from_slice(&to_bytes(resp3.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let walk = crate::calendar::counts_by_walk(temp.path(), *now_ref.lock().unwrap()).unwrap();
        let expected_sum: u64 = walk
            .iter()
            .filter(|(d, _)| d.starts_with("202601"))
            .map(|(_, c)| *c)
            .sum();
        assert_eq!(b3["months"]["202601"], expected_sum);

        // Writes one missed producer line per day
        let mut found_missed_producer = false;
        if let Ok(entries) = fs::read_dir(temp.path().join("chronicle")) {
            for day_entry in entries.flatten() {
                let health_dir = day_entry.path().join("health");
                if let Ok(h_entries) = fs::read_dir(health_dir) {
                    for h_file in h_entries.flatten() {
                        if let Ok(content) = fs::read_to_string(h_file.path()) {
                            if content.contains("chronicle day 20260101: missed producer") {
                                found_missed_producer = true;
                            }
                        }
                    }
                }
            }
        }
        assert!(found_missed_producer, "must write missed producer line");

        // Records one day notification per day
        let notifs = test_support::recorded_notifications(identity);
        assert!(
            notifs.contains(&("day".to_string(), "20260101".to_string())),
            "must record day notification"
        );

        // restart_wipe records nothing and the following read keeps the corrected count
        test_support::clear_recorded_notifications(identity);
        restart_wipe(identity);
        assert!(
            test_support::recorded_notifications(identity).is_empty(),
            "restart_wipe records nothing"
        );

        let resp4 = app
            .clone()
            .oneshot(
                Request::get("/app/transcripts/api/index")
                    .extension(root.clone())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let b4: Value =
            serde_json::from_slice(&to_bytes(resp4.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(b4["months"]["202601"], expected_sum);
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

        // After each edit, write a fresh stats.json that does not contain the new count
        fs::write(
            today_path.join("stats.json"),
            b"{\"stats\":{\"transcript_ranges\":0,\"percept_ranges\":0,\"browser_segments\":0}}\n",
        )
        .unwrap();
        fs::write(
            yesterday_path.join("stats.json"),
            b"{\"stats\":{\"transcript_ranges\":0,\"percept_ranges\":0,\"browser_segments\":0}}\n",
        )
        .unwrap();

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

        let walk = crate::calendar::counts_by_walk(temp.path(), now).unwrap();
        assert_eq!(
            b["months"]["202608"],
            walk.get("20260801").copied().unwrap_or(0)
        );
        assert!(
            walk.get("20260801").copied().unwrap_or(0) > 0,
            "count must have changed"
        );
        assert!(
            walk.get("20260731").copied().unwrap_or(0) > 0,
            "yesterday count changed"
        );
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
            fs::create_dir_all(&bad_seg).unwrap();
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
                if let Ok(content) = fs::read_to_string(h_file.path()) {
                    if content.contains("chronicle day 20260309: count failed") {
                        assert!(
                            !content.contains(temp.path().to_str().unwrap()),
                            "health line must not contain path"
                        );
                        found_health_line = true;
                    }
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
            scan.stats.transcript_ranges = *walk.get(d).unwrap_or(&0);
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

        // Event 1: a new raw-only segment and a browser segment (create the files and bump_stream_marker)
        let before_1 = initial_walk.get("20260703").copied().unwrap_or(0);
        let raw_seg = temp.path().join("chronicle/20260703/field/140000_60");
        fs::create_dir_all(&raw_seg).unwrap();
        fs::write(raw_seg.join("mic.raw"), b"raw audio bytes").unwrap();

        let new_b_seg = temp
            .path()
            .join("chronicle/20260703/workstation.browser/150000_60");
        fs::create_dir_all(&new_b_seg).unwrap();
        fs::write(
            new_b_seg.join("stream.json"),
            b"{\"stream\":\"workstation.browser\",\"seq\":2}\n",
        )
        .unwrap();
        fs::write(new_b_seg.join("browser_page.jsonl"), b"{\"t\":\"segment_start\",\"ts\":1783006240000,\"rel\":40000,\"site\":\"example.com\",\"url\":\"https://example.com\",\"title\":\"T2\",\"adapter\":\"docs\",\"ctx\":\"doc\",\"n\":1,\"blocks\":[]}\n").unwrap();
        let mut scan1 = solstone_core_journal_stats_cli::DayScan::default();
        scan1.stats.transcript_ranges = 1;
        scan1.stats.browser_segments = 1;
        let stats_path1 = temp.path().join("chronicle/20260703/stats.json");
        solstone_core_journal_io::write_json(
            &stats_path1,
            &scan1,
            solstone_core_journal_io::JsonWriteOptions::default(),
        )
        .unwrap();
        filetime::set_file_mtime(&stats_path1, future_time).unwrap();
        solstone_core_journal_io::bump_stream_marker(temp.path(), "20260703").unwrap();
        let walk1 = assert_matches(&app, &root, temp.path(), now).await;
        let after_1 = walk1.get("20260703").copied().unwrap_or(0);
        assert_ne!(before_1, after_1);

        // Event 2: segment delete through DELETE /app/transcripts/api/segment/{day}/{stream}/{segment_key}
        let before_2 = walk1.get("20260703").copied().unwrap_or(0);
        let del_req = Request::builder()
            .method("DELETE")
            .uri("/app/transcripts/api/segment/20260703/workstation.browser/150000_60")
            .extension(root.clone())
            .body(Body::empty())
            .unwrap();
        let del_resp = app.clone().oneshot(del_req).await.unwrap();
        assert!(del_resp.status().is_success());
        let walk2 = assert_matches(&app, &root, temp.path(), now).await;
        let after_2 = walk2.get("20260703").copied().unwrap_or(0);
        assert_ne!(before_2, after_2);

        // Event 3: retention removal through solstone_core_retention::door::remove_segments
        let before_3 = walk2.get("20260702").copied().unwrap_or(0);
        let target = solstone_core_retention::Target {
            day: "20260702".to_owned(),
            stream: "workstation.browser".to_owned(),
            dir: "100000_60".to_owned(),
        };
        let _ = solstone_core_retention::door::remove_segments(
            temp.path(),
            std::slice::from_ref(&target),
            "2026-07-02T22:00:00Z",
            solstone_core_retention::RemovalReason::OwnerSegmentDelete,
            "sha256:abc",
        );
        let walk3 = assert_matches(&app, &root, temp.path(), now).await;
        let after_3 = walk3.get("20260702").copied().unwrap_or(0);
        assert_ne!(before_3, after_3);

        // Event 4: solstone_core_segment::relocate_segment across two days
        let before_4_src = walk3.get("20260101").copied().unwrap_or(0);
        let before_4_dst = walk3.get("20260102").copied().unwrap_or(0);
        let src_dir = temp.path().join("chronicle/20260101/field/100000_60");
        let dst_dir = temp.path().join("chronicle/20260102/field/100000_60");
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
            journal: temp.path(),
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

        // Event 5: whole-journal restore through pause_admitted_work
        test_support::clear_recorded_notifications(identity);
        let _hooks = solstone_core_convey_shell::recovery_drain::register_recovery_hooks(
            "parity-test-restore",
            root.clone(),
            |_| Ok(()),
            |_| Ok(()),
        );
        let paused =
            solstone_core_convey_shell::recovery_drain::pause_admitted_work(root.clone()).unwrap();
        paused.resume(&|| false).unwrap();
        let restore_notifications = test_support::recorded_notifications(identity);
        assert_eq!(
            restore_notifications,
            vec![("system".to_string(), "calendar".to_string())]
        );
        let walk5 = assert_matches(&app, &root, temp.path(), now).await;

        // Event 6: restart_wipe leaves the count unchanged and emits nothing
        test_support::clear_recorded_notifications(identity);
        restart_wipe(identity);
        assert!(test_support::recorded_notifications(identity).is_empty());
        let walk6 = assert_matches(&app, &root, temp.path(), now).await;
        assert_eq!(walk5, walk6);

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
        let before_import = walk6.get(created_day.as_str()).copied().unwrap_or(0);
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
        emit_notification(temp.path(), identity, "day", day);
        emit_notification(temp.path(), identity, "system", "calendar");

        let mut received = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let frame_opt = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await;
                let Some(Ok(frame)) = frame_opt else { break };
                if let Some(bytes) = frame.data_ref() {
                    let text = String::from_utf8_lossy(bytes);
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(mut val) = serde_json::from_str::<Value>(data) {
                                if val.get("tract") == Some(&json!("change")) {
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
