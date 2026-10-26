// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![cfg(unix)]

use super::*;
use crate::test_support::TempDir;
use std::cell::RefCell;
use std::os::unix::fs::{MetadataExt, symlink};
use std::process::{Command, Stdio};

thread_local! {
    static REPLACEMENT: RefCell<Option<(PathBuf, PathBuf)>> = const { RefCell::new(None) };
}
fn replace_sidecar() {
    REPLACEMENT.with(|slot| {
        let (from, to) = slot.borrow_mut().take().unwrap();
        fs::rename(from, to).unwrap();
    });
}

#[test]
fn ordinary_bound_parent_lock_preserves_legacy_mode_exclusion_and_named_guard() {
    let temp = TempDir::new();
    let protected = temp.path().join("identity");
    drop(hold_lock(&protected, LockOptions::default()).unwrap());
    let sidecar = temp.path().join("identity.lock");
    let before = fs::metadata(&sidecar).unwrap();
    let parent = crate::JournalRoot::open(temp.path()).unwrap();
    let lock = hold_exclusive_parent_lock_bound_checked(
        &parent,
        OsStr::new("identity"),
        temp.path(),
        LockOptions::default(),
        &|| Ok(()),
    )
    .unwrap();
    let now = fs::metadata(&sidecar).unwrap();
    assert_eq!((before.ino(), before.mode()), (now.ino(), now.mode()));
    assert!(
        hold_lock(
            &protected,
            LockOptions {
                timeout: Duration::ZERO,
                ..Default::default()
            }
        )
        .is_err()
    );
    lock.revalidate_bound(&parent, OsStr::new("identity.lock"))
        .unwrap();
    fs::rename(&sidecar, temp.path().join("retired.lock")).unwrap();
    fs::write(&sidecar, b"late-inherited-lock").unwrap();
    assert!(
        lock.revalidate_bound(&parent, OsStr::new("identity.lock"))
            .is_err()
    );
    assert_eq!(fs::read(&sidecar).unwrap(), b"late-inherited-lock");
    drop(lock);
    assert!(
        hold_lock(
            &protected,
            LockOptions {
                timeout: Duration::ZERO,
                ..Default::default()
            }
        )
        .is_ok()
    );
}

#[test]
fn ordinary_bound_parent_lock_nonregular_open_is_bounded_even_after_inspection_race() {
    const CHILD: &str = "SOLSTONE_TEST_ORDINARY_BOUND_LOCK_CHILD";
    const ROOT: &str = "SOLSTONE_TEST_ORDINARY_BOUND_LOCK_ROOT";
    if let Ok(kind) = std::env::var(CHILD) {
        let path = PathBuf::from(std::env::var_os(ROOT).unwrap());
        let parent = crate::JournalRoot::open(&path).unwrap();
        let hook = if kind == "fifo-race" {
            REPLACEMENT.with(|slot| {
                *slot.borrow_mut() = Some((path.join("replacement"), path.join("identity.lock")))
            });
            Some(RaceHook::install(
                BEFORE_ORDINARY_BOUND_LOCK_OPEN,
                replace_sidecar,
            ))
        } else {
            None
        };
        let result = hold_exclusive_parent_lock_bound_checked(
            &parent,
            OsStr::new("identity"),
            &path,
            LockOptions::default(),
            &|| Ok(()),
        );
        assert!(result.is_err(), "unsafe sidecar must refuse: {kind}");
        drop(hook);
        return;
    }
    for kind in ["fifo", "fifo-race", "directory", "symlink"] {
        let temp = TempDir::new();
        let path = temp.path().join("identity.lock");
        fs::write(temp.path().join("target"), b"inherited-target").unwrap();
        let expected = match kind {
            "fifo" => {
                nix::unistd::mkfifo(&path, Mode::from_bits_truncate(0o600)).unwrap();
                path.clone()
            }
            "fifo-race" => {
                fs::write(&path, b"old-lock").unwrap();
                let replacement = temp.path().join("replacement");
                nix::unistd::mkfifo(&replacement, Mode::from_bits_truncate(0o600)).unwrap();
                replacement
            }
            "directory" => {
                fs::create_dir(&path).unwrap();
                path.clone()
            }
            "symlink" => {
                symlink("target", &path).unwrap();
                path.clone()
            }
            _ => unreachable!(),
        };
        let before = fs::symlink_metadata(&expected).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "locking::ordinary_bound_parent_lock_tests::ordinary_bound_parent_lock_nonregular_open_is_bounded_even_after_inspection_race", "--nocapture"])
            .env(CHILD, kind).env(ROOT, temp.path()).stdout(Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "unsafe sidecar control: {kind}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("unsafe sidecar acquisition blocked: {kind}");
            }
            thread::sleep(Duration::from_millis(10));
        }
        let after = fs::symlink_metadata(&path).unwrap();
        assert_eq!((before.ino(), before.mode()), (after.ino(), after.mode()));
        assert_eq!(
            fs::read(temp.path().join("target")).unwrap(),
            b"inherited-target"
        );
    }
}
