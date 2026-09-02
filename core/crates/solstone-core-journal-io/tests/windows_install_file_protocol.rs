// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, WindowsInstallPrimitive as Primitive,
    WindowsInstallTrace, install_file, run_with_windows_install_faults,
    run_with_windows_install_faults_and_barrier,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
};

const PAYLOAD: &[u8] = b"install-protocol-payload";
const COMPETITOR: &[u8] = b"source-name-competitor";

fn temporary(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(label).tempdir().unwrap()
}

fn assert_complete(trace: &WindowsInstallTrace) {
    assert!(trace.faults_consumed, "unconsumed fault: {trace:?}");
    assert!(trace.barriers_fired, "unfired barrier: {trace:?}");
}

fn count(trace: &WindowsInstallTrace, primitive: Primitive) -> usize {
    trace
        .attempted
        .iter()
        .filter(|candidate| **candidate == primitive)
        .count()
}

fn write_pair(parent: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let source = parent.join("source.bin");
    let destination = parent.join("destination.bin");
    fs::write(&source, PAYLOAD).unwrap();
    (source, destination)
}

#[test]
fn every_retryable_move_class_waits_once_then_publishes() {
    for raw_error in [
        ERROR_SHARING_VIOLATION,
        ERROR_LOCK_VIOLATION,
        ERROR_ACCESS_DENIED,
    ] {
        let temporary = temporary("install-retry-class-");
        let (source, destination) = write_pair(temporary.path());
        let (result, trace) =
            run_with_windows_install_faults([(Primitive::Move, 1, raw_error as i32)], || {
                install_file(&source, &destination, AtomicWriteOptions::default())
            });

        result.unwrap();
        assert_complete(&trace);
        assert_eq!(count(&trace, Primitive::Move), 2, "{trace:?}");
        assert_eq!(trace.real_moves, 1, "{trace:?}");
        assert_eq!(trace.backoffs, vec![Duration::from_millis(250)]);
        assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
        assert!(!source.exists());
    }
}

