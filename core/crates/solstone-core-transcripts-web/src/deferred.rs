// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-lifetime deferred segment-deletion scheduling.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A process exit before the deadline intentionally loses a scheduled deletion,
/// matching the Python reference's in-memory timer behavior.
#[derive(Clone, Default)]
pub(crate) struct DeferredDeleteRegistry {
    handles: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
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
            commit();
        });
        self.handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
            .insert(pending_id, task.abort_handle());
    }

    pub(crate) fn cancel(&self, pending_id: &str) -> bool {
        let handle = self
            .handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
            .remove(pending_id);
        if let Some(handle) = handle {
            handle.abort();
            true
        } else {
            false
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
}
