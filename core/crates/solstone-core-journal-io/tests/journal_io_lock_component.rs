// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
#![cfg(any(unix, windows))]

use std::ffi::OsStr;
#[cfg(any(all(unix, not(target_os = "macos")), windows))]
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
#[cfg(windows)]
use std::io::Read;
#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::{Child, Stdio};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{Flock, FlockArg};
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::sys::stat::{Mode, umask};
#[cfg(unix)]
use nix::unistd::Pid;
#[cfg(unix)]
use solstone_core_journal_io::{
    AtomicWriteOptions, ClaimDurability, ClaimName, ClaimRemovalOutcome, ClaimRemovalPrimitive,
    FlatDirectory, IdentityChangeDisposition, JournalRoot, StagedDirOptions, atomic_replace,
    atomic_replace_bound, claim_and_remove_observed, publish_staged_dir, read_observed_file,
    run_with_claim_removal_barrier, run_with_two_claim_removal_barriers,
};
use solstone_core_journal_io::{
    ExistingParentLock, ExistingParentLockError, LeaseOptions, LockError, LockOptions,
    acquire_existing_parent_lock, acquire_existing_parent_lock_bound, acquire_file_lease,
    hold_lock,
};
#[cfg(windows)]
use solstone_core_journal_io::{
    WindowsLockFileExSubstitution, WindowsUnlockFileExObservation,
    run_with_forced_post_lock_identity_mismatch, run_with_windows_lock_file_ex_substitution,
    run_with_windows_lock_file_ex_trace, run_with_windows_unlock_file_ex_observation,
};

#[cfg(unix)]
#[test]
fn atomic_pause_helper() {
    let Ok(target) = std::env::var("JOURNAL_IO_HELPER_TARGET") else {
        return;
    };
    atomic_replace(target, b"new-content", AtomicWriteOptions::default()).unwrap();
}

#[cfg(unix)]
#[test]
fn atomic_replace_survives_kill_at_every_boundary() {
    let temporary = tempfile::TempDir::new().unwrap();
    for step in [
        "temp-create",
        "write",
        "fsync-file",
        "chmod",
        "close",
        "rename",
        "fsync-parent-dir",
    ] {
        let target = temporary.path().join(format!("{step}.txt"));
        let marker = temporary.path().join(format!("{step}.ready"));
        fs::write(&target, b"old-content").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "atomic_pause_helper", "--nocapture"])
            .env("JOURNAL_IO_HELPER_TARGET", &target)
            .env("JOURNAL_IO_TEST_PAUSE_AT", step)
            .env("JOURNAL_IO_TEST_MARKER", &marker)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "helper did not reach {step}");
        kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
        child.wait().unwrap();
        let contents = fs::read(&target).unwrap();
        assert!(
            contents == b"old-content" || contents == b"new-content",
            "{step}"
        );
    }
}

#[cfg(unix)]
#[test]
fn bound_atomic_pause_helper() {
    let Some(parent) = std::env::var_os("JOURNAL_IO_BOUND_PAUSE_PARENT") else {
        return;
    };
    let root = JournalRoot::open(Path::new(&parent)).unwrap();
    let directory = FlatDirectory::open(&root, Path::new("bound")).unwrap();
    atomic_replace_bound(&directory, OsStr::new("unit.service"), b"new", 0o600).unwrap();
}

#[cfg(unix)]
fn wait_for_pause_marker(marker: &Path, step: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if fs::read(marker).ok().as_deref() == Some(step.as_bytes()) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("bound publication did not pause at {step}");
}

#[cfg(unix)]
#[test]
fn bound_atomic_replace_subprocess_kill_leaves_only_safe_states() {
    for (step, expected) in [("temp-create", b"old".as_slice()), ("rename", b"new")] {
        let temporary = tempfile::TempDir::new().unwrap();
        let parent = temporary.path().join("bound");
        let target = parent.join("unit.service");
        let marker = temporary.path().join("pause-marker");
        fs::create_dir(&parent).unwrap();
        fs::write(&target, b"old").unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "bound_atomic_pause_helper", "--nocapture"])
            .env("JOURNAL_IO_BOUND_PAUSE_PARENT", temporary.path())
            .env("JOURNAL_IO_TEST_PAUSE_AT", step)
            .env("JOURNAL_IO_TEST_MARKER", &marker)
            .spawn()
            .unwrap();
        wait_for_pause_marker(&marker, step);
        child.kill().unwrap();
        child.wait().unwrap();

        assert_eq!(fs::read(&target).unwrap(), expected);
        assert!(fs::read_dir(&parent).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            name == OsStr::new("unit.service") || name.as_encoded_bytes().starts_with(b".tmp_")
        }));
    }
}

#[cfg(unix)]
#[test]
fn staged_pause_helper() {
    let Ok(destination) = std::env::var("JOURNAL_IO_HELPER_STAGED_DESTINATION") else {
        return;
    };
    publish_staged_dir(
        Path::new(&destination),
        StagedDirOptions {
            directory_mode: Some(0o700),
        },
        |staging| {
            fs::write(staging.join("manifest.json"), b"{\"complete\":true}\n")?;
            // Production pause_at is env-driven; this mid-populate write is the
            // helper-owned checkpoint the parent kills against.
            if std::env::var("JOURNAL_IO_TEST_PAUSE_AT").ok().as_deref() == Some("mid-populate") {
                if let Ok(marker) = std::env::var("JOURNAL_IO_TEST_MARKER") {
                    let _ = fs::write(marker, "mid-populate");
                }
                loop {
                    thread::sleep(Duration::from_millis(25));
                }
            }
            fs::write(staging.join("payload.bin"), b"complete-payload")?;
            Ok::<_, io::Error>(())
        },
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn killed_publish_never_exposes_a_torn_set() {
    let temporary = tempfile::TempDir::new().unwrap();
    for checkpoint in [
        "before-staging-dir-create",
        "after-staging-dir-create",
        "mid-populate",
        "after-populate",
        "after-staging-sync",
        "after-rename",
    ] {
        let destination = temporary.path().join(format!("bundle-{checkpoint}"));
        let marker = temporary.path().join(format!("{checkpoint}.ready"));
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "staged_pause_helper", "--nocapture"])
            .env("JOURNAL_IO_HELPER_STAGED_DESTINATION", &destination)
            .env("JOURNAL_IO_TEST_PAUSE_AT", checkpoint)
            .env("JOURNAL_IO_TEST_MARKER", &marker)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "helper did not reach {checkpoint}");
        kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
        child.wait().unwrap();

        if checkpoint == "after-rename" {
            assert!(destination.exists(), "destination missing after rename");
            assert_eq!(
                fs::read(destination.join("manifest.json")).unwrap(),
                b"{\"complete\":true}\n"
            );
            assert_eq!(
                fs::read(destination.join("payload.bin")).unwrap(),
                b"complete-payload"
            );
        } else {
            assert!(
                !destination.exists(),
                "destination appeared before rename at {checkpoint}"
            );
        }
    }
}

