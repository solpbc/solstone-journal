// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A confirmed owner delete, held for its cancel window, then carried out.
//!
//! The hold used to live only in memory, so a journal that stopped inside the
//! window lost the delete while the page had already told the owner it was
//! happening. The durable half here is a record written before the delete
//! route answers. The next start resumes it, and it carries the outcome
//! afterwards so the page and cancel can report what really happened. The
//! in-process half, [`Registry`], only decides whether the deadline or a
//! cancel got there first.
//!
//! ⛔ Cancel means the delete has not run yet. Nothing here commits first and
//! undoes later.
//!
//! Each kind of delete supplies its own target (what the owner confirmed and
//! how to recognise it again) and its own removal. A resumed delete must remove
//! only what the owner confirmed: the target's check runs on every commit, and
//! a target that no longer matches is kept.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockError, LockOptions, atomic_replace, hold_lock,
};

/// How long a finished record is kept for status and cancel to read.
const FINISHED_RETENTION_DAYS: i64 = 7;
/// How long past its deadline a delete may stay pending before it is reported.
const OVERDUE_MINUTES: i64 = 5;
/// How long a commit waits for its record. Contention means another commit of
/// the same delete holds it; the record stays pending and the next start
/// retries if that one did not finish.
const COMMIT_LOCK_WAIT: Duration = Duration::from_secs(5);
/// The longest a resumed record waits for its deadline.
const MAX_RESUME_WAIT: Duration = Duration::from_secs(60);
/// How long a cancel waits. A commit holds the lock while it removes, so a
/// cancel that cannot take it promptly is too late.
const CANCEL_LOCK_WAIT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteState {
    /// Confirmed and waiting for its window, or waiting for the next start.
    Pending,
    /// The target is gone.
    Deleted,
    /// The target was kept. `reason` says why when there is an owner reason.
    NotDeleted,
    /// Removal started and did not finish: some of the target may be gone.
    Incomplete,
    /// The owner cancelled inside the window.
    Cancelled,
}

impl DeleteState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Deleted => "deleted",
            Self::NotDeleted => "not_deleted",
            Self::Incomplete => "incomplete",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One confirmed delete. The target's fields sit at the top level of the file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Record<T> {
    pub pending_id: String,
    #[serde(flatten)]
    pub target: T,
    pub requested_at: String,
    pub commit_at_ms: i64,
    pub state: DeleteState,
    /// Set when a commit has claimed the record. A cancel after this is too late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// Set by a removal that marks the moment it starts changing the journal,
    /// so a run that takes over a stopped claim knows whether anything it
    /// finds missing was removed by this delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removal_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

impl<T> Record<T> {
    /// A new pending record whose window closes `window` from now.
    pub fn pending(pending_id: String, target: T, window: Duration) -> Self {
        let now = Utc::now();
        Self {
            pending_id,
            target,
            requested_at: now.to_rfc3339(),
            commit_at_ms: now
                .timestamp_millis()
                .saturating_add(i64::try_from(window.as_millis()).unwrap_or(i64::MAX)),
            state: DeleteState::Pending,
            started_at: None,
            removal_started_at: None,
            reason: None,
            finished_at: None,
        }
    }

    pub fn finish(&mut self, state: DeleteState, reason: Option<String>) {
        self.state = state;
        self.reason = reason;
        self.finished_at = Some(Utc::now().to_rfc3339());
    }

    /// What is left of the window, in whole seconds rounded up.
    pub fn remaining_seconds(&self) -> i64 {
        (self
            .commit_at_ms
            .saturating_sub(Utc::now().timestamp_millis())
            .max(0)
            + 999)
            / 1000
    }
}

/// Where one kind of delete keeps its records, relative to the journal root.
///
/// ⛔ Never under `health/`: that tree is derived runtime state, which tooling
/// and recovery steps are free to clear. An owner's confirmed delete is intent,
/// and it lives in `config/`, beside the action log.
#[derive(Clone, Copy, Debug)]
pub struct Store {
    dir: &'static str,
}

