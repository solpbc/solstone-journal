// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_journal_io::atomic::{
    atomic_replace_detailed, run_with_windows_detailed_atomic_backoffs,
    run_with_windows_detailed_atomic_barrier, run_with_windows_detailed_atomic_faults,
    run_with_windows_detailed_atomic_faults_and_barrier,
};
use solstone_core_journal_io::cortex_use::{
    CortexUseCandidateRead, CortexUseDestinationCheck, CortexUseRefusal,
    check_cortex_use_destination, inspect_cortex_use_root, read_cortex_use_request,
};
use solstone_core_journal_io::{
    DetailedAtomicOutcome, exercise_windows_managed_log_reference_substrate,
    hold_managed_log_alias_then_publish, publish_test_managed_log_alias,
    root_test_managed_log_alias_name, try_test_managed_log_alias_lock,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_DISK_FULL, ERROR_INVALID_FUNCTION, ERROR_LOCK_VIOLATION,
    ERROR_REPARSE_TAG_INVALID, ERROR_SHARING_VIOLATION, ERROR_USER_MAPPED_FILE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetVolumeInformationByHandleW,
    OPEN_EXISTING,
};

const OLD: &[u8] = b"old-content";
const NEW: &[u8] = b"new-content";
const OUTSIDE_SENTINEL: &[u8] = b"outside-before";

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn wait_for_marker(marker: &Path, step: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if fs::read(marker).ok().as_deref() == Some(step.as_bytes()) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("detailed publication did not pause at {step}");
}

fn kill_paused_detailed_atomic_helper(child: &mut Child, marker: &Path, step: &str) -> ExitStatus {
    wait_for_marker(marker, step);
    confirm_still_blocked_and_kill(child, step)
}

struct MoveReceipt {
    pid: u32,
    stage_name: String,
    volume_serial: u64,
    file_id: [u8; 16],
    terminal_move_snapshot_present: bool,
    terminal_move_snapshot_count: usize,
}

fn parse_move_receipt(bytes: &[u8]) -> Option<MoveReceipt> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut fields = std::collections::HashMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=')?;
        fields.insert(key, value);
    }
    let file_id_hex = *fields.get("file_id")?;
    if file_id_hex.len() != 32 {
        return None;
    }
    let mut file_id = [0u8; 16];
    for (index, byte) in file_id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&file_id_hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(MoveReceipt {
        pid: fields.get("pid")?.parse().ok()?,
        stage_name: (*fields.get("stage")?).to_owned(),
        volume_serial: fields.get("volume_serial")?.parse().ok()?,
        file_id,
        terminal_move_snapshot_present: *fields.get("terminal_move_snapshot_present")? == "1",
        terminal_move_snapshot_count: fields.get("terminal_move_snapshot_count")?.parse().ok()?,
    })
}

fn confirm_still_blocked_and_kill(child: &mut Child, context: &str) -> ExitStatus {
    match child.try_wait() {
        Ok(None) => {}
        Ok(Some(status)) => {
            panic!("detailed publication helper exited before kill at {context}: {status}")
        }
        Err(error) => {
            panic!(
                "could not query detailed publication helper status before kill at {context}: {error}"
            )
        }
    }
    child.kill().unwrap();
    child.wait().unwrap()
}

fn wait_for_move_receipt(child: &Child, marker: &Path) -> MoveReceipt {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(bytes) = fs::read(marker) {
            if let Some(receipt) = parse_move_receipt(&bytes) {
                assert_eq!(
                    receipt.pid,
                    child.id(),
                    "terminal-move acknowledgement token does not match the spawned child"
                );
                return receipt;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("detailed publication did not produce a terminal-move acknowledgement in time");
}

fn assert_complete_stable_file(path: &Path, expected: &[u8]) {
    let metadata = fs::metadata(path)
        .unwrap_or_else(|error| panic!("read metadata for {}: {error}", path.display()));
    assert!(
        metadata.is_file(),
        "{} must remain a regular file",
        path.display()
    );
    let before = file_identity(path);
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let after = file_identity(path);
    assert_eq!(
        after,
        before,
        "{} changed identity while its content was read",
        path.display()
    );
    assert_eq!(
        bytes,
        expected,
        "unexpected complete content at {}",
        path.display()
    );
}

fn assert_file_not_found(path: &Path) {
    match fs::metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => panic!("{} unexpectedly exists", path.display()),
        Err(error) => panic!(
            "expected metadata for {} to return NotFound, got {error}",
            path.display()
        ),
    }
}

fn target_fixture(label: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    target_fixture_in(None, label)
}

fn target_fixture_in(root: Option<&Path>, label: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let mut builder = tempfile::Builder::new();
    builder.prefix(label);
    let temporary = match root {
        Some(root) => builder.tempdir_in(root).unwrap(),
        None => builder.tempdir().unwrap(),
    };
    let parent = temporary.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let target = parent.join("unit.service");
    fs::write(&target, OLD).unwrap();
    (temporary, parent, target)
}

fn outside_sentinel(parent: &Path) -> PathBuf {
    let sentinel = parent.parent().unwrap().join("outside-sentinel");
    fs::write(&sentinel, OUTSIDE_SENTINEL).unwrap();
    sentinel
}

fn assert_sentinel_unchanged(sentinel: &Path) {
    assert_eq!(fs::read(sentinel).unwrap(), OUTSIDE_SENTINEL);
}

fn stage_names(parent: &Path) -> Vec<std::ffi::OsString> {
    fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(".tmp_"))
        .collect()
}