#[test]
fn lease_pause_helper() {
    let Ok(path) = std::env::var("JOURNAL_IO_HELPER_LEASE_PATH") else {
        return;
    };
    let lease = acquire_file_lease(path, LeaseOptions::default())
        .unwrap()
        .expect("helper acquires lease");
    let marker = std::env::var("JOURNAL_IO_TEST_MARKER").unwrap();
    fs::write(marker, "locked").unwrap();
    let _keep_guard_alive = lease;
    loop {
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn contention_returns_none_from_another_process() {
    let temporary = tempfile::TempDir::new().unwrap();
    let path = temporary.path().join("refresh.lease");
    let holder = spawn_lease_holder(&path, temporary.path());
    wait_for_marker(&holder.marker);

    let contention = acquire_file_lease(
        &path,
        LeaseOptions {
            attempts: 2,
            retry_max: Duration::from_millis(25),
            ..LeaseOptions::default()
        },
    );
    assert!(matches!(contention, Ok(None)));
    kill_holder(holder);
}

#[test]
fn lease_is_released_when_the_holder_dies() {
    let temporary = tempfile::TempDir::new().unwrap();
    let path = temporary.path().join("refresh.lease");
    let mut holder = spawn_lease_holder(&path, temporary.path());
    wait_for_marker(&holder.marker);

    assert!(matches!(
        acquire_file_lease(
            &path,
            LeaseOptions {
                retry_max: Duration::ZERO,
                ..LeaseOptions::default()
            },
        ),
        Ok(None)
    ));
    kill_child(&mut holder.child);
    fs::remove_file(&holder.marker).unwrap();
    fs::remove_file(holder.marker.with_extension("pid")).unwrap();

    let started = Instant::now();
    assert!(
        acquire_file_lease(&path, LeaseOptions::default())
            .unwrap()
            .is_some()
    );
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[test]
fn zero_attempts_and_retry_window_make_one_immediate_attempt() {
    let temporary = tempfile::TempDir::new().unwrap();
    let path = temporary.path().join("refresh.lease");
    let holder = spawn_lease_holder(&path, temporary.path());
    wait_for_marker(&holder.marker);

    let started = Instant::now();
    let result = acquire_file_lease(
        &path,
        LeaseOptions {
            attempts: 0,
            retry_max: Duration::ZERO,
            ..LeaseOptions::default()
        },
    );
    assert!(matches!(result, Ok(None)));
    assert!(started.elapsed() < Duration::from_millis(100));
    kill_holder(holder);
}

struct LeaseHolder {
    child: std::process::Child,
    marker: PathBuf,
}

fn kill_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
    #[cfg(windows)]
    child.kill().unwrap();
    child.wait().unwrap();
}

fn spawn_lease_holder(path: &Path, temporary: &Path) -> LeaseHolder {
    let marker = temporary.join(format!("lease-holder-{}.ready", std::process::id()));
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_HELPER_LEASE_PATH", path)
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    fs::write(marker.with_extension("pid"), child.id().to_string()).unwrap();
    LeaseHolder { child, marker }
}

fn wait_for_marker(marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "helper did not acquire the lease");
}

#[cfg(windows)]
fn read_windows_hook_output(child: &mut Child) -> (String, String) {
    fn read_stream(stream: &mut impl Read) -> String {
        let mut output = String::new();
        match stream.read_to_string(&mut output) {
            Ok(_) => output,
            Err(error) => format!("<failed to read helper output: {error}>"),
        }
    }

    let stdout = child
        .stdout
        .as_mut()
        .map(read_stream)
        .unwrap_or_else(|| "<stdout was not captured>".to_owned());
    let stderr = child
        .stderr
        .as_mut()
        .map(read_stream)
        .unwrap_or_else(|| "<stderr was not captured>".to_owned());
    (stdout, stderr)
}

#[cfg(windows)]
fn panic_windows_hook_exit(
    child: &mut Child,
    helper: &str,
    event: &str,
    status: std::process::ExitStatus,
) -> ! {
    let (stdout, stderr) = read_windows_hook_output(child);
    panic!("{helper} {event}: {status}; stdout:\n{stdout}\nstderr:\n{stderr}");
}

#[cfg(windows)]
fn wait_for_marker_or_die(child: &mut Child, marker: &Path, helper: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let marker_exists = marker.exists();
        match child.try_wait() {
            Ok(None) if marker_exists => return,
            Ok(Some(status)) => {
                let event = if marker_exists {
                    "exited after writing its readiness marker"
                } else {
                    "exited before writing its readiness marker"
                };
                panic_windows_hook_exit(child, helper, event, status);
            }
            Ok(None) => {}
            Err(error) => panic!("{helper} liveness check failed before readiness: {error}"),
        }

        if Instant::now() >= deadline {
            let termination_error = child.kill().err();
            let status = child.wait().unwrap_or_else(|error| {
                panic!("{helper} did not reach readiness and could not be reaped: {error}")
            });
            let event = match termination_error {
                Some(error) => {
                    format!(
                        "did not write its readiness marker before timeout; termination failed: {error}"
                    )
                }
                None => "did not write its readiness marker before timeout".to_owned(),
            };
            panic_windows_hook_exit(child, helper, &event, status);
        }

        thread::sleep(Duration::from_millis(10));
    }
}

