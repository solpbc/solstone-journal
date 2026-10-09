// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use solstone_core_backup_runtime::{
    Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance, JournalMaintenanceError,
    NativeRestoreRecorder, ToolOutput, ToolRequest, ToolRunner,
};
use solstone_core_installed_payload::{
    COMPILED_VERSION, INSTALLED_PAYLOAD_MANIFEST, PRODUCT, code, compiled_target, guidance,
    render_installed_payload,
};

use crate::operation::Terminal;
use crate::{BackupWebDeps, resolve_tools};

struct NoOpClock;
impl Clock for NoOpClock {
    fn now_unix(&self) -> i64 {
        1_700_000_000
    }
    fn iso_week(&self) -> u8 {
        1
    }
}

struct NoOpMaintenance;
impl JournalMaintenance for NoOpMaintenance {
    fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        Ok(())
    }
    fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        Ok(())
    }
}

struct DummyHttp;
impl HttpTransport for DummyHttp {
    fn execute(
        &self,
        _: &HttpRequest,
    ) -> Result<HttpResponse, solstone_core_backup_runtime::hosted_runtime::HttpError> {
        panic!("unexpected http call")
    }
}

struct NoSpawnRunner(std::sync::Mutex<Vec<PathBuf>>);
impl ToolRunner for NoSpawnRunner {
    fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
        self.0
            .lock()
            .unwrap()
            .push(PathBuf::from(request.program.clone()));
        panic!("must not spawn tool: {:?}", request.program);
    }
}

fn test_deps(
    executable: PathBuf,
    journal_root: PathBuf,
    runner: Arc<dyn ToolRunner + Send + Sync>,
) -> BackupWebDeps {
    BackupWebDeps {
        journal_root: journal_root.clone(),
        cache: crate::measurement::new(&journal_root),
        operations: crate::operation::new_slot(),
        runner,
        http: Arc::new(DummyHttp),
        clock: Arc::new(NoOpClock),
        journal_maintenance: Arc::new(NoOpMaintenance),
        restore_recorder: Arc::new(NativeRestoreRecorder),
        executable,
        portal_base: "https://services.solstone.app".into(),
        version: "test",
        handoff_poll_lease: Arc::new(AtomicBool::new(false)),
        restore_prepare: crate::restore_prepare::new_shared(),
    }
}

fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
}

fn build_package(root: &Path, version: &str) -> PathBuf {
    let exe = root.join("bin/solstone");
    write_file(root, "bin/solstone", b"launcher");
    write_file(root, "lib/solstone-restic/restic", b"restic-member");
    write_file(root, "lib/solstone-rclone/rclone", b"rclone-member");
    let manifest =
        render_installed_payload(root, PRODUCT, version, compiled_target(), "commit-fixture")
            .unwrap();
    write_file(root, INSTALLED_PAYLOAD_MANIFEST, &manifest);
    exe
}

struct PlantedFallbacks {
    _home: tempfile::TempDir,
    _path_dir: tempfile::TempDir,
    _bundle_dir: tempfile::TempDir,
    script_dirs: Vec<PathBuf>,
    recorded_files: Vec<(PathBuf, Vec<u8>)>,
    prev_home: Option<std::ffi::OsString>,
    prev_path: Option<std::ffi::OsString>,
    prev_restic_bundle: Option<std::ffi::OsString>,
    prev_rclone_bundle: Option<std::ffi::OsString>,
}

#[allow(unsafe_code)]
impl Drop for PlantedFallbacks {
    fn drop(&mut self) {
        unsafe {
            if let Some(ref val) = self.prev_home {
                std::env::set_var("HOME", val);
            } else {
                std::env::remove_var("HOME");
            }
            if let Some(ref val) = self.prev_path {
                std::env::set_var("PATH", val);
            } else {
                std::env::remove_var("PATH");
            }
            if let Some(ref val) = self.prev_restic_bundle {
                std::env::set_var("SOLSTONE_RESTIC_BUNDLE", val);
            } else {
                std::env::remove_var("SOLSTONE_RESTIC_BUNDLE");
            }
            if let Some(ref val) = self.prev_rclone_bundle {
                std::env::set_var("SOLSTONE_RCLONE_BUNDLE", val);
            } else {
                std::env::remove_var("SOLSTONE_RCLONE_BUNDLE");
            }
        }
    }
}

