// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::ffi::OsStr;
#[cfg(all(unix, not(target_os = "macos")))]
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::sys::signal::{Signal, kill};
use nix::sys::stat::{Mode, umask};
use nix::unistd::Pid;
use solstone_core_journal_io::{
    AtomicWriteOptions, ClaimDurability, ClaimName, ClaimRemovalOutcome, ClaimRemovalPrimitive,
    ExistingParentLock, ExistingParentLockError, FlatDirectory, IdentityChangeDisposition,
    JournalRoot, LeaseOptions, LockError, LockOptions, StagedDirOptions,
    acquire_existing_parent_lock, acquire_file_lease, atomic_replace, claim_and_remove_observed,
    hold_lock, publish_staged_dir, read_observed_file, run_with_claim_removal_barrier,
    run_with_two_claim_removal_barriers,
};

#[test]
fn atomic_pause_helper() {
    let Ok(target) = std::env::var("JOURNAL_IO_HELPER_TARGET") else {
        return;
    };
    atomic_replace(target, b"new-content", AtomicWriteOptions::default()).unwrap();
}

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
    kill(Pid::from_raw(holder.child.id() as i32), Signal::SIGKILL).unwrap();
    holder.child.wait().unwrap();
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

fn kill_holder(mut holder: LeaseHolder) {
    kill(Pid::from_raw(holder.child.id() as i32), Signal::SIGKILL).unwrap();
    holder.child.wait().unwrap();
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
    kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL).unwrap();
    child.wait().unwrap();

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

struct UmaskRestore(Mode);

impl UmaskRestore {
    fn set(mask: u32) -> Self {
        Self(umask(Mode::from_bits_truncate(mask as nix::libc::mode_t)))
    }
}

impl Drop for UmaskRestore {
    fn drop(&mut self) {
        umask(self.0);
    }
}

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
    assert_eq!(entry_mode(&parent.join("fresh")), 0o600);
}

#[test]
fn existing_parent_lock_crosses_the_real_flock_boundary_in_both_directions() {
    let temporary = tempfile::TempDir::new().unwrap();
    let parent = temporary.path().join("locks");
    fs::create_dir(&parent).unwrap();
    let entry = parent.join("fresh");
    drop(acquire(&parent, OsStr::new("fresh"), Duration::from_secs(1)).unwrap());
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

fn claim_name(operation: u64) -> ClaimName {
    ClaimName::parse(&format!("!solstone-claim-00000001-{operation:016x}")).unwrap()
}

fn claim_directory(temporary: &tempfile::TempDir) -> FlatDirectory {
    fs::create_dir(temporary.path().join("flat")).unwrap();
    let root = JournalRoot::open(temporary.path()).unwrap();
    FlatDirectory::open(&root, Path::new("flat")).unwrap()
}

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
