// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

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
            port: 5015,
            service_status_timeout: Duration::from_millis(10),
            service_status_command_override: None,
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

pub(crate) fn site_packages(context: &CheckContext, python: &str) -> PathBuf {
    let prefix = context
        .install_bin_dir
        .parent()
        .expect("staged install bin has a prefix");
    let site_packages = prefix.join("lib").join(python).join("site-packages");
    fs::create_dir_all(site_packages.join("solstone")).expect("create staged solstone package");
    fs::write(site_packages.join("solstone/__init__.py"), "").expect("write package marker");
    site_packages
}

pub(crate) fn metadata(
    site_packages: &std::path::Path,
    directory: &str,
    name: &str,
    version: &str,
    requires_python: Option<&str>,
) {
    let dist_info = site_packages.join(directory);
    fs::create_dir_all(&dist_info).expect("create staged dist-info");
    let requires_python = requires_python
        .map(|value| format!("Requires-Python: {value}\n"))
        .unwrap_or_default();
    fs::write(
        dist_info.join("METADATA"),
        format!("Name: {name}\nVersion: {version}\n{requires_python}\n"),
    )
    .expect("write staged metadata");
}
