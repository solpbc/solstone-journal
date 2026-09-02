// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, WindowsCreateOnlyPrimitive as Primitive,
    WindowsCreateOnlyTrace, run_with_windows_create_only_barrier,
    run_with_windows_create_only_faults, run_with_windows_create_only_faults_and_barrier,
    write_bytes_exclusive, write_reader_exclusive,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

const PAYLOAD: &[u8] = b"create-only protocol payload";
const COMPETITOR: &[u8] = b"late competitor";

fn temporary(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(label).tempdir().unwrap()
}

fn stage_paths(parent: &Path) -> Vec<PathBuf> {
    fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            (name.starts_with(".tmp_") || name.starts_with("_.tmp_")).then_some(path)
        })
        .collect()
}

fn assert_no_stage(parent: &Path) {
    let stages = stage_paths(parent);
    assert!(stages.is_empty(), "unexpected stages: {stages:?}");
}

fn assert_complete(trace: &WindowsCreateOnlyTrace) {
    assert!(trace.faults_consumed, "unconsumed fault: {trace:?}");
    assert!(trace.barriers_fired, "unfired barrier: {trace:?}");
}

fn count(trace: &WindowsCreateOnlyTrace, primitive: Primitive) -> usize {
    trace
        .attempted
        .iter()
        .filter(|candidate| **candidate == primitive)
        .count()
}

fn assert_already_exists(error: AtomicWriteError) {
    match error {
        AtomicWriteError::Io { source, .. } => {
            assert_eq!(source.kind(), io::ErrorKind::AlreadyExists, "{source}");
        }
        AtomicWriteError::PublicationUncertain { .. } => {
            panic!("late collision is a pre-publication refusal")
        }
    }
}

struct NamedFailure;

impl Read for NamedFailure {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "named protocol reader failure",
        ))
    }
}

#[test]
fn late_collision_before_first_move_refuses_without_a_move_attempt() {
    let temporary = temporary("create-only-late-first-");
    let destination = temporary.path().join("destination.bin");
    let competitor = destination.clone();
    let (result, trace) = run_with_windows_create_only_barrier(
        Primitive::BeforeMove,
        1,
        move || fs::write(competitor, COMPETITOR).unwrap(),
        || write_bytes_exclusive(&destination, PAYLOAD, AtomicWriteOptions::default()),
    );

    assert_already_exists(result.expect_err("late competitor must win"));
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 0, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert!(trace.backoffs.is_empty(), "{trace:?}");
    assert_eq!(fs::read(&destination).unwrap(), COMPETITOR);
    assert_no_stage(temporary.path());
}

#[test]
fn collision_during_failed_move_reclassification_stops_before_backoff() {
    let temporary = temporary("create-only-late-reclass-");
    let destination = temporary.path().join("destination.bin");
    let competitor = destination.clone();
    let (result, trace) = run_with_windows_create_only_faults_and_barrier(
        [(Primitive::Move, 1, ERROR_SHARING_VIOLATION as i32)],
        Primitive::ReclassifyCapability,
        1,
        move || fs::write(competitor, COMPETITOR).unwrap(),
        || write_bytes_exclusive(&destination, PAYLOAD, AtomicWriteOptions::default()),
    );

    assert_already_exists(result.expect_err("reclassification must see competitor"));
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert!(trace.backoffs.is_empty(), "{trace:?}");
    assert_eq!(fs::read(&destination).unwrap(), COMPETITOR);
    assert_no_stage(temporary.path());
}

#[test]
fn every_retryable_move_class_waits_once_then_publishes() {
    for raw_error in [
        ERROR_SHARING_VIOLATION,
        ERROR_LOCK_VIOLATION,
        ERROR_ACCESS_DENIED,
    ] {
        let temporary = temporary("create-only-retry-class-");
        let destination = temporary.path().join("destination.bin");
        let (result, trace) =
            run_with_windows_create_only_faults([(Primitive::Move, 1, raw_error as i32)], || {
                write_bytes_exclusive(&destination, PAYLOAD, AtomicWriteOptions::default())
            });

        result.unwrap();
        assert_complete(&trace);
        assert_eq!(count(&trace, Primitive::Move), 2, "{trace:?}");
        assert_eq!(trace.real_moves, 1, "{trace:?}");
        assert_eq!(trace.backoffs, vec![Duration::from_millis(250)]);
        assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
        assert_no_stage(temporary.path());
    }
}

