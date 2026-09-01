// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_journal_io::atomic::{
    atomic_replace_detailed, run_with_windows_detailed_atomic_backoffs,
    run_with_windows_detailed_atomic_barrier, run_with_windows_detailed_atomic_faults,
    run_with_windows_detailed_atomic_faults_and_barrier,
};
use solstone_core_journal_io::cortex_use::{
    CortexCensus, CortexCensusError, CortexCensusPrimitive, CortexNamespaceLock,
    CortexNamespaceLockError, CortexUseCandidateRead, CortexUseDestinationCheck, CortexUseRefusal,
    acquire_cortex_namespace_lock, acquire_cortex_namespace_lock_with_test_timing,
    census_cortex_namespace, check_cortex_use_destination, create_or_admit_cortex_namespace,
    inspect_cortex_use_root, parse_cortex_lifecycle_name, read_cortex_use_request,
    run_with_cortex_census_barrier,
};
use solstone_core_journal_io::{
    DetailedAtomicOutcome, ExistingParentLockError, JournalRoot, WindowsLockFileExSubstitution,
    acquire_existing_parent_lock, exercise_windows_managed_log_logical_coordinates,
    exercise_windows_managed_log_reference_substrate, hold_managed_log_alias_then_publish,
    list_windows_flat_directory, publish_test_managed_log_alias, root_test_managed_log_alias_name,
    run_with_forced_post_lock_identity_mismatch, run_with_windows_lock_file_ex_substitution,
    try_test_managed_log_alias_lock,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_DISK_FULL, ERROR_INVALID_FUNCTION, ERROR_LOCK_VIOLATION,
    ERROR_REPARSE_TAG_INVALID, ERROR_SHARING_VIOLATION, ERROR_USER_MAPPED_FILE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetVolumeInformationByHandleW, OPEN_EXISTING,
};

const OLD: &[u8] = b"old-content";
const NEW: &[u8] = b"new-content";
const OUTSIDE_SENTINEL: &[u8] = b"outside-before";
const CORTEX_LOCK_CHILD_MARKER_ENV: &str = "JOURNAL_WIN_CI_CORTEX_LOCK_CHILD";
const CORTEX_LOCK_CHILD_MARKER_VALUE: &str = "cortex-lock-child-v1";
const CORTEX_LOCK_CHILD_EXPECT_ENV: &str = "JOURNAL_WIN_CI_CORTEX_LOCK_EXPECT";
const CORTEX_LOCK_CHILD_ROOT_ENV: &str = "JOURNAL_WIN_CI_CORTEX_LOCK_ROOT";
const CORTEX_LOCK_CHILD_BUSY: &str = "CORTEX_LOCK_CHILD_BUSY";
const CORTEX_LOCK_CHILD_ACQUIRED: &str = "CORTEX_LOCK_CHILD_ACQUIRED";
const LOGICAL_FIELD_SHAPES: &[&str] = &[
    "maintenance:backup:run",
    "/leading",
    "embedded/slash",
    r"embedded\backslash",
    ".",
    "..",
    "<",
    ">",
    "\"",
    "|",
    "?",
    "*",
    "CON",
    "COM1",
    "trailing.",
    "trailing ",
];

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

