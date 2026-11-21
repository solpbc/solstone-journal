// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(unix)]
use solstone_core_journal::{LAYOUT_BUNDLE_ANCHOR, LAYOUT_LAYOUT_ANCHOR, LAYOUT_TEMPLATE_ANCHOR};
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::PathBuf;
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
    time::Duration,
};
use tempfile::TempDir;

use crate::{
    context::CheckContext,
    vocabulary::{Check, Platform, Severity},
};

#[derive(Clone)]
pub(crate) struct StagedContext {
    pub context: CheckContext,
    _root: Arc<TempDir>,
}

impl Deref for StagedContext {
    type Target = CheckContext;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl DerefMut for StagedContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}

pub(crate) fn context_with_prefix(prefix: &str) -> StagedContext {
    let dir = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("create staged test root");
    let root = dir.path();
    StagedContext {
        context: CheckContext {
            report_form: crate::vocabulary::ReportForm::Structured,
            home_dir: root.join("home"),
            install_bin_dir: root.join("install/bin"),
            journal_path: root.join("journal"),
            callosum_socket_path: root.join("journal/health/callosum.sock"),
            platform: Platform::Linux,
            now: "2026-01-01T00:00:00Z".parse::<jiff::Timestamp>().unwrap(),
            host_arch: "x86_64".into(),
            hostname: "test-host".into(),
            checkout_root: None,
            payload_root: None,
            port: 5015,
            service_status_timeout: Duration::from_millis(10),
            service_status_command_override: None,
            parakeet_server_probe_override: None,
            speakers_analyze_resolvers: None,
            vad_runtime_probe: None,
            free_space_bytes_override: None,
            windows_service_probe: None,
        },
        _root: Arc::new(dir),
    }
}

pub(crate) fn context() -> StagedContext {
    context_with_prefix("solstone-doctor-check-test-")
}

pub(crate) fn check(name: &'static str, severity: Severity) -> Check {
    Check {
        name,
        severity,
        platforms: &[Platform::Linux],
    }
}

#[cfg(unix)]
pub(crate) fn layout_install_root(context: &CheckContext) -> PathBuf {
    let prefix = context
        .install_bin_dir
        .parent()
        .expect("staged install bin has a prefix");
    let share = prefix.join("share");
    for anchor in [
        LAYOUT_BUNDLE_ANCHOR,
        LAYOUT_LAYOUT_ANCHOR,
        LAYOUT_TEMPLATE_ANCHOR,
    ] {
        let path = share.join(anchor);
        fs::create_dir_all(path.parent().expect("anchor has a parent"))
            .expect("create staged layout anchor parent");
        fs::write(path, "").expect("write staged layout anchor");
    }
    prefix.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::context_with_prefix;

    #[test]
    fn staged_context_uses_unique_roots_and_cleans_up_after_last_clone_drops() {
        let first = context_with_prefix("check-");
        let second = context_with_prefix("check-");
        let first_root = first.journal_path.parent().unwrap().to_path_buf();
        let second_root = second.journal_path.parent().unwrap().to_path_buf();

        assert_ne!(first_root, second_root);
        assert!(first_root.is_dir());
        assert!(second_root.is_dir());

        let cloned = first.clone();
        drop(first);
        assert!(first_root.is_dir(), "cloned handle keeps temp root alive");

        drop(cloned);
        drop(second);
        assert!(!first_root.exists(), "temp root removed on final drop");
        assert!(!second_root.exists(), "temp root removed on drop");
    }
}
