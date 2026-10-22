// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The one search index health value: ok, behind, failing or building.
//!
//! Every surface that reports on the index (the health page, `solstone call health`,
//! doctor) reads this value; none derives its own. It measures the index against
//! the disk through the read-only status and never writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_indexer_store::db::IndexBuildLifecycle;
use solstone_core_indexer_store::status::{
    ClassificationProgress, IndexDatabase, IndexStatus, inspect_index,
};

use crate::indexing_observation::{IndexingObservations, read_indexing_observations};

pub const INDEX_TEXT_OK: &str = "search is up to date with your journal.";
pub const INDEX_TEXT_BUILDING: &str = "search is still being built.";
pub const INDEX_TEXT_FAILED_FILES: &str = "some journal updates couldn't be added to search.";
pub const INDEX_TEXT_UNREADABLE: &str = "search can't read its index right now.";
pub const INDEX_TEXT_NEWER_GENERATION: &str =
    "a newer version of solstone last updated search, so this version won't add new changes to it.";
pub const INDEX_TEXT_REPAIR: &str = "search stopped updating and needs a repair.";
pub const INDEX_TEXT_CLASSIFICATION_STALLED: &str =
    "search stopped sorting some older entries into your facets and needs a repair.";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexHealthState {
    Ok,
    Behind,
    Failing,
    Building,
}

impl IndexHealthState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Behind => "behind",
            Self::Failing => "failing",
            Self::Building => "building",
        }
    }
}

/// Why the index is failing, when it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexFailure {
    /// The index file exists but cannot be read.
    Unreadable,
    /// A newer version wrote the index; this one refuses to write it.
    NewerGeneration,
    /// Classification memberships are missing; every write refuses until a reset.
    MembershipsMissing,
    /// Classification stopped at a path it cannot get past.
    ClassificationStalled,
    /// Recorded updates failed and those files are still not current.
    FailedFiles,
    /// The last scan stopped before finishing, or the journal could not be walked.
    ScanFailed,
    /// The index's generation stamp cannot be read; every write refuses.
    StampUnreadable,
    FailedDeletions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexHealth {
    pub state: IndexHealthState,
    pub failure: Option<IndexFailure>,
    pub text: String,
    /// Changes on disk not yet in the index (stale + missing + orphaned).
    pub pending: usize,
    pub stale: usize,
    pub missing: usize,
    pub orphaned: usize,
    /// Rows for removed entries that only a full rescan removes.
    pub retained: usize,
    /// Files that failed to index (recorded updates, or the last scan) and are
    /// still not current.
    pub failed: usize,
    pub discovered: usize,
    pub indexed: usize,
    pub generation: Option<i64>,
    pub last_scan_at_ms: Option<i64>,
    #[serde(default)]
    pub queued: usize,
    #[serde(default)]
    pub failed_deletions: usize,
    #[serde(default)]
    pub last_finished_check_at_ms: Option<i64>,
    #[serde(default)]
    pub check_indexed: usize,
    #[serde(default)]
    pub check_removed: usize,
}

/// Measure the index and fold recent failed updates into one value.
pub fn evaluate_index_health(journal: &Path, now: DateTime<Utc>) -> IndexHealth {
    evaluate_index_health_with(journal, &read_indexing_observations(journal, now))
}

/// As [`evaluate_index_health`], for a caller that already read the outcomes.
pub fn evaluate_index_health_with(
    journal: &Path,
    observations: &IndexingObservations,
) -> IndexHealth {
    match inspect_index(journal) {
        Ok(status) => with_work_list(journal, index_health_from(&status, observations)),
        Err(error) => {
            log::warn!("search index status unavailable: {error}");
            with_work_list(journal, unmeasured())
        }
    }
}

/// A measurement that takes longer than this is reused by the web surfaces.
const EXPENSIVE_MEASUREMENT: Duration = Duration::from_millis(250);
/// How long a reused measurement serves before a background refresh starts.
const REUSE_FOR: Duration = Duration::from_secs(60);

struct Measured {
    status: Arc<IndexStatus>,
    at: Instant,
    refreshing: bool,
}