#[test]
fn detailed_atomic_pause_helper() {
    let Some(target) = std::env::var_os("JOURNAL_IO_DETAILED_TARGET") else {
        return;
    };
    if std::env::var_os("JOURNAL_IO_DETAILED_FAIL_BEFORE_CLEANUP").is_some() {
        let (result, _, _) = run_with_windows_detailed_atomic_faults(
            [("write", 1, ERROR_ACCESS_DENIED as i32)],
            || atomic_replace_detailed(Path::new(&target), NEW, 0o600),
        );
        assert!(
            result.is_err(),
            "injected write failure unexpectedly published"
        );
        return;
    }
    atomic_replace_detailed(Path::new(&target), NEW, 0o600).unwrap();
}

#[test]
fn detailed_atomic_replace_survives_kill_at_every_checkpoint() {
    for step in [
        "temp-create",
        "write",
        "fsync-file",
        "close",
        "pre-publication-validation",
        "rename",
        "post-publication-observation",
        "cleanup",
    ] {
        let (_temporary, parent, target) = target_fixture(step);
        let sentinel = outside_sentinel(&parent);
        let marker = parent.join("pause-marker");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "detailed_atomic_pause_helper", "--nocapture"])
            .env("JOURNAL_IO_DETAILED_TARGET", &target)
            .env("JOURNAL_IO_TEST_PAUSE_AT", step)
            .env("JOURNAL_IO_TEST_MARKER", &marker);
        if step == "cleanup" {
            command.env("JOURNAL_IO_DETAILED_FAIL_BEFORE_CLEANUP", "1");
        }
        let mut child = command.spawn().unwrap();
        let status = kill_paused_detailed_atomic_helper(&mut child, &marker, step);
        assert!(!status.success(), "helper unexpectedly completed at {step}");
        let expected = match step {
            "rename" | "post-publication-observation" => NEW,
            _ => OLD,
        };
        assert_eq!(fs::read(&target).unwrap(), expected, "checkpoint {step}");
        assert!(fs::read_dir(&parent).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            name == OsStr::new("unit.service")
                || name == OsStr::new("pause-marker")
                || name.to_string_lossy().starts_with(".tmp_")
        }));
        assert_sentinel_unchanged(&sentinel);
    }
}