fn kill_holder(mut holder: LeaseHolder) {
    kill_child(&mut holder.child);
    fs::remove_file(&holder.marker).unwrap();
    fs::remove_file(holder.marker.with_extension("pid")).unwrap();
}

#[test]
fn lock_pause_helper() {
    let Some(path) = std::env::var_os("JOURNAL_IO_HELPER_LOCK_PATH") else {
        return;
    };
    let lock = hold_lock(path, LockOptions::default()).unwrap();
    let marker = std::env::var("JOURNAL_IO_TEST_MARKER").unwrap();
    fs::write(marker, "locked").unwrap();
    let _keep_guard_alive = lock;
    loop {
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn lock_is_released_when_the_holder_dies() {
    let temporary = tempfile::TempDir::new().unwrap();
    let path = temporary.path().join("config.json");
    let marker = temporary.path().join("locked.ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_HELPER_LOCK_PATH", &path)
        .env("JOURNAL_IO_TEST_MARKER", &marker)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "helper did not acquire the lock");
    let contention = hold_lock(
        &path,
        LockOptions {
            timeout: Duration::from_millis(100),
            ..LockOptions::default()
        },
    );
    assert!(
        matches!(contention, Err(LockError::Timeout(_))),
        "child did not create real lock contention"
    );
    kill_child(&mut child);

    let started = Instant::now();
    let _lock = hold_lock(
        &path,
        LockOptions {
            timeout: Duration::from_secs(1),
            ..LockOptions::default()
        },
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[cfg(unix)]
struct UmaskRestore(Mode);

#[cfg(unix)]
impl UmaskRestore {
    fn set(mask: u32) -> Self {
        Self(umask(Mode::from_bits_truncate(mask as nix::libc::mode_t)))
    }
}

#[cfg(unix)]
impl Drop for UmaskRestore {
    fn drop(&mut self) {
        umask(self.0);
    }
}

#[cfg(unix)]
fn entry_mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn acquire(
    parent: &Path,
    name: &OsStr,
    timeout: Duration,
) -> Result<ExistingParentLock, ExistingParentLockError> {
    acquire_existing_parent_lock(parent, name, timeout, Duration::from_millis(10))
}

#[cfg(unix)]
#[test]
fn existing_parent_lock_umask_helper() {
    let Some(parent) = std::env::var_os("JOURNAL_IO_UMASK_PARENT") else {
        return;
    };
    let parent = PathBuf::from(parent);
    let _restore = UmaskRestore::set(0o200);
    let error = acquire(&parent, OsStr::new("lock"), Duration::from_secs(1)).unwrap_err();
    assert!(matches!(
        error,
        ExistingParentLockError::WrongMode {
            observed: 0o400,
            ..
        }
    ));
    assert_eq!(entry_mode(&parent.join("lock")), 0o400);
}

#[cfg(unix)]
#[test]
fn existing_parent_lock_leaves_umask_restricted_creation_unrepaired() {
    let temporary = tempfile::TempDir::new().unwrap();
    let parent = temporary.path().join("locks");
    fs::create_dir(&parent).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "existing_parent_lock_umask_helper",
            "--nocapture",
        ])
        .env("JOURNAL_IO_UMASK_PARENT", &parent)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(entry_mode(&parent.join("lock")), 0o400);
}

#[test]
fn existing_parent_lock_first_time_contenders_produce_one_winner() {
    let temporary = tempfile::TempDir::new().unwrap();
    let parent = temporary.path().join("locks");
    fs::create_dir(&parent).unwrap();
    let start = Arc::new(Barrier::new(3));
    let finish = Arc::new(Barrier::new(3));
    let (sender, receiver) = mpsc::channel();
    for _ in 0..2 {
        let parent = parent.clone();
        let start = Arc::clone(&start);
        let finish = Arc::clone(&finish);
        let sender = sender.clone();
        thread::spawn(move || {
            start.wait();
            let result = acquire(&parent, OsStr::new("fresh"), Duration::from_millis(100));
            sender.send(result.is_ok()).unwrap();
            finish.wait();
        });
    }
    start.wait();
    assert_eq!(
        [receiver.recv().unwrap(), receiver.recv().unwrap()]
            .into_iter()
            .filter(|won| *won)
            .count(),
        1
    );
    finish.wait();
    #[cfg(unix)]
    assert_eq!(entry_mode(&parent.join("fresh")), 0o600);
}

#[cfg(windows)]
struct RawWindowsLock {
    file: File,
    overlapped: windows_sys::Win32::System::IO::OVERLAPPED,
}

#[cfg(windows)]
impl Drop for RawWindowsLock {
    fn drop(&mut self) {
        // SAFETY: this test helper owns the file and unlocks the same whole-file range it locked.
        #[allow(unsafe_code)]
        unsafe {
            windows_sys::Win32::Storage::FileSystem::UnlockFileEx(
                self.file.as_raw_handle(),
                0,
                u32::MAX,
                u32::MAX,
                &mut self.overlapped,
            );
        }
    }
}

#[cfg(windows)]
fn raw_windows_lock(file: File) -> Result<RawWindowsLock, io::Error> {
    let mut overlapped = windows_sys::Win32::System::IO::OVERLAPPED::default();
    // SAFETY: the file handle is live and the zeroed OVERLAPPED describes the whole-file range.
    #[allow(unsafe_code)]
    let result = unsafe {
        windows_sys::Win32::Storage::FileSystem::LockFileEx(
            file.as_raw_handle(),
            windows_sys::Win32::Storage::FileSystem::LOCKFILE_EXCLUSIVE_LOCK
                | windows_sys::Win32::Storage::FileSystem::LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(RawWindowsLock { file, overlapped })
}

#[cfg(windows)]
struct WindowsLockHook {
    child: Child,
    ready_marker: PathBuf,
    drop_now_marker: PathBuf,
    post_drop_marker: PathBuf,
}

#[cfg(windows)]
fn wait_for_windows_lock_drop(marker: &Path) {
    while !marker.exists() {
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
fn hold_windows_lock_until_dropped<T, U>(
    guard: T,
    _keep_alive: U,
    marker: &Path,
    marker_contents: &str,
    drop_now_marker: &Path,
    post_drop_marker: &Path,
    expected_handle: RawHandle,
) -> ! {
    fs::write(marker, marker_contents).unwrap();
    wait_for_windows_lock_drop(drop_now_marker);
    let ((), observations) = run_with_windows_unlock_file_ex_observation(|| drop(guard));
    assert_eq!(
        observations,
        vec![WindowsUnlockFileExObservation {
            handle: expected_handle,
            length_low: u32::MAX,
            length_high: u32::MAX,
            succeeded: true,
            error: None,
        }]
    );
    fs::write(post_drop_marker, "unlocked").unwrap();
    loop {
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(windows)]
#[test]
fn windows_lock_hook_pause_helper() {
    let Some(kind) = std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_HOOK") else {
        return;
    };
    let target = PathBuf::from(std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_TARGET").unwrap());
    let marker = PathBuf::from(std::env::var_os("JOURNAL_IO_TEST_MARKER").unwrap());
    match kind.to_string_lossy().as_ref() {
        "unlocked" => {
            let ((guard, trace), consumed) = run_with_windows_lock_file_ex_substitution(
                1,
                WindowsLockFileExSubstitution::Skip,
                || {
                    run_with_windows_lock_file_ex_trace(|| {
                        hold_lock(&target, LockOptions::default()).unwrap()
                    })
                },
            );
            assert!(consumed);
            assert!(trace.is_empty());
            fs::write(marker, "unlocked").unwrap();
            let _keep_guard_alive = guard;
            loop {
                thread::sleep(Duration::from_millis(25));
            }
        }
        "wrong-handle" => {
            let redirected =
                PathBuf::from(std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_REDIRECT").unwrap());
            let redirected_file = File::options()
                .read(true)
                .write(true)
                .create(true)
                .open(&redirected)
                .unwrap();
            let ((guard, trace), consumed) = run_with_windows_lock_file_ex_substitution(
                1,
                WindowsLockFileExSubstitution::ReplaceHandle(redirected_file.as_raw_handle()),
                || {
                    run_with_windows_lock_file_ex_trace(|| {
                        hold_lock(&target, LockOptions::default()).unwrap()
                    })
                },
            );
            assert!(consumed);
            assert_eq!(trace, vec![redirected_file.as_raw_handle()]);
            let drop_now_marker =
                PathBuf::from(std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_DROP_NOW_MARKER").unwrap());
            let post_drop_marker = PathBuf::from(
                std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_POST_DROP_MARKER").unwrap(),
            );
            hold_windows_lock_until_dropped(
                guard,
                redirected_file,
                &marker,
                "wrong-handle",
                &drop_now_marker,
                &post_drop_marker,
                trace[0],
            );
        }
        "persistent-api-hold" => {
            let parent = target
                .parent()
                .expect("persistent lock target has a parent");
            let name = target
                .file_name()
                .expect("persistent lock target has a file name");
            let (guard, trace) = run_with_windows_lock_file_ex_trace(|| {
                acquire(parent, name, Duration::from_secs(1)).unwrap()
            });
            assert_eq!(trace.len(), 1);
            let drop_now_marker =
                PathBuf::from(std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_DROP_NOW_MARKER").unwrap());
            let post_drop_marker = PathBuf::from(
                std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_POST_DROP_MARKER").unwrap(),
            );
            hold_windows_lock_until_dropped(
                guard,
                (),
                &marker,
                "persistent-api-hold",
                &drop_now_marker,
                &post_drop_marker,
                trace[0],
            );
        }
        "sidecar-api-hold" => {
            let (guard, trace) = run_with_windows_lock_file_ex_trace(|| {
                hold_lock(&target, LockOptions::default()).unwrap()
            });
            assert_eq!(trace.len(), 1);
            let drop_now_marker =
                PathBuf::from(std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_DROP_NOW_MARKER").unwrap());
            let post_drop_marker = PathBuf::from(
                std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_POST_DROP_MARKER").unwrap(),
            );
            hold_windows_lock_until_dropped(
                guard,
                (),
                &marker,
                "sidecar-api-hold",
                &drop_now_marker,
                &post_drop_marker,
                trace[0],
            );
        }
        "lease-api-hold" => {
            let (lease, trace) = run_with_windows_lock_file_ex_trace(|| {
                acquire_file_lease(&target, LeaseOptions::default())
                    .unwrap()
                    .expect("lease hook acquires lease")
            });
            assert_eq!(trace.len(), 1);
            let drop_now_marker =
                PathBuf::from(std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_DROP_NOW_MARKER").unwrap());
            let post_drop_marker = PathBuf::from(
                std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_POST_DROP_MARKER").unwrap(),
            );
            hold_windows_lock_until_dropped(
                lease,
                (),
                &marker,
                "lease-api-hold",
                &drop_now_marker,
                &post_drop_marker,
                trace[0],
            );
        }
        "persistent-raw-hold" => {
            let guard = raw_windows_lock(
                File::options()
                    .read(true)
                    .write(true)
                    .open(&target)
                    .unwrap(),
            )
            .unwrap();
            fs::write(marker, "persistent-raw-hold").unwrap();
            let _keep_guard_alive = guard;
            loop {
                thread::sleep(Duration::from_millis(25));
            }
        }
        "identity-mismatch" => {
            let parent = PathBuf::from(std::env::var_os("JOURNAL_IO_WINDOWS_LOCK_PARENT").unwrap());
            let (result, consumed) = run_with_forced_post_lock_identity_mismatch(1, || {
                acquire_existing_parent_lock(
                    &parent,
                    OsStr::new("entry"),
                    Duration::from_secs(1),
                    Duration::from_millis(10),
                )
            });
            assert!(consumed);
            assert!(matches!(
                result,
                Err(ExistingParentLockError::NamespaceChanged { .. })
            ));
            fs::write(marker, "identity-mismatch").unwrap();
        }
        other => panic!("unknown Windows lock hook {other}"),
    }
}

#[cfg(windows)]
fn spawn_windows_lock_hook_helper(
    kind: &str,
    target: &Path,
    temporary: &Path,
    redirect: Option<&Path>,
    parent: Option<&Path>,
) -> WindowsLockHook {
    let ready_marker = temporary.join(format!("windows-lock-{kind}.ready"));
    let drop_now_marker = temporary.join(format!("windows-lock-{kind}.drop-now"));
    let post_drop_marker = temporary.join(format!("windows-lock-{kind}.post-drop"));
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "windows_lock_hook_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_WINDOWS_LOCK_HOOK", kind)
        .env("JOURNAL_IO_WINDOWS_LOCK_TARGET", target)
        .env("JOURNAL_IO_TEST_MARKER", &ready_marker)
        .env("JOURNAL_IO_WINDOWS_LOCK_DROP_NOW_MARKER", &drop_now_marker)
        .env(
            "JOURNAL_IO_WINDOWS_LOCK_POST_DROP_MARKER",
            &post_drop_marker,
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(redirect) = redirect {
        command.env("JOURNAL_IO_WINDOWS_LOCK_REDIRECT", redirect);
    }
    if let Some(parent) = parent {
        command.env("JOURNAL_IO_WINDOWS_LOCK_PARENT", parent);
    }
    WindowsLockHook {
        child: command.spawn().unwrap(),
        ready_marker,
        drop_now_marker,
        post_drop_marker,
    }
}

#[cfg(windows)]
fn prove_windows_lock_lifecycle(holder: &mut WindowsLockHook, path: &Path, helper: &str) {
    wait_for_marker_or_die(&mut holder.child, &holder.ready_marker, helper);
    let initial_probe =
        raw_windows_lock(File::options().read(true).write(true).open(path).unwrap());
    let initially_excluded = matches!(
        &initial_probe,
        Err(error)
            if error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION as i32)
    );
    drop(initial_probe);
    if !initially_excluded {
        kill_child(&mut holder.child);
    }
    assert!(
        initially_excluded,
        "{helper} did not exclude the raw peer probe"
    );

    fs::write(&holder.drop_now_marker, "drop").unwrap();
    wait_for_marker_or_die(&mut holder.child, &holder.post_drop_marker, helper);
    let reacquired = raw_windows_lock(File::options().read(true).write(true).open(path).unwrap());
    let reacquired_after_drop = reacquired.is_ok();
    drop(reacquired);
    if !reacquired_after_drop {
        kill_child(&mut holder.child);
    }
    assert!(
        reacquired_after_drop,
        "{helper} did not release the raw peer lock after drop"
    );
    kill_child(&mut holder.child);
}

#[cfg(windows)]
#[test]
fn unlocked_lock_file_ex_hook_is_falsifiable_across_processes() {
    let temporary = tempfile::TempDir::new().unwrap();
    let target = temporary.path().join("target");
    let mut holder =
        spawn_windows_lock_hook_helper("unlocked", &target, temporary.path(), None, None);
    wait_for_marker(&holder.ready_marker);
    assert!(hold_lock(&target, LockOptions::default()).is_ok());
    kill_child(&mut holder.child);
}

#[cfg(windows)]
#[test]
fn wrong_handle_lock_file_ex_hook_is_falsifiable_across_processes() {
    let temporary = tempfile::TempDir::new().unwrap();
    let target = temporary.path().join("target");
    let redirected = temporary.path().join("redirected");
    let mut holder = spawn_windows_lock_hook_helper(
        "wrong-handle",
        &target,
        temporary.path(),
        Some(&redirected),
        None,
    );
    wait_for_marker_or_die(
        &mut holder.child,
        &holder.ready_marker,
        "wrong-handle lock helper",
    );
    let target_lock = hold_lock(&target, LockOptions::default());
    if target_lock.is_err() {
        kill_child(&mut holder.child);
    }
    assert!(target_lock.is_ok());
    prove_windows_lock_lifecycle(&mut holder, &redirected, "wrong-handle lock helper");
}

#[cfg(windows)]
#[test]
fn forced_identity_mismatch_hook_releases_the_real_lock_across_processes() {
    let temporary = tempfile::TempDir::new().unwrap();
    let parent = temporary.path().join("locks");
    fs::create_dir(&parent).unwrap();
    let target = parent.join("entry");
    let mut holder = spawn_windows_lock_hook_helper(
        "identity-mismatch",
        &target,
        temporary.path(),
        None,
        Some(&parent),
    );
    wait_for_marker(&holder.ready_marker);
    assert!(holder.child.wait().unwrap().success());
    assert!(acquire(&parent, OsStr::new("entry"), Duration::from_secs(1)).is_ok());
}

#[test]
fn existing_parent_lock_crosses_the_real_flock_boundary_in_both_directions() {
    let temporary = tempfile::TempDir::new().unwrap();
    let parent = temporary.path().join("locks");
    fs::create_dir(&parent).unwrap();
    let entry = parent.join("fresh");
    drop(acquire(&parent, OsStr::new("fresh"), Duration::from_secs(1)).unwrap());
    #[cfg(unix)]
    {
        let api_guard = acquire(&parent, OsStr::new("fresh"), Duration::from_secs(1)).unwrap();
        let raw = File::open(&entry).unwrap();
        assert!(matches!(
            Flock::lock(raw, FlockArg::LockExclusiveNonblock),
            Err((_, Errno::EACCES | Errno::EAGAIN))
        ));
        drop(api_guard);
        let raw_guard =
            Flock::lock(File::open(&entry).unwrap(), FlockArg::LockExclusiveNonblock).unwrap();
        let (sender, receiver) = mpsc::channel();
        let parent_for_thread = parent.clone();
        thread::spawn(move || {
            sender
                .send(acquire(
                    &parent_for_thread,
                    OsStr::new("fresh"),
                    Duration::from_millis(50),
                ))
                .unwrap()
        });
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), Err(ExistingParentLockError::Timeout(timeout)) if timeout.timeout == Duration::from_millis(50))
        );
        drop(raw_guard);
    }
    #[cfg(windows)]
    {
        let mut api_holder = spawn_windows_lock_hook_helper(
            "persistent-api-hold",
            &entry,
            temporary.path(),
            None,
            None,
        );
        prove_windows_lock_lifecycle(&mut api_holder, &entry, "persistent API lock helper");

        let mut raw_holder = spawn_windows_lock_hook_helper(
            "persistent-raw-hold",
            &entry,
            temporary.path(),
            None,
            None,
        );
        wait_for_marker_or_die(
            &mut raw_holder.child,
            &raw_holder.ready_marker,
            "persistent raw lock helper",
        );
        let api_probe = acquire(&parent, OsStr::new("fresh"), Duration::from_millis(50));
        kill_child(&mut raw_holder.child);
        assert!(
            matches!(api_probe, Err(ExistingParentLockError::Timeout(timeout)) if timeout.timeout == Duration::from_millis(50))
        );
    }
}

#[cfg(windows)]
#[test]
fn sidecar_lock_crosses_the_real_lock_file_ex_boundary_and_releases_while_alive() {
    let temporary = tempfile::TempDir::new().unwrap();
    let target = temporary.path().join("target");
    let sidecar = temporary.path().join("target.lock");
    let mut holder =
        spawn_windows_lock_hook_helper("sidecar-api-hold", &target, temporary.path(), None, None);
    prove_windows_lock_lifecycle(&mut holder, &sidecar, "sidecar API lock helper");
}

#[cfg(windows)]
#[test]
fn lease_lock_crosses_the_real_lock_file_ex_boundary_and_releases_while_alive() {
    let temporary = tempfile::TempDir::new().unwrap();
    let path = temporary.path().join("refresh.lease");
    let mut holder =
        spawn_windows_lock_hook_helper("lease-api-hold", &path, temporary.path(), None, None);
    prove_windows_lock_lifecycle(&mut holder, &path, "lease API lock helper");
}

#[cfg(windows)]
fn open_windows_parent_for_bound_test(path: &Path) -> File {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    // SAFETY: `wide` is NUL-terminated and stays live through the synchronous directory open.
    #[allow(unsafe_code)]
    let handle = unsafe {
        windows_sys::Win32::Storage::FileSystem::CreateFileW(
            wide.as_ptr(),
            windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES
                | windows_sys::Win32::Storage::FileSystem::FILE_LIST_DIRECTORY
                | windows_sys::Win32::Storage::FileSystem::FILE_TRAVERSE,
            windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
            std::ptr::null(),
            windows_sys::Win32::Storage::FileSystem::OPEN_EXISTING,
            windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS
                | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(handle, windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);
    // SAFETY: CreateFileW returned a fresh owned directory handle exactly once.
    #[allow(unsafe_code)]
    unsafe {
        File::from_raw_handle(handle)
    }
}

#[test]
fn bound_parent_lock_survives_parent_path_replacement() {
    let temporary = tempfile::TempDir::new().unwrap();
    let parent = temporary.path().join("parent");
    let moved = temporary.path().join("moved");
    fs::create_dir(&parent).unwrap();
    #[cfg(unix)]
    let parent_handle = nix::fcntl::openat(
        nix::fcntl::AT_FDCWD,
        &parent,
        nix::fcntl::OFlag::O_RDONLY
            .union(nix::fcntl::OFlag::O_DIRECTORY)
            .union(nix::fcntl::OFlag::O_CLOEXEC)
            .union(nix::fcntl::OFlag::O_NOFOLLOW),
        nix::sys::stat::Mode::empty(),
    )
    .unwrap();
    #[cfg(windows)]
    let parent_handle = open_windows_parent_for_bound_test(&parent);
    fs::rename(&parent, &moved).unwrap();
    fs::create_dir(&parent).unwrap();
    let _guard = acquire_existing_parent_lock_bound(
        &parent_handle,
        OsStr::new("state.lock"),
        Duration::from_secs(1),
        Duration::from_millis(10),
    )
    .unwrap();
    assert!(moved.join("state.lock").exists());
    assert!(!parent.join("state.lock").exists());
}

#[cfg(all(unix, not(target_os = "macos")))]
const FF: &[u8] = b"seg-\xff";
#[cfg(all(unix, not(target_os = "macos")))]
const FE: &[u8] = b"seg-\xfe";

#[cfg(all(unix, not(target_os = "macos")))]
fn os_path(dir: &Path, bytes: &[u8]) -> PathBuf {
    dir.join(OsString::from_vec(bytes.to_vec()))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_lock_holder(path: &Path, marker: &Path) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_HELPER_LOCK_PATH", path)
        .env("JOURNAL_IO_TEST_MARKER", marker)
        .spawn()
        .unwrap()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn kill_lock_holder(mut child: std::process::Child) {
    kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
    child.wait().unwrap();
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn exact_name_locks_are_independent_across_processes() {
    let temporary = tempfile::TempDir::new().unwrap();
    let locks = temporary.path().join("locks");
    fs::create_dir(&locks).unwrap();
    let ff = os_path(&locks, FF);
    let fe = os_path(&locks, FE);
    let marker = temporary.path().join("locked.ready");
    let child = spawn_lock_holder(&ff, &marker);
    wait_for_marker(&marker);
    let started = Instant::now();
    let _fe_lock = hold_lock(&fe, LockOptions::default()).unwrap();
    assert!(started.elapsed() < Duration::from_millis(200));
    kill_lock_holder(child);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn same_invalid_name_contends_across_processes() {
    let temporary = tempfile::TempDir::new().unwrap();
    let locks = temporary.path().join("locks");
    fs::create_dir(&locks).unwrap();
    let ff = os_path(&locks, FF);
    let marker = temporary.path().join("locked.ready");
    let child = spawn_lock_holder(&ff, &marker);
    wait_for_marker(&marker);
    let contention = hold_lock(
        &ff,
        LockOptions {
            timeout: Duration::from_millis(100),
            ..LockOptions::default()
        },
    );
    assert!(
        matches!(contention, Err(LockError::Timeout(_))),
        "child did not create real lock contention"
    );
    kill_lock_holder(child);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn invalid_name_lock_is_released_when_the_holder_dies() {
    let temporary = tempfile::TempDir::new().unwrap();
    let locks = temporary.path().join("locks");
    fs::create_dir(&locks).unwrap();
    let ff = os_path(&locks, FF);
    let marker = temporary.path().join("locked.ready");
    let mut child = spawn_lock_holder(&ff, &marker);
    wait_for_marker(&marker);
    let contention = hold_lock(
        &ff,
        LockOptions {
            timeout: Duration::from_millis(100),
            ..LockOptions::default()
        },
    );
    assert!(
        matches!(contention, Err(LockError::Timeout(_))),
        "child did not create real lock contention"
    );
    kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
    child.wait().unwrap();
    let started = Instant::now();
    let _lock = hold_lock(
        &ff,
        LockOptions {
            timeout: Duration::from_secs(1),
            ..LockOptions::default()
        },
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_millis(200));
}

#[cfg(windows)]
const SURROGATE_A: &[u16] = &[b's' as u16, b'e' as u16, b'g' as u16, b'-' as u16, 0xD800];
#[cfg(windows)]
const SURROGATE_B: &[u16] = &[b's' as u16, b'e' as u16, b'g' as u16, b'-' as u16, 0xD801];

#[cfg(windows)]
fn surrogate_path(dir: &Path, units: &[u16]) -> PathBuf {
    dir.join(OsString::from_wide(units))
}

#[cfg(windows)]
fn spawn_surrogate_lock_holder(path: &Path, marker: &Path) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_pause_helper", "--nocapture"])
        .env("JOURNAL_IO_HELPER_LOCK_PATH", path)
        .env("JOURNAL_IO_TEST_MARKER", marker)
        .spawn()
        .unwrap()
}

#[cfg(windows)]
#[test]
fn surrogate_names_lock_independently_across_processes() {
    let temporary = tempfile::TempDir::new().unwrap();
    let locks = temporary.path().join("locks");
    fs::create_dir(&locks).unwrap();
    let first = surrogate_path(&locks, SURROGATE_A);
    let second = surrogate_path(&locks, SURROGATE_B);
    let marker = temporary.path().join("locked.ready");
    let mut child = spawn_surrogate_lock_holder(&first, &marker);
    wait_for_marker(&marker);
    let started = Instant::now();
    let _second = hold_lock(&second, LockOptions::default()).unwrap();
    assert!(started.elapsed() < Duration::from_millis(200));
    kill_child(&mut child);
}

#[cfg(windows)]
#[test]
fn same_surrogate_name_contends_across_processes() {
    let temporary = tempfile::TempDir::new().unwrap();
    let locks = temporary.path().join("locks");
    fs::create_dir(&locks).unwrap();
    let path = surrogate_path(&locks, SURROGATE_A);
    let marker = temporary.path().join("locked.ready");
    let mut child = spawn_surrogate_lock_holder(&path, &marker);
    wait_for_marker(&marker);
    assert!(matches!(
        hold_lock(
            &path,
            LockOptions {
                timeout: Duration::from_millis(100),
                ..LockOptions::default()
            }
        ),
        Err(LockError::Timeout(_))
    ));
    kill_child(&mut child);
}

#[cfg(windows)]
#[test]
fn surrogate_name_lock_is_released_when_the_holder_dies() {
    let temporary = tempfile::TempDir::new().unwrap();
    let locks = temporary.path().join("locks");
    fs::create_dir(&locks).unwrap();
    let path = surrogate_path(&locks, SURROGATE_A);
    let marker = temporary.path().join("locked.ready");
    let mut child = spawn_surrogate_lock_holder(&path, &marker);
    wait_for_marker(&marker);
    kill_child(&mut child);
    let _lock = hold_lock(
        &path,
        LockOptions {
            timeout: Duration::from_secs(1),
            ..LockOptions::default()
        },
    )
    .unwrap();
}

#[cfg(unix)]
fn claim_name(operation: u64) -> ClaimName {
    ClaimName::parse(&format!("!solstone-claim-00000001-{operation:016x}")).unwrap()
}

#[cfg(unix)]
fn claim_directory(temporary: &tempfile::TempDir) -> FlatDirectory {
    fs::create_dir(temporary.path().join("flat")).unwrap();
    let root = JournalRoot::open(temporary.path()).unwrap();
    FlatDirectory::open(&root, Path::new("flat")).unwrap()
}

#[cfg(unix)]
#[test]
fn claim_barriers_preserve_the_observed_object_or_a_new_original() {
    let temporary = tempfile::TempDir::new().unwrap();
    let directory = claim_directory(&temporary);
    let entry = temporary.path().join("flat/entry");

    fs::write(&entry, b"observed").unwrap();
    let prior = read_observed_file(&directory, OsStr::new("entry"))
        .unwrap()
        .unwrap();
    let changed_entry = entry.clone();
    let (result, fired) = run_with_claim_removal_barrier(
        ClaimRemovalPrimitive::BeforeClaim,
        1,
        move || fs::write(changed_entry, b"changed-before-claim").unwrap(),
        || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim_name(1)),
    );
    assert!(fired);
    assert_eq!(
        result.unwrap(),
        ClaimRemovalOutcome::IdentityChanged {
            disposition: IdentityChangeDisposition::Restored,
            durability: ClaimDurability::Synced,
        }
    );
    assert_eq!(fs::read(&entry).unwrap(), b"changed-before-claim");

    fs::write(&entry, b"observed-again").unwrap();
    let prior = read_observed_file(&directory, OsStr::new("entry"))
        .unwrap()
        .unwrap();
    let replacement = entry.clone();
    let (result, fired) = run_with_claim_removal_barrier(
        ClaimRemovalPrimitive::AfterClaim,
        1,
        move || fs::write(replacement, b"replacement-after-claim").unwrap(),
        || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim_name(2)),
    );
    assert!(fired);
    assert_eq!(result.unwrap(), ClaimRemovalOutcome::Removed);
    assert_eq!(fs::read(&entry).unwrap(), b"replacement-after-claim");
}

#[cfg(unix)]
#[test]
fn claim_inspection_unlink_and_restore_barriers_never_overwrite_an_original() {
    let temporary = tempfile::TempDir::new().unwrap();
    let directory = claim_directory(&temporary);
    let entry = temporary.path().join("flat/entry");

    fs::write(&entry, b"observed").unwrap();
    let prior = read_observed_file(&directory, OsStr::new("entry"))
        .unwrap()
        .unwrap();
    let claim = claim_name(3);
    let claim_path = temporary.path().join("flat").join(claim.as_str());
    let replacement = entry.clone();
    let (result, fired) = run_with_claim_removal_barrier(
        ClaimRemovalPrimitive::BeforeInspection,
        1,
        move || {
            fs::write(claim_path, b"changed-claim").unwrap();
            fs::write(replacement, b"replacement-before-restore").unwrap();
        },
        || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
    );
    assert!(fired);
    assert_eq!(
        result.unwrap(),
        ClaimRemovalOutcome::IdentityChanged {
            disposition: IdentityChangeDisposition::RetainedClaim {
                claim: claim.clone(),
            },
            durability: ClaimDurability::Synced,
        }
    );
    assert_eq!(fs::read(&entry).unwrap(), b"replacement-before-restore");
    assert_eq!(
        fs::read(temporary.path().join("flat").join(claim.as_str())).unwrap(),
        b"changed-claim"
    );

    fs::remove_file(temporary.path().join("flat").join(claim.as_str())).unwrap();
    fs::write(&entry, b"observed-unlink").unwrap();
    let prior = read_observed_file(&directory, OsStr::new("entry"))
        .unwrap()
        .unwrap();
    let replacement = entry.clone();
    let (result, fired) = run_with_claim_removal_barrier(
        ClaimRemovalPrimitive::BeforeUnlink,
        1,
        move || fs::write(replacement, b"replacement-before-unlink").unwrap(),
        || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim_name(4)),
    );
    assert!(fired);
    assert_eq!(result.unwrap(), ClaimRemovalOutcome::Removed);
    assert_eq!(fs::read(&entry).unwrap(), b"replacement-before-unlink");

    fs::write(&entry, b"observed-restore").unwrap();
    let prior = read_observed_file(&directory, OsStr::new("entry"))
        .unwrap()
        .unwrap();
    let changed = entry.clone();
    let replacement = entry.clone();
    let claim = claim_name(5);
    let (result, fired) = run_with_two_claim_removal_barriers(
        ClaimRemovalPrimitive::BeforeClaim,
        1,
        move || fs::write(changed, b"changed-before-restore").unwrap(),
        ClaimRemovalPrimitive::BeforeRestore,
        1,
        move || fs::write(replacement, b"replacement-during-restore").unwrap(),
        || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim),
    );
    assert_eq!(fired, 2);
    assert_eq!(
        result.unwrap(),
        ClaimRemovalOutcome::IdentityChanged {
            disposition: IdentityChangeDisposition::RetainedClaim {
                claim: claim.clone(),
            },
            durability: ClaimDurability::Synced,
        }
    );
    assert_eq!(fs::read(&entry).unwrap(), b"replacement-during-restore");
}

#[cfg(unix)]
#[test]
fn concurrent_claimers_report_unknown_for_the_loser() {
    let temporary = tempfile::TempDir::new().unwrap();
    let directory = Arc::new(claim_directory(&temporary));
    let entry = temporary.path().join("flat/entry");
    fs::write(&entry, b"observed").unwrap();
    let prior = read_observed_file(&directory, OsStr::new("entry"))
        .unwrap()
        .unwrap();
    let (claimed_tx, claimed_rx) = mpsc::channel();
    let (loser_tx, loser_rx) = mpsc::channel();
    let loser_directory = Arc::clone(&directory);
    let loser_prior = prior.clone();
    let loser = thread::spawn(move || {
        claimed_rx.recv().unwrap();
        let result = claim_and_remove_observed(
            &loser_directory,
            OsStr::new("entry"),
            &loser_prior,
            &claim_name(7),
        );
        loser_tx.send(result).unwrap();
    });
    let (winner, fired) = run_with_claim_removal_barrier(
        ClaimRemovalPrimitive::AfterClaim,
        1,
        move || {
            claimed_tx.send(()).unwrap();
            let result = loser_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(
                result.unwrap(),
                ClaimRemovalOutcome::IdentityChanged {
                    disposition: IdentityChangeDisposition::UnknownLocation,
                    durability: ClaimDurability::NotEstablished,
                }
            );
        },
        || claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim_name(6)),
    );
    assert!(fired);
    assert_eq!(winner.unwrap(), ClaimRemovalOutcome::Removed);
    loser.join().unwrap();
}

#[cfg(unix)]
#[test]
fn bound_flat_directory_survives_parent_path_replacement() {
    let temporary = tempfile::TempDir::new().unwrap();
    let journal = temporary.path().join("journal");
    let parent = journal.join("parent");
    fs::create_dir_all(parent.join("flat")).unwrap();
    fs::write(parent.join("flat/entry"), b"observed").unwrap();
    let root = JournalRoot::open(&journal).unwrap();
    let directory = FlatDirectory::open(&root, Path::new("parent/flat")).unwrap();
    let prior = read_observed_file(&directory, OsStr::new("entry"))
        .unwrap()
        .unwrap();
    fs::rename(&parent, journal.join("moved")).unwrap();
    fs::create_dir_all(parent.join("flat")).unwrap();
    fs::write(parent.join("flat/entry"), b"replacement").unwrap();
    assert_eq!(
        claim_and_remove_observed(&directory, OsStr::new("entry"), &prior, &claim_name(8)).unwrap(),
        ClaimRemovalOutcome::Removed
    );
    assert!(!journal.join("moved/flat/entry").exists());
    assert_eq!(fs::read(parent.join("flat/entry")).unwrap(), b"replacement");
}
