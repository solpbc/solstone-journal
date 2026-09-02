// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, write_bytes_exclusive, write_reader_exclusive,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};

const COMPETITOR: &[u8] = b"competitor-bytes";
const PAYLOAD: &[u8] = b"exclusive-payload";
const MAX_EXTENDED_PATH_UTF16: usize = 32_767;
const VOLUME_GUID_ESTIMATE: usize = 49;
const VOLUME_GUID_PREFIX_MIN: usize = 48;
const VOLUME_GUID_PREFIX_MAX: usize = 64;
const WORST_CASE_STAGE_UTF16: usize = 88;

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

fn directory_is_empty(parent: &Path) {
    let leftover: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(leftover.is_empty(), "unexpected entries: {leftover:?}");
}

fn already_exists(error: &AtomicWriteError) -> bool {
    matches!(
        error,
        AtomicWriteError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists
    )
}

fn invalid_input(error: &AtomicWriteError) -> bool {
    matches!(
        error,
        AtomicWriteError::Io { source, .. } if source.kind() == io::ErrorKind::InvalidInput
    )
}

fn file_identity(path: &Path) -> (u32, u64) {
    let file = File::open(path).unwrap();
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `information` points to writable storage for the exact structure, and `file` remains
    // live through the synchronous query.
    #[allow(unsafe_code)]
    let result = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
    assert_ne!(result, 0, "query identity for {}", path.display());
    (
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    )
}

struct FailingReader {
    chunks: VecDeque<Vec<u8>>,
    error: io::Error,
}

impl Read for FailingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.chunks.pop_front() {
            Some(chunk) => {
                assert!(chunk.len() <= buf.len());
                buf[..chunk.len()].copy_from_slice(&chunk);
                Ok(chunk.len())
            }
            None => Err(io::Error::new(self.error.kind(), self.error.to_string())),
        }
    }
}

static CWD_TEST_LOCK: Mutex<()> = Mutex::new(());

fn cwd_test_lock() -> MutexGuard<'static, ()> {
    CWD_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct RestoreCwd(PathBuf);

impl Drop for RestoreCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

fn set_cwd(path: &Path) -> RestoreCwd {
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(path).unwrap();
    RestoreCwd(previous)
}

fn cwd_ancestor_contrib(cwd: &Path) -> usize {
    let text = cwd.to_str().expect("cwd is UTF-8");
    let rest = if text.len() >= 3 && text.as_bytes()[1] == b':' {
        &text[2..]
    } else {
        text
    };
    rest.split(['\\', '/'])
        .filter(|component| !component.is_empty())
        .map(|component| 1 + component.encode_utf16().count())
        .sum()
}

fn missing_ancestor_path(cwd: &Path, dest_leaf: &str) -> PathBuf {
    let target_parent = MAX_EXTENDED_PATH_UTF16 - 45;
    let base = VOLUME_GUID_ESTIMATE + cwd_ancestor_contrib(cwd);
    pad_relative_ancestors(target_parent.saturating_sub(base), dest_leaf)
}

fn pad_relative_ancestors(mut needed: usize, dest_leaf: &str) -> PathBuf {
    let mut path = PathBuf::new();
    path.push(format!("absent{}", "x".repeat(249)));
    needed = needed.saturating_sub(256);
    while needed >= 256 {
        path.push("x".repeat(255));
        needed -= 256;
    }
    match needed {
        0 => {}
        1 => path.push("w"),
        n => path.push("y".repeat(n - 1)),
    }
    path.push(dest_leaf);
    path
}

fn relative_ancestor_units(path: &Path) -> usize {
    let mut components = path
        .components()
        .map(|component| component.as_os_str().to_str().expect("UTF-8 component"))
        .collect::<Vec<_>>();
    components.pop();
    components
        .iter()
        .map(|component| 1 + component.encode_utf16().count())
        .sum()
}

fn leaf_units(path: &Path) -> usize {
    path.file_name()
        .and_then(|name| name.to_str())
        .expect("UTF-8 leaf")
        .encode_utf16()
        .count()
}

