// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The single search-index writer consumes the file-backed work list.

use crate::StoreError;
use crate::writer_admission::IndexAdmission;
use serde::Serialize;
pub use solstone_core_indexer_todo::{
    Entry, TodoStatus, Token, enqueue_path, enqueue_path_bound, enqueue_token, has_due, inspect,
    listed_attempts, now_ms,
};
use solstone_core_indexer_todo::{
    entry_paths, path_absent, publish_if_absent, read_entry, rewrite, root,
};
use solstone_core_journal_io::append::append_jsonl;
use solstone_core_journal_io::locking::{LockOptions, hold_lock_checked};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const BATCH_SIZE: usize = 32;
const RETRY_MS: u64 = 120_000;
/// After three failures a kept entry, a token or a removal that could not be
/// made, retries hourly.
const LONG_RETRY_MS: u64 = 3_600_000;
/// The list is read again after this many paths, so a save made during a long
/// backlog waits for at most one snapshot.
#[cfg(not(test))]
const SNAPSHOT_ENTRIES: usize = 256;
#[cfg(test)]
const SNAPSHOT_ENTRIES: usize = 8;
/// After this long, a drain ends at the next snapshot boundary if a key it
/// already served has been saved again: the supervisor's next drain, within
/// 30 s, serves that save in its first snapshot.
#[cfg(not(test))]
const DRAIN_SLICE: Duration = Duration::from_secs(120);
#[cfg(test)]
const DRAIN_SLICE: Duration = Duration::from_millis(300);
/// A whole-journal token is not started with less budget than this left, so a
/// drain stops itself rather than being stopped inside a long pass.
const TOKEN_MARGIN: Duration = Duration::from_secs(300);

/// The limits of a whole-journal pass that takes turns with saves. Tests shrink
/// them per thread; every other test runs on these same values.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScanLimits {
    /// Under this many files to index, the pass runs in one go, as it always
    /// has: that's about what the daily check costs.
    pub(crate) turns_min: usize,
    /// How long one turn holds the writer.
    pub(crate) turn: Duration,
    /// How long a drain takes turns, from the end of its plan. A forced drain
    /// waits on this one, and its budget is 600 s.
    pub(crate) run: Duration,
    /// A turn indexes this many files before it can be asked to give way, so
    /// a steady stream of saves can't stop the rebuild.
    pub(crate) min_turn_files: usize,
    /// How often a turn asks whether a save is waiting, in files. It also asks
    /// every 250 ms.
    pub(crate) check_every_files: usize,
}

const SCAN_LIMITS: ScanLimits = ScanLimits {
    turns_min: 2_000,
    turn: Duration::from_secs(5),
    run: Duration::from_secs(240),
    min_turn_files: 64,
    check_every_files: 64,
};

/// A run ends in order when less than this is left of the drain's budget.
const SCAN_RUN_STOP_MARGIN: Duration = Duration::from_secs(30);

