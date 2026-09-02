// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

use solstone_core_journal_io::atomic::{
    run_with_windows_detailed_atomic_barrier,
    run_with_windows_detailed_atomic_faults_and_two_barriers,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, DetailedAtomicError, ExclusivePublication, FinalNameConfirmation,
    MetadataDurability, StageCleanup, WindowsCreateOnlyPrimitive,
    run_with_windows_create_only_barrier, write_bytes_exclusive_detailed,
    write_reader_exclusive_detailed,
};
use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

const COMPETITOR: &[u8] = b"competitor-bytes";
const PAYLOAD: &[u8] = b"exclusive-detailed-payload";

struct NamedFailure;

impl Read for NamedFailure {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "named detailed reader failure",
        ))
    }
}

fn temporary(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(label).tempdir().unwrap()
}

fn assert_no_stage_residue(parent: &Path) {
    for entry in fs::read_dir(parent).unwrap() {
        let name = entry.unwrap().file_name();
        let lossy = name.to_string_lossy();
        assert!(
            !lossy.starts_with(".tmp_") && !lossy.starts_with("_.tmp_"),
            "stage residue remained: {lossy}"
        );
    }
}

fn stage_names(parent: &Path) -> Vec<std::ffi::OsString> {
    fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| {
            let lossy = name.to_string_lossy();
            lossy.starts_with(".tmp_") || lossy.starts_with("_.tmp_")
        })
        .collect()
}

fn already_exists(error: &DetailedAtomicError) -> bool {
    error.source.kind() == io::ErrorKind::AlreadyExists
}

fn assert_windows_unproven(published: &ExclusivePublication) {
    assert!(
        matches!(
            published.durability,
            MetadataDurability::Unproven { source: None }
        ),
        "Windows metadata durability is always Unproven with no syscall"
    );
    assert!(
        !published.is_fully_confirmed(),
        "Windows cannot be fully confirmed because directory-entry durability is unproven"
    );
}

#[test]
fn detailed_exclusive_publication_confirmed_success() {
    let temporary = temporary("create-only-detailed-ok-");
    let bytes_path = temporary.path().join("bytes.bin");
    let published =
        write_bytes_exclusive_detailed(&bytes_path, PAYLOAD, AtomicWriteOptions::default())
            .unwrap();
    assert_windows_unproven(&published);
    assert_eq!(published.bytes_written, PAYLOAD.len() as u64);
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Confirmed { ref observation } if observation.bytes == PAYLOAD
    ));
    assert!(matches!(published.cleanup, StageCleanup::Removed));
    assert_eq!(fs::read(&bytes_path).unwrap(), PAYLOAD);
    assert_no_stage_residue(temporary.path());

    let reader_path = temporary.path().join("reader.bin");
    let mut reader = &PAYLOAD[..];
    let published =
        write_reader_exclusive_detailed(&reader_path, &mut reader, AtomicWriteOptions::default())
            .unwrap();
    assert_windows_unproven(&published);
    assert_eq!(published.bytes_written, PAYLOAD.len() as u64);
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Confirmed { ref observation } if observation.bytes == PAYLOAD
    ));
    assert!(matches!(published.cleanup, StageCleanup::Removed));
    assert_eq!(fs::read(&reader_path).unwrap(), PAYLOAD);
    assert_no_stage_residue(temporary.path());
}

#[test]
fn detailed_exclusive_publication_preexisting_dest_is_untouched() {
    let temporary = temporary("create-only-detailed-held-");
    let path = temporary.path().join("held.bin");
    fs::write(&path, COMPETITOR).unwrap();
    let before = fs::metadata(&path).unwrap();
    let error = write_bytes_exclusive_detailed(&path, PAYLOAD, AtomicWriteOptions::default())
        .expect_err("occupied destination must refuse");
    assert!(already_exists(&error), "{error}");
    assert_eq!(fs::read(&path).unwrap(), COMPETITOR);
    let after = fs::metadata(&path).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after.modified().ok(), before.modified().ok());
    assert_no_stage_residue(temporary.path());
}

#[test]
fn detailed_exclusive_publication_exclusive_stage_collision_preserved() {
    let temporary = temporary("create-only-detailed-collision-");
    let dest = temporary.path().join("record.bin");
    let dest_for_barrier = dest.clone();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "exclusive-publish",
        1,
        move || fs::write(&dest_for_barrier, COMPETITOR).unwrap(),
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert!(fired);
    let error = result.expect_err("occupied destination during publish must refuse");
    assert!(already_exists(&error), "{error}");
    assert_eq!(fs::read(&dest).unwrap(), COMPETITOR);
    assert_no_stage_residue(temporary.path());
}

