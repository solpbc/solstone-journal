// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use chrono::TimeZone;
use solstone_core_journal::{LAYOUT_BUNDLE_ANCHOR, LAYOUT_LAYOUT_ANCHOR, LAYOUT_TEMPLATE_ANCHOR};
use std::{
    fs,
    ops::Deref,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use crate::{
    context::CheckContext,
    vocabulary::{Check, Platform, Severity},
};

static NEXT_CONTEXT: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct StagedContext {
    pub context: CheckContext,
    _root: TempRoot,
}

impl Deref for StagedContext {
    type Target = CheckContext;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn context() -> StagedContext {
    let root = std::env::temp_dir().join(format!(
        "solstone-doctor-check-test-{}-{}",
        std::process::id(),
        NEXT_CONTEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).expect("create staged test root");
    StagedContext {
        context: CheckContext {
            home_dir: root.join("home"),
            install_bin_dir: root.join("install/bin"),
            journal_path: root.join("journal"),
            callosum_socket_path: root.join("journal/health/callosum.sock"),
            platform: Platform::Linux,
            now: chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
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
        },
        _root: TempRoot(root),
    }
}

pub(crate) fn check(name: &'static str, severity: Severity) -> Check {
    Check {
        name,
        severity,
        platforms: &[Platform::Linux],
    }
}

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