#[cfg(test)]
thread_local! {
    pub(crate) static TEST_SCAN_LIMITS: std::cell::Cell<Option<ScanLimits>> =
        const { std::cell::Cell::new(None) };
    /// Runs after each turn has released the lease.
    pub(crate) static TEST_AFTER_TURN: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

fn scan_limits() -> ScanLimits {
    #[cfg(test)]
    if let Some(limits) = TEST_SCAN_LIMITS.get() {
        return limits;
    }
    SCAN_LIMITS
}

pub(crate) fn scan_turns_min() -> usize {
    scan_limits().turns_min
}

#[derive(Clone, Copy, Debug)]
pub struct DrainOptions {
    /// Daily think ignores retry delays so its talents can read their inputs.
    pub force: bool,
    pub budget: Duration,
}

impl Default for DrainOptions {
    fn default() -> Self {
        Self {
            force: false,
            budget: Duration::from_secs(110 * 60),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DrainReport {
    pub batches: usize,
    pub processed: usize,
    pub failed: usize,
    pub recovered: usize,
    pub timed_out: bool,
    pub remaining: TodoStatus,
}

fn record_attempt(journal: &Path, entry: &Entry, outcome: &str, cause: Option<&str>) {
    // Filed under the owner's today: a UTC day makes a folder dated tomorrow.
    let now = chrono::Local::now();
    let relative = solstone_core_journal_io::durability::artifact(
        solstone_core_journal_io::durability::ArtifactId::IndexAttempts,
    )
    .path
    .replacen('*', &now.format("%Y%m%d").to_string(), 1);
    let log_path = journal.join(relative);
    let mut event = serde_json::json!({
        "event": "index.attempt", "ts": now.timestamp_millis(),
        "path": entry.key, "outcome": outcome, "cause": cause,
    });
    // How long the entry waited since it was first queued, retries included.
    // A connections refresh re-queues itself by design, so its wait is not lag.
    if entry.is_search_work() {
        event["queued_ms"] = serde_json::json!(now_ms().saturating_sub(entry.enqueued_ms));
    }
    if let Err(error) = append_jsonl(&log_path, &event) {
        log::warn!(target: "solstone::indexer", "could not record index attempt for {}: {error}", entry.key);
    }
}

/// Only called with the persistent drain lock held. A producer cannot replace
/// the fresh entry while its attempts are carried forward here.
///
/// After three failures a present path is dropped: the catch-all retries it.
/// A token, which has no such backstop, and a removal that could not be made
/// are kept and retry hourly.
fn return_failed(journal: &Path, mut entry: Entry, claim: &Path, cause: &str) -> io::Result<()> {
    let destination = root(journal).join(entry.filename());
    if let Some(fresh) = read_entry(&destination, false)? {
        entry.attempts = entry.attempts.max(fresh.attempts);
    }
    entry.attempts = entry.attempts.saturating_add(1);
    // A crash while returning work must not recover the claim's older count.
    rewrite(claim, &entry)?;
    record_attempt(journal, &entry, "failed", Some(cause));
    let token = entry.kind != "path";
    let failed_deletion = !token && path_absent(journal, &entry.key);
    let failing = entry.attempts >= 3;
    // A kept token's own re-queue must not clear its delay: the change that
    // makes `@reclassify` fail is the change that queues it again.
    let fresh_not_before = if failing && token {
        now_ms().saturating_add(LONG_RETRY_MS)
    } else {
        0
    };
    if failing && !token && !failed_deletion {
        // A fresh save does not reset the crash budget of the claim it replaced.
        if read_entry(&destination, false)?.is_some() {
            fs::remove_file(&destination)?;
        }
    } else if let Some(mut fresh) = read_entry(&destination, false)? {
        fresh.attempts = fresh.attempts.max(entry.attempts);
        fresh.not_before_ms = fresh_not_before;
        rewrite(&destination, &fresh)?;
    } else {
        entry.not_before_ms =
            now_ms().saturating_add(if failing { LONG_RETRY_MS } else { RETRY_MS });
        publish_if_absent(&destination, &entry)?;
        // A producer can win publication after the preceding read. Carry the
        // attempts onto that entry as well; only the drain ever rewrites it.
        if let Some(mut fresh) = read_entry(&destination, false)?
            && fresh.attempts < entry.attempts
        {
            fresh.attempts = entry.attempts;
            fresh.not_before_ms = fresh_not_before;
            rewrite(&destination, &fresh)?;
        }
    }
    fs::remove_file(claim)
}

/// This drain's own budget or checkpoint stopped the entry: that is not a
/// failure of the entry, so it goes back as it was, without an attempt. A fresh
/// entry a producer published meanwhile takes its place and keeps the higher
/// attempt count.
fn put_back(journal: &Path, entry: &Entry, claim: &Path) -> io::Result<()> {
    let destination = root(journal).join(entry.filename());
    // Create-only: a fresh entry a producer published meanwhile is kept.
    publish_if_absent(&destination, entry)?;
    if let Some(mut fresh) = read_entry(&destination, false)?
        && fresh.attempts < entry.attempts
    {
        fresh.attempts = entry.attempts;
        rewrite(&destination, &fresh)?;
    }
    fs::remove_file(claim)
}

/// Serialize drains across scheduled, explicit, daily-think and mobile runs.
/// The writer lease is released between bounded batches, independently of this lock.
pub fn drain(journal: &Path, options: DrainOptions) -> Result<DrainReport, StoreError> {
    drain_with_checkpoint(journal, options, &|| Ok(()))
}

pub fn drain_with_checkpoint(
    journal: &Path,
    options: DrainOptions,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<DrainReport, StoreError> {
    let result = run_drain(journal, options, checkpoint);
    if let Err(error) = &result {
        crate::recovery::report_if_damage(journal, error);
    }
    result
}

fn run_drain(
    journal: &Path,
    options: DrainOptions,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<DrainReport, StoreError> {
    let started = Instant::now();
    let check = || {
        checkpoint()?;
        if started.elapsed() >= options.budget {
            Err("index work wait expired".to_owned())
        } else {
            Ok(())
        }
    };
    let _drain_lock = match hold_lock_checked(
        journal.join("indexer/drain"),
        LockOptions {
            timeout: options.budget,
            ..LockOptions::default()
        },
        &check,
    ) {
        Ok(lock) => lock,
        Err(solstone_core_journal_io::errors::LockError::Timeout(_)) => {
            return Ok(DrainReport {
                timed_out: true,
                remaining: inspect(journal)?,
                ..DrainReport::default()
            });
        }
        Err(_) if check().is_err() => {
            return Ok(DrainReport {
                timed_out: true,
                remaining: inspect(journal)?,
                ..DrainReport::default()
            });
        }
        Err(error) => return Err(StoreError::Io(io::Error::other(error))),
    };
    fs::create_dir_all(root(journal).join("claimed"))?;
    let mut report = DrainReport::default();
    for claim in entry_paths(&root(journal).join("claimed"))? {
        if let Some(entry) = read_entry(&claim, true)? {
            return_failed(
                journal,
                entry,
                &claim,
                "the previous index task stopped before finishing this entry",
            )?;
            report.recovered += 1;
        }
    }
    if crate::recovery::check_index_damage(journal, &check) == crate::recovery::AfterCheck::EndDrain
    {
        report.timed_out = true;
        report.remaining = inspect(journal)?;
        return Ok(report);
    }
    let mut list = WorkList::default();
    let mut run: Option<ScanRun> = None;
    let mut scan_decided = false;
    // The full pass was just cleared to run: nothing ends this drain before it.
    let mut pass_is_next = false;
    let acquire = || loop {
        match IndexAdmission::acquire_bounded(
            journal,
            "todo",
            options
                .budget
                .saturating_sub(started.elapsed())
                .min(Duration::from_secs(10)),
            &check,
        ) {
            Err(StoreError::WriterBusy { .. }) if check().is_ok() => continue,
            Err(StoreError::WriterBusy { .. } | StoreError::WriterAdmission { .. })
                if check().is_err() =>
            {
                return Ok(None);
            }
            other => return other.map(Some),
        }
    };
    'snapshots: loop {
        if check().is_err() {
            report.timed_out = true;
            break;
        }
        if run.is_none()
            && !std::mem::take(&mut pass_is_next)
            && started.elapsed() >= DRAIN_SLICE
            && list.resaved(journal, options.force)?
        {
            break;
        }
        let token_room = options.budget.saturating_sub(started.elapsed()) >= TOKEN_MARGIN;
        list.paths_only = run.is_some();
        let mut snapshot =
            list.next_snapshot(journal, options.force, token_room, SNAPSHOT_ENTRIES)?;
        if snapshot.is_empty() && run.is_none() {
            break;
        }
        // The first time this drain reaches a full pass, its paths go first and
        // then it decides whether the pass takes turns. An unclaimed token is
        // the point: restarts during hours of turns must not read as failures.
        let scan_token = (!scan_decided && snapshot.last().is_some_and(|e| e.rank() == Some(1)))
            .then(|| snapshot.pop())
            .flatten();
        // Paths in bounded batches, then the token in a batch of its own.
        for batch in snapshot.chunks(BATCH_SIZE) {
            if check().is_err() {
                report.timed_out = true;
                break 'snapshots;
            }
            let Some(admission) = acquire()? else {
                report.timed_out = true;
                break 'snapshots;
            };
            report.batches += 1;
            // Hides are read once a batch: every hint in it was saved, and
            // its hide written, before this snapshot was read.
            let mut hides = crate::hide::HideCache::default();
            for entry in batch {
                if check().is_err() {
                    report.timed_out = true;
                    break;
                }
                list.served(entry);
                let path = root(journal).join(entry.filename());
                let claim = root(journal).join("claimed").join(entry.filename());
                if entry.kind == "path" && entry.attempts >= 2 {
                    // Do this before claiming: if invalidation cannot commit, the
                    // hint keeps its remaining attempt. A third crash can then drop
                    // the hint without letting a same-second source look current.
                    crate::db::invalidate_paths_admitted(journal, &[&entry.key], &admission)?;
                }
                match fs::rename(&path, &claim) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                }
                crate::writer_admission::check_test_seam("todo-claimed");
                let result = process(journal, entry, &admission, &check, &mut hides);
                match result {
                    Ok(Some(outcome)) => {
                        record_attempt(journal, entry, outcome, None);
                        fs::remove_file(&claim)?;
                        report.processed += 1;
                    }
                    Ok(None) => {
                        fs::remove_file(&claim)?;
                    }
                    // This drain's own budget or checkpoint stopped it.
                    Err(StoreError::WriterAdmission { .. }) if check().is_err() => {
                        put_back(journal, entry, &claim)?;
                        report.timed_out = true;
                        break;
                    }
                    Err(error) => {
                        crate::recovery::report_if_damage(journal, &error);
                        return_failed(journal, entry.clone(), &claim, &error.to_string())?;
                        list.failed(entry);
                        report.failed += 1;
                    }
                }
            }
            drop(admission);
            crate::writer_admission::check_test_seam("todo-batch-released");
            if report.timed_out {
                break 'snapshots;
            }
        }
        if let Some(token) = scan_token {
            scan_decided = true;
            run = begin_scan_run(journal, token, &mut list, &acquire, &check)?;
            // Under the threshold, or it can't take turns: the token comes back
            // through the next snapshot and runs as it always has. Otherwise any
            // save made during the walk goes before the first turn.
            pass_is_next = run.is_none();
            continue;
        }
        let Some(active) = run.as_mut() else {
            continue;
        };
        list.forgive_served_paths();
        let turn_started = Instant::now();
        let limits = scan_limits();
        let should_yield = || {
            turn_started.elapsed() >= limits.turn
                || solstone_core_indexer_todo::any_due_path(
                    journal,
                    if options.force { u64::MAX } else { now_ms() },
                    &|key| list.is_served(key),
                )
                .unwrap_or(false)
        };
        let Some(admission) = acquire()? else {
            report.timed_out = true;
            break 'snapshots;
        };
        let turn = crate::scan::index_scan_turn(
            journal,
            &mut active.plan,
            &admission,
            &check,
            limits,
            &should_yield,
        );
        drop(admission);
        #[cfg(test)]
        TEST_AFTER_TURN.with_borrow_mut(|hook| {
            if let Some(hook) = hook {
                hook();
            }
        });
        let ended = match turn {
            Ok(turn) => {
                active.committed += turn.committed;
                if turn.committed > 0 {
                    active.settle(journal, &mut list)?;
                }
                let budget_left = options.budget.saturating_sub(started.elapsed());
                if active.plan.is_empty() {
                    RunEnd::Finish
                } else if active.since_plan.elapsed() >= limits.run
                    || budget_left < SCAN_RUN_STOP_MARGIN
                {
                    // A run that indexed nothing is not making progress: a full
                    // disk looks like this. Today's pass reports what's wrong.
                    if active.committed == 0 {
                        RunEnd::Finish
                    } else {
                        RunEnd::NextDrain
                    }
                } else {
                    RunEnd::Continue
                }
            }
            Err(StoreError::WriterAdmission { .. }) if check().is_err() => {
                report.timed_out = true;
                RunEnd::NextDrain
            }
            Err(error) => {
                log::warn!(
                    target: "solstone::indexer",
                    "full pass stopped taking turns: {error}"
                );
                RunEnd::Finish
            }
        };
        match ended {
            RunEnd::Continue => {}
            RunEnd::Finish => {
                active.settle(journal, &mut list)?;
                list.release(&active.token);
                log::info!(
                    target: "solstone::indexer",
                    "full pass took turns for {} file(s) in this drain and is finishing",
                    active.committed
                );
                run = None;
                pass_is_next = true;
            }
            RunEnd::NextDrain => {
                active.settle(journal, &mut list)?;
                log::info!(
                    target: "solstone::indexer",
                    "full pass took turns for {} file(s) in this drain, {} left",
                    active.committed,
                    active.plan.len()
                );
                break 'snapshots;
            }
        }
    }
    // However this drain stopped, stopping is not the crash that attempt is for.
    if let Some(mut active) = run.take() {
        active.settle(journal, &mut list)?;
    }
    report.remaining = inspect(journal)?;
    Ok(report)
}

/// A whole-journal pass working through a large difference in turns.
struct ScanRun {
    token: Entry,
    plan: VecDeque<crate::scan::PlannedFile>,
    committed: usize,
    since_plan: Instant,
    /// The attempt count to put back, until that's done.
    unsettled: Option<u32>,
}

enum RunEnd {
    Continue,
    Finish,
    NextDrain,
}

impl ScanRun {
    /// Take back the attempt this run held against a crash.
    fn settle(&mut self, journal: &Path, list: &mut WorkList) -> io::Result<()> {
        if let Some(attempts) = self.unsettled.take() {
            set_attempts(journal, &self.token, attempts)?;
            list.forget(&self.token);
        }
        Ok(())
    }
}

/// The token's file is on the list, and only the drain rewrites it.
fn set_attempts(journal: &Path, token: &Entry, attempts: u32) -> io::Result<()> {
    let path = root(journal).join(token.filename());
    if let Some(mut entry) = read_entry(&path, false)? {
        entry.attempts = attempts;
        rewrite(&path, &entry)?;
    }
    Ok(())
}

/// Decide how a full pass runs. `None` sends it through the next snapshot and
/// on to today's claim, including when it has died before it could commit:
/// the attempt held here is what gets it there, and on to Health.
fn begin_scan_run(
    journal: &Path,
    token: Entry,
    list: &mut WorkList,
    lease: &dyn Fn() -> Result<Option<IndexAdmission>, StoreError>,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<Option<ScanRun>, StoreError> {
    if token.attempts >= 2 {
        return Ok(None);
    }
    set_attempts(journal, &token, token.attempts.saturating_add(1))?;
    list.forget(&token);
    let planned = crate::scan::plan_scan_turns(journal, lease, check);
    let plan = match planned {
        Ok(Some(plan)) if plan.len() >= scan_limits().turns_min => plan,
        Ok(_) => {
            set_attempts(journal, &token, token.attempts)?;
            list.forget(&token);
            return Ok(None);
        }
        Err(error) => {
            log::warn!(
                target: "solstone::indexer",
                "full pass could not plan its turns: {error}"
            );
            set_attempts(journal, &token, token.attempts)?;
            list.forget(&token);
            return Ok(None);
        }
    };
    list.hold(&token);
    Ok(Some(ScanRun {
        unsettled: Some(token.attempts),
        token,
        plan,
        committed: 0,
        since_plan: Instant::now(),
    }))
}

/// The drain's view of the list across snapshots. Producers only create
/// entries and only the drain rewrites or removes them, under the drain lock,
/// so an entry this drain has not served since it read it is unchanged.
#[derive(Default)]
struct WorkList {
    /// Decoded entries by file name; `None` for one this binary leaves alone.
    cache: HashMap<std::ffi::OsString, Option<Entry>>,
    /// Keys served in this drain. Each is served at most once per drain: a
    /// save made after its turn waits for the next drain.
    served: HashSet<String>,
    /// Keys that failed in this drain. They stay served even while a full pass
    /// takes turns, because being served is their retry spacing here.
    failed: HashSet<String>,
    /// A full pass is taking turns: the snapshots offer paths and nothing else.
    paths_only: bool,
}

impl WorkList {
    fn served(&mut self, entry: &Entry) {
        self.served.insert(entry.key.clone());
        // The drain is about to claim, remove or rewrite this file.
        self.cache.remove(std::ffi::OsStr::new(&entry.filename()));
    }

    fn failed(&mut self, entry: &Entry) {
        self.failed.insert(entry.key.clone());
    }

    /// This drain rewrote the entry: read it again.
    fn forget(&mut self, entry: &Entry) {
        self.cache.remove(std::ffi::OsStr::new(&entry.filename()));
    }

    /// A pass taking turns holds its token here, unclaimed.
    fn hold(&mut self, entry: &Entry) {
        self.served.insert(entry.key.clone());
    }

    /// The token comes back through the next snapshot.
    fn release(&mut self, entry: &Entry) {
        self.served.remove(&entry.key);
        self.forget(entry);
    }

    /// While a full pass takes turns, a re-saved file doesn't wait for the next
    /// drain. Turns are short, so this can't spin the way one drain could.
    fn forgive_served_paths(&mut self) {
        let failed = &self.failed;
        self.served
            .retain(|key| key.starts_with('@') || failed.contains(key));
    }

    fn is_served(&self, key: &str) -> bool {
        self.served.contains(key)
    }

    /// Read the list again: decode only files this drain has not seen.
    fn refresh(&mut self, journal: &Path) -> io::Result<()> {
        let directory = root(journal);
        let listed = entry_paths(&directory)?
            .into_iter()
            .filter_map(|path| path.file_name().map(ToOwned::to_owned))
            .collect::<HashSet<_>>();
        self.cache.retain(|name, _| listed.contains(name));
        for name in listed {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.cache.entry(name) {
                let entry = read_entry(&directory.join(slot.key()), true)?;
                slot.insert(entry);
            }
        }
        Ok(())
    }

    /// Whether a path this drain already served is due again: saved again
    /// after its turn.
    fn resaved(&mut self, journal: &Path, force: bool) -> io::Result<bool> {
        self.refresh(journal)?;
        let at_ms = now_ms();
        Ok(self.cache.values().flatten().any(|entry| {
            entry.rank() == Some(0)
                && (force || entry.not_before_ms <= at_ms)
                && self.served.contains(&entry.key)
        }))
    }

    /// Up to `limit` eligible paths, newest day first, then at
    /// most one eligible token. With neither, the connections refresh, which
    /// is last in line. Empty when nothing is left for this drain.
    ///
    /// A forced drain runs for the inputs of the day's thinking, so its
    /// tokens wait until no path is left. Without `token_room` no token runs.
    /// While a full pass takes turns, no other token runs until it has finished.
    fn next_snapshot(
        &mut self,
        journal: &Path,
        force: bool,
        token_room: bool,
        limit: usize,
    ) -> io::Result<Vec<Entry>> {
        self.refresh(journal)?;
        let at_ms = now_ms();
        let today = chrono::Local::now().format("%Y%m%d").to_string();
        let mut paths = Vec::new();
        let mut tokens = Vec::new();
        let mut edges = None;
        for entry in self.cache.values().flatten() {
            if !(force || entry.not_before_ms <= at_ms) || self.served.contains(&entry.key) {
                continue;
            }
            match entry.rank() {
                Some(0) => paths.push((
                    solstone_core_indexer_todo::sort_day(&entry.key, &today).to_owned(),
                    entry,
                )),
                Some(4) => edges = Some(entry),
                // The index check runs on its own, before any work.
                Some(5) => {}
                Some(_) => tokens.push(entry),
                None => {}
            }
        }
        // Newest day first, then the most recent save, then the key.
        let order = |(a_day, a): &(String, &Entry), (b_day, b): &(String, &Entry)| {
            b_day
                .cmp(a_day)
                .then(b.enqueued_ms.cmp(&a.enqueued_ms))
                .then(a.key.cmp(&b.key))
        };
        if paths.len() > limit {
            paths.select_nth_unstable_by(limit, order);
            paths.truncate(limit);
        }
        paths.sort_by(order);
        let token = tokens
            .into_iter()
            .min_by_key(|entry| entry.rank())
            .filter(|_| !self.paths_only && token_room && !(force && !paths.is_empty()));
        let mut snapshot = paths
            .into_iter()
            .map(|(_, entry)| entry.clone())
            .collect::<Vec<_>>();
        if snapshot.is_empty() && token.is_none() {
            snapshot.extend(edges.cloned().filter(|_| token_room && !self.paths_only));
        } else {
            snapshot.extend(token.cloned());
        }
        Ok(snapshot)
    }
}

/// How long one queued connections refresh holds the writer before it
/// re-queues itself behind newer work.
const EDGE_SLICE: Duration = Duration::from_secs(5);

fn process(
    journal: &Path,
    entry: &Entry,
    admission: &IndexAdmission,
    checkpoint: &dyn Fn() -> Result<(), String>,
    hides: &mut crate::hide::HideCache,
) -> Result<Option<&'static str>, StoreError> {
    match entry.rank() {
        Some(0) => {
            crate::scan::reconcile_path_admitted(journal, &entry.key, admission, checkpoint, hides)
                .map(Some)
        }
        Some(1) => {
            // A pass that finishes has done this token's work. Files it could
            // not index stay on its receipt, show on Health, and are retried
            // by the daily check; only a pass that stopped keeps the token.
            crate::scan::scan_queued_full_admitted(journal, admission, checkpoint)?;
            Ok(Some("indexed"))
        }
        Some(2) => {
            let mut snapshot = || {
                let _trust = solstone_core_entity::hold_facet_trust_lock(journal)
                    .map_err(|error| StoreError::Io(io::Error::other(error)))?;
                crate::classification::FacetDeclarationSet::from_journal(journal)
            };
            let report = crate::reconcile::reconcile_stale_classifications_admitted(
                journal,
                &mut snapshot,
                admission,
            )?;
            if report.incomplete {
                // Facets changing under the pass is not a failure: try again
                // shortly, once they have settled.
                log::info!(
                    target: "solstone::indexer",
                    "facet classifications kept changing; sorting again shortly"
                );
                let mut again = entry.clone();
                again.not_before_ms = now_ms().saturating_add(RETRY_MS);
                let destination = root(journal).join(again.filename());
                solstone_core_indexer_todo::publish_if_absent(&destination, &again)?;
                return Ok(None);
            }
            Ok(Some("indexed"))
        }
        Some(3) => {
            crate::scan::update_entity_search_admitted(journal, admission)?;
            // An entity changed: mark the connections it reaches. The refresh
            // itself waits its turn behind everything else in the queue.
            match crate::scan::invalidate_edges_admitted(journal, admission, checkpoint) {
                Ok(0) => {}
                Ok(_) => enqueue_token(journal, Token::Edges)?,
                Err(error @ StoreError::WriterAdmission { .. }) => return Err(error),
                Err(error) => log::warn!(
                    target: "solstone::indexer",
                    "connections not checked after an entity change: {error}"
                ),
            }
            Ok(Some("indexed"))
        }
        Some(4) => {
            let report =
                crate::scan::refresh_edges_admitted(journal, admission, checkpoint, EDGE_SLICE)?;
            if report.failed > 0 {
                log::warn!(
                    target: "solstone::indexer",
                    "connections refresh left {} file(s) failed: {}",
                    report.failed,
                    report.warnings.join("; ")
                );
            }
            if report.files + report.failed > 0 && report.remaining > 0 {
                enqueue_token(journal, Token::Edges)?;
            }
            Ok(Some("indexed"))
        }
        _ => unreachable!("only known entries are claimed"),
    }
}

/// How long an owner's trash change waits for the index writer before it
/// leaves its paths to the queued drain.
pub const REFRESH_NOW_WAIT: Duration = Duration::from_secs(3);

/// Bring search up to date for `paths` now, after an owner's trash change has
/// queued them and released its own holds: take the writer lease for at most
/// `wait`, then reconcile each path as the drain would, with this writer's
/// hide decisions. The hints stay queued, so a path this can't reach (the
/// lease busy, an error) is still done by the drain, and one it did is a
/// no-op there. Returns how many paths were reconciled.
///
/// # Errors
///
/// `WriterBusy` when the lease wasn't free within `wait`, or the first error
/// a path met.
pub fn refresh_now(journal: &Path, paths: &[String], wait: Duration) -> Result<usize, StoreError> {
    if paths.is_empty() {
        return Ok(0);
    }
    let admission = crate::writer_admission::IndexAdmission::acquire_bounded(
        journal,
        "refresh-now",
        wait,
        &|| Ok(()),
    )?;
    let mut hides = crate::hide::HideCache::default();
    let mut done = 0;
    for path in paths {
        let rel = path.strip_prefix("chronicle/").unwrap_or(path);
        crate::scan::reconcile_path_admitted(journal, rel, &admission, &|| Ok(()), &mut hides)?;
        done += 1;
    }
    Ok(done)
}

/// Test fixtures that save through production seams can explicitly converge.
#[cfg(any(test, feature = "test-fixtures"))]
pub fn drain_now(journal: &Path) -> Result<DrainReport, StoreError> {
    drain(
        journal,
        DrainOptions {
            force: true,
            ..DrainOptions::default()
        },
    )
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "todo_tests.rs"]
mod tests;

#[cfg(all(test, feature = "full-tests"))]
#[path = "todo_rebuild_tests.rs"]
mod rebuild_tests;
