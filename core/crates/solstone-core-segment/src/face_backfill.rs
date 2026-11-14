// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
//! Private describe-helper protocol shared by maintenance and the producer.
pub const OPTION: &str = "--backfill-faces";
pub const UNSUPPORTED: i32 = 65;
pub const YIELDED: i32 = 75;
pub const MAX_SECONDS: u64 = 30 * 60;

use solstone_core_journal_io::{FileLock, LockError, LockOptions, hold_lock};
use std::{path::Path, time::Duration};

/// One model pair. Live intent prevents history reacquiring between segments.
pub struct ModelLease {
    _model: FileLock,
    _live: Option<FileLock>,
}
fn options(timeout: Duration) -> LockOptions {
    LockOptions {
        timeout,
        poll_interval: Duration::from_millis(20),
        ..Default::default()
    }
}
fn intent(journal: &Path, timeout: Duration) -> Result<FileLock, LockError> {
    hold_lock(journal.join("health").join("face-live"), options(timeout))
}
/// Probes a coordination lock only; it contains no journal content.
pub fn live_requested(journal: &Path) -> Result<bool, LockError> {
    solstone_core_journal_io::lock_is_held(journal.join("health").join("face-live"))
}
/// Speaker maintenance announces priority without taking the live model lease.
/// Scheduled partitions already serialize each routine; distinct routines keep
/// distinct locks so one finishing cannot hide another still running.
const SPEAKER_ROUTINES: [&str; 6] = [
    "discover-voices",
    "candidate-pair-suggestions",
    "name-variants",
    "consolidate-pool",
    "backfill-pool",
    "backfill-labels",
];
fn speaker_path(journal: &Path, name: &str) -> std::path::PathBuf {
    journal
        .join("health")
        .join(format!("face-priority-speakers-{name}"))
}
pub fn speaker_priority_lease(journal: &Path, id: &str) -> Result<Option<FileLock>, LockError> {
    let Some(name) = id
        .strip_prefix("speakers:")
        .filter(|n| SPEAKER_ROUTINES.contains(n))
    else {
        return Ok(None);
    };
    match hold_lock(speaker_path(journal, name), options(Duration::ZERO)) {
        Ok(lock) => Ok(Some(lock)),
        Err(LockError::Timeout(_)) => Ok(None), // the same routine already announced priority
        Err(error) => Err(error),
    }
}
pub fn priority_requested(journal: &Path) -> Result<bool, LockError> {
    if live_requested(journal)? {
        return Ok(true);
    }
    for name in SPEAKER_ROUTINES {
        if solstone_core_journal_io::lock_is_held(speaker_path(journal, name))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The native child and its parent both stop when live intent appears. The
/// managed launch's bounded termination releases a wedged backfill's model too.
pub fn backfill_lease(journal: &Path) -> Result<ModelLease, LockError> {
    if priority_requested(journal)? {
        return Err(LockError::Timeout(solstone_core_journal_io::LockTimeout {
            path: journal.join("health").join("face-live"),
            timeout: Duration::ZERO,
        }));
    }
    let model = hold_lock(
        journal.join("health").join("face-inference"),
        options(Duration::ZERO),
    )?;
    Ok(ModelLease {
        _model: model,
        _live: None,
    })
}
/// Live work holds intent until its pass ends. Fifteen seconds covers the
/// backfill parent's 200ms observation and two-second native termination budget.
pub fn live_lease(journal: &Path) -> Result<ModelLease, LockError> {
    let live = intent(journal, Duration::from_secs(15))?;
    let model = hold_lock(
        journal.join("health").join("face-inference"),
        options(Duration::from_secs(15)),
    )?;
    Ok(ModelLease {
        _model: model,
        _live: Some(live),
    })
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    #[test]
    fn speaker_priority_yields_history_without_blocking_live() {
        let root = crate::test_support::TempDir::new();
        let one = speaker_priority_lease(root.path(), "speakers:discover-voices").unwrap();
        let two = speaker_priority_lease(root.path(), "speakers:backfill-labels").unwrap();
        assert!(backfill_lease(root.path()).is_err());
        drop(one);
        assert!(priority_requested(root.path()).unwrap());
        let live = live_lease(root.path()).unwrap();
        drop(live);
        drop(two);
        assert!(!priority_requested(root.path()).unwrap());
        assert!(backfill_lease(root.path()).is_ok());
    }
    #[test]
    fn live_preempts_history_and_history_cannot_reacquire_until_live_finishes() {
        let root = crate::test_support::TempDir::new();
        let history = backfill_lease(root.path()).unwrap();
        let (send, recv) = std::sync::mpsc::channel();
        let journal = root.path().to_owned();
        let live = std::thread::spawn(move || {
            let lease = live_lease(&journal).unwrap();
            send.send(()).unwrap();
            lease
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !live_requested(root.path()).unwrap() {
            assert!(
                std::time::Instant::now() < deadline,
                "live waiter never announced intent"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(backfill_lease(root.path()).is_err());
        assert!(recv.try_recv().is_err());
        // The same condition makes the native backfill helper yield.
        drop(history);
        recv.recv_timeout(Duration::from_secs(5)).unwrap();
        let lease = live.join().unwrap();
        assert!(backfill_lease(root.path()).is_err());
        drop(lease);
        assert!(backfill_lease(root.path()).is_ok());
    }
}