impl Store {
    pub const fn new(dir: &'static str) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &'static str {
        self.dir
    }

    fn record_path(&self, journal_root: &Path, pending_id: &str) -> PathBuf {
        journal_root
            .join(self.dir)
            .join(format!("{pending_id}.json"))
    }

    /// Serialize every read-modify-write of one record, across processes. A
    /// commit holds it from its claim until the outcome is written.
    pub fn lock(
        &self,
        journal_root: &Path,
        pending_id: &str,
        wait: Duration,
    ) -> Result<FileLock, LockError> {
        hold_lock(
            self.record_path(journal_root, pending_id),
            LockOptions {
                timeout: wait,
                poll_interval: Duration::from_millis(20),
                mode: Some(0o600),
            },
        )
    }

    pub fn write<T: Serialize>(
        &self,
        journal_root: &Path,
        record: &Record<T>,
    ) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        atomic_replace(
            self.record_path(journal_root, &record.pending_id),
            &bytes,
            AtomicWriteOptions { mode: Some(0o600) },
        )
        .map_err(|error| error.to_string())
    }

    pub fn read<T: DeserializeOwned>(
        &self,
        journal_root: &Path,
        pending_id: &str,
    ) -> Option<Record<T>> {
        let bytes = fs::read(self.record_path(journal_root, pending_id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Every readable record.
    fn all<T: DeserializeOwned>(&self, journal_root: &Path) -> Vec<(PathBuf, Record<T>)> {
        let Ok(entries) = fs::read_dir(journal_root.join(self.dir)) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
            .filter_map(|path| {
                let record = fs::read(&path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Record<T>>(&bytes).ok())?;
                Some((path, record))
            })
            .collect()
    }

    /// Every record still waiting to run, earliest deadline first. Finished
    /// records past their retention are removed on the way.
    pub fn pending_and_prune<T: DeserializeOwned>(
        &self,
        journal_root: &Path,
        now: DateTime<Utc>,
    ) -> Vec<Record<T>> {
        let mut pending = Vec::new();
        for (path, record) in self.all::<T>(journal_root) {
            if record.state == DeleteState::Pending {
                pending.push(record);
            } else if finished_days_ago(&record, now)
                .is_some_and(|days| days >= FINISHED_RETENTION_DAYS)
            {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("json.lock"));
            }
        }
        pending.sort_by_key(|record| record.commit_at_ms);
        pending
    }

    /// Held while a delete request checks for a waiting delete and writes its own.
    pub fn create_lock(&self, journal_root: &Path) -> Result<FileLock, LockError> {
        hold_lock(
            journal_root.join(self.dir).join("create"),
            LockOptions {
                timeout: Duration::from_secs(5),
                poll_interval: Duration::from_millis(20),
                mode: Some(0o600),
            },
        )
    }

    /// A delete of the same target still waiting for its window, so a second
    /// request joins it instead of racing it.
    pub fn waiting_for<T: DeserializeOwned>(
        &self,
        journal_root: &Path,
        same_target: impl Fn(&T) -> bool,
    ) -> Option<Record<T>> {
        self.all::<T>(journal_root)
            .into_iter()
            .map(|(_, record)| record)
            .find(|record| {
                record.state == DeleteState::Pending
                    && record.started_at.is_none()
                    && same_target(&record.target)
            })
    }

    /// A pending delete of the same target whose removal has not started,
    /// claimed or not: one a commit may be waiting on, or one a stopped or
    /// giving-up commit left, which a new request should re-arm rather than
    /// run beside.
    pub fn unremoved_for<T: DeserializeOwned>(
        &self,
        journal_root: &Path,
        same_target: impl Fn(&T) -> bool,
    ) -> Option<Record<T>> {
        self.all::<T>(journal_root)
            .into_iter()
            .map(|(_, record)| record)
            .find(|record| {
                record.state == DeleteState::Pending
                    && record.removal_started_at.is_none()
                    && same_target(&record.target)
            })
    }

    /// Deletes that ended without removing their target, still inside
    /// retention, and deletes stuck pending well past their deadline.
    pub fn unfinished_outcomes<T: DeserializeOwned>(
        &self,
        journal_root: &Path,
        now: DateTime<Utc>,
    ) -> Vec<Record<T>> {
        let mut outcomes = self
            .all::<T>(journal_root)
            .into_iter()
            .map(|(_, record)| record)
            .filter(|record| match record.state {
                DeleteState::NotDeleted | DeleteState::Incomplete => finished_days_ago(record, now)
                    .is_some_and(|days| days < FINISHED_RETENTION_DAYS),
                DeleteState::Pending => {
                    now.timestamp_millis() - record.commit_at_ms > OVERDUE_MINUTES * 60_000
                }
                DeleteState::Deleted | DeleteState::Cancelled => false,
            })
            .collect::<Vec<_>>();
        outcomes.sort_by(|left, right| left.finished_at.cmp(&right.finished_at));
        outcomes
    }

    /// Run one delete to its outcome, holding the record's lock from the claim
    /// to the written result, so no other commit or cancel can interleave.
    ///
    /// `run` performs the removal and writes its own action-log row. It
    /// returns the owner outcome, or `None` to leave the record pending with
    /// its claim, so the next start settles it from what is then on disk.
    pub fn commit<T>(
        &self,
        journal_root: &Path,
        pending_id: &str,
        resumed: bool,
        run: impl FnOnce(&Record<T>, Claim) -> Option<(DeleteState, Option<String>)>,
    ) where
        T: Serialize + DeserializeOwned,
    {
        let Ok(_lock) = self.lock(journal_root, pending_id, COMMIT_LOCK_WAIT) else {
            return;
        };
        let Some(mut record) = self.read::<T>(journal_root, pending_id) else {
            return;
        };
        if record.state != DeleteState::Pending {
            return;
        }
        // A claim left by a run that stopped part-way is taken over as a resume.
        let taken_over = record.started_at.is_some();
        let claim = Claim {
            resumed: resumed || taken_over,
            taken_over,
            removal_started: record.removal_started_at.is_some(),
        };
        record.started_at = Some(Utc::now().to_rfc3339());
        if self.write(journal_root, &record).is_err() {
            return;
        }
        let Some((state, reason)) = run(&record, claim) else {
            return;
        };
        // Keep a removal marker the run wrote, so the finished record says so.
        if let Some(latest) = self.read::<T>(journal_root, pending_id) {
            record.removal_started_at = latest.removal_started_at;
        }
        // Written last: a stop before this line leaves the record pending and
        // claimed, and the next start settles it from what is on disk.
        record.finish(state, reason);
        let _ = self.write(journal_root, &record);
    }

    /// Mark, on disk, that this commit's removal is about to change the
    /// journal. Called by a removal under its own locks, while [`commit`]
    /// holds the record's lock.
    ///
    /// [`commit`]: Self::commit
    pub fn mark_removal_started<T>(
        &self,
        journal_root: &Path,
        record: &Record<T>,
    ) -> Result<(), String>
    where
        T: Clone + Serialize,
    {
        let mut marked = record.clone();
        marked.removal_started_at = Some(Utc::now().to_rfc3339());
        self.write(journal_root, &marked)
    }

    /// Settle a cancel against the record. `on_cancelled` runs only when this
    /// call is the one that cancelled it, after the record says so.
    pub fn settle_cancel<T>(
        &self,
        journal_root: &Path,
        pending_id: &str,
        on_cancelled: impl FnOnce(),
    ) -> Settled<T>
    where
        T: Serialize + DeserializeOwned,
    {
        self.settle_unclaimed(
            journal_root,
            pending_id,
            DeleteState::Cancelled,
            None,
            on_cancelled,
        )
    }

    /// Settle a record no commit has claimed, as a cancel does, with the given
    /// outcome. A claimed or finished record is returned unchanged.
    pub fn settle_unclaimed<T>(
        &self,
        journal_root: &Path,
        pending_id: &str,
        state: DeleteState,
        reason: Option<String>,
        on_settled: impl FnOnce(),
    ) -> Settled<T>
    where
        T: Serialize + DeserializeOwned,
    {
        let _lock = match self.lock(journal_root, pending_id, CANCEL_LOCK_WAIT) {
            Ok(lock) => lock,
            Err(LockError::Timeout(_)) => return Settled::Busy,
            Err(error) => return Settled::Failed(error.to_string()),
        };
        let Some(mut record) = self.read::<T>(journal_root, pending_id) else {
            return Settled::Failed("the record could not be read".to_owned());
        };
        if record.state != DeleteState::Pending || record.started_at.is_some() {
            return Settled::Record(Box::new(record));
        }
        record.finish(state, reason);
        if let Err(error) = self.write(journal_root, &record) {
            return Settled::Failed(error);
        }
        on_settled();
        Settled::Record(Box::new(record))
    }

    /// Resume every delete a previous run confirmed and never finished.
    ///
    /// Called once per router, at start. A record still inside its window
    /// waits out the rest of it and can still be cancelled under the same id;
    /// a record whose window passed while the journal was stopped runs
    /// straight away, because the owner confirmed it and let the window go by.
    ///
    /// The router is built before the async runtime starts, so the wait runs
    /// on its own thread rather than as a runtime task. If that thread cannot
    /// start, the records stay pending on disk and the next start tries again.
    ///
    /// The records live in `config/`, which travels with backups: `resumable`
    /// keeps only what the delete route itself would have accepted.
    pub fn resume<T>(
        &self,
        journal_root: &Path,
        registry: &Registry,
        thread_name: &str,
        resumable: impl Fn(&Record<T>) -> bool,
        commit: impl Fn(&Path, &str) + Send + 'static,
    ) where
        T: DeserializeOwned,
    {
        let ids = self
            .pending_and_prune::<T>(journal_root, Utc::now())
            .into_iter()
            .filter(|record| valid_pending_id(&record.pending_id) && resumable(record))
            .map(|record| (record.pending_id, record.commit_at_ms))
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        for (pending_id, _) in &ids {
            registry.hold(pending_id.clone());
        }
        let root = journal_root.to_path_buf();
        let waiter = registry.clone();
        let _ = std::thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                for (pending_id, commit_at_ms) in ids {
                    // No real window is longer than a few seconds; a damaged
                    // deadline must not hold every later delete behind it.
                    let wait = commit_at_ms.saturating_sub(Utc::now().timestamp_millis());
                    if let Ok(wait) = u64::try_from(wait) {
                        std::thread::sleep(Duration::from_millis(wait).min(MAX_RESUME_WAIT));
                    }
                    if waiter.claim(&pending_id) {
                        commit(&root, &pending_id);
                    }
                }
            });
    }
}