fn create_directory_junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("launch cmd.exe for Cortex namespace junction fixture");
    assert!(
        output.status.success(),
        "create Cortex namespace junction fixture {} -> {}: status={} stdout={} stderr={}",
        link.display(),
        target.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn create_file_symlink(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .args(["/d", "/c", "mklink"])
        .arg(link)
        .arg(target)
        .output()
        .expect("launch cmd.exe for Cortex file-reparse fixture");
    assert!(
        output.status.success(),
        "create Cortex file-reparse fixture {} -> {}: status={} stdout={} stderr={}",
        link.display(),
        target.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn cortex_namespace_failure(root: &Path) -> String {
    match create_or_admit_cortex_namespace(JournalRoot::open(root).unwrap()) {
        Ok(_) => panic!("Cortex namespace fixture unexpectedly admitted"),
        Err(error) => error.to_string(),
    }
}

fn cortex_lock_error(result: Result<CortexNamespaceLock, CortexNamespaceLockError>) -> String {
    match result {
        Ok(_) => panic!("Cortex namespace lock fixture unexpectedly acquired"),
        Err(error) => error.to_string(),
    }
}

/// `CortexCensus` does not implement `Debug` (by design -- it retains an admitted
/// namespace authority and a live directory handle), so `Result::expect_err`/`unwrap_err`
/// cannot be called directly on `Result<CortexCensus, CortexCensusError>` -- both require
/// `T: Debug` to format a panic message on the `Ok` arm. Match explicitly instead, the same
/// idiom census.rs's own internal unit tests use (`census_err`).
fn census_err(result: Result<CortexCensus, CortexCensusError>, message: &str) -> CortexCensusError {
    match result {
        Ok(_) => panic!("{message}"),
        Err(error) => error,
    }
}

/// Minimal preservation-accounting snapshot for one successful, non-adversarial census
/// receipt (F3 §5 item 9 / the F3 caller-owned-gate's "preservation" property) --
/// deliberately smaller than census.rs's own internal `snapshot_tree`/`Snap`, which also
/// tracks renames for its many adversarial barrier fixtures; this receipt only needs one
/// shape: "nothing changed except the F2 lock entry's first creation." Records kind, size,
/// and (for regular files) exact content bytes for every entry under `root`. A directory
/// junction/reparse point is never recursed into -- checked via the raw
/// `FILE_ATTRIBUTE_REPARSE_POINT` bit (the same `.file_attributes()` idiom already used
/// elsewhere in this file), not `FileType::is_dir()`/`is_symlink()`, whose exact behavior
/// for a Windows junction this receipt does not need to depend on either way -- and is
/// recorded only as an opaque "other" leaf, matching how the census itself treats it.
///
/// Caller contract: never snapshot while a live `CortexCensus`/`CortexNamespaceLock` for
/// this same root is still held -- reading `cortex-use.lock`'s own bytes through a fresh
/// handle while its exclusive Windows byte-range lock is live is the exact
/// `ERROR_LOCK_VIOLATION` (Os error 33) trap this validation pass found in census.rs's
/// own internal test suite (separately scope-checked repair pending, not part of this
/// receipt). Drop any holder first, as the caller here does.
fn snapshot_journal_tree(
    root: &Path,
) -> std::collections::BTreeMap<PathBuf, (&'static str, u64, Vec<u8>)> {
    fn walk(
        dir: &Path,
        root: &Path,
        out: &mut std::collections::BTreeMap<PathBuf, (&'static str, u64, Vec<u8>)>,
    ) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let is_reparse_point = metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            let file_type = metadata.file_type();
            let (kind, bytes) = if is_reparse_point {
                ("other", Vec::new())
            } else if file_type.is_dir() {
                ("dir", Vec::new())
            } else if file_type.is_file() {
                ("file", fs::read(&path).unwrap())
            } else {
                ("other", Vec::new())
            };
            let recurse = !is_reparse_point && kind == "dir";
            out.insert(relative, (kind, metadata.len(), bytes));
            if recurse {
                walk(&path, root, out);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Assert that `after` differs from `before` by nothing except one added
/// `cortex-use.lock` entry (the F2 lock's first-ever creation on this namespace).
fn assert_preserved_except_lock_creation(
    before: &std::collections::BTreeMap<PathBuf, (&'static str, u64, Vec<u8>)>,
    after: &std::collections::BTreeMap<PathBuf, (&'static str, u64, Vec<u8>)>,
) {
    for (path, value) in before {
        assert_eq!(
            after.get(path),
            Some(value),
            "a successful census must never mutate a pre-existing entry: {path:?}"
        );
    }
    let lock_entry = PathBuf::from("cortex-use.lock");
    let added: Vec<_> = after
        .keys()
        .filter(|path| !before.contains_key(*path))
        .collect();
    assert!(
        added.iter().all(|path| **path == lock_entry),
        "a successful census must add nothing except the F2 lock entry, found: {added:?}"
    );
}

fn run_cortex_lock_child(root: &Path, test_name: &str, expected: &str) -> Output {
    Command::new(std::env::current_exe().expect("current Windows test executable"))
        .args(["--exact", test_name, "--ignored", "--nocapture"])
        .env(CORTEX_LOCK_CHILD_MARKER_ENV, CORTEX_LOCK_CHILD_MARKER_VALUE)
        .env(CORTEX_LOCK_CHILD_EXPECT_ENV, expected)
        .env(CORTEX_LOCK_CHILD_ROOT_ENV, root)
        .output()
        .expect("run Cortex namespace lock child")
}

fn require_cortex_lock_child(output: Output, receipt: &str) {
    assert!(
        output.status.success(),
        "Cortex namespace lock child failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("child stdout is UTF-8");
    assert_eq!(stdout.matches(receipt).count(), 1, "child stdout: {stdout}");
}

struct DeniedFileAcl {
    path: PathBuf,
    account: String,
    active: bool,
}

impl DeniedFileAcl {
    fn install(path: &Path) -> Self {
        let account = std::env::var("SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT")
            .expect("native Windows rail supplies its ordinary owner account");
        let guard = Self {
            path: path.to_path_buf(),
            account,
            active: true,
        };
        let output = Command::new("icacls")
            .arg(path)
            .arg("/deny")
            .arg(format!("{}:(R,W)", guard.account))
            .output()
            .unwrap_or_else(|error| panic!("launch icacls to deny the lock entry: {error}"));
        assert!(
            output.status.success(),
            "deny lock-entry ACL: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        guard
    }

    fn restore(mut self) {
        let output = self
            .restore_output()
            .unwrap_or_else(|error| panic!("launch icacls to restore the lock entry: {error}"));
        assert!(
            output.status.success(),
            "restore lock-entry ACL: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        self.active = false;
    }

    fn restore_output(&self) -> io::Result<Output> {
        Command::new("icacls")
            .arg(&self.path)
            .arg("/remove:d")
            .arg(&self.account)
            .output()
    }
}

impl Drop for DeniedFileAcl {
    fn drop(&mut self) {
        if self.active {
            match self.restore_output() {
                Ok(output) if !output.status.success() => eprintln!(
                    "failed to restore lock-entry ACL during cleanup: status={} stdout={} stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => {
                    eprintln!("failed to launch lock-entry ACL cleanup: {error}");
                }
                Ok(_) => {}
            }
        }
    }
}

fn run_marked_cortex_lock_child() -> bool {
    let Some(marker) = std::env::var_os(CORTEX_LOCK_CHILD_MARKER_ENV) else {
        return false;
    };
    if marker != OsStr::new(CORTEX_LOCK_CHILD_MARKER_VALUE) {
        return false;
    }
    let root = std::env::var_os(CORTEX_LOCK_CHILD_ROOT_ENV)
        .map(PathBuf::from)
        .expect("marked Cortex lock child requires a root");
    let expected = std::env::var(CORTEX_LOCK_CHILD_EXPECT_ENV)
        .expect("marked Cortex lock child requires an expected outcome");
    let authority = create_or_admit_cortex_namespace(JournalRoot::open(&root).unwrap()).unwrap();
    match expected.as_str() {
        "busy" => {
            assert_eq!(
                cortex_lock_error(acquire_cortex_namespace_lock(&authority)),
                "cortex_namespace_lock_busy"
            );
            println!("{CORTEX_LOCK_CHILD_BUSY}");
        }
        "acquired" => {
            let _lock = acquire_cortex_namespace_lock(&authority).unwrap();
            println!("{CORTEX_LOCK_CHILD_ACQUIRED}");
        }
        other => panic!("unknown Cortex lock child expectation: {other}"),
    }
    true
}

fn exercise_cortex_namespace_receipt(root: &Path) {
    let create_root = root.join("cortex-namespace-create-admit");
    fs::create_dir(&create_root).unwrap();
    fs::write(create_root.join("unrelated"), b"root-before").unwrap();

    let created = create_or_admit_cortex_namespace(JournalRoot::open(&create_root).unwrap())
        .expect("create both missing Cortex namespace children");
    assert!(create_root.join("health").is_dir());
    assert!(create_root.join("talents").is_dir());
    assert_eq!(
        list_windows_flat_directory(created.health(), 8)
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>(),
        Vec::<std::ffi::OsString>::new()
    );
    drop(created);
    fs::write(create_root.join("health/preserved"), b"health-before").unwrap();
    let admitted = create_or_admit_cortex_namespace(JournalRoot::open(&create_root).unwrap())
        .expect("admit existing Cortex namespace children");
    assert!(
        list_windows_flat_directory(admitted.talents(), 8)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read(create_root.join("health/preserved")).unwrap(),
        b"health-before"
    );
    assert_eq!(
        fs::read(create_root.join("unrelated")).unwrap(),
        b"root-before"
    );
    drop(admitted);

    let wrong_kind_root = root.join("cortex-namespace-wrong-kind");
    fs::create_dir(&wrong_kind_root).unwrap();
    fs::write(wrong_kind_root.join("health"), b"wrong-kind-before").unwrap();
    fs::create_dir(wrong_kind_root.join("talents")).unwrap();
    fs::write(wrong_kind_root.join("talents/preserved"), b"talents-before").unwrap();
    let wrong_kind_health_identity = file_identity(&wrong_kind_root.join("health"));
    let wrong_kind_talents_identity = file_identity(&wrong_kind_root.join("talents"));
    assert_eq!(
        cortex_namespace_failure(&wrong_kind_root),
        "cortex_namespace_health_unsafe"
    );
    assert_eq!(
        file_identity(&wrong_kind_root.join("health")),
        wrong_kind_health_identity
    );
    assert_eq!(
        file_identity(&wrong_kind_root.join("talents")),
        wrong_kind_talents_identity
    );
    assert_eq!(
        fs::read(wrong_kind_root.join("health")).unwrap(),
        b"wrong-kind-before"
    );
    assert_eq!(
        fs::read(wrong_kind_root.join("talents/preserved")).unwrap(),
        b"talents-before"
    );
    let mut wrong_kind_entries = fs::read_dir(&wrong_kind_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    wrong_kind_entries.sort();
    assert_eq!(
        wrong_kind_entries,
        vec![
            OsStr::new("health").to_os_string(),
            OsStr::new("talents").to_os_string()
        ]
    );

    let reparse_root = root.join("cortex-namespace-reparse");
    let reparse_target = root.join("cortex-namespace-reparse-target");
    fs::create_dir(&reparse_root).unwrap();
    fs::create_dir(&reparse_target).unwrap();
    fs::write(reparse_root.join("unrelated"), b"root-before").unwrap();
    fs::write(reparse_target.join("outside"), b"outside-before").unwrap();
    create_directory_junction(&reparse_root.join("talents"), &reparse_target);
    let reparse_identity = file_identity(&reparse_root.join("talents"));
    let unrelated_identity = file_identity(&reparse_root.join("unrelated"));
    let outside_identity = file_identity(&reparse_target.join("outside"));
    assert_eq!(
        cortex_namespace_failure(&reparse_root),
        "cortex_namespace_talents_unsafe"
    );
    assert!(reparse_root.join("health").is_dir());
    assert!(
        fs::read_dir(reparse_root.join("health"))
            .unwrap()
            .next()
            .is_none(),
        "talents refusal must leave only the empty created health residual"
    );
    assert_eq!(
        file_identity(&reparse_root.join("talents")),
        reparse_identity
    );
    assert_ne!(
        fs::symlink_metadata(reparse_root.join("talents"))
            .unwrap()
            .file_attributes()
            & FILE_ATTRIBUTE_REPARSE_POINT,
        0
    );
    assert_eq!(
        file_identity(&reparse_root.join("unrelated")),
        unrelated_identity
    );
    assert_eq!(
        file_identity(&reparse_target.join("outside")),
        outside_identity
    );
    assert_eq!(
        fs::read(reparse_root.join("unrelated")).unwrap(),
        b"root-before"
    );
    assert_eq!(
        fs::read(reparse_target.join("outside")).unwrap(),
        b"outside-before"
    );
    let mut reparse_entries = fs::read_dir(&reparse_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    reparse_entries.sort();
    assert_eq!(
        reparse_entries,
        vec![
            OsStr::new("health").to_os_string(),
            OsStr::new("talents").to_os_string(),
            OsStr::new("unrelated").to_os_string()
        ]
    );

    let root_replacement_parent = root.join("cortex-namespace-retained-root");
    let original_root = root_replacement_parent.join("journal");
    let moved_root = root_replacement_parent.join("journal-moved");
    fs::create_dir(&root_replacement_parent).unwrap();
    fs::create_dir(&original_root).unwrap();
    let retained_root = JournalRoot::open(&original_root).unwrap();
    fs::rename(&original_root, &moved_root).unwrap();
    fs::create_dir(&original_root).unwrap();
    fs::write(original_root.join("replacement"), b"replacement-before").unwrap();
    let rooted = create_or_admit_cortex_namespace(retained_root).unwrap();
    assert!(moved_root.join("health").is_dir());
    assert!(moved_root.join("talents").is_dir());
    assert!(!original_root.join("health").exists());
    assert!(!original_root.join("talents").exists());
    assert_eq!(
        fs::read(original_root.join("replacement")).unwrap(),
        b"replacement-before"
    );
    drop(rooted);

    let health_replacement_root = root.join("cortex-namespace-retained-health");
    fs::create_dir(&health_replacement_root).unwrap();
    let retained_health =
        create_or_admit_cortex_namespace(JournalRoot::open(&health_replacement_root).unwrap())
            .unwrap();
    fs::write(
        health_replacement_root.join("health/original"),
        b"original-before",
    )
    .unwrap();
    fs::rename(
        health_replacement_root.join("health"),
        health_replacement_root.join("health-moved"),
    )
    .unwrap();
    fs::create_dir(health_replacement_root.join("health")).unwrap();
    fs::write(
        health_replacement_root.join("health/replacement"),
        b"replacement-before",
    )
    .unwrap();
    let retained_entries = list_windows_flat_directory(retained_health.health(), 8)
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    assert_eq!(
        retained_entries,
        vec![OsStr::new("original").to_os_string()]
    );
    assert_eq!(
        fs::read(health_replacement_root.join("health/replacement")).unwrap(),
        b"replacement-before"
    );
}

fn exercise_cortex_namespace_lock_receipt(root: &Path, child_test: &str) {
    let lock_root = root.join("cortex-namespace-lock");
    let health = lock_root.join("health");
    let moved_health = lock_root.join("health-moved");
    fs::create_dir(&lock_root).unwrap();
    fs::write(lock_root.join("root-sentinel"), b"root").unwrap();
    let authority_a =
        create_or_admit_cortex_namespace(JournalRoot::open(&lock_root).unwrap()).unwrap();
    fs::write(lock_root.join("health/sentinel"), b"old-health").unwrap();
    fs::write(lock_root.join("talents/sentinel"), b"talents").unwrap();
    let root_identity = file_identity(&lock_root);
    let talents_identity = file_identity(&lock_root.join("talents"));
    let old_health_identity = file_identity(&health);
    let lock_entry = lock_root.join("cortex-use.lock");
    fs::write(&lock_entry, b"persistent-lock-entry").unwrap();
    let lock_identity = file_identity(&lock_entry);
    let lock_bytes = fs::read(&lock_entry).unwrap();
    let parent_lock = acquire_cortex_namespace_lock(&authority_a).unwrap();

    fs::rename(&health, &moved_health).unwrap();
    fs::create_dir(&health).unwrap();
    fs::write(health.join("sentinel"), b"replacement-health").unwrap();
    let new_health_identity = file_identity(&health);
    let _authority_b =
        create_or_admit_cortex_namespace(JournalRoot::open(&lock_root).unwrap()).unwrap();
    let assert_namespace_identities = || {
        assert_eq!(file_identity(&lock_root), root_identity);
        assert_eq!(file_identity(&lock_root.join("talents")), talents_identity);
        assert_eq!(file_identity(&moved_health), old_health_identity);
        assert_eq!(file_identity(&health), new_health_identity);
    };
    require_cortex_lock_child(
        run_cortex_lock_child(&lock_root, child_test, "busy"),
        CORTEX_LOCK_CHILD_BUSY,
    );
    assert_namespace_identities();
    assert_eq!(fs::read(lock_root.join("root-sentinel")).unwrap(), b"root");
    assert_eq!(
        fs::read(lock_root.join("health-moved/sentinel")).unwrap(),
        b"old-health"
    );
    assert_eq!(
        fs::read(lock_root.join("health/sentinel")).unwrap(),
        b"replacement-health"
    );
    assert_eq!(
        fs::read(lock_root.join("talents/sentinel")).unwrap(),
        b"talents"
    );
    assert!(!lock_root.join("health-moved/cortex-use.lock").exists());
    assert!(!lock_root.join("health/cortex-use.lock").exists());
    drop(parent_lock);

    require_cortex_lock_child(
        run_cortex_lock_child(&lock_root, child_test, "acquired"),
        CORTEX_LOCK_CHILD_ACQUIRED,
    );
    assert_namespace_identities();
    assert_eq!(file_identity(&lock_entry), lock_identity);
    assert_eq!(fs::read(&lock_entry).unwrap(), lock_bytes);

    let left_root = root.join("cortex-lock-left");
    let right_root = root.join("cortex-lock-right");
    fs::create_dir(&left_root).unwrap();
    fs::create_dir(&right_root).unwrap();
    let left_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&left_root).unwrap()).unwrap();
    let right_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&right_root).unwrap()).unwrap();
    let left_entry = left_root.join("cortex-use.lock");
    let right_entry = right_root.join("cortex-use.lock");
    fs::write(&left_entry, b"byte-identical-valid-entry").unwrap();
    fs::write(&right_entry, b"byte-identical-valid-entry").unwrap();
    let left_identity = file_identity(&left_entry);
    let right_identity = file_identity(&right_entry);
    assert_ne!(left_identity, right_identity);
    let left_lock = acquire_cortex_namespace_lock(&left_authority).unwrap();
    let right_lock = acquire_cortex_namespace_lock(&right_authority).unwrap();
    drop(right_lock);
    drop(left_lock);
    assert_eq!(file_identity(&left_entry), left_identity);
    assert_eq!(file_identity(&right_entry), right_identity);
    assert_eq!(
        fs::read(&left_entry).unwrap(),
        b"byte-identical-valid-entry"
    );
    assert_eq!(
        fs::read(&right_entry).unwrap(),
        b"byte-identical-valid-entry"
    );
    let wrong_root = root.join("cortex-lock-wrong-kind");
    fs::create_dir(&wrong_root).unwrap();
    let wrong_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&wrong_root).unwrap()).unwrap();
    let wrong_entry = wrong_root.join("cortex-use.lock");
    fs::create_dir(&wrong_entry).unwrap();
    let wrong_identity = file_identity(&wrong_entry);
    let generic_wrong = acquire_existing_parent_lock(
        &wrong_root,
        OsStr::new("cortex-use.lock"),
        Duration::ZERO,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(matches!(
        generic_wrong,
        ExistingParentLockError::UnsafeLockEntry {
            kind: "directory",
            ..
        }
    ));
    assert_eq!(
        cortex_lock_error(acquire_cortex_namespace_lock(&wrong_authority)),
        "cortex_namespace_lock_unsafe"
    );
    assert_eq!(file_identity(&wrong_entry), wrong_identity);

    let reparse_root = root.join("cortex-lock-reparse");
    let reparse_target = root.join("cortex-lock-reparse-target");
    fs::create_dir(&reparse_root).unwrap();
    fs::create_dir(&reparse_target).unwrap();
    let reparse_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&reparse_root).unwrap()).unwrap();
    fs::write(reparse_target.join("outside"), b"outside").unwrap();
    let outside_identity = file_identity(&reparse_target.join("outside"));
    let reparse_entry = reparse_root.join("cortex-use.lock");
    create_directory_junction(&reparse_entry, &reparse_target);
    let reparse_identity = file_identity(&reparse_entry);
    let generic_reparse = acquire_existing_parent_lock(
        &reparse_root,
        OsStr::new("cortex-use.lock"),
        Duration::ZERO,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(matches!(
        generic_reparse,
        ExistingParentLockError::UnsafeLockEntry { .. }
    ));
    assert_eq!(
        cortex_lock_error(acquire_cortex_namespace_lock(&reparse_authority)),
        "cortex_namespace_lock_unsafe"
    );
    assert_eq!(file_identity(&reparse_entry), reparse_identity);
    assert_eq!(
        file_identity(&reparse_target.join("outside")),
        outside_identity
    );
    assert_eq!(
        fs::read(reparse_target.join("outside")).unwrap(),
        b"outside"
    );

    let file_reparse_root = root.join("cortex-lock-file-reparse");
    fs::create_dir(&file_reparse_root).unwrap();
    let file_reparse_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&file_reparse_root).unwrap()).unwrap();
    let file_reparse_target = file_reparse_root.join("target");
    let file_reparse_entry = file_reparse_root.join("cortex-use.lock");
    fs::write(&file_reparse_target, b"file-reparse-target").unwrap();
    create_file_symlink(&file_reparse_entry, &file_reparse_target);
    let file_reparse_identity = file_identity(&file_reparse_entry);
    let file_reparse_target_identity = file_identity(&file_reparse_target);
    let generic_file_reparse = acquire_existing_parent_lock(
        &file_reparse_root,
        OsStr::new("cortex-use.lock"),
        Duration::ZERO,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(matches!(
        generic_file_reparse,
        ExistingParentLockError::UnsafeLockEntry {
            kind: "reparse point",
            ..
        }
    ));
    assert_eq!(
        cortex_lock_error(acquire_cortex_namespace_lock(&file_reparse_authority)),
        "cortex_namespace_lock_unsafe"
    );
    assert_eq!(file_identity(&file_reparse_entry), file_reparse_identity);
    assert_ne!(
        fs::symlink_metadata(&file_reparse_entry)
            .unwrap()
            .file_attributes()
            & FILE_ATTRIBUTE_REPARSE_POINT,
        0
    );
    assert_eq!(
        file_identity(&file_reparse_target),
        file_reparse_target_identity
    );
    assert_eq!(
        fs::read(&file_reparse_target).unwrap(),
        b"file-reparse-target"
    );

    let denied_root = root.join("cortex-lock-acl-denied");
    fs::create_dir(&denied_root).unwrap();
    let denied_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&denied_root).unwrap()).unwrap();
    let denied_entry = denied_root.join("cortex-use.lock");
    fs::write(&denied_entry, b"acl-denied-lock-entry").unwrap();
    let denied_identity = file_identity(&denied_entry);
    let denied_bytes = fs::read(&denied_entry).unwrap();
    let denied_acl = DeniedFileAcl::install(&denied_entry);
    let generic_denied = acquire_existing_parent_lock(
        &denied_root,
        OsStr::new("cortex-use.lock"),
        Duration::ZERO,
        Duration::ZERO,
    )
    .unwrap_err();
    assert!(matches!(generic_denied, ExistingParentLockError::Io { .. }));
    assert_eq!(
        cortex_lock_error(acquire_cortex_namespace_lock(&denied_authority)),
        "cortex_namespace_lock_io"
    );
    denied_acl.restore();
    assert_eq!(file_identity(&denied_entry), denied_identity);
    assert_eq!(fs::read(&denied_entry).unwrap(), denied_bytes);

    let identity_root = root.join("cortex-lock-identity-change");
    fs::create_dir(&identity_root).unwrap();
    let identity_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&identity_root).unwrap()).unwrap();
    let (identity_result, identity_consumed) =
        run_with_forced_post_lock_identity_mismatch(1, || {
            acquire_cortex_namespace_lock(&identity_authority)
        });
    assert!(identity_consumed);
    assert_eq!(
        cortex_lock_error(identity_result),
        "cortex_namespace_lock_identity_changed"
    );
    drop(acquire_cortex_namespace_lock(&identity_authority).unwrap());

    let io_root = root.join("cortex-lock-io");
    fs::create_dir(&io_root).unwrap();
    let io_authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&io_root).unwrap()).unwrap();
    let (io_result, io_consumed) = run_with_windows_lock_file_ex_substitution(
        1,
        WindowsLockFileExSubstitution::ReplaceHandle(INVALID_HANDLE_VALUE),
        || acquire_cortex_namespace_lock(&io_authority),
    );
    assert!(io_consumed);
    assert_eq!(cortex_lock_error(io_result), "cortex_namespace_lock_io");
    drop(acquire_cortex_namespace_lock(&io_authority).unwrap());
}