#[test]
fn partial_destination_mutation_is_observed_as_unverified() {
    let (_temporary, parent, target) = target_fixture("partial-mutation");
    let sentinel = outside_sentinel(&parent);
    let mutator_target = target.clone();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "post-publication-observation",
        1,
        move || fs::write(&mutator_target, b"partial").unwrap(),
        || atomic_replace_detailed(&target, NEW, 0o600),
    );
    assert!(fired, "post-publication mutator did not run");
    assert!(matches!(
        result.unwrap(),
        DetailedAtomicOutcome::PublishedParentPathUnverified { .. }
    ));
    let partial_is_rejected = catch_unwind(AssertUnwindSafe(|| {
        assert!(b"partial" == OLD || b"partial" == NEW)
    }));
    assert!(
        partial_is_rejected.is_err(),
        "old-or-new assertion is a no-op"
    );
    assert_eq!(fs::read(target).unwrap(), b"partial");
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn post_publication_validation_failure_is_observed_as_unverified() {
    let (_temporary, parent, target) = target_fixture("post-publication-validation");
    let sentinel = outside_sentinel(&parent);
    let (result, attempted, _) = run_with_windows_detailed_atomic_faults(
        [(
            "post-publication-observation",
            1,
            ERROR_ACCESS_DENIED as i32,
        )],
        || atomic_replace_detailed(&target, NEW, 0o600),
    );
    assert!(matches!(
        result.unwrap(),
        DetailedAtomicOutcome::PublishedParentPathUnverified { .. }
    ));
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_eq!(
        attempted
            .iter()
            .filter(|step| **step == "post-publication-observation")
            .count(),
        1
    );
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn absent_destination_publishes_and_missing_parent_is_not_created() {
    let temporary = tempfile::TempDir::new().unwrap();
    let parent = temporary.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let sentinel = outside_sentinel(&parent);
    let absent = parent.join("absent.service");
    assert!(matches!(
        atomic_replace_detailed(&absent, NEW, 0o600).unwrap(),
        DetailedAtomicOutcome::Published
    ));
    assert_eq!(fs::read(&absent).unwrap(), NEW);
    assert_sentinel_unchanged(&sentinel);

    let missing_parent = temporary.path().join("missing");
    let missing = missing_parent.join("unit.service");
    assert!(atomic_replace_detailed(&missing, NEW, 0o600).is_err());
    assert!(!missing_parent.exists());
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn temp_redirection_cannot_move_the_stage_outside_the_destination_parent() {
    let (_temporary, parent, target) = target_fixture("temp-redirection");
    let sentinel = outside_sentinel(&parent);
    let foreign = tempfile::TempDir::new().unwrap();
    let marker = parent.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "detailed_atomic_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_DETAILED_TARGET", &target)
        .env("JOURNAL_IO_TEST_PAUSE_AT", "temp-create")
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .env("TEMP", foreign.path())
        .env("TMP", foreign.path())
        .spawn()
        .unwrap();
    let _ = kill_paused_detailed_atomic_helper(&mut child, &marker, "temp-create");
    assert!(!stage_names(&parent).is_empty());
    assert!(stage_names(foreign.path()).is_empty());
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn leaked_stage_then_republish_succeeds() {
    let (_temporary, parent, target) = target_fixture("leaked-stage-republish");
    let sentinel = outside_sentinel(&parent);
    let marker = parent.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "detailed_atomic_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_DETAILED_TARGET", &target)
        .env("JOURNAL_IO_TEST_PAUSE_AT", "temp-create")
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    let _ = kill_paused_detailed_atomic_helper(&mut child, &marker, "temp-create");
    assert!(!stage_names(&parent).is_empty());
    assert!(matches!(
        atomic_replace_detailed(&target, NEW, 0o600).unwrap(),
        DetailedAtomicOutcome::Published
    ));
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn pre_publication_cleanup_failure_reports_the_orphan_stage() {
    let (_temporary, parent, target) = target_fixture("cleanup-failure");
    let sentinel = outside_sentinel(&parent);
    let (result, attempted, _) = run_with_windows_detailed_atomic_faults(
        [
            ("write", 1, ERROR_ACCESS_DENIED as i32),
            ("cleanup", 1, ERROR_ACCESS_DENIED as i32),
        ],
        || atomic_replace_detailed(&target, NEW, 0o600),
    );
    let error = result.unwrap_err();
    assert_eq!(fs::read(&target).unwrap(), OLD);
    assert!(error.orphan_stage.is_some());
    assert!(error.cleanup_error.is_some());
    assert!(attempted.contains(&"cleanup"));
    for stage in stage_names(&parent) {
        fs::remove_file(parent.join(stage)).unwrap();
    }
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn retryable_publication_errors_have_four_attempts_and_three_recorded_backoffs() {
    for (label, raw_error) in [
        ("sharing", ERROR_SHARING_VIOLATION),
        ("lock", ERROR_LOCK_VIOLATION),
        ("access-denied", ERROR_ACCESS_DENIED),
    ] {
        let (_temporary, parent, target) = target_fixture(label);
        let sentinel = outside_sentinel(&parent);
        let ((result, attempted, _), backoffs) = run_with_windows_detailed_atomic_backoffs(|| {
            run_with_windows_detailed_atomic_faults(
                [
                    ("rename", 1, raw_error as i32),
                    ("rename", 2, raw_error as i32),
                    ("rename", 3, raw_error as i32),
                ],
                || atomic_replace_detailed(&target, NEW, 0o600),
            )
        });
        assert!(matches!(result.unwrap(), DetailedAtomicOutcome::Published));
        assert_eq!(
            attempted.iter().filter(|step| **step == "rename").count(),
            4,
            "{label}"
        );
        assert_eq!(backoffs, vec![Duration::from_millis(250); 3], "{label}");
        assert_eq!(fs::read(&target).unwrap(), NEW);
        assert_sentinel_unchanged(&sentinel);
    }
}

#[test]
fn retryable_publication_errors_exhaust_without_replacing_the_destination() {
    for (label, raw_error) in [
        ("sharing-exhaust", ERROR_SHARING_VIOLATION),
        ("lock-exhaust", ERROR_LOCK_VIOLATION),
        ("access-denied-exhaust", ERROR_ACCESS_DENIED),
    ] {
        let (_temporary, parent, target) = target_fixture(label);
        let sentinel = outside_sentinel(&parent);
        let ((result, attempted, _), backoffs) = run_with_windows_detailed_atomic_backoffs(|| {
            run_with_windows_detailed_atomic_faults(
                [
                    ("rename", 1, raw_error as i32),
                    ("rename", 2, raw_error as i32),
                    ("rename", 3, raw_error as i32),
                    ("rename", 4, raw_error as i32),
                ],
                || atomic_replace_detailed(&target, NEW, 0o600),
            )
        });
        assert!(result.is_err(), "{label}");
        assert_eq!(
            attempted.iter().filter(|step| **step == "rename").count(),
            4,
            "{label}"
        );
        assert_eq!(backoffs, vec![Duration::from_millis(250); 3], "{label}");
        assert_eq!(fs::read(&target).unwrap(), OLD);
        assert_sentinel_unchanged(&sentinel);
    }
}

#[test]
fn one_shot_publication_errors_are_not_retried() {
    for (label, raw_error) in [
        ("user-mapped", ERROR_USER_MAPPED_FILE),
        ("reparse", ERROR_REPARSE_TAG_INVALID),
        ("disk", ERROR_DISK_FULL),
        ("unknown", ERROR_INVALID_FUNCTION),
    ] {
        let (_temporary, parent, target) = target_fixture(label);
        let sentinel = outside_sentinel(&parent);
        let ((result, attempted, _), backoffs) = run_with_windows_detailed_atomic_backoffs(|| {
            run_with_windows_detailed_atomic_faults([("rename", 1, raw_error as i32)], || {
                atomic_replace_detailed(&target, NEW, 0o600)
            })
        });
        assert!(result.is_err(), "{label}");
        assert_eq!(
            attempted.iter().filter(|step| **step == "rename").count(),
            1,
            "{label}"
        );
        assert!(backoffs.is_empty(), "{label}");
        assert_eq!(fs::read(&target).unwrap(), OLD);
        assert_sentinel_unchanged(&sentinel);
    }

    let (_temporary, parent, target) = target_fixture("validation");
    let sentinel = outside_sentinel(&parent);
    let ((result, attempted, _), backoffs) = run_with_windows_detailed_atomic_backoffs(|| {
        run_with_windows_detailed_atomic_faults(
            [(
                "pre-publication-validation",
                1,
                ERROR_INVALID_FUNCTION as i32,
            )],
            || atomic_replace_detailed(&target, NEW, 0o600),
        )
    });
    assert!(result.is_err());
    assert_eq!(
        attempted
            .iter()
            .filter(|step| **step == "pre-publication-validation")
            .count(),
        1
    );
    assert_eq!(
        attempted.iter().filter(|step| **step == "rename").count(),
        0
    );
    assert!(backoffs.is_empty());
    assert_eq!(fs::read(&target).unwrap(), OLD);
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn changed_evidence_during_backoff_refuses_before_a_later_move() {
    let (_temporary, parent, target) = target_fixture("backoff-race");
    let outside = outside_sentinel(&parent);
    let displaced_target = parent.join("displaced.service");
    let raced_target = target.clone();
    let raced_displaced_target = displaced_target.clone();
    let ((result, attempted, fired), backoffs) = run_with_windows_detailed_atomic_backoffs(|| {
        run_with_windows_detailed_atomic_faults_and_barrier(
            [("rename", 1, ERROR_SHARING_VIOLATION as i32)],
            "before-publication-revalidation",
            2,
            move || {
                // The publication attempt intentionally retains the staged child handle, so
                // Windows cannot rename the parent directory here. Race the destination entry
                // that production revalidates instead.
                fs::rename(&raced_target, &raced_displaced_target).unwrap();
                fs::write(&raced_target, b"raced-location").unwrap();
            },
            || atomic_replace_detailed(&target, NEW, 0o600),
        )
    });
    assert!(fired);
    assert!(result.is_err());
    assert_eq!(
        attempted.iter().filter(|step| **step == "rename").count(),
        1
    );
    assert_eq!(backoffs, vec![Duration::from_millis(250)]);
    assert_eq!(
        fs::read(parent.join("unit.service")).unwrap(),
        b"raced-location"
    );
    assert_eq!(fs::read(displaced_target).unwrap(), OLD);
    assert_sentinel_unchanged(&outside);
}

#[test]
fn parent_namespace_race_refuses_before_publication_and_preserves_sentinel() {
    let (_temporary, parent, target) = target_fixture("namespace-race");
    let moved_parent = parent.with_extension("moved");
    let outside = outside_sentinel(&parent);
    let raced_parent = parent.clone();
    let raced_target = target.clone();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "close",
        1,
        move || {
            fs::rename(&raced_parent, &moved_parent).unwrap();
            fs::create_dir(&raced_parent).unwrap();
            fs::write(&raced_target, b"raced-location").unwrap();
        },
        || atomic_replace_detailed(&target, NEW, 0o600),
    );
    assert!(fired);
    assert!(result.is_err());
    assert_eq!(
        fs::read(parent.join("unit.service")).unwrap(),
        b"raced-location"
    );
    assert_sentinel_unchanged(&outside);
}

#[test]
fn hard_link_alias_retains_the_prepublication_file() {
    let (_temporary, parent, target) = target_fixture("hard-link");
    let sentinel = outside_sentinel(&parent);
    let alias = parent.join("alias.service");
    fs::hard_link(&target, &alias).unwrap();
    let before = file_identity(&alias);
    assert!(matches!(
        atomic_replace_detailed(&target, NEW, 0o600).unwrap(),
        DetailedAtomicOutcome::Published
    ));
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_eq!(fs::read(&alias).unwrap(), OLD);
    assert_eq!(file_identity(&alias), before);
    assert_ne!(file_identity(&target), before);
    assert_sentinel_unchanged(&sentinel);
}

fn existing_destination_publication_receipt(root: &Path) {
    let (_temporary, parent, target) = target_fixture_in(Some(root), "publication-existing");
    let sentinel = outside_sentinel(&parent);
    assert!(stage_names(&parent).is_empty(), "clean stage baseline");
    let marker = parent.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "detailed_atomic_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_DETAILED_TARGET", &target)
        .env("JOURNAL_IO_TEST_PAUSE_AT", "terminal-move")
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    let receipt = wait_for_move_receipt(&child, &marker);
    assert!(receipt.terminal_move_snapshot_present);
    assert_eq!(
        receipt.terminal_move_snapshot_count, 0,
        "no real move may have occurred before the paused terminal-move acknowledgement"
    );
    let status = confirm_still_blocked_and_kill(&mut child, "terminal-move");
    assert!(
        !status.success(),
        "helper unexpectedly completed before existing-destination publication"
    );
    assert_complete_stable_file(&target, OLD);
    let leaked = stage_names(&parent);
    assert_eq!(
        leaked,
        vec![std::ffi::OsString::from(&receipt.stage_name)],
        "exactly one same-parent stage must be leaked by the interrupted call"
    );
    assert_eq!(
        file_identity(&parent.join(&receipt.stage_name)),
        (receipt.volume_serial, receipt.file_id),
        "leaked stage identity must match the acknowledgement"
    );
    assert!(matches!(
        atomic_replace_detailed(&target, NEW, 0o600).unwrap(),
        DetailedAtomicOutcome::Published
    ));
    assert_complete_stable_file(&target, NEW);
    assert_sentinel_unchanged(&sentinel);
}

fn absent_destination_publication_receipt(root: &Path) {
    let (temporary, parent, absent) = target_fixture_in(Some(root), "publication-absent");
    let sentinel = outside_sentinel(&parent);
    fs::remove_file(&absent).unwrap();
    assert_file_not_found(&absent);
    assert!(stage_names(&parent).is_empty(), "clean stage baseline");
    let marker = parent.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "detailed_atomic_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_DETAILED_TARGET", &absent)
        .env("JOURNAL_IO_TEST_PAUSE_AT", "terminal-move")
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    let receipt = wait_for_move_receipt(&child, &marker);
    assert!(receipt.terminal_move_snapshot_present);
    assert_eq!(
        receipt.terminal_move_snapshot_count, 0,
        "no real move may have occurred before the paused terminal-move acknowledgement"
    );
    let status = confirm_still_blocked_and_kill(&mut child, "terminal-move");
    assert!(
        !status.success(),
        "helper unexpectedly completed before absent-destination publication"
    );
    assert_file_not_found(&absent);
    let leaked = stage_names(&parent);
    assert_eq!(
        leaked,
        vec![std::ffi::OsString::from(&receipt.stage_name)],
        "exactly one same-parent stage must be leaked by the interrupted call"
    );
    assert_eq!(
        file_identity(&parent.join(&receipt.stage_name)),
        (receipt.volume_serial, receipt.file_id),
        "leaked stage identity must match the acknowledgement"
    );
    assert!(matches!(
        atomic_replace_detailed(&absent, NEW, 0o600).unwrap(),
        DetailedAtomicOutcome::Published
    ));
    assert_complete_stable_file(&absent, NEW);

    let missing_parent = temporary.path().join("missing");
    let missing = missing_parent.join("unit.service");
    assert!(atomic_replace_detailed(&missing, NEW, 0o600).is_err());
    assert!(!missing_parent.exists());
    assert_sentinel_unchanged(&sentinel);
}

fn leaked_stage_then_recovery_publication_receipt(root: &Path) {
    let (_temporary, parent, target) = target_fixture_in(Some(root), "publication-leaked-stage");
    let sentinel = outside_sentinel(&parent);
    let marker = parent.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "detailed_atomic_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_DETAILED_TARGET", &target)
        .env("JOURNAL_IO_TEST_PAUSE_AT", "temp-create")
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    let _ = kill_paused_detailed_atomic_helper(&mut child, &marker, "temp-create");
    assert!(!stage_names(&parent).is_empty());
    assert!(matches!(
        atomic_replace_detailed(&target, NEW, 0o600).unwrap(),
        DetailedAtomicOutcome::Published
    ));
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_sentinel_unchanged(&sentinel);
}

