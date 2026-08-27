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
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_journal_io::DetailedAtomicOutcome;
use solstone_core_journal_io::atomic::{
    atomic_replace_detailed, run_with_windows_detailed_atomic_barrier,
    run_with_windows_detailed_atomic_faults,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, INVALID_HANDLE_VALUE,
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

fn assert_old_or_new(bytes: &[u8]) {
    assert!(
        bytes == OLD || bytes == NEW,
        "partial destination: {bytes:?}"
    );
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

fn target_fixture(label: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::Builder::new().prefix(label).tempdir().unwrap();
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
        let (result, _) = run_with_windows_detailed_atomic_faults(
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
        "fsync-bound-parent-dir",
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
        wait_for_marker(&marker, step);
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success(), "helper unexpectedly completed at {step}");
        assert_old_or_new(&fs::read(&target).unwrap());
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
    let partial_is_rejected = catch_unwind(AssertUnwindSafe(|| assert_old_or_new(b"partial")));
    assert!(
        partial_is_rejected.is_err(),
        "old-or-new assertion is a no-op"
    );
    assert_eq!(fs::read(target).unwrap(), b"partial");
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
            | DetailedAtomicOutcome::PublishedDurabilityUncertain { .. }
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
    wait_for_marker(&marker, "temp-create");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!stage_names(&parent).is_empty());
    assert!(stage_names(foreign.path()).is_empty());
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn pre_publication_cleanup_failure_reports_the_orphan_stage() {
    let (_temporary, parent, target) = target_fixture("cleanup-failure");
    let sentinel = outside_sentinel(&parent);
    let (result, attempted) = run_with_windows_detailed_atomic_faults(
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
fn transient_and_permanent_destination_failures_have_bounded_attempts() {
    let (_temporary, parent, target) = target_fixture("transient-success");
    let sentinel = outside_sentinel(&parent);
    let (result, attempted) = run_with_windows_detailed_atomic_faults(
        [("rename", 1, ERROR_SHARING_VIOLATION as i32)],
        || atomic_replace_detailed(&target, NEW, 0o600),
    );
    assert!(matches!(
        result.unwrap(),
        DetailedAtomicOutcome::Published
            | DetailedAtomicOutcome::PublishedDurabilityUncertain { .. }
    ));
    assert_eq!(
        attempted.iter().filter(|step| **step == "rename").count(),
        2
    );
    assert_sentinel_unchanged(&sentinel);

    let (_temporary, parent, target) = target_fixture("permanent-hold");
    let sentinel = outside_sentinel(&parent);
    let (result, attempted) = run_with_windows_detailed_atomic_faults(
        [
            ("rename", 1, ERROR_LOCK_VIOLATION as i32),
            ("rename", 2, ERROR_LOCK_VIOLATION as i32),
            ("rename", 3, ERROR_LOCK_VIOLATION as i32),
        ],
        || atomic_replace_detailed(&target, NEW, 0o600),
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&target).unwrap(), OLD);
    assert_eq!(
        attempted.iter().filter(|step| **step == "rename").count(),
        3
    );
    assert_sentinel_unchanged(&sentinel);
}

#[test]
fn permanent_publication_error_is_not_retried() {
    let (_temporary, parent, target) = target_fixture("permanent-error");
    let sentinel = outside_sentinel(&parent);
    let (result, attempted) = run_with_windows_detailed_atomic_faults(
        [("rename", 1, ERROR_ACCESS_DENIED as i32)],
        || atomic_replace_detailed(&target, NEW, 0o600),
    );
    assert!(result.is_err());
    assert_eq!(fs::read(&target).unwrap(), OLD);
    assert_eq!(
        attempted.iter().filter(|step| **step == "rename").count(),
        1
    );
    assert_sentinel_unchanged(&sentinel);
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
            | DetailedAtomicOutcome::PublishedDurabilityUncertain { .. }
    ));
    assert_eq!(fs::read(&target).unwrap(), NEW);
    assert_eq!(fs::read(&alias).unwrap(), OLD);
    assert_eq!(file_identity(&alias), before);
    assert_ne!(file_identity(&target), before);
    assert_sentinel_unchanged(&sentinel);
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
    let parent = temporary.path().join("parent");
    fs::create_dir(&parent).unwrap();
    let sentinel = outside_sentinel(&parent);
    let target = parent.join("unit.service");
    fs::write(&target, OLD).unwrap();
    let temp_volume = file_identity(&std::env::temp_dir());
    let destination_volume = file_identity(temporary.path());
    assert_ne!(
        temp_volume.0, destination_volume.0,
        "ReFS receipt requires TEMP/TMP on a different volume from the destination"
    );
    let marker = parent.join("pause-marker");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "detailed_atomic_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_DETAILED_TARGET", &target)
        .env("JOURNAL_IO_TEST_PAUSE_AT", "temp-create")
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    wait_for_marker(&marker, "temp-create");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        !stage_names(&parent).is_empty(),
        "stage did not remain beneath the ReFS destination parent"
    );
    assert!(matches!(
        atomic_replace_detailed(&target, NEW, 0o600).unwrap(),
        DetailedAtomicOutcome::Published
            | DetailedAtomicOutcome::PublishedDurabilityUncertain { .. }
    ));
    assert_eq!(fs::read(target).unwrap(), NEW);
    assert_sentinel_unchanged(&sentinel);
    println!("JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=ReFS");
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