/// Caller-owned native-Windows receipt for R1A1b-F3 (Cortex census/lifecycle parsing).
///
/// DRAFT — not yet run against native Windows. Covers proof items 1-9 from the F3
/// scope's §5 (lock lifetime already covered by `exercise_cortex_namespace_lock_receipt`,
/// which every caller of this function already runs alongside it). Item 10's Windows
/// non-skippable exact-case NTFS `Alpha`/`alpha` + `Use.jsonl`/`use.jsonl` fixture is
/// deliberately NOT implemented here — see the trailing comment block for why.
fn exercise_cortex_census_receipt(root: &Path) {
    // --- fixture: two real talent directories, one junction (must not be traversed as
    // a talent), a two-projection ambiguous leaf, a malformed-name leaf, and an unrelated
    // top-level entry.
    let census_root = root.join("cortex-census");
    fs::create_dir(&census_root).unwrap();
    let authority = create_or_admit_cortex_namespace(JournalRoot::open(&census_root).unwrap())
        .expect("admit Cortex namespace for census fixture");
    let talents = census_root.join("talents");
    fs::create_dir_all(talents.join("alpha")).unwrap();
    fs::create_dir_all(talents.join("beta")).unwrap();
    fs::write(talents.join("alpha").join("one.jsonl"), b"completed").unwrap();
    fs::write(talents.join("alpha").join("two_active.jsonl"), b"ambiguous").unwrap();
    fs::write(talents.join("beta").join("plain.txt"), b"unrelated").unwrap();
    fs::write(talents.join("daily-index.jsonl"), b"top-level").unwrap();
    let junction_target = census_root.join("cortex-census-junction-target");
    fs::create_dir(&junction_target).unwrap();
    create_directory_junction(&talents.join("linked"), &junction_target);

    // Preservation snapshot, taken over the fully-built fixture before any census walk
    // touches it. Compared below once the census is dropped (§5 item 9 "mutation
    // accounting" / the F3 caller-owned-gate's "preservation" property).
    let before_census = snapshot_journal_tree(&census_root);

    // Expected cardinality: 3 root entries (alpha, beta, linked) + daily-index.jsonl = 4,
    // plus 2 (alpha) + 1 (beta) = 3 talent-child entries = 7 total.
    let census = census_cortex_namespace(authority, 7).expect("census within exact limit");
    assert_eq!(census.observed_entry_count(), 7);
    let alpha = census
        .talents()
        .iter()
        .find(|talent| talent.name() == OsStr::new("alpha"))
        .expect("alpha talent present");
    let ambiguous = alpha
        .entries()
        .iter()
        .find(|leaf| leaf.name() == OsStr::new("two_active.jsonl"))
        .expect("two_active.jsonl leaf present");
    // `two_active.jsonl` also ends in `.jsonl`, so `parse_cortex_lifecycle_name` reports
    // BOTH projections at once (active="two", completed="two_active") -- confirmed
    // against the real parser (census.rs `parse_cortex_lifecycle_name`) and its own
    // `parser_matrix` unit-test row for `alpha_active.jsonl`. This is exactly the
    // "two-projection ambiguous leaf" the fixture comment above names; the wrong
    // expectation here (`completed() == None`) would have been a silent own-goal on
    // the first native run, unrelated to anything this receipt is meant to validate.
    assert_eq!(ambiguous.projections().active(), Some("two"));
    assert_eq!(ambiguous.projections().completed(), Some("two_active"));
    let completed = alpha
        .entries()
        .iter()
        .find(|leaf| leaf.name() == OsStr::new("one.jsonl"))
        .expect("one.jsonl leaf present");
    assert_eq!(completed.projections().completed(), Some("one"));

    // Junction is a root entry, never traversed as a talent (JournalEntryKind must not
    // classify it as a real directory the census recurses into).
    assert!(
        census
            .talents()
            .iter()
            .all(|talent| talent.name() != OsStr::new("linked")),
        "a directory junction under talents/ must never be traversed as a talent directory"
    );

    // `CortexCensus` retains the acquired `cortex-use.lock` guard for its own lifetime
    // (F2's design: the lock stays held as long as the census value is alive). Drop it
    // explicitly before acquiring a fresh authority on the same `census_root` below, or
    // the next acquisition sees `cortex_namespace_lock_busy` instead of exercising the
    // cardinality limit this next block is actually testing.
    //
    // The drop also has to happen BEFORE the preservation snapshot immediately below:
    // reading `cortex-use.lock`'s own bytes via a fresh handle while this value still
    // holds its exclusive Windows byte-range lock is exactly the `ERROR_LOCK_VIOLATION`
    // (Os error 33) trap this validation pass found in census.rs's own internal test
    // suite (separately scope-checked repair pending) -- drop-before-snapshot avoids it
    // here by construction rather than by accident.
    drop(census);

    // Preservation: nothing under `census_root` changed except the first-ever creation
    // of the F2 lock entry. A successful, non-adversarial census reads only; it must
    // never touch fixture content.
    let after_census = snapshot_journal_tree(&census_root);
    assert_preserved_except_lock_creation(&before_census, &after_census);

    // One-less-than-cardinality is the closed limit-exceeded token, not a partial census.
    let authority_for_limit =
        create_or_admit_cortex_namespace(JournalRoot::open(&census_root).unwrap()).unwrap();
    let limited = census_cortex_namespace(authority_for_limit, 6);
    assert_eq!(
        census_err(limited, "one below exact cardinality must refuse").to_string(),
        "cortex_census_limit_exceeded"
    );

    // Pre-open replacement: barrier fires after `alpha` is classified as a real directory
    // but before it is opened; replace it with a fresh directory of the same name in
    // between. The census must reject with the talent_open identity_changed token, never
    // silently describe the replacement.
    {
        let authority =
            create_or_admit_cortex_namespace(JournalRoot::open(&census_root).unwrap()).unwrap();
        let census_root = census_root.clone();
        let (result, fired) = run_with_cortex_census_barrier(
            CortexCensusPrimitive::PreTalentOpen,
            1,
            move || {
                let alpha = census_root.join("talents").join("alpha");
                fs::rename(&alpha, census_root.join("talents").join("alpha-displaced")).unwrap();
                fs::create_dir(&alpha).unwrap();
            },
            move || census_cortex_namespace(authority, 32),
        );
        assert!(fired);
        assert_eq!(
            census_err(result, "pre-open replacement must be refused").to_string(),
            "cortex_census_talent_open_identity_changed"
        );
    }

    // Post-open (final binding) replacement: barrier fires immediately before the final
    // parent-relative authority/talent binding pass, well after `alpha` was already
    // listed and opened successfully. Replacing it here must still be caught by the
    // final check, not accepted as a stale-but-successful snapshot.
    {
        let census_root2 = census_root.join("cortex-census-final");
        fs::create_dir(&census_root2).unwrap();
        let authority =
            create_or_admit_cortex_namespace(JournalRoot::open(&census_root2).unwrap()).unwrap();
        fs::create_dir_all(census_root2.join("talents").join("gamma")).unwrap();
        let root_for_barrier = census_root2.clone();
        let (result, fired) = run_with_cortex_census_barrier(
            CortexCensusPrimitive::PreFinalAuthorityCheck,
            1,
            move || {
                let gamma = root_for_barrier.join("talents").join("gamma");
                fs::rename(
                    &gamma,
                    root_for_barrier.join("talents").join("gamma-displaced"),
                )
                .unwrap();
                fs::create_dir(&gamma).unwrap();
            },
            move || census_cortex_namespace(authority, 32),
        );
        assert!(fired);
        assert_eq!(
            census_err(result, "final-pass replacement must be refused").to_string(),
            "cortex_census_talent_binding_identity_changed"
        );
    }

    // Child (leaf) replacement mid-listing: barrier fires after `delta`'s children are
    // enumerated but before they're observed; remove the one leaf in between.
    //
    // Vanished-leaf opens in `list_windows_native_entries` were a real classification
    // gap (bare Io). That is repaired; the token is now the spec-correct
    // `cortex_census_talent_list_identity_changed`.
    {
        let census_root3 = census_root.join("cortex-census-child");
        fs::create_dir(&census_root3).unwrap();
        let authority =
            create_or_admit_cortex_namespace(JournalRoot::open(&census_root3).unwrap()).unwrap();
        let delta = census_root3.join("talents").join("delta");
        fs::create_dir_all(&delta).unwrap();
        fs::write(delta.join("only.jsonl"), b"will be removed").unwrap();
        let delta_for_barrier = delta.clone();
        let (result, fired) = run_with_cortex_census_barrier(
            CortexCensusPrimitive::PostLeafEnumeration,
            1,
            move || {
                fs::remove_file(delta_for_barrier.join("only.jsonl")).unwrap();
            },
            move || census_cortex_namespace(authority, 32),
        );
        assert!(fired);
        assert_eq!(
            census_err(result, "child removal mid-listing must be refused, not silently reflected as a shorter-than-listed census").to_string(),
            "cortex_census_talent_list_identity_changed"
        );
    }

    // Continuous F2 exclusion around a LIVE `CortexCensus`, not just a raw
    // `CortexNamespaceLock`. `exercise_cortex_namespace_lock_receipt` (called by both
    // `ntfs_cortex_use_receipt`/`refs_cortex_use_receipt` alongside this function)
    // already proves F2's raw lock semantics -- busy/reacquire, cross-process contention
    // -- but never constructs a `CortexCensus`, so it cannot show that `CortexCensus`
    // itself retains that lock for its own lifetime while enumerating. This barrier-based
    // check mirrors census.rs's own `lock_lifetime_and_exact_authority` unit test shape:
    // pause mid-walk, prove a same-process contender sees `busy` while the census is
    // alive and mid-enumeration (not merely at acquisition), prove it is still `busy`
    // immediately after the census *returns* (not only mid-walk), then prove the lock is
    // free the instant the census value is dropped.
    {
        let census_root4 = census_root.join("cortex-census-f2-exclusion");
        fs::create_dir(&census_root4).unwrap();
        let authority =
            create_or_admit_cortex_namespace(JournalRoot::open(&census_root4).unwrap()).unwrap();
        fs::create_dir_all(census_root4.join("talents").join("epsilon")).unwrap();
        let contender_root = census_root4.clone();
        let (result, fired) = run_with_cortex_census_barrier(
            CortexCensusPrimitive::PostRootList,
            1,
            move || {
                let contender =
                    create_or_admit_cortex_namespace(JournalRoot::open(&contender_root).unwrap())
                        .unwrap();
                assert_eq!(
                    cortex_lock_error(acquire_cortex_namespace_lock_with_test_timing(
                        &contender,
                        Duration::ZERO,
                        Duration::ZERO,
                    )),
                    "cortex_namespace_lock_busy",
                    "a live CortexCensus mid-walk must keep the F2 lock held"
                );
            },
            move || census_cortex_namespace(authority, 32),
        );
        assert!(fired);
        let census =
            result.expect("census must still succeed once the barrier's contention probe returns");
        let lock_path = census_root4.join("cortex-use.lock");
        assert!(lock_path.exists());
        let post_return_contender =
            create_or_admit_cortex_namespace(JournalRoot::open(&census_root4).unwrap()).unwrap();
        assert_eq!(
            cortex_lock_error(acquire_cortex_namespace_lock_with_test_timing(
                &post_return_contender,
                Duration::ZERO,
                Duration::ZERO,
            )),
            "cortex_namespace_lock_busy",
            "the lock must still be held immediately after the census returns, not only mid-walk"
        );
        drop(census);
        let reacquired = acquire_cortex_namespace_lock_with_test_timing(
            &post_return_contender,
            Duration::ZERO,
            Duration::ZERO,
        )
        .expect("the lock must be free once the live CortexCensus value is dropped");
        drop(reacquired);
    }

    // Parser matrix (pure — no filesystem fixture required; runs identically on every
    // platform), included here so the Windows receipt also pins it against native
    // WTF-16 names, not only the portable unit-test corpus. Full row set mirrors
    // census.rs's own `parser_matrix` unit test exactly (empty, `.jsonl`, `_active.jsonl`,
    // `alpha.jsonl`, `alpha_active.jsonl`, `alpha_active_active.jsonl`, mixed-case
    // suffixes, extra suffixes, path-separator characters embedded in the stem,
    // control characters, and non-ASCII) plus one extra non-ASCII-without-suffix case
    // (`nö-suffix`) for additional coverage, plus the ill-formed WTF-16 unpaired
    // surrogate appended after the loop -- the one row census.rs's own author noted is
    // most likely to actually differ between native hardware and a cross-build, and
    // which was entirely absent from this native receipt before this addition.
    for (name, active, completed) in [
        ("", None, None),
        (".jsonl", None, None),
        ("_active.jsonl", None, Some("_active")),
        ("alpha.jsonl", None, Some("alpha")),
        ("alpha_active.jsonl", Some("alpha"), Some("alpha_active")),
        (
            "alpha_active_active.jsonl",
            Some("alpha_active"),
            Some("alpha_active_active"),
        ),
        ("alpha.JSONL", None, None),
        ("alpha_ACTIVE.jsonl", None, Some("alpha_ACTIVE")),
        ("alpha_Active.jsonl", None, Some("alpha_Active")),
        ("Alpha_active.jsonl", Some("Alpha"), Some("Alpha_active")),
        ("alpha.jsonl.bak", None, None),
        ("alpha_active.jsonl.extra", None, None),
        ("alpha.jsonl.jsonl", None, Some("alpha.jsonl")),
        ("a/b.jsonl", None, Some("a/b")),
        ("a\\b.jsonl", None, Some("a\\b")),
        ("alpha\n.jsonl", None, Some("alpha\n")),
        ("α.jsonl", None, Some("α")),
        ("alpha_актив.jsonl", None, Some("alpha_актив")),
        ("nö-suffix", None, None),
        // `_active.jsonl` also ends in `.jsonl`, so both projections are always populated
        // together for this suffix shape (same fix as the "two_active.jsonl" fixture leaf
        // above): completed is the full stem before `.jsonl`, not None.
        ("plain_active.jsonl", Some("plain"), Some("plain_active")),
    ] {
        let projections = parse_cortex_lifecycle_name(OsStr::new(name));
        assert_eq!(
            projections.active(),
            active,
            "active projection for {name:?}"
        );
        assert_eq!(
            projections.completed(),
            completed,
            "completed projection for {name:?}"
        );
    }

    // Native ill-formed WTF-16 (an unpaired UTF-16 surrogate): `OsStr::to_str()` fails
    // for this name, so both projections must be the empty default -- pinned here
    // against the real native `OsString::from_wide` on Windows, not only the portable
    // unit-test corpus, matching census.rs's own `#[cfg(windows)]` parser_matrix case.
    let ill_formed = parse_cortex_lifecycle_name(&std::ffi::OsString::from_wide(&[0xD800, 0x0061]));
    assert_eq!(
        ill_formed.active(),
        None,
        "ill-formed WTF-16 must have no active projection"
    );
    assert_eq!(
        ill_formed.completed(),
        None,
        "ill-formed WTF-16 must have no completed projection"
    );

    // Bounded diagnostics: every error's Display/Debug is the bare closed token, nothing
    // path- or name-shaped leaks through.
    let leaked_root = census_root.join("this-path-must-never-appear-in-a-diagnostic");
    fs::create_dir(&leaked_root).unwrap();
    let authority =
        create_or_admit_cortex_namespace(JournalRoot::open(&leaked_root).unwrap()).unwrap();
    fs::remove_dir_all(&leaked_root).unwrap();
    let err = census_err(
        census_cortex_namespace(authority, 32),
        "removed root must fail closed",
    );
    let display = err.to_string();
    let debug = format!("{err:?}");
    assert_eq!(display, debug);
    assert!(!display.contains("this-path-must-never-appear"));
    assert!(display.starts_with("cortex_census_") || display.starts_with("cortex_namespace_lock_"));

    println!("JOURNAL_WIN_CI_CORTEX_CENSUS=executed/pass");
}