fn spelling_units(guid_prefix: usize, cwd_units: usize, extra_units: usize, leaf: usize) -> usize {
    guid_prefix + cwd_units + extra_units + 1 + leaf
}

fn dest_fits_stage_overflows_path(cwd: &Path, dest_leaf: &str) -> PathBuf {
    let cwd_units = cwd_ancestor_contrib(cwd);
    let needed = (MAX_EXTENDED_PATH_UTF16 - WORST_CASE_STAGE_UTF16)
        .saturating_sub(VOLUME_GUID_PREFIX_MIN)
        .saturating_sub(cwd_units);
    pad_relative_ancestors(needed, dest_leaf)
}

fn assert_dest_only_fits_stage_overflows(cwd: &Path, dest: &Path) {
    let cwd_units = cwd_ancestor_contrib(cwd);
    let extra_units = relative_ancestor_units(dest);
    let dest_leaf = leaf_units(dest);
    let dest_only_max = spelling_units(VOLUME_GUID_PREFIX_MAX, cwd_units, extra_units, dest_leaf);
    let stage_min = spelling_units(
        VOLUME_GUID_PREFIX_MIN,
        cwd_units,
        extra_units,
        WORST_CASE_STAGE_UTF16,
    );
    assert!(
        dest_only_max <= MAX_EXTENDED_PATH_UTF16,
        "precondition failed: dest-only spelling must fit even with the maximum local volume-GUID prefix ({VOLUME_GUID_PREFIX_MAX}); dest-only is {dest_only_max} UTF-16 units"
    );
    assert!(
        stage_min > MAX_EXTENDED_PATH_UTF16,
        "precondition failed: worst-case stage spelling must overflow even with the minimum local volume-GUID prefix ({VOLUME_GUID_PREFIX_MIN}); stage spelling is {stage_min} UTF-16 units"
    );
}