fn classified_retry_then_real_move_publication_receipt(root: &Path) {
    let (_temporary, parent, target) = target_fixture_in(Some(root), "publication-retry");
    let sentinel = outside_sentinel(&parent);
    let ((result, attempted, real_moves), backoffs) =
        run_with_windows_detailed_atomic_backoffs(|| {
            run_with_windows_detailed_atomic_faults(
                [("rename", 1, ERROR_SHARING_VIOLATION as i32)],
                || atomic_replace_detailed(&target, NEW, 0o600),
            )
        });
    assert!(matches!(result.unwrap(), DetailedAtomicOutcome::Published));
    assert_eq!(
        attempted.iter().filter(|step| **step == "rename").count(),
        2,
        "the injected first terminal-move fault must be followed by a real move"
    );
    assert_eq!(
        real_moves, 1,
        "the injected rename fault must not invoke MoveFileExW, and the retry must invoke it once"
    );
    assert_eq!(
        backoffs,
        vec![Duration::from_millis(250)],
        "the injected first terminal-move fault must be consumed"
    );
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_sentinel_unchanged(&sentinel);
}

fn publication_receipt(root: &Path) {
    existing_destination_publication_receipt(root);
    absent_destination_publication_receipt(root);
    leaked_stage_then_recovery_publication_receipt(root);
    classified_retry_then_real_move_publication_receipt(root);
}