/// Caller-owned native-Windows receipt for R1A1b-F3 proof item 10: a non-skippable NTFS
/// fixture proving the census's exact-case `NtCreateFile` variant (`nt_create_relative_exact`
/// in `windows_ntcreate.rs`, `object_attributes = 0`, no `OBJ_CASE_INSENSITIVE`) genuinely
/// distinguishes `Alpha`/`alpha` and `Use.jsonl`/`use.jsonl` rather than silently folding
/// through the default `OBJ_CASE_INSENSITIVE` path every other caller (`nt_create_relative`)
/// still uses.
///
/// Empirically verified on `sol-winbuild` (Windows 11 Pro, build 26200) before this fixture
/// was written, via disposable `fsutil.exe` probes outside the crate: an ordinary directory
/// folds `Alpha`/`alpha` (`mkdir alpha` after `mkdir Alpha` fails `ERROR_ALREADY_EXISTS`,
/// exit 1); `fsutil.exe file setCaseSensitiveInfo <dir> enable` on a fresh directory flips
/// that -- both `mkdir Alpha` and `mkdir alpha` then exit 0 and both list distinctly, two
/// files `Use.jsonl`/`use.jsonl` round-trip distinct byte content, and newly-created child
/// directories inherit the parent's case-sensitive attribute. This is not a version-gated
/// WSL-optional-feature requirement on this OS build -- the toggle worked immediately as
/// `solbuild`, no elevation or feature-enable step needed.
///
/// The in-fixture control below repeats the cheap half of that proof (ordinary NTFS folds)
/// on every run, so a future host or OS update that silently stops honoring the toggle
/// fails this test loudly at the control instead of letting the treatment assertions pass
/// vacuously (both names folding to one census talent would otherwise just look like an
/// `Alpha`-only or `alpha`-only namespace, which nothing else in this file would catch).
fn exercise_cortex_census_exact_case_receipt(root: &Path) {
    let census_root = root.join("cortex-census-exact-case");
    fs::create_dir(&census_root).unwrap();

    // Negative control: an ordinary NTFS directory (case sensitivity never toggled) must
    // fold Alpha/alpha. No fsutil involved -- this is the baseline every Windows directory
    // has until explicitly opted out.
    let control = census_root.join("case-fold-control");
    fs::create_dir(&control).unwrap();
    fs::create_dir(control.join("Alpha")).unwrap();
    assert!(
        fs::create_dir(control.join("alpha")).is_err(),
        "ordinary (non-case-sensitive) NTFS must fold Alpha/alpha; the exact-case \
         treatment below is meaningless as a proof if this control no longer fails"
    );

    let authority = create_or_admit_cortex_namespace(JournalRoot::open(&census_root).unwrap())
        .expect("admit Cortex namespace for exact-case fixture");
    let talents = census_root.join("talents");
    enable_case_sensitive(&talents);
    fs::create_dir(talents.join("Alpha")).unwrap();
    fs::create_dir(talents.join("alpha")).unwrap();
    // Belt and suspenders: the parent toggle is inherited by newly-created children
    // (verified on sol-winbuild), but set it directly on each talent too so this fixture
    // does not silently depend on inheritance semantics holding on a future host.
    enable_case_sensitive(&talents.join("Alpha"));
    enable_case_sensitive(&talents.join("alpha"));
    fs::write(talents.join("Alpha").join("Use.jsonl"), b"upper").unwrap();
    fs::write(
        talents.join("Alpha").join("use.jsonl"),
        b"lower-case-marker",
    )
    .unwrap();
    fs::write(
        talents.join("alpha").join("marker.jsonl"),
        b"distinct-talent",
    )
    .unwrap();

    let census = census_cortex_namespace(authority, 32).expect("exact-case census");
    assert_eq!(
        census.talents().len(),
        2,
        "Alpha and alpha must both survive census as distinct talents, not fold to one"
    );
    let upper = census
        .talents()
        .iter()
        .find(|talent| talent.name() == OsStr::new("Alpha"))
        .expect("exact-case 'Alpha' talent present");
    let lower = census
        .talents()
        .iter()
        .find(|talent| talent.name() == OsStr::new("alpha"))
        .expect("exact-case 'alpha' talent present");

    assert_eq!(
        upper.entries().len(),
        2,
        "Alpha must retain both Use.jsonl and use.jsonl as distinct leaves"
    );
    let use_upper = upper
        .entries()
        .iter()
        .find(|leaf| leaf.name() == OsStr::new("Use.jsonl"))
        .expect("Use.jsonl leaf present");
    let use_lower = upper
        .entries()
        .iter()
        .find(|leaf| leaf.name() == OsStr::new("use.jsonl"))
        .expect("use.jsonl leaf present");
    // Distinct byte lengths double as a content-identity proxy: if the exact-case open
    // actually resolved both listed names to the same underlying file (a subtler fold
    // than the directory-count check above would catch), the sizes would collide too.
    assert_eq!(
        use_upper.size(),
        5,
        "Use.jsonl must report its own distinct byte length"
    );
    assert_eq!(
        use_lower.size(),
        17,
        "use.jsonl must report its own distinct byte length, not Use.jsonl's"
    );

    assert_eq!(
        lower.entries().len(),
        1,
        "alpha must hold only its own marker.jsonl, not any of Alpha's leaves"
    );
    assert_eq!(lower.entries()[0].name(), OsStr::new("marker.jsonl"));

    println!("JOURNAL_WIN_CI_CORTEX_CENSUS_EXACT_CASE=executed/pass");
}