#[test]
fn retry_exhaustion_is_exactly_four_attempts_and_three_backoffs() {
    let temporary = temporary("install-retry-exhaust-");
    let (source, destination) = write_pair(temporary.path());
    let (result, trace) = run_with_windows_install_faults(
        (1..=4).map(|ordinal| (Primitive::Move, ordinal, ERROR_SHARING_VIOLATION as i32)),
        || install_file(&source, &destination, AtomicWriteOptions::default()),
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
    assert!(!source.exists());
    assert!(!destination.exists());
}

#[test]
fn non_retryable_other_never_retries() {
    let temporary = temporary("install-other-");
    let (source, destination) = write_pair(temporary.path());
    let (result, trace) = run_with_windows_install_faults(
        [(Primitive::Move, 1, ERROR_INVALID_PARAMETER as i32)],
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("non-retryable move must stop");
    assert!(matches!(error, AtomicWriteError::Io { .. }), "{error}");
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert!(trace.backoffs.is_empty(), "{trace:?}");
    assert!(!source.exists());
    assert!(!destination.exists());
}

#[test]
fn landed_when_source_is_absent_and_destination_is_source() {
    let temporary = temporary("install-landed-absent-");
    let (source, destination) = write_pair(temporary.path());
    let moved_from = source.clone();
    let moved_to = destination.clone();
    let (result, trace) = run_with_windows_install_faults_and_barrier(
        [(Primitive::Move, 1, ERROR_SHARING_VIOLATION as i32)],
        Primitive::ReclassifySource,
        1,
        move || fs::rename(&moved_from, &moved_to).unwrap(),
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    result.unwrap();
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert!(trace.backoffs.is_empty(), "{trace:?}");
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert!(!source.exists());
}

#[test]
fn landed_when_source_name_has_a_competitor() {
    let temporary = temporary("install-landed-competitor-");
    let (source, destination) = write_pair(temporary.path());
    let moved_from = source.clone();
    let moved_to = destination.clone();
    let competitor_at = source.clone();
    let (result, trace) = run_with_windows_install_faults_and_barrier(
        [(Primitive::Move, 1, ERROR_SHARING_VIOLATION as i32)],
        Primitive::ReclassifySource,
        1,
        move || {
            fs::rename(&moved_from, &moved_to).unwrap();
            fs::write(&competitor_at, COMPETITOR).unwrap();
        },
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    result.unwrap();
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert!(trace.backoffs.is_empty(), "{trace:?}");
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert_eq!(fs::read(&source).unwrap(), COMPETITOR);
}

#[test]
fn both_names_retained_is_reconcile_uncertainty_without_cleanup() {
    let temporary = temporary("install-both-names-");
    let (source, destination) = write_pair(temporary.path());
    let linked_from = source.clone();
    let linked_to = destination.clone();
    if fs::hard_link(&source, &destination).is_err() {
        // Probe viability, then remove the dest name so the protocol starts from dest-absent.
        let _ = fs::remove_file(&destination);
        eprintln!("skipping both-names-retained test: hard link is not viable here");
        return;
    }
    fs::remove_file(&destination).unwrap();

    let (result, trace) = run_with_windows_install_faults_and_barrier(
        [(Primitive::Move, 1, ERROR_SHARING_VIOLATION as i32)],
        Primitive::ReclassifySource,
        1,
        move || {
            fs::hard_link(&linked_from, &linked_to).expect("hard link remains viable in barrier");
        },
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    match result.expect_err("both names retained must be uncertain") {
        AtomicWriteError::PublicationUncertain {
            operation,
            source: inner,
            ..
        } => {
            assert_eq!(operation, "reconcile install after failed move");
            assert_eq!(inner.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        }
        AtomicWriteError::Io { .. } => panic!("both-names-retained must not clean up"),
    }
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert!(trace.backoffs.is_empty(), "{trace:?}");
    assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
}

#[test]
fn flush_move_and_cleanup_faults_preserve_primary_kind() {
    let flush_dir = temporary("install-flush-fault-");
    let (source, destination) = write_pair(flush_dir.path());
    let (result, trace) = run_with_windows_install_faults(
        [(Primitive::Flush, 1, ERROR_ACCESS_DENIED as i32)],
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );
    let error = result.expect_err("flush fault must fail");
    match error {
        AtomicWriteError::Io { source: inner, .. } => {
            assert_eq!(inner.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            assert!(!inner.to_string().contains("could not remove stage"));
        }
        AtomicWriteError::PublicationUncertain { .. } => panic!("flush is pre-publication"),
    }
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 0, "{trace:?}");
    assert!(!source.exists());
    assert!(!destination.exists());

    let move_dir = temporary("install-move-other-");
    let (source, destination) = write_pair(move_dir.path());
    let (result, trace) = run_with_windows_install_faults(
        [(Primitive::Move, 1, ERROR_INVALID_PARAMETER as i32)],
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );
    let error = result.expect_err("other move fault must stop with cleanup");
    match error {
        AtomicWriteError::Io { source: inner, .. } => {
            assert_eq!(inner.raw_os_error(), Some(ERROR_INVALID_PARAMETER as i32));
        }
        AtomicWriteError::PublicationUncertain { .. } => panic!("other is StopCleanup"),
    }
    assert_complete(&trace);
    assert!(!source.exists());
    assert!(!destination.exists());

    let cleanup_dir = temporary("install-cleanup-fault-");
    let (source, destination) = write_pair(cleanup_dir.path());
    let (result, trace) = run_with_windows_install_faults(
        [
            (Primitive::Flush, 1, ERROR_ACCESS_DENIED as i32),
            (Primitive::Cleanup, 1, ERROR_LOCK_VIOLATION as i32),
        ],
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );
    let error = result.expect_err("cleanup fault must wrap the primary");
    match error {
        AtomicWriteError::Io { source: inner, .. } => {
            assert_eq!(inner.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(inner.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            let display = inner.to_string();
            assert!(display.contains("could not remove stage"), "{display}");
        }
        AtomicWriteError::PublicationUncertain { .. } => panic!("cleanup wrapping stays on Io"),
    }
    assert_complete(&trace);
    assert!(source.exists());
    assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
    assert!(!destination.exists());
}

#[test]
fn uncertain_reclassification_never_cleans_up_the_source() {
    for failed in [
        Primitive::ReclassifyCapability,
        Primitive::ReclassifySource,
        Primitive::ReclassifyDestination,
    ] {
        let temporary = temporary("install-uncertain-reclass-");
        let (source, destination) = write_pair(temporary.path());
        let (result, trace) = run_with_windows_install_faults(
            [
                (Primitive::Move, 1, ERROR_SHARING_VIOLATION as i32),
                (failed, 1, ERROR_ACCESS_DENIED as i32),
            ],
            || install_file(&source, &destination, AtomicWriteOptions::default()),
        );

        match result.expect_err("reclassification fault must be uncertain") {
            AtomicWriteError::PublicationUncertain { operation, .. } => {
                assert_eq!(operation, "reconcile install after failed move");
            }
            AtomicWriteError::Io { .. } => panic!("{failed:?} must not clean up"),
        }
        assert_complete(&trace);
        assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
        assert_eq!(trace.real_moves, 0, "{trace:?}");
        assert!(trace.backoffs.is_empty(), "{trace:?}");
        assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
        assert!(!destination.exists());
    }
}

#[test]
fn post_move_observation_failures_are_publication_uncertain() {
    for (primitive, expected_operation) in [
        (
            Primitive::PostMoveCapability,
            "revalidate install paths after move",
        ),
        (
            Primitive::PostMoveDestination,
            "observe installed destination after move",
        ),
    ] {
        let temporary = temporary("install-post-move-");
        let (source, destination) = write_pair(temporary.path());
        let (result, trace) =
            run_with_windows_install_faults([(primitive, 1, ERROR_ACCESS_DENIED as i32)], || {
                install_file(&source, &destination, AtomicWriteOptions::default())
            });

        match result.expect_err("post-move fault must report uncertainty") {
            AtomicWriteError::PublicationUncertain {
                operation,
                source: inner,
                ..
            } => {
                assert_eq!(operation, expected_operation);
                assert_eq!(inner.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            }
            AtomicWriteError::Io { .. } => panic!("publication already landed"),
        }
        assert_complete(&trace);
        assert_eq!(trace.real_moves, 1, "{trace:?}");
        assert!(trace.backoffs.is_empty(), "{trace:?}");
        assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
        assert!(!source.exists());
    }
}