fn measurements() -> &'static Mutex<HashMap<PathBuf, Measured>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Measured>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn measure(journal: &Path) -> Result<Arc<IndexStatus>, String> {
    let started = Instant::now();
    let measured = inspect_index(journal);
    let mut cache = measurements()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // A measurement that began before the stored one finished is older than
    // it: it neither replaces nor removes it.
    let superseded = cache.get(journal).is_some_and(|entry| entry.at > started);
    let status = match measured {
        Ok(status) => Arc::new(status),
        Err(error) => {
            if !superseded {
                cache.remove(journal);
            }
            return Err(error.to_string());
        }
    };
    if superseded {
        return Ok(status);
    }
    if started.elapsed() >= EXPENSIVE_MEASUREMENT
        && !matches!(status.database, IndexDatabase::Unreadable(_))
    {
        cache.insert(
            journal.to_path_buf(),
            Measured {
                status: Arc::clone(&status),
                at: Instant::now(),
                refreshing: false,
            },
        );
    } else {
        cache.remove(journal);
    }
    Ok(status)
}

/// As [`evaluate_index_health_with`] for a page that is read often. A journal
/// whose measurement is cheap is measured every time. One that is expensive
/// (hundreds of thousands of files) reuses its last measurement for up to a
/// minute, then serves it while one background refresh runs, so a page load
/// never waits on a whole-journal walk twice. Doctor and the CLI measure fresh.
pub fn evaluate_index_health_recent(
    journal: &Path,
    observations: &IndexingObservations,
) -> IndexHealth {
    let reused = {
        let mut cache = measurements()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match cache.get_mut(journal) {
            Some(entry) if entry.at.elapsed() < REUSE_FOR => Some(Arc::clone(&entry.status)),
            // A refresh that has not come back in two more periods is not
            // waited on: measure here instead of serving an old reading.
            Some(entry) if entry.at.elapsed() >= REUSE_FOR * 3 => None,
            Some(entry) => {
                if !entry.refreshing {
                    entry.refreshing = true;
                    let journal = journal.to_path_buf();
                    std::thread::spawn(move || {
                        // A failed refresh drops the entry (in `measure`), so the
                        // next read measures for itself.
                        if let Err(error) = measure(&journal) {
                            log::warn!("search index status refresh failed: {error}");
                        }
                    });
                }
                Some(Arc::clone(&entry.status))
            }
            None => None,
        }
    };
    match reused.map_or_else(|| measure(journal), Ok) {
        Ok(status) => with_work_list(journal, index_health_from(&status, observations)),
        Err(error) => {
            log::warn!("search index status unavailable: {error}");
            with_work_list(journal, unmeasured())
        }
    }
}

/// The journal walk itself failed; the scan's walk fails the same way.
fn unmeasured() -> IndexHealth {
    IndexHealth {
        state: IndexHealthState::Failing,
        failure: Some(IndexFailure::ScanFailed),
        text: INDEX_TEXT_FAILED_FILES.to_owned(),
        pending: 0,
        stale: 0,
        missing: 0,
        orphaned: 0,
        retained: 0,
        failed: 0,
        discovered: 0,
        indexed: 0,
        generation: None,
        last_scan_at_ms: None,
        queued: 0,
        failed_deletions: 0,
        last_finished_check_at_ms: None,
        check_indexed: 0,
        check_removed: 0,
    }
}