#[test]
fn existing_destination_is_untouched_and_leaves_no_stage() {
    let temporary = temporary("create-only-collision-");
    let path = temporary.path().join("held.bin");
    fs::write(&path, COMPETITOR).unwrap();
    let before = fs::metadata(&path).unwrap();
    let before_identity = file_identity(&path);

    let bytes_error = write_bytes_exclusive(&path, PAYLOAD, AtomicWriteOptions::default())
        .expect_err("occupied destination must refuse bytes exclusive");
    assert!(already_exists(&bytes_error), "{bytes_error}");
    assert_eq!(fs::read(&path).unwrap(), COMPETITOR);
    let after_bytes = fs::metadata(&path).unwrap();
    assert_eq!(after_bytes.len(), before.len());
    assert_eq!(after_bytes.created().ok(), before.created().ok());
    assert_eq!(file_identity(&path), before_identity);

    let mut reader = &PAYLOAD[..];
    let reader_error = write_reader_exclusive(&path, &mut reader, AtomicWriteOptions::default())
        .expect_err("occupied destination must refuse reader exclusive");
    assert!(already_exists(&reader_error), "{reader_error}");
    assert_eq!(fs::read(&path).unwrap(), COMPETITOR);
    let after_reader = fs::metadata(&path).unwrap();
    assert_eq!(after_reader.len(), before.len());
    assert_eq!(after_reader.created().ok(), before.created().ok());
    assert_eq!(file_identity(&path), before_identity);
    assert_eq!(
        fs::read_dir(temporary.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        vec![path.file_name().unwrap().to_os_string()]
    );
    assert_no_stage_residue(temporary.path());
}

#[test]
fn existing_directory_is_an_already_exists_collision() {
    let temporary = temporary("create-only-directory-collision-");
    let path = temporary.path().join("held");
    fs::create_dir(&path).unwrap();

    let bytes_error = write_bytes_exclusive(&path, PAYLOAD, AtomicWriteOptions::default())
        .expect_err("occupied directory must refuse bytes exclusive");
    assert!(already_exists(&bytes_error), "{bytes_error}");
    let mut reader = &PAYLOAD[..];
    let reader_error = write_reader_exclusive(&path, &mut reader, AtomicWriteOptions::default())
        .expect_err("occupied directory must refuse reader exclusive");
    assert!(already_exists(&reader_error), "{reader_error}");
    assert!(path.is_dir());
    assert_no_stage_residue(temporary.path());
}

#[test]
fn over_budget_relative_destination_under_missing_ancestor_does_not_mutate() {
    let _lock = cwd_test_lock();
    let temporary = temporary("create-only-over-budget-");
    let _cwd = set_cwd(temporary.path());
    let dest = missing_ancestor_path(temporary.path(), &"d".repeat(255));
    let first = dest.components().next().unwrap().as_os_str();

    let bytes_error = write_bytes_exclusive(&dest, PAYLOAD, AtomicWriteOptions::default())
        .expect_err("over-budget destination must refuse");
    assert!(invalid_input(&bytes_error), "{bytes_error}");
    let mut reader = &PAYLOAD[..];
    let reader_error = write_reader_exclusive(&dest, &mut reader, AtomicWriteOptions::default())
        .expect_err("over-budget destination must refuse the reader path");
    assert!(invalid_input(&reader_error), "{reader_error}");
    assert!(!temporary.path().join(first).exists());
    directory_is_empty(temporary.path());
}

#[test]
fn short_destination_with_over_budget_stage_does_not_mutate() {
    let _lock = cwd_test_lock();
    let temporary = temporary("create-only-stage-budget-");
    let _cwd = set_cwd(temporary.path());
    let dest = dest_fits_stage_overflows_path(temporary.path(), "z");
    assert_dest_only_fits_stage_overflows(temporary.path(), &dest);
    let first = dest.components().next().unwrap().as_os_str();

    let bytes_error = write_bytes_exclusive(&dest, PAYLOAD, AtomicWriteOptions::default())
        .expect_err("over-budget stage leaf must refuse");
    assert!(invalid_input(&bytes_error), "{bytes_error}");
    let mut reader = &PAYLOAD[..];
    let reader_error = write_reader_exclusive(&dest, &mut reader, AtomicWriteOptions::default())
        .expect_err("over-budget stage leaf must refuse the reader path");
    assert!(invalid_input(&reader_error), "{reader_error}");
    assert!(!temporary.path().join(first).exists());
    directory_is_empty(temporary.path());
}

#[test]
fn exclusive_writers_publish_exact_bytes() {
    let temporary = temporary("create-only-success-");
    let bytes_path = temporary.path().join("bytes.bin");
    write_bytes_exclusive(&bytes_path, PAYLOAD, AtomicWriteOptions::default()).unwrap();
    assert_eq!(fs::read(&bytes_path).unwrap(), PAYLOAD);

    let reader_path = temporary.path().join("reader.bin");
    let mut reader = &PAYLOAD[..];
    let copied =
        write_reader_exclusive(&reader_path, &mut reader, AtomicWriteOptions::default()).unwrap();
    assert_eq!(copied, PAYLOAD.len() as u64);
    assert_eq!(fs::read(&reader_path).unwrap(), PAYLOAD);
    assert_no_stage_residue(temporary.path());
}

#[test]
fn reader_failure_leaves_destination_and_stage_absent() {
    let temporary = temporary("create-only-reader-fail-");
    let path = temporary.path().join("partial.bin");
    let mut reader = FailingReader {
        chunks: VecDeque::from([b"abc".to_vec(), b"defg".to_vec()]),
        error: io::Error::new(io::ErrorKind::UnexpectedEof, "named reader failure"),
    };

    let error = write_reader_exclusive(&path, &mut reader, AtomicWriteOptions::default())
        .expect_err("reader failure must not publish");
    match error {
        AtomicWriteError::Io { source, .. } => {
            assert_eq!(source.kind(), io::ErrorKind::UnexpectedEof);
            assert!(
                source.to_string().contains("named reader failure"),
                "{source}"
            );
        }
        AtomicWriteError::PublicationUncertain { .. } => {
            panic!("reader failure is pre-publication")
        }
    }
    assert!(!path.exists());
    directory_is_empty(temporary.path());
}

#[test]
#[ignore = "source-origin marker for the native Windows gate"]
fn journal_win_ci_windows_create_only_marker() {
    println!("JOURNAL_WIN_CI_TARGET_WINDOWS_CREATE_ONLY=executed/pass");
}