fn exercise_cortex_use_receipt(root: &Path) {
    assert!(inspect_cortex_use_root(root).is_ok());
    let talent = root.join("conversation");
    fs::create_dir(&talent).unwrap();
    let active = talent.join("one_active.jsonl");
    fs::write(&active, b"{\"name\":\"conversation\",\"use_id\":\"one\"}\n").unwrap();
    let request = match read_cortex_use_request(&talent, active.file_name().unwrap()) {
        CortexUseCandidateRead::Accepted(request) => request,
        other => panic!("valid Cortex-use request was refused: {other:?}"),
    };
    assert_eq!(
        check_cortex_use_destination(&talent, &request),
        CortexUseDestinationCheck::Vacant
    );

    let invalid = talent.join("invalid_active.jsonl");
    fs::write(&invalid, b"not-json\n").unwrap();
    assert_eq!(
        read_cortex_use_request(&talent, invalid.file_name().unwrap()),
        CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
    );

    let nonregular = talent.join("directory_active.jsonl");
    fs::create_dir(&nonregular).unwrap();
    assert_eq!(
        read_cortex_use_request(&talent, nonregular.file_name().unwrap()),
        CortexUseCandidateRead::Refused(CortexUseRefusal::CandidateNonregular)
    );

    fs::write(talent.join("one.jsonl"), b"completed\n").unwrap();
    assert_eq!(
        check_cortex_use_destination(&talent, &request),
        CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationOccupied)
    );
}