#[test]
fn detailed_exclusive_publication_stage_identity_mismatch_refused() {
    let temporary = temporary("create-only-detailed-mismatch-");
    let dest = temporary.path().join("record.bin");
    let parent = temporary.path().to_path_buf();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "exclusive-publish",
        1,
        move || {
            for name in stage_names(&parent) {
                fs::remove_file(parent.join(name)).unwrap();
            }
        },
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert!(fired);
    assert!(result.is_err());
    assert!(!dest.exists());
    assert_no_stage_residue(temporary.path());
}

#[test]
fn detailed_reader_failure_preserves_directory_substituted_at_stage_name() {
    let temporary = temporary("create-only-detailed-stage-directory-");
    let dest = temporary.path().join("record.bin");
    let parent = temporary.path().to_path_buf();
    let moved_stage = temporary.path().join("moved-stage.bin");
    let substituted = Arc::new(Mutex::new(None));
    let substituted_at_barrier = Arc::clone(&substituted);
    let moved_at_barrier = moved_stage.clone();
    let mut reader = NamedFailure;
    let (result, trace) = run_with_windows_create_only_barrier(
        WindowsCreateOnlyPrimitive::StageReady,
        1,
        move || {
            let names = stage_names(&parent);
            assert_eq!(names.len(), 1, "expected one live stage: {names:?}");
            let stage = parent.join(&names[0]);
            fs::rename(&stage, &moved_at_barrier).unwrap();
            fs::create_dir(&stage).unwrap();
            *substituted_at_barrier.lock().unwrap() = Some(stage);
        },
        || write_reader_exclusive_detailed(&dest, &mut reader, AtomicWriteOptions::default()),
    );

    let error = result.expect_err("reader failure must refuse publication");
    assert_eq!(error.source.kind(), io::ErrorKind::UnexpectedEof, "{error}");
    assert!(trace.barriers_fired, "{trace:?}");
    assert!(trace.faults_consumed, "{trace:?}");
    assert!(!dest.exists());
    let substituted = substituted.lock().unwrap().clone().unwrap();
    assert!(
        substituted.is_dir(),
        "foreign directory substituted at the stage name was removed"
    );
    assert_eq!(fs::read(&moved_stage).unwrap(), b"");
    fs::remove_dir(substituted).unwrap();
    fs::remove_file(moved_stage).unwrap();
}

#[test]
fn detailed_exclusive_publication_race_before_observation_one_is_unverified() {
    let temporary = temporary("create-only-detailed-obs1-");
    let dest = temporary.path().join("record.bin");
    let dest_for_barrier = dest.clone();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "exclusive-observe-1",
        1,
        move || fs::remove_file(&dest_for_barrier).unwrap(),
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert!(fired);
    let published = result.unwrap();
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Unverified { ref destination, ref reason }
            if destination == &dest && reason.kind() == io::ErrorKind::NotFound
    ));
    assert_windows_unproven(&published);
    assert!(matches!(published.cleanup, StageCleanup::Removed));
    assert!(!dest.exists());
    assert_no_stage_residue(temporary.path());
}

#[test]
fn detailed_exclusive_publication_forced_cleanup_failure_is_retained() {
    let temporary = temporary("create-only-detailed-cleanup-");
    let dest = temporary.path().join("record.bin");
    let parent = temporary.path().to_path_buf();
    let captured = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&captured);
    let parent_for_publish = parent.clone();
    let dest_for_observe = dest.clone();
    let captured_for_observe = Arc::clone(&captured);
    let (result, _, barriers) = run_with_windows_detailed_atomic_faults_and_two_barriers(
        [("stage-cleanup", 1, ERROR_ACCESS_DENIED as i32)],
        "exclusive-publish",
        1,
        move || {
            let names = stage_names(&parent_for_publish);
            *capture.lock().unwrap() = names.into_iter().next();
        },
        "exclusive-observe-1",
        1,
        move || {
            let stage = captured_for_observe
                .lock()
                .unwrap()
                .clone()
                .expect("stage name captured before move");
            fs::hard_link(
                &dest_for_observe,
                dest_for_observe.parent().unwrap().join(stage),
            )
            .unwrap();
        },
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert_eq!(barriers, 2);
    let published = result.unwrap();
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Confirmed { ref observation } if observation.bytes == PAYLOAD
    ));
    assert_windows_unproven(&published);
    assert!(matches!(published.cleanup, StageCleanup::Retained { .. }));
    assert_eq!(fs::read(&dest).unwrap(), PAYLOAD);
    assert_eq!(stage_names(temporary.path()).len(), 1);
}