impl PlantedFallbacks {
    fn verify(&self) {
        for dir in &self.script_dirs {
            assert!(
                !dir.join("marker").exists(),
                "marker file was created in {:?}",
                dir
            );
        }
        for (path, expected_bytes) in &self.recorded_files {
            let actual = fs::read(path).unwrap_or_else(|_| panic!("read {:?}", path));
            assert_eq!(&actual, expected_bytes, "file {:?} was modified", path);
        }
    }
}

#[allow(unsafe_code)]
fn plant_fallbacks(pkg_root: &Path) -> PlantedFallbacks {
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    let bundle_dir = tempfile::tempdir().unwrap();

    let prev_home = std::env::var_os("HOME");
    let prev_path = std::env::var_os("PATH");
    let prev_restic_bundle = std::env::var_os("SOLSTONE_RESTIC_BUNDLE");
    let prev_rclone_bundle = std::env::var_os("SOLSTONE_RCLONE_BUNDLE");

    let script_content = b"#!/bin/sh\ntouch \"$(dirname \"$0\")/marker\"\n";
    let sentinel_content = b"sentinel-complete";
    let license_content = b"LICENSE-TEXT";

    let mut recorded_files = Vec::new();
    let mut script_dirs = Vec::new();

    let mut plant = |dir: &Path, name: &str| -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let script = dir.join(name);
        fs::write(&script, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        recorded_files.push((script.clone(), script_content.to_vec()));

        let sentinel = dir.join(".install-complete");
        fs::write(&sentinel, sentinel_content).unwrap();
        recorded_files.push((sentinel, sentinel_content.to_vec()));

        let license = dir.join("LICENSE");
        fs::write(&license, license_content).unwrap();
        recorded_files.push((license, license_content.to_vec()));

        if !script_dirs.contains(&dir.to_path_buf()) {
            script_dirs.push(dir.to_path_buf());
        }
        script
    };

    let cache_dir = home.path().join(".cache/solstone");
    plant(&cache_dir, "restic");
    plant(&cache_dir, "rclone");

    let app_support_dir = home.path().join("Library/Application Support/solstone");
    plant(&app_support_dir, "restic");
    plant(&app_support_dir, "rclone");

    let pkg_bin_dir = pkg_root.join("_bin");
    plant(&pkg_bin_dir, "restic");
    plant(&pkg_bin_dir, "rclone");

    let restic_bundle = plant(bundle_dir.path(), "bundle-restic");
    let rclone_bundle = plant(bundle_dir.path(), "bundle-rclone");

    plant(path_dir.path(), "restic");
    plant(path_dir.path(), "rclone");

    unsafe {
        std::env::set_var("HOME", home.path());
        std::env::set_var("SOLSTONE_RESTIC_BUNDLE", &restic_bundle);
        std::env::set_var("SOLSTONE_RCLONE_BUNDLE", &rclone_bundle);
    }

    let new_path = if let Some(ref old) = prev_path {
        let mut p = path_dir.path().as_os_str().to_os_string();
        p.push(":");
        p.push(old);
        p
    } else {
        path_dir.path().as_os_str().to_os_string()
    };
    unsafe {
        std::env::set_var("PATH", new_path);
    }

    PlantedFallbacks {
        _home: home,
        _path_dir: path_dir,
        _bundle_dir: bundle_dir,
        script_dirs,
        recorded_files,
        prev_home,
        prev_path,
        prev_restic_bundle,
        prev_rclone_bundle,
    }
}

#[test]
fn fallback_negative_ignores_markers_and_env() {
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _lock = ENV_LOCK.lock().unwrap();

    let pkg = tempfile::tempdir().unwrap();
    let exe = pkg.path().join("bin/solstone");
    write_file(pkg.path(), "bin/solstone", b"launcher");

    let fallbacks = plant_fallbacks(pkg.path());

    let journal = tempfile::tempdir().unwrap();
    let runner = Arc::new(NoSpawnRunner(std::sync::Mutex::new(vec![])));
    let deps = test_deps(exe, journal.path().to_path_buf(), runner.clone());

    let err = resolve_tools(&deps).unwrap_err();
    let terminal = Terminal::tool_error(err);

    assert_eq!(terminal.reason_code.as_deref(), Some("restic_unavailable"));
    assert_eq!(terminal.detail.as_deref(), Some(code::MANIFEST_MISSING));
    assert_eq!(
        terminal.guidance.as_deref(),
        Some(guidance::MANIFEST_MISSING)
    );

    assert!(runner.0.lock().unwrap().is_empty());
    fallbacks.verify();
}