#[test]
#[ignore = "requires a native NTFS filesystem"]
fn ntfs_publication_receipt() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(filesystem_name(root.path()).unwrap(), "NTFS");
    publication_receipt(root.path());
    println!("JOURNAL_WIN_CI_NTFS_PUBLICATION=executed/pass");
    println!("JOURNAL_WIN_CI_NTFS_PUBLICATION_FILESYSTEM=NTFS");
}

#[test]
#[ignore = "requires the native ReFS fixture selected by win-ci.cmd"]
fn refs_publication_receipt() {
    let root = std::env::var_os("SOLSTONE_JOURNAL_WIN_REFS_ROOT")
        .map(PathBuf::from)
        .expect("ReFS publication receipt requires SOLSTONE_JOURNAL_WIN_REFS_ROOT");
    assert_eq!(filesystem_name(&root).unwrap(), "ReFS");
    let temporary = tempfile::Builder::new()
        .prefix("solstone-refs-publication-")
        .tempdir_in(&root)
        .unwrap();
    publication_receipt(temporary.path());
    println!("JOURNAL_WIN_CI_REFS_PUBLICATION=executed/pass");
    println!("JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=ReFS");
}

#[test]
#[ignore = "requires a native NTFS filesystem"]
fn ntfs_cortex_use_receipt() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(filesystem_name(root.path()).unwrap(), "NTFS");
    exercise_cortex_use_receipt(root.path());
    println!("JOURNAL_WIN_CI_CORTEX_USE_NTFS=executed/pass");
    println!("JOURNAL_WIN_CI_CORTEX_USE_NTFS_FILESYSTEM=NTFS");
}

