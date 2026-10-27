// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_backup_web::{OffloadDispatchProbe, VerificationSubmitter};
use solstone_core_journal_io::JournalRoot;
use std::sync::Arc;
use tokio::sync::watch;

/// A fixed queue door belonging to this particular serving lifetime.
pub(super) fn submitter(
    root: &Arc<JournalRoot>,
    stop: watch::Receiver<bool>,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> VerificationSubmitter {
    let root = Arc::downgrade(root);
    Arc::new(move || {
        if *stop.borrow() || stop.has_changed().is_err() || cancelled() {
            return Err("queue unavailable");
        }
        let root = root.upgrade().ok_or("queue unavailable")?;
        super::foreground::submit_owner_maintenance(&root, "backup:verify")
    })
}

/// Observe the existing command admission without enqueueing work. Missing
/// compiled offload dispatch stays unavailable even with valid native tools.
pub(super) fn offload_dispatch_probe(
    root: &Arc<JournalRoot>,
    stop: watch::Receiver<bool>,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> OffloadDispatchProbe {
    dispatch_probe(root, stop, cancelled, "backup:offload")
}

fn dispatch_probe(
    root: &Arc<JournalRoot>,
    stop: watch::Receiver<bool>,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    routine: &'static str,
) -> OffloadDispatchProbe {
    let root = Arc::downgrade(root);
    let command =
        solstone_core_system::partition::canonical_journal_command(["maintenance", "run", routine]);
    Arc::new(move || {
        if *stop.borrow() || stop.has_changed().is_err() || cancelled() {
            return false;
        }
        let Some(root) = root.upgrade() else {
            return false;
        };
        root.revalidate_canonical_binding().is_ok()
            && super::foreground::admit_command(root.canonical_path(), &command)
            && !cancelled()
            && root.revalidate_canonical_binding().is_ok()
    })
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::foreground::{ForegroundProcessingHost, TEST_LOCK};
    use solstone_core_backup_web::BackupWebDeps;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn dispatch_observation_does_not_submit_and_preserves_restore_stop_and_lifetime_refusal() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let (stop, watch) = watch::channel(false);
        let probe = dispatch_probe(&root, watch, Arc::new(|| false), "backup:verify");
        // No foreground queue is installed for this root. A probe that submits
        // instead of observing the admitted command would fail this twin.
        assert!(
            super::super::foreground::submit_owner_maintenance(&root, "backup:verify").is_err()
        );
        assert!(probe());
        assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
        solstone_core_journal_io::whole_journal_restore::hold_target_for_restore(temp.path());
        assert!(!probe());
        solstone_core_journal_io::whole_journal_restore::release_target_for_restore(temp.path());
        assert!(probe());
        stop.send_replace(true);
        assert!(!probe());
        let (gone, receiver) = watch::channel(false);
        let lost = dispatch_probe(&root, receiver, Arc::new(|| false), "backup:verify");
        drop(gone);
        assert!(!lost());
    }

    #[test]
    fn original_serving_stop_revokes_callback_after_foreground_reopens() {
        let _serial = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let payload = temp.path().join("journal-payload");
        for path in [
            "solstone/talent",
            "solstone/apps",
            "solstone/think/templates",
        ] {
            std::fs::create_dir_all(payload.join(path)).unwrap();
        }
        std::fs::create_dir(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"},"processing":{"history_confirmed":true}}"#,
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let host = ForegroundProcessingHost::open(
            temp.path(),
            &payload,
            BackupWebDeps::with_root(Arc::clone(&root)).unwrap(),
        )
        .unwrap();
        let (old_stop, old_watch) = watch::channel(false);
        let old = submitter(&root, old_watch, Arc::new(|| false));
        let first = old().expect("actual queue accepts fixed verification");
        host.shutdown().unwrap();
        assert!(
            host.queue()
                .history()
                .iter()
                .any(|row| row.reference == first
                    && row.command.iter().map(String::as_str).eq([
                        "solstone",
                        "journal",
                        "maintenance",
                        "run",
                        "backup:verify"
                    ]))
        );
        drop(host);
        old_stop.send_replace(true);
        let reopened = ForegroundProcessingHost::open(
            temp.path(),
            &payload,
            BackupWebDeps::with_root(Arc::clone(&root)).unwrap(),
        )
        .unwrap();
        assert!(
            old().is_err(),
            "a later queue cannot revive the old serving callback"
        );
        let (fresh_stop, fresh_watch) = watch::channel(false);
        let cancelled = Arc::new(AtomicBool::new(false));
        let check = Arc::clone(&cancelled);
        let fresh = submitter(
            &root,
            fresh_watch,
            Arc::new(move || check.load(Ordering::Acquire)),
        );
        assert!(
            fresh().is_ok(),
            "same owner can use its fresh serving callback"
        );
        cancelled.store(true, Ordering::Release);
        assert!(
            fresh().is_err(),
            "startup cancellation revokes the delegate"
        );
        cancelled.store(false, Ordering::Release);
        drop(fresh_stop);
        assert!(
            fresh().is_err(),
            "lost serving lifetime refuses even while queue lives"
        );
        reopened.shutdown().unwrap();
    }
}