/// The health value for a measured status and the recorded update outcomes.
#[must_use]
pub fn index_health_from(status: &IndexStatus, observations: &IndexingObservations) -> IndexHealth {
    let behind = |path: &String| {
        status.stale.contains(path) || status.missing.contains(path) || status.failed.contains(path)
    };
    let failed = observations
        .outcomes
        .iter()
        .filter(|o| matches!(o.outcome.as_str(), "failed" | "declined" | "ambiguous"))
        .map(|o| &o.identity)
        .filter(|identity| behind(identity))
        .chain(status.failed.iter())
        // A file whose modification time cannot be read is one the scan skips
        // and warns about every time.
        .chain(status.unreadable.iter())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        + status.unattributed_failures;
    let pending = status.pending();
    let retained = status.retained.len();
    let scan_stopped = status
        .last_scan
        .as_ref()
        .is_some_and(|scan| scan.error.is_some());

    let failure = if matches!(status.database, IndexDatabase::Unreadable(_)) {
        Some(IndexFailure::Unreadable)
    } else if status.stamp_error.is_some() {
        Some(IndexFailure::StampUnreadable)
    } else if !status.writable() {
        Some(IndexFailure::NewerGeneration)
    } else if status.memberships_missing {
        Some(IndexFailure::MembershipsMissing)
    } else if matches!(
        status.classification,
        ClassificationProgress::Stalled { .. }
    ) {
        Some(IndexFailure::ClassificationStalled)
    } else if scan_stopped {
        Some(IndexFailure::ScanFailed)
    } else if failed > 0 {
        Some(IndexFailure::FailedFiles)
    } else {
        None
    };

    let state = if failure.is_some() {
        IndexHealthState::Failing
    } else if building(status) {
        IndexHealthState::Building
    } else if pending > 0
        || retained > 0
        || status.classification == ClassificationProgress::Incomplete
    {
        IndexHealthState::Behind
    } else {
        IndexHealthState::Ok
    };

    let text = match (state, failure) {
        (_, Some(IndexFailure::Unreadable)) => INDEX_TEXT_UNREADABLE.to_owned(),
        (_, Some(IndexFailure::NewerGeneration)) => INDEX_TEXT_NEWER_GENERATION.to_owned(),
        (_, Some(IndexFailure::MembershipsMissing | IndexFailure::StampUnreadable)) => {
            INDEX_TEXT_REPAIR.to_owned()
        }
        (_, Some(IndexFailure::ClassificationStalled)) => {
            INDEX_TEXT_CLASSIFICATION_STALLED.to_owned()
        }
        (_, Some(IndexFailure::FailedFiles | IndexFailure::ScanFailed)) => {
            INDEX_TEXT_FAILED_FILES.to_owned()
        }
        (IndexHealthState::Building, None) => INDEX_TEXT_BUILDING.to_owned(),
        // Nothing the next scan does removes retained rows, so their line makes
        // no catching-up promise.
        (IndexHealthState::Behind, None) if pending == 0 && retained > 0 => retained_text(retained),
        (IndexHealthState::Behind, None) => behind_text(pending),
        _ => INDEX_TEXT_OK.to_owned(),
    };

    IndexHealth {
        state,
        failure,
        text,
        pending,
        stale: status.stale.len(),
        missing: status.missing.len(),
        orphaned: status.orphaned.len(),
        retained,
        failed,
        discovered: status.discovered,
        indexed: status.indexed,
        generation: status
            .stamp
            .as_ref()
            .map(solstone_core_indexer_store::generation::IndexStamp::effective_generation),
        last_scan_at_ms: status.last_scan.as_ref().map(|scan| scan.finished_at_ms),
        queued: 0,
        failed_deletions: 0,
        last_finished_check_at_ms: None,
        check_indexed: 0,
        check_removed: 0,
    }
}

fn with_work_list(journal: &Path, mut health: IndexHealth) -> IndexHealth {
    match solstone_core_indexer_store::todo::inspect(journal) {
        Ok(todo) => {
            health.queued = todo.keys.len();
            health.failed_deletions = todo.failed_deletions;
            health.pending = health.pending.max(health.queued);
            if health.failed_deletions > 0
                && health.failure.is_none_or(|failure| {
                    matches!(
                        failure,
                        IndexFailure::FailedFiles | IndexFailure::ScanFailed
                    )
                })
            {
                health.state = IndexHealthState::Failing;
                health.failure = Some(IndexFailure::FailedDeletions);
                health.text = "some removed entries couldn't be taken out of search.".to_owned();
            } else if health.queued > 0 && health.state == IndexHealthState::Ok {
                health.state = IndexHealthState::Behind;
                health.text = "search is catching up with your journal.".to_owned();
            }
        }
        Err(error) => {
            log::warn!("search work list could not be read: {error}");
            health.state = IndexHealthState::Failing;
            health.failure = Some(IndexFailure::ScanFailed);
            health.text = "search can't check its queued updates right now.".to_owned();
        }
    }
    if let Ok(conn) = solstone_core_indexer_store::open_index_reader(journal) {
        match solstone_core_indexer_store::generation::read_last_finished_check(&conn) {
            Ok(Some(check)) => {
                health.last_finished_check_at_ms = Some(check.finished_at_ms);
                health.check_indexed = check.indexed;
                health.check_removed = check.removed;
            }
            Ok(None) => {}
            Err(error) => log::warn!("last finished search check could not be read: {error}"),
        }
    }
    health
}

/// Building means the index holds less than half of what it should: nothing
/// at all yet, or a rebuild that has not passed the halfway mark. A finished
/// rebuild whose flag never cleared, with ordinary changes pending, is behind.
fn building(status: &IndexStatus) -> bool {
    let missing = status.missing.len();
    missing > 0
        && (status.current == 0
            || (status.build == Some(IndexBuildLifecycle::Building) && missing > status.current))
}

