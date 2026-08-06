// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-lifetime deferred journal-entity deletion scheduling.

use std::collections::HashMap;
use std::path::PathBuf;
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
            if handles
                .lock()
                .expect("deferred-delete registry mutex is not poisoned")
                .remove(&task_pending_id)
                .is_none()
            {
                return;
            }
            let delete_root = journal_root.clone();
            let delete_entity_id = entity_id.clone();
            let facets_deleted = solstone_core_serving::seam::run_blocking(move || {
                solstone_core_facets::delete_journal_entity(&delete_root, &delete_entity_id)
                    .map(|report| report.facets_deleted)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
            let _ = action_log::committed(
                &journal_root,
                &entity_id,
                &task_pending_id,
                &facets_deleted,
            );
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
