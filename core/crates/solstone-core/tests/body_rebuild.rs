// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use solstone_core_journal_io::{LockOptions, hold_lock};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-core-body-command-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary journal creates");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn body_rebuild_command_emits_machine_result_and_refuses_torn_native_history() {
    let temporary = TempDir::new();
    let success = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["body", "rebuild", "--journal"])
        .arg(temporary.path())
        .arg("--json")
        .output()
        .expect("body rebuild command runs");
    assert!(success.status.success(), "{:?}", success);
    let result: Value = serde_json::from_slice(&success.stdout).expect("JSON result parses");
    assert_eq!(result["schema"], "solstone.body.rebuild.result.v1");
    assert_eq!(result["native_bundles"], 0);
    assert_eq!(result["legacy_bundles"], 0);
    assert_eq!(result["rows"], 0);
    assert!(
        temporary
            .path()
            .join("imports/health-dedupe.sqlite")
            .is_file()
    );

    let database = temporary.path().join("imports/health-dedupe.sqlite");
    let held = hold_lock(&database, LockOptions::default()).expect("hold rebuild lock");
    let timeout = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["body", "rebuild", "--journal"])
        .arg(temporary.path())
        .arg("--json")
        .output()
        .expect("contended body rebuild command runs");
    assert_eq!(timeout.status.code(), Some(75));
    assert!(timeout.stdout.is_empty());
    assert_eq!(
        String::from_utf8(timeout.stderr).expect("stderr is UTF-8"),
        "body rebuild failed: body-rebuild publication: database_lock_timeout\n"
    );
    drop(held);

    let lock_sidecar = temporary.path().join("imports/health-dedupe.sqlite.lock");
    fs::remove_file(&lock_sidecar).expect("regular lock sidecar removes");
    let outside = temporary.path().join("outside-lock-target");
    fs::write(&outside, b"outside-owner-sentinel").expect("outside sentinel writes");
    symlink(&outside, &lock_sidecar).expect("lock sidecar symlink creates");
    let lock_io = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["body", "rebuild", "--journal"])
        .arg(temporary.path())
        .arg("--json")
        .output()
        .expect("invalid lock body rebuild command runs");
    assert_eq!(lock_io.status.code(), Some(74));
    assert!(lock_io.stdout.is_empty());
    assert_eq!(
        String::from_utf8(lock_io.stderr).expect("stderr is UTF-8"),
        "body rebuild failed: body-rebuild publication: database_lock\n"
    );
    assert_eq!(
        fs::read(&outside).expect("outside sentinel reads"),
        b"outside-owner-sentinel"
    );
    fs::remove_file(&lock_sidecar).expect("lock sidecar symlink removes");

    fs::create_dir_all(
        temporary
            .path()
            .join("imports/body-01J9ZK2F5M7Q8R3S4T6V0W1X32"),
    )
    .expect("torn native directory creates");
    let failure = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["body", "rebuild", "--journal"])
        .arg(temporary.path())
        .arg("--json")
        .output()
        .expect("failing body rebuild command runs");
    assert_eq!(failure.status.code(), Some(65));
    assert!(failure.stdout.is_empty());
    assert_eq!(
        String::from_utf8(failure.stderr).expect("stderr is UTF-8"),
        "body rebuild failed: body-rebuild authority: native_authority\n"
    );
}