/// How a commit came to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim {
    /// Run after a restart, or taking over a claim a stopped run left.
    pub resumed: bool,
    /// A previous run had claimed the record and stopped before its outcome
    /// was written, so some of the removal may already have happened.
    pub taken_over: bool,
    /// That previous run had marked its removal as started.
    pub removal_started: bool,
}

/// What a cancel found.
pub enum Settled<T> {
    /// A commit holds the record: it is removing now.
    Busy,
    Failed(String),
    Record(Box<Record<T>>),
}

fn finished_days_ago<T>(record: &Record<T>, now: DateTime<Utc>) -> Option<i64> {
    let finished = DateTime::parse_from_rfc3339(record.finished_at.as_deref()?).ok()?;
    Some(
        now.signed_duration_since(finished.with_timezone(&Utc))
            .num_days(),
    )
}

/// The ids this journal issues: 32 lowercase hex digits.
pub fn valid_pending_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// In-process arbitration between a held delete and its cancel. Whichever
/// removes the entry wins.
#[derive(Clone, Default)]
pub struct Registry {
    handles: Arc<Mutex<HashMap<String, Option<tokio::task::AbortHandle>>>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Schedule one commit. The task removes its own handle before invoking the
    /// closure, so cancellation after the deadline observes it as unavailable.
    ///
    /// A zero delay commits on this thread before `schedule` returns. Spawning
    /// that path made the commit a race against handle insertion and against
    /// any caller polling with cooperative yields.
    pub fn schedule(
        &self,
        pending_id: String,
        delay: Duration,
        commit: impl FnOnce() + Send + 'static,
    ) {
        if delay.is_zero() {
            commit();
            return;
        }
        // Hold the id before the task exists, so a deadline that fires at
        // once still finds it and commits.
        self.hold(pending_id.clone());
        let handles = Arc::clone(&self.handles);
        let task_pending_id = pending_id.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if handles
                .lock()
                .expect("held-delete registry mutex is not poisoned")
                .remove(&task_pending_id)
                .is_none()
            {
                return;
            }
            // Removal is blocking filesystem work; keep it off the async workers.
            let _ = tokio::task::spawn_blocking(commit).await;
        });
        if let Some(slot) = self
            .handles
            .lock()
            .expect("held-delete registry mutex is not poisoned")
            .get_mut(&pending_id)
        {
            *slot = Some(task.abort_handle());
        }
    }

    /// Hold an id whose deadline a resume thread is waiting on, so a cancel
    /// can still reach it. The waiter must [`claim`](Self::claim) it first.
    pub fn hold(&self, pending_id: String) {
        self.handles
            .lock()
            .expect("held-delete registry mutex is not poisoned")
            .insert(pending_id, None);
    }

    /// Whether this process still has a timer or a waiter for the id.
    pub fn contains(&self, pending_id: &str) -> bool {
        self.handles
            .lock()
            .expect("held-delete registry mutex is not poisoned")
            .contains_key(pending_id)
    }

    /// Take a held id at its deadline. `false` means a cancel got there first.
    pub fn claim(&self, pending_id: &str) -> bool {
        self.handles
            .lock()
            .expect("held-delete registry mutex is not poisoned")
            .remove(pending_id)
            .is_some()
    }

    pub fn cancel(&self, pending_id: &str) -> bool {
        let handle = self
            .handles
            .lock()
            .expect("held-delete registry mutex is not poisoned")
            .remove(pending_id);
        match handle {
            Some(Some(handle)) => {
                handle.abort();
                true
            }
            Some(None) => true,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use chrono::Utc;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    use super::{Claim, DeleteState, Record, Registry, Store};

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Thing {
        name: String,
    }

    const STORE: Store = Store::new("config/test-deletes");

    fn record(id: &str, state: DeleteState, commit_at_ms: i64) -> Record<Thing> {
        let mut record = Record::pending(id.repeat(32), Thing { name: id.into() }, Duration::ZERO);
        record.state = state;
        record.commit_at_ms = commit_at_ms;
        record
    }

    #[tokio::test]
    async fn cancellation_prevents_the_scheduled_commit() {
        let registry = Registry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let commit_calls = Arc::clone(&calls);
        registry.schedule("a".into(), Duration::from_secs(60), move || {
            commit_calls.fetch_add(1, Ordering::SeqCst);
        });
        assert!(registry.cancel("a"));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!registry.cancel("a"));
    }

    #[tokio::test]
    async fn deadline_commits_once_and_makes_the_id_unavailable() {
        let registry = Registry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let commit_calls = Arc::clone(&calls);
        let done = Arc::new(tokio::sync::Notify::new());
        let commit_done = Arc::clone(&done);
        registry.schedule("b".into(), Duration::from_millis(1), move || {
            commit_calls.fetch_add(1, Ordering::SeqCst);
            commit_done.notify_one();
        });
        done.notified().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!registry.cancel("b"));
    }

    #[test]
    fn zero_delay_commits_before_schedule_returns() {
        let registry = Registry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let commit_calls = Arc::clone(&calls);
        registry.schedule("c".into(), Duration::ZERO, move || {
            commit_calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!registry.cancel("c"));
    }

    #[test]
    fn a_held_id_goes_to_whichever_of_cancel_and_claim_comes_first() {
        let registry = Registry::new();
        registry.hold("d".into());
        assert!(registry.cancel("d"));
        assert!(!registry.claim("d"));
        registry.hold("e".into());
        assert!(registry.claim("e"));
        assert!(!registry.cancel("e"));
    }

    #[test]
    fn the_target_sits_at_the_top_level_of_the_record() {
        let root = TempDir::new().unwrap();
        let written = record("a", DeleteState::Pending, 10);
        STORE.write(root.path(), &written).unwrap();
        let bytes = std::fs::read(
            root.path()
                .join("config/test-deletes")
                .join(format!("{}.json", "a".repeat(32))),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["name"], "a");
        assert_eq!(
            STORE.read::<Thing>(root.path(), &"a".repeat(32)),
            Some(written)
        );
    }

    #[test]
    fn pending_records_are_returned_by_deadline_and_old_finished_ones_are_pruned() {
        let root = TempDir::new().unwrap();
        STORE
            .write(root.path(), &record("b", DeleteState::Pending, 20))
            .unwrap();
        STORE
            .write(root.path(), &record("a", DeleteState::Pending, 10))
            .unwrap();
        let mut old = record("c", DeleteState::Deleted, 0);
        old.finished_at = Some((Utc::now() - chrono::Duration::days(8)).to_rfc3339());
        STORE.write(root.path(), &old).unwrap();
        let mut recent = record("d", DeleteState::NotDeleted, 0);
        recent.finished_at = Some(Utc::now().to_rfc3339());
        STORE.write(root.path(), &recent).unwrap();

        let pending = STORE.pending_and_prune::<Thing>(root.path(), Utc::now());
        assert_eq!(
            pending
                .iter()
                .map(|record| record.commit_at_ms)
                .collect::<Vec<_>>(),
            vec![10, 20]
        );
        assert!(STORE.read::<Thing>(root.path(), &"c".repeat(32)).is_none());
        assert_eq!(
            STORE
                .read::<Thing>(root.path(), &"d".repeat(32))
                .unwrap()
                .state,
            DeleteState::NotDeleted
        );
    }

    #[test]
    fn a_commit_that_leaves_no_outcome_keeps_the_record_pending_and_claimed() {
        let root = TempDir::new().unwrap();
        let id = "f".repeat(32);
        STORE
            .write(root.path(), &record("f", DeleteState::Pending, 0))
            .unwrap();
        STORE.commit::<Thing>(root.path(), &id, false, |_, _| None);
        let left = STORE.read::<Thing>(root.path(), &id).unwrap();
        assert_eq!(left.state, DeleteState::Pending);
        assert!(left.started_at.is_some());

        // The next run takes the claim over as a resume and settles it.
        let mut seen = None;
        STORE.commit::<Thing>(root.path(), &id, false, |_, claim| {
            seen = Some(claim);
            Some((DeleteState::Deleted, None))
        });
        assert_eq!(
            seen,
            Some(Claim {
                resumed: true,
                taken_over: true,
                removal_started: false,
            })
        );
        assert_eq!(
            STORE.read::<Thing>(root.path(), &id).unwrap().state,
            DeleteState::Deleted
        );
    }
}
