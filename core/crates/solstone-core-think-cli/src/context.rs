// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use solstone_core_cortex_client::{
    CortexRequest, CortexRequestClient, CortexRequestPolicy, DispatchError, UseIdAllocator,
    WaitForUsesReport,
};

use crate::helpers::ThinkStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DispatchFailure {
    Unavailable,
    NotClaimed { use_id: String },
}

pub(crate) trait CortexBoundary: Send + Sync {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, DispatchFailure>;
    fn wait(
        &self,
        runtime: &tokio::runtime::Runtime,
        use_ids: &[String],
        deadline: Option<Duration>,
    ) -> Result<WaitForUsesReport, String>;
}

pub(crate) trait IndexBoundary: Send + Sync {
    fn rescan_file(&self, journal: &Path, path: &Path);
}

struct NativeIndexBoundary;

impl IndexBoundary for NativeIndexBoundary {
    fn rescan_file(&self, journal: &Path, path: &Path) {
        // Source-derived, not measured: thinking.py:240-242 queues the rescan
        // and ignores its outcome, so the native port deliberately does too.
        let _ = solstone_core_indexer_store::scan::rescan_file(journal, path);
    }
}

struct NativeCortexBoundary(CortexRequestClient);

impl CortexBoundary for NativeCortexBoundary {
    fn dispatch(
        &self,
        runtime: &tokio::runtime::Runtime,
        request: &CortexRequest,
    ) -> Result<String, DispatchFailure> {
        runtime
            .block_on(self.0.dispatch(request))
            .map_err(|error| match error {
                DispatchError::Unavailable => DispatchFailure::Unavailable,
                DispatchError::NotClaimed { use_id } => DispatchFailure::NotClaimed { use_id },
            })
    }

    fn wait(
        &self,
        runtime: &tokio::runtime::Runtime,
        use_ids: &[String],
        deadline: Option<Duration>,
    ) -> Result<WaitForUsesReport, String> {
        runtime
            .block_on(self.0.wait_for_uses_with_deadline(use_ids, deadline))
            .map_err(|error| format!("cortex wait failed: {error:?}"))
    }
}

#[derive(Clone)]
pub(crate) struct ThinkContext {
    pub(crate) journal: PathBuf,
    pub(crate) day: String,
    pub(crate) day_dir: PathBuf,
    pub(crate) now_ms: i64,
    event_clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    pub(crate) talent_root: PathBuf,
    pub(crate) apps_root: PathBuf,
    pub(crate) cortex: Arc<dyn CortexBoundary>,
    pub(crate) index: Arc<dyn IndexBoundary>,
    pub(crate) status: ThinkStatus,
}

impl ThinkContext {
    #[cfg(test)]
    pub(crate) fn new(
        journal: &Path,
        day: String,
        day_dir: PathBuf,
        now_ms: i64,
    ) -> Result<Self, String> {
        Self::new_with_event_clock(journal, day, day_dir, now_ms, Arc::new(move || now_ms))
    }

    pub(crate) fn new_with_event_clock(
        journal: &Path,
        day: String,
        day_dir: PathBuf,
        now_ms: i64,
        event_clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self, String> {
        let (talent_root, apps_root) = package_roots()?;
        let allocator_now = now_ms;
        Ok(Self {
            journal: journal.to_path_buf(),
            day,
            day_dir,
            now_ms,
            event_clock,
            talent_root,
            apps_root,
            cortex: Arc::new(NativeCortexBoundary(CortexRequestClient::with_allocator(
                journal,
                CortexRequestPolicy::think(),
                UseIdAllocator::new(move || Some(allocator_now)),
            ))),
            index: Arc::new(NativeIndexBoundary),
            status: ThinkStatus::default(),
        })
    }

    pub(crate) fn event_now_ms(&self) -> i64 {
        (self.event_clock)()
    }

    pub(crate) fn event_clock(&self) -> Arc<dyn Fn() -> i64 + Send + Sync> {
        Arc::clone(&self.event_clock)
    }

    #[cfg(test)]
    pub(crate) fn with_boundary(mut self, boundary: Arc<dyn CortexBoundary>) -> Self {
        self.cortex = boundary;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_event_clock(
        mut self,
        event_clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        self.event_clock = event_clock;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_index_boundary(mut self, boundary: Arc<dyn IndexBoundary>) -> Self {
        self.index = boundary;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_talent_roots(mut self, talent_root: PathBuf, apps_root: PathBuf) -> Self {
        self.talent_root = talent_root;
        self.apps_root = apps_root;
        self
    }
}

fn package_roots() -> Result<(PathBuf, PathBuf), String> {
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let directory = executable_dir.as_deref().unwrap_or(Path::new(""));
    package_roots_from_executable_dir(directory)
        .ok_or_else(|| solstone_core_journal::describe_package_roots_miss(directory))
}

fn package_roots_from_executable_dir(executable_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let root =
        solstone_core_journal::resolve_installation_root_from_executable_dir(executable_dir)?;
    let talent = root.join("solstone/talent");
    let apps = root.join("solstone/apps");
    (talent.is_dir() && apps.is_dir()).then_some((talent, apps))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn share_layout_requires_both_roots_and_fails_when_anchor_removed() {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("tree");
        let bin = prefix.join("bin");
        let share = prefix.join("share");
        fs::create_dir_all(&bin).unwrap();
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, relative).unwrap();
        }
        assert!(
            package_roots_from_executable_dir(&bin).is_none(),
            "talent without apps must fail closed"
        );
        let missing_apps = solstone_core_journal::describe_package_roots_miss(&bin);
        assert!(
            missing_apps.contains(&share.join("solstone/apps").display().to_string()),
            "directory-miss diagnostic must name solstone/apps: {missing_apps}"
        );
        fs::create_dir_all(share.join("solstone/apps")).unwrap();
        let (talent, apps) = package_roots_from_executable_dir(&bin).unwrap();
        assert_eq!(talent, share.join("solstone/talent"));
        assert_eq!(apps, share.join("solstone/apps"));
        fs::remove_file(share.join(solstone_core_journal::LAYOUT_BUNDLE_ANCHOR)).unwrap();
        assert!(package_roots_from_executable_dir(&bin).is_none());
    }
}