fn enable_case_sensitive(path: &Path) {
    let output = Command::new("fsutil.exe")
        .args(["file", "setCaseSensitiveInfo"])
        .arg(path)
        .arg("enable")
        .output()
        .expect("launch fsutil.exe for Cortex exact-case fixture");
    assert!(
        output.status.success(),
        "enable case sensitivity on {}: status={} stdout={} stderr={}",
        path.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // Round-trip the query rather than trusting the enable command's exit code alone --
    // exactly the "silently vacuous" failure mode this fixture exists to rule out.
    let query = Command::new("fsutil.exe")
        .args(["file", "queryCaseSensitiveInfo"])
        .arg(path)
        .output()
        .expect("launch fsutil.exe to verify Cortex exact-case fixture");
    let reported = String::from_utf8_lossy(&query.stdout);
    assert!(
        query.status.success() && reported.contains("is enabled"),
        "case sensitivity did not take effect on {}: status={} stdout={} stderr={}",
        path.display(),
        query.status,
        reported,
        String::from_utf8_lossy(&query.stderr),
    );
}

fn print_cortex_namespace_receipts(token: &str, filesystem: &str) {
    for category in [
        "CREATE_ADMIT",
        "WRONG_KIND_REPARSE",
        "RETAINED_ROOT",
        "RETAINED_HEALTH",
        "FAILURE_MAPPING",
        "PRESERVATION",
        "LOCK",
    ] {
        println!("JOURNAL_WIN_CI_CORTEX_NAMESPACE_{token}_{category}=executed/pass");
        println!("JOURNAL_WIN_CI_CORTEX_NAMESPACE_{token}_{category}_FILESYSTEM={filesystem}");
    }
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
    if run_marked_cortex_lock_child() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    assert_eq!(filesystem_name(root.path()).unwrap(), "NTFS");
    exercise_cortex_use_receipt(root.path());
    exercise_cortex_namespace_receipt(root.path());
    exercise_cortex_namespace_lock_receipt(root.path(), "ntfs_cortex_use_receipt");
    exercise_cortex_census_receipt(root.path());
    exercise_cortex_census_exact_case_receipt(root.path());
    print_cortex_namespace_receipts("NTFS", "NTFS");
    println!("JOURNAL_WIN_CI_CORTEX_USE_NTFS=executed/pass");
    println!("JOURNAL_WIN_CI_CORTEX_USE_NTFS_FILESYSTEM=NTFS");
}

#[test]
#[ignore = "requires a native ReFS filesystem"]
fn refs_cortex_use_receipt() {
    if run_marked_cortex_lock_child() {
        return;
    }
    let root = std::env::var_os("SOLSTONE_JOURNAL_WIN_REFS_ROOT")
        .map(PathBuf::from)
        .expect("ReFS Cortex-use receipt requires SOLSTONE_JOURNAL_WIN_REFS_ROOT");
    assert_eq!(filesystem_name(&root).unwrap(), "ReFS");
    let temporary = tempfile::Builder::new()
        .prefix("solstone-refs-cortex-use-")
        .tempdir_in(&root)
        .unwrap();
    exercise_cortex_use_receipt(temporary.path());
    exercise_cortex_namespace_receipt(temporary.path());
    exercise_cortex_namespace_lock_receipt(temporary.path(), "refs_cortex_use_receipt");
    exercise_cortex_census_receipt(temporary.path());
    exercise_cortex_census_exact_case_receipt(temporary.path());
    print_cortex_namespace_receipts("REFS", "ReFS");
    println!("JOURNAL_WIN_CI_CORTEX_USE_REFS=executed/pass");
    println!("JOURNAL_WIN_CI_CORTEX_USE_REFS_FILESYSTEM=ReFS");
}

fn managed_log_reference_receipt(root: &Path) {
    let single_process = root.join("single-process");
    fs::create_dir(&single_process).unwrap();
    exercise_windows_managed_log_reference_substrate(&single_process);

    let logical_coordinates = root.join("logical-coordinates");
    fs::create_dir(&logical_coordinates).unwrap();
    let mut index = 0;
    for shape in LOGICAL_FIELD_SHAPES {
        for (reference, name) in [(*shape, "stream"), ("writer", *shape)] {
            let pair_root = logical_coordinates.join(format!("logical-{index}"));
            fs::create_dir(&pair_root).unwrap();
            exercise_windows_managed_log_logical_coordinates(&pair_root, reference, name);
            index += 1;
        }
    }

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
