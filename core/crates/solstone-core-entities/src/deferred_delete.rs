// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-lifetime deferred journal-entity deletion scheduling.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::action_log;

/// A process exit before the delay elapses loses the scheduled commit after the
/// client has received a successful response. This intentionally matches the
/// Python reference's process-lifetime `threading.Timer` behavior.
#[derive(Clone)]
pub(crate) struct DeferredDeleteRegistry {
    handles: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
}

impl DeferredDeleteRegistry {
    pub(crate) fn new() -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn schedule(
        &self,
        journal_root: PathBuf,
        entity_id: String,
        pending_id: String,
        delay: Duration,
    ) {
        let _ = action_log::pending(&journal_root, &entity_id, &pending_id);
        let handles = Arc::clone(&self.handles);
        let task_pending_id = pending_id.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = solstone_core_serving::seam::run_blocking(move || {
                Self::commit_if_still_scheduled(
                    &handles,
                    &journal_root,
                    &entity_id,
                    &task_pending_id,
                );
            })
            .await;
        });
        self.handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
            .insert(pending_id, task.abort_handle());
    }

    #[cfg(test)]
    pub(crate) fn commit_if_pending(
        &self,
        journal_root: &Path,
        entity_id: &str,
        pending_id: &str,
    ) {
        Self::commit_if_still_scheduled(&self.handles, journal_root, entity_id, pending_id);
    }

    fn commit_if_still_scheduled(
        handles: &Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
        journal_root: &Path,
        entity_id: &str,
        pending_id: &str,
    ) {
        if handles
            .lock()
            .expect("deferred-delete registry mutex is not poisoned")
            .remove(pending_id)
            .is_none()
        {
            return;
        }
        let facets_deleted = solstone_core_facets::delete_journal_entity(journal_root, entity_id)
            .map(|report| report.facets_deleted)
            .unwrap_or_default();
        let _ = action_log::committed(journal_root, entity_id, pending_id, &facets_deleted);
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
