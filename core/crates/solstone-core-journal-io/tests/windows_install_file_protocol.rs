// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;
use std::time::Duration;

use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, WindowsInstallPrimitive as Primitive,
    WindowsInstallTrace, install_file, run_with_windows_install_barrier,
    run_with_windows_install_faults, run_with_windows_install_faults_and_barrier,
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
fn destination_source_does_not_override_a_source_observation_failure() {
    let temporary = temporary("install-source-observation-fault-");
    let (source, destination) = write_pair(temporary.path());
    let moved_from = source.clone();
    let moved_to = destination.clone();
    let (result, trace) = run_with_windows_install_faults_and_barrier(
        [
            (Primitive::Move, 1, ERROR_SHARING_VIOLATION as i32),
            (Primitive::ReclassifySource, 1, ERROR_ACCESS_DENIED as i32),
        ],
        Primitive::ReclassifyCapability,
        1,
        move || fs::rename(&moved_from, &moved_to).unwrap(),
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    match result.expect_err("unobservable source disposition must remain uncertain") {
        AtomicWriteError::PublicationUncertain {
            operation,
            source: inner,
            ..
        } => {
            assert_eq!(operation, "reconcile install after failed move");
            assert_eq!(inner.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
        }
        AtomicWriteError::Io { .. } => panic!("source observation failure must not clean up"),
    }
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 1, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Cleanup), 0, "{trace:?}");
    assert!(!source.exists());
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
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
fn pre_move_destination_substitution_preserves_competitor_and_cleans_source() {
    let temporary = temporary("install-pre-move-dest-substitution-");
    let (source, destination) = write_pair(temporary.path());
    let competitor_at = destination.clone();
    let (result, trace) = run_with_windows_install_barrier(
        Primitive::BeforeMove,
        1,
        move || fs::write(&competitor_at, COMPETITOR).unwrap(),
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("destination substitution must refuse before move");
    assert!(matches!(error, AtomicWriteError::Io { .. }), "{error}");
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 0, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Cleanup), 1, "{trace:?}");
    assert!(!source.exists());
    assert_eq!(fs::read(&destination).unwrap(), COMPETITOR);
}

#[test]
fn source_swap_between_probe_and_retained_open_preserves_both_files() {
    let temporary = temporary("install-source-probe-swap-");
    let (source, destination) = write_pair(temporary.path());
    let source_to_move = source.clone();
    let relocated = temporary.path().join("relocated.bin");
    let relocated_by_barrier = relocated.clone();
    let competitor_at = source.clone();
    let (result, trace) = run_with_windows_install_barrier(
        Primitive::SourceProbed,
        1,
        move || {
            fs::rename(&source_to_move, &relocated_by_barrier).unwrap();
            fs::write(&competitor_at, COMPETITOR).unwrap();
        },
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("source identity swap must refuse before ownership");
    assert!(matches!(error, AtomicWriteError::Io { .. }), "{error}");
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::SourceProbed), 1, "{trace:?}");
    assert_eq!(count(&trace, Primitive::SourceReady), 0, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Cleanup), 0, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Move), 0, "{trace:?}");
    assert_eq!(fs::read(&source).unwrap(), COMPETITOR);
    assert_eq!(fs::read(&relocated).unwrap(), PAYLOAD);
    assert!(!destination.exists());
}

#[test]
fn pre_move_source_substitution_preserves_competitor_and_cleans_retained_file() {
    let temporary = temporary("install-pre-move-source-substitution-");
    let (source, destination) = write_pair(temporary.path());
    let source_to_move = source.clone();
    let relocated = temporary.path().join("relocated.bin");
    let relocated_by_barrier = relocated.clone();
    let competitor_at = source.clone();
    let (result, trace) = run_with_windows_install_barrier(
        Primitive::BeforeMove,
        1,
        move || {
            fs::rename(&source_to_move, &relocated_by_barrier).unwrap();
            fs::write(&competitor_at, COMPETITOR).unwrap();
        },
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("source substitution must refuse before move");
    assert!(matches!(error, AtomicWriteError::Io { .. }), "{error}");
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 0, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Cleanup), 1, "{trace:?}");
    assert_eq!(fs::read(&source).unwrap(), COMPETITOR);
    assert!(!relocated.exists());
    assert!(!destination.exists());
}

