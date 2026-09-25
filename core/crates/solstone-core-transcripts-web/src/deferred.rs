// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process arbitration between a held segment delete and its cancel.
//!
//! The durable half lives in [`crate::pending`]: a process exit inside the
//! window no longer loses a confirmed delete, because the next start resumes
//! the record. This registry only decides, inside one process, whether the
//! deadline or the cancel got there first. Whichever removes the entry wins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
pub(crate) struct DeferredDeleteRegistry {
    handles: Arc<Mutex<HashMap<String, Option<tokio::task::AbortHandle>>>>,
}

impl DeferredDeleteRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Schedule one commit. The task removes its own handle before invoking the
    /// closure, so cancellation after the deadline observes it as unavailable.
    ///
    /// A zero delay commits on this thread before `schedule` returns. Spawning
    /// that path made the commit a race against handle insertion and against
    /// any caller polling with cooperative yields.
    pub(crate) fn schedule(
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
                .expect("deferred-delete registry mutex is not poisoned")
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
            .expect("deferred-delete registry mutex is not poisoned")
            .get_mut(&pending_id)
        {
            *slot = Some(task.abort_handle());
        }
    }

    /// Hold an id whose deadline a resume thread is waiting on, so a cancel
    /// can still reach it. The waiter must [`claim`](Self::claim) it first.
    pub(crate) fn hold(&self, pending_id: String) {
        self.handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
            .insert(pending_id, None);
    }

    /// Whether this process still has a timer or a waiter for the id.
    pub(crate) fn contains(&self, pending_id: &str) -> bool {
        self.handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
            .contains_key(pending_id)
    }

    /// Take a held id at its deadline. `false` means a cancel got there first.
    pub(crate) fn claim(&self, pending_id: &str) -> bool {
        self.handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
            .remove(pending_id)
            .is_some()
    }

    pub(crate) fn cancel(&self, pending_id: &str) -> bool {
        let handle = self
            .handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
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

    use super::DeferredDeleteRegistry;

    #[tokio::test]
    async fn cancellation_prevents_the_scheduled_commit() {
        let registry = DeferredDeleteRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let commit_calls = Arc::clone(&calls);
        let done = Arc::new(tokio::sync::Notify::new());
        let commit_done = Arc::clone(&done);
        registry.schedule("a".into(), Duration::from_secs(60), move || {
            commit_calls.fetch_add(1, Ordering::SeqCst);
            commit_done.notify_one();
        });
        assert!(registry.cancel("a"));
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!registry.cancel("a"));
    }

    #[test]
    fn zero_delay_commits_before_schedule_returns() {
        let registry = DeferredDeleteRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let commit_calls = Arc::clone(&calls);
        registry.schedule("c".into(), Duration::ZERO, move || {
            commit_calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!registry.cancel("c"));
    }

    #[tokio::test]
    async fn deadline_commits_once_and_makes_the_id_unavailable() {
        let registry = DeferredDeleteRegistry::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let commit_calls = Arc::clone(&calls);
        let done = Arc::new(tokio::sync::Notify::new());
        let commit_done = Arc::clone(&done);
        registry.schedule("b".into(), Duration::ZERO, move || {
            commit_calls.fetch_add(1, Ordering::SeqCst);
            commit_done.notify_one();
        });
        done.notified().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!registry.cancel("b"));
    }

    #[test]
    fn a_held_id_goes_to_whichever_of_cancel_and_claim_comes_first() {
        let registry = DeferredDeleteRegistry::new();
        registry.hold("d".into());
        assert!(registry.cancel("d"));
        assert!(!registry.claim("d"));
        registry.hold("e".into());
        assert!(registry.claim("e"));
        assert!(!registry.cancel("e"));
    }
}