#[test]
fn detailed_exclusive_publication_final_observation_and_durability_combined_failure() {
    let temporary = temporary("create-only-detailed-obs2-dur-");
    let dest = temporary.path().join("record.bin");
    let dest_for_barrier = dest.clone();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "exclusive-observe-2",
        1,
        move || fs::remove_file(&dest_for_barrier).unwrap(),
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert!(fired);
    let published = result.unwrap();
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Unverified { ref reason, .. }
            if reason.kind() == io::ErrorKind::NotFound
    ));
    assert_windows_unproven(&published);
    assert!(matches!(published.cleanup, StageCleanup::Removed));
    assert!(!dest.exists());
    assert_no_stage_residue(temporary.path());
}

#[test]
fn detailed_exclusive_publication_observation_one_race_and_cleanup_failure() {
    let temporary = temporary("create-only-detailed-obs1-cleanup-");
    let dest = temporary.path().join("record.bin");
    let parent = temporary.path().to_path_buf();
    let captured = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&captured);
    let parent_for_publish = parent.clone();
    let dest_for_observe = dest.clone();
    let captured_for_observe = Arc::clone(&captured);
    let (result, _, barriers) = run_with_windows_detailed_atomic_faults_and_two_barriers(
        [("stage-cleanup", 1, ERROR_ACCESS_DENIED as i32)],
        "exclusive-publish",
        1,
        move || {
            let names = stage_names(&parent_for_publish);
            *capture.lock().unwrap() = names.into_iter().next();
        },
        "exclusive-observe-1",
        1,
        move || {
            let stage = captured_for_observe
                .lock()
                .unwrap()
                .clone()
                .expect("stage name captured before move");
            fs::hard_link(
                &dest_for_observe,
                dest_for_observe.parent().unwrap().join(stage),
            )
            .unwrap();
            fs::remove_file(&dest_for_observe).unwrap();
        },
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert_eq!(barriers, 2);
    let published = result.unwrap();
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Unverified { .. }
    ));
    assert_windows_unproven(&published);
    assert!(matches!(published.cleanup, StageCleanup::Retained { .. }));
    assert!(!dest.exists());
    assert_eq!(stage_names(temporary.path()).len(), 1);
}

#[test]
fn detailed_exclusive_publication_all_three_facts_fail() {
    let temporary = temporary("create-only-detailed-all-");
    let dest = temporary.path().join("record.bin");
    let parent = temporary.path().to_path_buf();
    let captured = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&captured);
    let parent_for_publish = parent.clone();
    let dest_for_observe = dest.clone();
    let captured_for_observe = Arc::clone(&captured);
    let (result, _, barriers) = run_with_windows_detailed_atomic_faults_and_two_barriers(
        [("stage-cleanup", 1, ERROR_ACCESS_DENIED as i32)],
        "exclusive-publish",
        1,
        move || {
            let names = stage_names(&parent_for_publish);
            *capture.lock().unwrap() = names.into_iter().next();
        },
        "exclusive-observe-1",
        1,
        move || {
            let stage = captured_for_observe
                .lock()
                .unwrap()
                .clone()
                .expect("stage name captured before move");
            fs::hard_link(
                &dest_for_observe,
                dest_for_observe.parent().unwrap().join(stage),
            )
            .unwrap();
            fs::remove_file(&dest_for_observe).unwrap();
        },
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert_eq!(barriers, 2);
    let published = result.unwrap();
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Unverified { .. }
    ));
    assert_windows_unproven(&published);
    assert!(matches!(published.cleanup, StageCleanup::Retained { .. }));
    assert!(!dest.exists());
    assert_eq!(stage_names(temporary.path()).len(), 1);
}

#[test]
fn detailed_exclusive_publication_race_before_observation_two_is_unverified() {
    let temporary = temporary("create-only-detailed-obs2-");
    let dest = temporary.path().join("record.bin");
    let dest_for_barrier = dest.clone();
    let (result, fired) = run_with_windows_detailed_atomic_barrier(
        "exclusive-observe-2",
        1,
        move || fs::remove_file(&dest_for_barrier).unwrap(),
        || write_bytes_exclusive_detailed(&dest, PAYLOAD, AtomicWriteOptions::default()),
    );
    assert!(fired);
    let published = result.unwrap();
    assert!(matches!(
        published.final_name,
        FinalNameConfirmation::Unverified { ref reason, .. }
            if reason.kind() == io::ErrorKind::NotFound
    ));
    assert_windows_unproven(&published);
    assert!(matches!(published.cleanup, StageCleanup::Removed));
    assert!(!dest.exists());
    assert_no_stage_residue(temporary.path());
}
