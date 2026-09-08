// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::sync::{Arc, Mutex, mpsc};

use solstone_core_system::process::{
    BoundedHelperCleanup, BoundedHelperFailure, BoundedHelperResources,
};

use crate::{ToolOutput, ToolRequest, ToolRunner};

/// Adds already-owned resources to a borrowed synchronous runner.
pub struct RetainingToolRunner<'a> {
    inner: &'a dyn ToolRunner,
    resources: BoundedHelperResources,
    cleanup: Mutex<Vec<BoundedHelperCleanup>>,
}

impl<'a> RetainingToolRunner<'a> {
    pub fn new(inner: &'a dyn ToolRunner, resources: BoundedHelperResources) -> Self {
        Self {
            inner,
            resources,
            cleanup: Mutex::new(Vec::new()),
        }
    }

    /// Observes only incomplete helpers launched through this scope.
    pub fn cleanup_pending(&self) -> bool {
        self.cleanup.try_lock().map_or(true, |owners| {
            owners.iter().any(|owner| {
                matches!(
                    owner.observe(),
                    solstone_core_system::process::HelperCleanupStatus::Pending
                )
            })
        })
    }
}

impl ToolRunner for RetainingToolRunner<'_> {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let mut request = request.clone();
        request.resources.extend(&self.resources);
        let result = self.inner.run(&request);
        if let Err(error) = &result
            && let Some(cleanup) = own_cleanup(error)
        {
            self.cleanup
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(cleanup);
        }
        result
    }
}

/// Correlates every incomplete helper from one synchronous web operation.
/// The caller keeps the receiver through execution and drains it before finishing.
pub fn track_cleanup(
    inner: Arc<dyn ToolRunner + Send + Sync>,
) -> (
    Arc<dyn ToolRunner + Send + Sync>,
    mpsc::Receiver<BoundedHelperCleanup>,
) {
    let (cleanup, receiver) = mpsc::channel();
    (
        Arc::new(CleanupTrackingToolRunner { inner, cleanup }),
        receiver,
    )
}

struct CleanupTrackingToolRunner {
    inner: Arc<dyn ToolRunner + Send + Sync>,
    cleanup: mpsc::Sender<BoundedHelperCleanup>,
}

impl ToolRunner for CleanupTrackingToolRunner {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let result = self.inner.run(request);
        if let Err(error) = &result
            && let Some(cleanup) = own_cleanup(error)
        {
            // The facade retains the same owner even if a caller unwinds and
            // drops its receiver. No native resource depends on channel delivery.
            let _ = self.cleanup.send(cleanup);
        }
        result
    }
}

fn own_cleanup(error: &io::Error) -> Option<BoundedHelperCleanup> {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<BoundedHelperFailure>())
        .and_then(|failure| failure.cleanup().cloned())
}