#[test]
#[ignore = "requires a native ReFS filesystem"]
fn refs_cortex_use_receipt() {
    let root = std::env::var_os("SOLSTONE_JOURNAL_WIN_REFS_ROOT")
        .map(PathBuf::from)
        .expect("ReFS Cortex-use receipt requires SOLSTONE_JOURNAL_WIN_REFS_ROOT");
    assert_eq!(filesystem_name(&root).unwrap(), "ReFS");
    let temporary = tempfile::Builder::new()
        .prefix("solstone-refs-cortex-use-")
        .tempdir_in(&root)
        .unwrap();
    exercise_cortex_use_receipt(temporary.path());
    println!("JOURNAL_WIN_CI_CORTEX_USE_REFS=executed/pass");
    println!("JOURNAL_WIN_CI_CORTEX_USE_REFS_FILESYSTEM=ReFS");
}

fn managed_log_reference_receipt(root: &Path) {
    let single_process = root.join("single-process");
    fs::create_dir(&single_process).unwrap();
    exercise_windows_managed_log_reference_substrate(&single_process);

    let process_root = root.join("process-boundary");
    fs::create_dir(&process_root).unwrap();
    let ready = process_root.join("old-parent-ready");
    let release = process_root.join("release-old-parent");
    let outcome = process_root.join("old-parent-outcome");
    let logical_name = "shared-process-alias";
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "managed_log_split_lock_child",
            "--nocapture",
        ])
        .env("SOLSTONE_MANAGED_LOG_CHILD_ROOT", &process_root)
        .env("SOLSTONE_MANAGED_LOG_CHILD_NAME", logical_name)
        .env("SOLSTONE_MANAGED_LOG_CHILD_READY", &ready)
        .env("SOLSTONE_MANAGED_LOG_CHILD_RELEASE", &release)
        .env("SOLSTONE_MANAGED_LOG_CHILD_OUTCOME", &outcome)
        .spawn()
        .unwrap();
    wait_for_marker(&ready, "ready");

    assert!(
        !try_test_managed_log_alias_lock(&process_root, logical_name, Duration::from_millis(150),),
        "a second process acquired the same persistent alias lock"
    );
    assert!(
        try_test_managed_log_alias_lock(
            &process_root,
            "independent-process-alias",
            Duration::from_millis(150),
        ),
        "an independent alias was blocked by a global lock"
    );

    let aliases = process_root.join("aliases");
    let retired = process_root.join("aliases-retired");
    let rename_error = fs::rename(&aliases, &retired).unwrap_err();
    assert_eq!(
        rename_error.raw_os_error(),
        Some(ERROR_ACCESS_DENIED as i32),
        "Windows did not fail closed while the persistent child lock was live"
    );
    fs::write(&release, b"release").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "split-lock child failed: {status}");
    assert_eq!(fs::read(&outcome).unwrap(), b"old-parent-published");

    fs::rename(&aliases, &retired).unwrap();
    fs::create_dir(&aliases).unwrap();
    publish_test_managed_log_alias(&process_root, logical_name, b"fresh-parent-published");

    let alias_name = root_test_managed_log_alias_name(logical_name);
    assert_eq!(
        fs::read(aliases.join(&alias_name)).unwrap(),
        b"fresh-parent-published"
    );
    assert!(
        fs::read(retired.join(&alias_name)).unwrap() == b"old-parent-published",
        "the retained parent did not contain exactly the child's publication"
    );
    assert!(
        fs::read_dir(&retired).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".lock")),
        "the old parent did not retain its persistent alias lock"
    );
    assert!(
        fs::read_dir(&aliases).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".lock")),
        "the fresh parent did not retain its persistent alias lock"
    );
}

