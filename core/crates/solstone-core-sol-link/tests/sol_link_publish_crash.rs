// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

use solstone_core_sol_link::ca::{jid_from_spki, load_ca};
use solstone_core_sol_link::establish::{
    bundle_path, current_candidate, load_committed, lock_in, run_env_paused_lock_in,
};
use solstone_core_sol_link::publish_test_hooks::PublishCheckpoint;

#[test]
fn lock_in_pause_helper() {
    if std::env::var("SOL_LINK_TEST_JOURNAL").is_err() {
        return;
    }
    run_env_paused_lock_in();
}

#[cfg(unix)]
#[test]
fn crash_before_staging_dir_create_leaves_no_bundle_and_retries() {
    assert_crash_before_publish_then_retry(PublishCheckpoint::BeforeStagingDirCreate);
}

#[cfg(unix)]
#[test]
fn crash_after_staging_dir_create_leaves_no_bundle_and_retries() {
    assert_crash_before_publish_then_retry(PublishCheckpoint::AfterStagingDirCreate);
}

#[cfg(unix)]
#[test]
fn crash_after_cert_write_leaves_no_bundle_and_retries() {
    assert_crash_before_publish_then_retry(PublishCheckpoint::MidPopulateCert);
}

#[cfg(unix)]
#[test]
fn crash_after_private_key_write_leaves_no_bundle_and_retries() {
    assert_crash_before_publish_then_retry(PublishCheckpoint::MidPopulateKey);
}

#[cfg(unix)]
#[test]
fn crash_after_populate_leaves_no_bundle_and_retries() {
    assert_crash_before_publish_then_retry(PublishCheckpoint::AfterPopulate);
}

#[cfg(unix)]
#[test]
fn crash_after_staging_sync_leaves_no_bundle_and_retries() {
    assert_crash_before_publish_then_retry(PublishCheckpoint::AfterStagingSync);
}

#[cfg(unix)]
#[test]
fn crash_after_rename_leaves_complete_bundle() {
    let temporary = TempDir::new();
    current_candidate(temporary.path()).unwrap();
    run_child_until_pause(temporary.path(), PublishCheckpoint::AfterRename);
    assert_complete_bundle(temporary.path());
}

#[cfg(unix)]
fn assert_crash_before_publish_then_retry(checkpoint: PublishCheckpoint) {
    let temporary = TempDir::new();
    current_candidate(temporary.path()).unwrap();
    run_child_until_pause(temporary.path(), checkpoint);
    assert!(
        !bundle_path(temporary.path()).exists(),
        "checkpoint: {}",
        checkpoint.as_str()
    );
    lock_in(temporary.path(), None).unwrap();
    assert_complete_bundle(temporary.path());
}

#[cfg(unix)]
fn run_child_until_pause(journal: &Path, checkpoint: PublishCheckpoint) {
    let marker = journal.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("lock_in_pause_helper")
        .arg("--exact")
        .env("SOL_LINK_TEST_JOURNAL", journal)
        .env("JOURNAL_IO_TEST_PAUSE_AT", checkpoint.as_str())
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    wait_for_marker(&mut child, &marker, checkpoint);
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(
        !status.success(),
        "child unexpectedly completed at {}",
        checkpoint.as_str()
    );
}

#[cfg(unix)]
fn wait_for_marker(child: &mut std::process::Child, marker: &Path, checkpoint: PublishCheckpoint) {
    let name = checkpoint.as_str();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_to_string(marker).ok().as_deref() == Some(name) {
            return;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("child exited before {name}: {status}");
        }
        assert!(Instant::now() < deadline, "timed out waiting for {name}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn assert_complete_bundle(journal: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let bundle = bundle_path(journal);
    assert!(bundle.join("cert.pem").is_file());
    assert!(bundle.join("private.pem").is_file());
    assert!(bundle.join("state.json").is_file());
    assert_eq!(
        fs::metadata(bundle.join("private.pem"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let state = load_committed(journal).unwrap().unwrap();
    let ca = load_ca(
        &fs::read_to_string(bundle.join("cert.pem")).unwrap(),
        &fs::read_to_string(bundle.join("private.pem")).unwrap(),
    )
    .unwrap();
    assert_eq!(state.instance_id, jid_from_spki(ca.spki_der()).unwrap());
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-sol-link-publish-crash-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
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