#[test]
fn retry_exhaustion_is_exactly_four_attempts_and_three_backoffs() {
    let temporary = temporary("create-only-retry-exhaust-");
    let destination = temporary.path().join("destination.bin");
    let (result, trace) = run_with_windows_create_only_faults(
        (1..=4).map(|ordinal| (Primitive::Move, ordinal, ERROR_SHARING_VIOLATION as i32)),
        || write_bytes_exclusive(&destination, PAYLOAD, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("four retryable failures must exhaust");
    assert!(matches!(error, AtomicWriteError::Io { .. }), "{error}");
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 4, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert_eq!(
        trace.backoffs,
        vec![Duration::from_millis(250); 3],
        "{trace:?}"
    );
    assert!(!destination.exists());
    assert_no_stage(temporary.path());
}

#[test]
fn reclassification_failures_are_never_retried() {
    for failed in [
        Primitive::ReclassifyCapability,
        Primitive::ReclassifyDestination,
        Primitive::ReclassifyStage,
    ] {
        let temporary = temporary("create-only-reclass-failure-");
        let destination = temporary.path().join("destination.bin");
        let (result, trace) = run_with_windows_create_only_faults(
            [
                (Primitive::Move, 1, ERROR_SHARING_VIOLATION as i32),
                (failed, 1, ERROR_ACCESS_DENIED as i32),
            ],
            || write_bytes_exclusive(&destination, PAYLOAD, AtomicWriteOptions::default()),
        );

        assert!(result.is_err(), "{failed:?} must stop publication");
        assert_complete(&trace);
        assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
        assert_eq!(trace.real_moves, 0, "{trace:?}");
        assert!(trace.backoffs.is_empty(), "{trace:?}");
        assert!(!destination.exists());
        assert_no_stage(temporary.path());
    }
}

#[test]
fn the_live_stage_denies_write_sharing_until_publication() {
    let temporary = temporary("create-only-stage-sharing-");
    let destination = temporary.path().join("destination.bin");
    let parent = temporary.path().to_path_buf();
    let observed_error = Rc::new(Cell::new(None));
    let callback_error = Rc::clone(&observed_error);
    let (result, trace) = run_with_windows_create_only_barrier(
        Primitive::StageReady,
        1,
        move || {
            let stages = stage_paths(&parent);
            assert_eq!(stages.len(), 1, "expected one live stage: {stages:?}");
            let error = OpenOptions::new()
                .write(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .open(&stages[0])
                .expect_err("live stage must deny a second writer");
            callback_error.set(error.raw_os_error());
        },
        || write_bytes_exclusive(&destination, PAYLOAD, AtomicWriteOptions::default()),
    );

    result.unwrap();
    assert_complete(&trace);
    assert_eq!(observed_error.get(), Some(ERROR_SHARING_VIOLATION as i32));
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert_no_stage(temporary.path());
}

#[test]
fn cleanup_failure_names_the_retained_stage_residue() {
    let temporary = temporary("create-only-cleanup-failure-");
    let destination = temporary.path().join("destination.bin");
    let mut reader = NamedFailure;
    let (result, trace) = run_with_windows_create_only_faults(
        [(Primitive::Cleanup, 1, ERROR_ACCESS_DENIED as i32)],
        || write_reader_exclusive(&destination, &mut reader, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("reader and cleanup must fail");
    match error {
        AtomicWriteError::Io { source, .. } => {
            assert_eq!(source.kind(), io::ErrorKind::UnexpectedEof);
            let display = source.to_string();
            assert!(
                display.contains("named protocol reader failure"),
                "{display}"
            );
            assert!(display.contains("could not remove stage"), "{display}");
        }
        AtomicWriteError::PublicationUncertain { .. } => {
            panic!("reader failure happens before publication")
        }
    }
    assert_complete(&trace);
    assert!(!destination.exists());
    let stages = stage_paths(temporary.path());
    assert_eq!(stages.len(), 1, "cleanup residue must be exact: {stages:?}");
    fs::remove_file(&stages[0]).unwrap();
}

#[test]
fn both_post_move_observation_failures_are_publication_uncertain() {
    for (primitive, expected_operation) in [
        (
            Primitive::PostMoveCapability,
            "revalidate publication path after move",
        ),
        (
            Primitive::PostMoveDestination,
            "observe published destination after move",
        ),
    ] {
        let temporary = temporary("create-only-uncertain-");
        let destination = temporary.path().join("destination.bin");
        let (result, trace) = run_with_windows_create_only_faults(
            [(primitive, 1, ERROR_ACCESS_DENIED as i32)],
            || write_bytes_exclusive(&destination, PAYLOAD, AtomicWriteOptions::default()),
        );

        match result.expect_err("post-move fault must report uncertainty") {
            AtomicWriteError::PublicationUncertain {
                operation, source, ..
            } => {
                assert_eq!(operation, expected_operation);
                assert_eq!(source.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            }
            AtomicWriteError::Io { .. } => panic!("publication already landed"),
        }
        assert_complete(&trace);
        assert_eq!(trace.real_moves, 1, "{trace:?}");
        assert!(trace.backoffs.is_empty(), "{trace:?}");
        assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
        assert_no_stage(temporary.path());
    }
}

#[test]
fn create_only_protocol_receipt_marker() {
    println!(
        "JOURNAL_WIN_CI_CREATE_ONLY_PROTOCOL=late-collision/retry/sharing/cleanup/uncertainty/pass"
    );
}