#[test]
#[ignore = "invoked only as a child by the managed-log native receipt"]
fn managed_log_split_lock_child() {
    hold_managed_log_alias_then_publish(
        &PathBuf::from(std::env::var_os("SOLSTONE_MANAGED_LOG_CHILD_ROOT").unwrap()),
        &std::env::var("SOLSTONE_MANAGED_LOG_CHILD_NAME").unwrap(),
        &PathBuf::from(std::env::var_os("SOLSTONE_MANAGED_LOG_CHILD_READY").unwrap()),
        &PathBuf::from(std::env::var_os("SOLSTONE_MANAGED_LOG_CHILD_RELEASE").unwrap()),
        &PathBuf::from(std::env::var_os("SOLSTONE_MANAGED_LOG_CHILD_OUTCOME").unwrap()),
    );
}

#[test]
#[ignore = "requires a native NTFS filesystem"]
fn ntfs_managed_log_reference_receipt() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(filesystem_name(root.path()).unwrap(), "NTFS");
    managed_log_reference_receipt(root.path());
    println!("JOURNAL_WIN_CI_NTFS_MANAGED_LOG_REFERENCE=executed/pass");
    println!("JOURNAL_WIN_CI_NTFS_MANAGED_LOG_REFERENCE_FILESYSTEM=NTFS");
}

#[test]
#[ignore = "requires the native ReFS fixture selected by win-ci.cmd"]
fn refs_managed_log_reference_receipt() {
    let root = std::env::var_os("SOLSTONE_JOURNAL_WIN_REFS_ROOT")
        .map(PathBuf::from)
        .expect("ReFS managed-log receipt requires SOLSTONE_JOURNAL_WIN_REFS_ROOT");
    assert_eq!(filesystem_name(&root).unwrap(), "ReFS");
    let temporary = tempfile::Builder::new()
        .prefix("solstone-refs-managed-log-reference-")
        .tempdir_in(&root)
        .unwrap();
    managed_log_reference_receipt(temporary.path());
    println!("JOURNAL_WIN_CI_REFS_MANAGED_LOG_REFERENCE=executed/pass");
    println!("JOURNAL_WIN_CI_REFS_MANAGED_LOG_REFERENCE_FILESYSTEM=ReFS");
}

fn file_identity(path: &Path) -> (u64, [u8; 16]) {
    let wide_path = wide(path.as_os_str());
    // SAFETY: `path` is NUL-terminated and the successful handle is owned exactly once.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        raw,
        INVALID_HANDLE_VALUE,
        "open identity handle for {}",
        path.display()
    );
    // SAFETY: `raw` passed the invalid-handle sentinel check and is uniquely owned here.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut filesystem_name = [0u16; 256];
    let mut volume_name = [0u16; 256];
    let mut serial = 0;
    let mut maximum_component_length = 0;
    let mut flags = 0;
    // SAFETY: the name buffers are writable for their exact supplied lengths and the handle is valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem_name.as_mut_ptr(),
            filesystem_name.len() as u32,
        )
    };
    assert_ne!(result, 0, "query volume serial");
    let mut info = windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO::default();
    // SAFETY: `info` is writable for its exact size and the handle is valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            windows_sys::Win32::Storage::FileSystem::FileIdInfo,
            (&mut info as *mut windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO).cast(),
            size_of::<windows_sys::Win32::Storage::FileSystem::FILE_ID_INFO>() as u32,
        )
    };
    assert_ne!(result, 0, "query file identity");
    (info.VolumeSerialNumber, info.FileId.Identifier)
}

fn filesystem_name(path: &Path) -> io::Result<String> {
    let wide_path = wide(path.as_os_str());
    // SAFETY: `path` is NUL-terminated and the successful handle is owned exactly once.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` passed the invalid-handle sentinel check and is uniquely owned here.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut filesystem_name = [0u16; 256];
    let mut volume_name = [0u16; 256];
    let mut serial = 0;
    let mut maximum_component_length = 0;
    let mut flags = 0;
    // SAFETY: the name buffers are writable for their exact supplied lengths and the handle is valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem_name.as_mut_ptr(),
            filesystem_name.len() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let terminator = filesystem_name
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "filesystem name is not terminated",
            )
        })?;
    String::from_utf16(&filesystem_name[..terminator])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "filesystem name is not UTF-16"))
}