#[test]
fn altered_member_refuses() {
    let pkg = tempfile::tempdir().unwrap();
    let exe = build_package(pkg.path(), COMPILED_VERSION);

    // Flip one byte of lib/solstone-restic/restic and keep same length
    let restic_path = pkg.path().join("lib/solstone-restic/restic");
    let mut bytes = fs::read(&restic_path).unwrap();
    bytes[0] ^= 0xFF;
    fs::write(&restic_path, bytes).unwrap();

    let journal = tempfile::tempdir().unwrap();
    let runner = Arc::new(NoSpawnRunner(std::sync::Mutex::new(vec![])));
    let deps = test_deps(exe, journal.path().to_path_buf(), runner.clone());

    let err = resolve_tools(&deps).unwrap_err();
    assert_eq!(err.to_string(), "restic_unavailable");
    assert_eq!(err.detail(), code::MEMBER_CHANGED);
    assert_eq!(err.guidance(), guidance::PACKAGE_MISMATCH);
    assert!(runner.0.lock().unwrap().is_empty());
}

#[test]
fn web_constant_pairing() {
    let runner = Arc::new(NoSpawnRunner(std::sync::Mutex::new(vec![])));

    struct Case {
        setup: Box<dyn Fn() -> (tempfile::TempDir, PathBuf)>,
        expected_detail: &'static str,
        expected_guidance: &'static str,
    }

    let cases = vec![
        // 1. manifest absent, exe at bin/solstone
        Case {
            setup: Box::new(|| {
                let pkg = tempfile::tempdir().unwrap();
                let exe = pkg.path().join("bin/solstone");
                write_file(pkg.path(), "bin/solstone", b"launcher");
                (pkg, exe)
            }),
            expected_detail: code::MANIFEST_MISSING,
            expected_guidance: guidance::MANIFEST_MISSING,
        },
        // 2. exe not under bin/
        Case {
            setup: Box::new(|| {
                let pkg = tempfile::tempdir().unwrap();
                let exe = pkg.path().join("solstone");
                fs::write(&exe, b"launcher").unwrap();
                (pkg, exe)
            }),
            expected_detail: code::UNSUPPORTED_LOCATION,
            expected_guidance: guidance::UNSUPPORTED_LOCATION,
        },
        // 3. member byte flipped, same length
        Case {
            setup: Box::new(|| {
                let pkg = tempfile::tempdir().unwrap();
                let exe = build_package(pkg.path(), COMPILED_VERSION);
                let restic_path = pkg.path().join("lib/solstone-restic/restic");
                let mut bytes = fs::read(&restic_path).unwrap();
                bytes[0] ^= 0x55;
                fs::write(&restic_path, bytes).unwrap();
                (pkg, exe)
            }),
            expected_detail: code::MEMBER_CHANGED,
            expected_guidance: guidance::PACKAGE_MISMATCH,
        },
        // 4. member removed
        Case {
            setup: Box::new(|| {
                let pkg = tempfile::tempdir().unwrap();
                let exe = build_package(pkg.path(), COMPILED_VERSION);
                fs::remove_file(pkg.path().join("lib/solstone-restic/restic")).unwrap();
                (pkg, exe)
            }),
            expected_detail: code::MEMBER_MISSING,
            expected_guidance: guidance::PACKAGE_MISMATCH,
        },
        // 5. manifest version 999.0.0
        Case {
            setup: Box::new(|| {
                let pkg = tempfile::tempdir().unwrap();
                let exe = build_package(pkg.path(), "999.0.0");
                (pkg, exe)
            }),
            expected_detail: code::RESTART_TO_FINISH_UPDATE,
            expected_guidance: guidance::RESTART_UPDATE,
        },
    ];

    for case in cases {
        let (_pkg, exe) = (case.setup)();
        let journal = tempfile::tempdir().unwrap();
        let deps = test_deps(exe, journal.path().to_path_buf(), runner.clone());

        let err = resolve_tools(&deps).unwrap_err();
        let terminal = Terminal::tool_error(err);

        assert_eq!(terminal.reason_code.as_deref(), Some("restic_unavailable"));
        assert_eq!(terminal.detail.as_deref(), Some(case.expected_detail));
        assert_eq!(terminal.guidance.as_deref(), Some(case.expected_guidance));
    }
}
