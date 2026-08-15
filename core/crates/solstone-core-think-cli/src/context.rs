// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use solstone_core_cortex_client::{
    CortexRequest, CortexRequestClient, CortexRequestPolicy, UseIdAllocator, WaitForUsesReport,
};

pub(crate) trait CortexBoundary: Send + Sync {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, String>;
    fn wait(
        &self,
        runtime: &tokio::runtime::Runtime,
        use_ids: &[String],
    ) -> Result<WaitForUsesReport, String>;
}

struct NativeCortexBoundary(CortexRequestClient);

impl CortexBoundary for NativeCortexBoundary {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, String> {
        runtime
            .block_on(self.0.dispatch(request))
            .map_err(|error| format!("cortex dispatch failed: {error:?}"))
    }

    fn wait(
        &self,
        runtime: &tokio::runtime::Runtime,
        use_ids: &[String],
    ) -> Result<WaitForUsesReport, String> {
        runtime
            .block_on(self.0.wait_for_uses(use_ids))
            .map_err(|error| format!("cortex wait failed: {error:?}"))
    }
}

pub(crate) struct ThinkContext {
    pub(crate) journal: PathBuf,
    pub(crate) day: String,
    pub(crate) day_dir: PathBuf,
    pub(crate) now_ms: i64,
    pub(crate) talent_root: PathBuf,
    pub(crate) apps_root: PathBuf,
    pub(crate) cortex: Arc<dyn CortexBoundary>,
}

impl ThinkContext {
    pub(crate) fn new(journal: &Path, day: String, day_dir: PathBuf, now_ms: i64) -> Self {
        let (talent_root, apps_root) = package_roots();
        let allocator_now = now_ms;
        Self {
            journal: journal.to_path_buf(),
            day,
            day_dir,
            now_ms,
            talent_root,
            apps_root,
            cortex: Arc::new(NativeCortexBoundary(CortexRequestClient::with_allocator(
                journal,
                CortexRequestPolicy::think(),
                UseIdAllocator::new(move || Some(allocator_now)),
            ))),
        }
    }

    pub(crate) fn cortex_policy_deadline(&self) -> Option<Duration> {
        CortexRequestPolicy::think().outcome_deadline()
    }

    #[cfg(test)]
    pub(crate) fn with_boundary(mut self, boundary: Arc<dyn CortexBoundary>) -> Self {
        self.cortex = boundary;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_talent_roots(mut self, talent_root: PathBuf, apps_root: PathBuf) -> Self {
        self.talent_root = talent_root;
        self.apps_root = apps_root;
        self
    }
}

fn package_roots() -> (PathBuf, PathBuf) {
    let starts = [
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
        std::env::current_dir().ok(),
    ];
    for start in starts.into_iter().flatten() {
        for root in start.ancestors() {
            let talent = root.join("solstone/talent");
            if talent.is_dir() {
                return (talent, root.join("solstone/apps"));
            }
        }
    }
    (
        PathBuf::from("solstone/talent"),
        PathBuf::from("solstone/apps"),
    )
}