/// The line for rows of removed entries that only a full rescan removes.
#[must_use]
pub fn retained_text(retained: usize) -> String {
    match retained {
        1 => "search still shows 1 entry that is no longer in your journal.".to_owned(),
        n => format!("search still shows {n} entries that are no longer in your journal."),
    }
}

/// The behind line. A classification pass with nothing pending on disk reads
/// as catching up without a count.
#[must_use]
pub fn behind_text(pending: usize) -> String {
    match pending {
        0 => "search is catching up with your journal.".to_owned(),
        1 => "search is catching up with 1 change in your journal.".to_owned(),
        n => format!("search is catching up with {n} changes in your journal."),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use solstone_core_indexer_store::generation::IndexStamp;

    use super::*;
    use crate::indexing_observation::{IndexingAttemptObservation, IndexingDiagnostics};

    #[test]
    fn a_dropped_present_failure_keeps_its_reason_until_the_file_indexes() {
        let journal = tempfile::tempdir().unwrap();
        let source = journal.path().join("imports/fixture/summary.md");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "original kestrel").unwrap();
        solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
        let original_mtime = std::fs::metadata(&source).unwrap().modified().unwrap();
        std::fs::write(&source, [0xff]).unwrap();
        // Setting a time needs a writable handle on Windows.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(original_mtime)
            .unwrap();
        solstone_core_indexer_store::todo::enqueue_path(journal.path(), &source).unwrap();
        for _ in 0..3 {
            let report = solstone_core_indexer_store::todo::drain(
                journal.path(),
                solstone_core_indexer_store::todo::DrainOptions {
                    force: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(report.failed, 1);
        }
        assert!(
            solstone_core_indexer_store::todo::inspect(journal.path())
                .unwrap()
                .keys
                .is_empty()
        );
        let health = evaluate_index_health(journal.path(), Utc::now());
        assert_eq!(health.failure, Some(IndexFailure::FailedFiles));
        assert!(health.failed > 0);
        let observations = read_indexing_observations(journal.path(), Utc::now());
        assert!(
            observations
                .outcomes
                .iter()
                .any(|entry| entry.path == "imports/fixture/summary.md"
                    && entry.outcome == "failed"
                    && entry.cause.is_some())
        );
        solstone_core_indexer_store::todo::drain(journal.path(), Default::default()).unwrap();
        assert!(evaluate_index_health(journal.path(), Utc::now()).failed > 0);
        std::fs::write(&source, "repaired kestrel").unwrap();
        // Setting a time needs a writable handle on Windows.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(original_mtime)
            .unwrap();
        let check = solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
        assert_eq!((check.indexed, check.failed), (1, 0));
        assert_eq!(evaluate_index_health(journal.path(), Utc::now()).failed, 0);
    }

    #[test]
    fn failed_deletions_stay_visible_after_an_empty_day_light_check() {
        let journal = tempfile::tempdir().unwrap();
        let key = "20260101/device/120000_60/talents/activity.md";
        solstone_core_indexer_store::todo::enqueue_path(journal.path(), Path::new(key)).unwrap();
        let path = std::fs::read_dir(journal.path().join("indexer/todo"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let mut entry: solstone_core_indexer_store::todo::Entry =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        entry.attempts = 3;
        entry.not_before_ms = u64::MAX;
        std::fs::write(path, serde_json::to_vec(&entry).unwrap()).unwrap();
        solstone_core_indexer_store::scan::scan_journal(journal.path(), false).unwrap();
        let health = evaluate_index_health_with(journal.path(), &observations(&[]));
        assert_eq!(health.failure, Some(IndexFailure::FailedDeletions));
        assert_eq!((health.queued, health.failed_deletions), (1, 1));
        assert!(health.last_finished_check_at_ms.is_some());
        assert_eq!(health.check_removed, 0);
        let mut unreadable = unmeasured();
        unreadable.failure = Some(IndexFailure::Unreadable);
        assert_eq!(
            with_work_list(journal.path(), unreadable).failure,
            Some(IndexFailure::Unreadable)
        );
    }

    fn status() -> IndexStatus {
        IndexStatus {
            database: IndexDatabase::Present,
            stamp: Some(IndexStamp::default()),
            build: Some(IndexBuildLifecycle::Complete),
            last_scan: None,
            classification: ClassificationProgress::Complete,
            memberships_missing: false,
            path_lookup_ready: true,
            discovered: 10,
            indexed: 10,
            current: 10,
            stale: BTreeSet::new(),
            missing: BTreeSet::new(),
            orphaned: BTreeSet::new(),
            retained: BTreeSet::new(),
            failed: BTreeSet::new(),
            unattributed_failures: 0,
            stamp_error: None,
            ineligible_rows: 0,
            unreadable: BTreeSet::new(),
            ineligible: 0,
        }
    }

    fn observations(failed: &[&str]) -> IndexingObservations {
        IndexingObservations {
            outcomes: failed
                .iter()
                .map(|path| IndexingAttemptObservation {
                    path: (*path).to_owned(),
                    identity: (*path).to_owned(),
                    outcome: "failed".to_owned(),
                    ts: 0,
                    warnings: Vec::new(),
                    cause: None,
                })
                .collect(),
            diagnostics: IndexingDiagnostics {
                think_days: Vec::new(),
                window_start_ms: 0,
                window_end_ms: 0,
                partial: false,
                lines: Vec::new(),
            },
        }
    }

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|path| (*path).to_owned()).collect()
    }

    #[test]
    fn a_failed_update_counts_only_while_its_file_is_still_behind() {
        let mut behind = status();
        behind.stale = set(&["20261005/a.md"]);
        behind.current = 9;

        // A failure the next scan repaired, or for a file now gone, is history.
        let repaired = index_health_from(&status(), &observations(&["20261005/a.md"]));
        assert_eq!((repaired.state, repaired.failed), (IndexHealthState::Ok, 0));
        assert_eq!(repaired.text, INDEX_TEXT_OK);

        let open = index_health_from(&behind, &observations(&["20261005/a.md"]));
        assert_eq!(open.state, IndexHealthState::Failing);
        assert_eq!(open.failure, Some(IndexFailure::FailedFiles));
        assert_eq!(open.failed, 1);
        assert_eq!(open.text, INDEX_TEXT_FAILED_FILES);
    }

    #[test]
    fn each_state_has_one_rule() {
        let mut pending = status();
        pending.missing = set(&["20261005/new.md"]);
        pending.orphaned = set(&["20261004/old.md"]);
        let behind = index_health_from(&pending, &observations(&[]));
        assert_eq!(behind.state, IndexHealthState::Behind);
        assert_eq!(
            behind.text,
            "search is catching up with 2 changes in your journal."
        );

        let mut empty = status();
        empty.database = IndexDatabase::Absent;
        empty.stamp = None;
        empty.current = 0;
        empty.indexed = 0;
        empty.missing = set(&["20261005/a.md"]);
        assert_eq!(
            index_health_from(&empty, &observations(&[])).state,
            IndexHealthState::Building
        );

        // A rebuild flag that never cleared does not make ordinary changes "building".
        let mut stuck_flag = pending.clone();
        stuck_flag.build = Some(IndexBuildLifecycle::Building);
        assert_eq!(
            index_health_from(&stuck_flag, &observations(&[])).state,
            IndexHealthState::Behind
        );

        let mut newer = status();
        newer.stamp = Some(IndexStamp {
            user_version: 2,
            ..IndexStamp::default()
        });
        let refused = index_health_from(&newer, &observations(&[]));
        assert_eq!(refused.failure, Some(IndexFailure::NewerGeneration));
        assert_eq!(refused.text, INDEX_TEXT_NEWER_GENERATION);

        // Rows for a removed day that only a full rescan removes: behind, with
        // a line that promises no catching up.
        let mut kept = status();
        kept.retained = set(&["20261001/talents/gone.md"]);
        let retained = index_health_from(&kept, &observations(&[]));
        assert_eq!(retained.state, IndexHealthState::Behind);
        assert_eq!(
            retained.text,
            "search still shows 1 entry that is no longer in your journal."
        );

        // A file the last scan could not index, and a scan that stopped.
        let mut scan_failed = status();
        scan_failed.failed = set(&["20261005/bad.md"]);
        let failed = index_health_from(&scan_failed, &observations(&["20261005/bad.md"]));
        assert_eq!(
            (failed.failure, failed.failed),
            (Some(IndexFailure::FailedFiles), 1)
        );
        let mut stopped = status();
        stopped.last_scan = Some(solstone_core_indexer_store::generation::ScanReceipt {
            error: Some("disk full".to_owned()),
            ..Default::default()
        });
        assert_eq!(
            index_health_from(&stopped, &observations(&[])).failure,
            Some(IndexFailure::ScanFailed)
        );

        let mut hard_stop = status();
        hard_stop.memberships_missing = true;
        assert_eq!(
            index_health_from(&hard_stop, &observations(&[])).failure,
            Some(IndexFailure::MembershipsMissing)
        );
    }
}