#[test]
fn pre_move_dynamic_alias_preserves_both_names_without_cleanup() {
    let temporary = temporary("install-pre-move-alias-");
    let (source, destination) = write_pair(temporary.path());
    if fs::hard_link(&source, &destination).is_err() {
        let _ = fs::remove_file(&destination);
        eprintln!("skipping pre-move alias test: hard link is not viable here");
        return;
    }
    fs::remove_file(&destination).unwrap();
    let linked_from = source.clone();
    let linked_to = destination.clone();
    let (result, trace) = run_with_windows_install_barrier(
        Primitive::BeforeMove,
        1,
        move || fs::hard_link(&linked_from, &linked_to).unwrap(),
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("dynamic alias must refuse before move");
    assert!(matches!(error, AtomicWriteError::Io { .. }), "{error}");
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 0, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Cleanup), 0, "{trace:?}");
    assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
}

#[test]
fn held_source_denies_a_competing_writer_until_publication_finishes() {
    let temporary = temporary("install-held-source-sharing-");
    let (source, destination) = write_pair(temporary.path());
    let held_name = source.clone();
    let (result, trace) = run_with_windows_install_barrier(
        Primitive::BeforeMove,
        1,
        move || {
            let error = OpenOptions::new()
                .write(true)
                .open(&held_name)
                .expect_err("the retained source must deny a competing writer");
            assert_eq!(error.raw_os_error(), Some(ERROR_SHARING_VIOLATION as i32));
        },
        || install_file(&source, &destination, AtomicWriteOptions::default()),
    );

    result.unwrap();
    assert_complete(&trace);
    assert_eq!(trace.real_moves, 1, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Cleanup), 0, "{trace:?}");
    assert!(!source.exists());
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
            let display = inner.to_string();
            assert!(display.contains("could not remove stage"), "{display}");
            let primary = inner
                .get_ref()
                .and_then(|error| std::error::Error::source(error))
                .and_then(|error| error.downcast_ref::<io::Error>())
                .expect("cleanup chain retains the primary io::Error as its source");
            assert_eq!(primary.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
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
            AtomicWriteError::PublicationUncertain {
                operation,
                source: inner,
                ..
            } => {
                assert_eq!(operation, "reconcile install after failed move");
                assert_eq!(inner.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
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

#[test]
fn cross_volume_refusal_cleans_the_held_source() {
    let Some(refs_root) = std::env::var_os("SOLSTONE_JOURNAL_WIN_REFS_ROOT") else {
        eprintln!("skipping cross-volume receipt without SOLSTONE_JOURNAL_WIN_REFS_ROOT");
        return;
    };
    let source_parent = temporary("install-cross-volume-source-");
    let destination_parent = tempfile::Builder::new()
        .prefix("install-cross-volume-destination-")
        .tempdir_in(refs_root)
        .unwrap();
    let source = source_parent.path().join("source.bin");
    let uncreated_destination_parent = destination_parent.path().join("uncreated");
    let destination = uncreated_destination_parent.join("destination.bin");
    fs::write(&source, PAYLOAD).unwrap();

    let (result, trace) =
        run_with_windows_install_faults(std::iter::empty::<(Primitive, usize, i32)>(), || {
            install_file(&source, &destination, AtomicWriteOptions::default())
        });
    let error = result.expect_err("an install spanning NTFS and ReFS must be refused");

    assert!(matches!(error, AtomicWriteError::Io { .. }), "{error}");
    assert_complete(&trace);
    assert_eq!(count(&trace, Primitive::Move), 0, "{trace:?}");
    assert_eq!(trace.real_moves, 0, "{trace:?}");
    assert_eq!(count(&trace, Primitive::Cleanup), 1, "{trace:?}");
    assert!(!source.exists(), "the admitted source must be cleaned up");
    assert!(
        !uncreated_destination_parent.exists(),
        "cross-volume refusal must precede destination ancestor creation"
    );
}

#[test]
fn install_file_protocol_receipt_marker() {
    println!(
        "JOURNAL_WIN_CI_INSTALL_FILE_PROTOCOL=admission/retry/sharing/reconciliation/cleanup/uncertainty/pass"
    );
}

#[test]
#[ignore = "source-origin marker for the native Windows gate"]
fn journal_win_ci_windows_install_file_protocol_marker() {
    println!("JOURNAL_WIN_CI_TARGET_WINDOWS_INSTALL_FILE_PROTOCOL=executed/pass");
}
