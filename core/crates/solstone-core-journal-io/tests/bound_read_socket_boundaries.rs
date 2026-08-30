// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixListener;

use nix::errno::Errno;
use nix::fcntl::{AT_FDCWD, OFlag, openat};
use nix::sys::stat::Mode;
use solstone_core_journal_io::{
    BoundReadPrimitive, ReadError, read_bytes_bound, run_with_bound_read_barrier,
    run_with_bound_read_fault,
};

fn open_directory(path: &std::path::Path) -> OwnedFd {
    openat(
        AT_FDCWD,
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .expect("bound directory opens")
}

fn read_record(directory: &OwnedFd) -> Result<Option<Vec<u8>>, ReadError> {
    read_bytes_bound(directory, OsStr::new("record"))
}

#[test]
fn initial_socket_is_rejected_without_an_open_attempt() {
    let root = tempfile::tempdir().expect("temporary root creates");
    let listener = UnixListener::bind(root.path().join("record")).expect("socket binds");
    let directory = open_directory(root.path());

    let (result, open_attempted) =
        run_with_bound_read_fault(BoundReadPrimitive::Open, 1, Errno::EIO as i32, || {
            read_record(&directory)
        });

    assert!(result.is_err());
    assert!(!open_attempted, "initial socket must not reach open");
    drop(listener);
}

#[test]
fn socket_substitution_before_open_is_rejected() {
    let root = tempfile::tempdir().expect("temporary root creates");
    let record = root.path().join("record");
    fs::write(&record, b"original").expect("record writes");
    let directory = open_directory(root.path());

    let (result, fired) = run_with_bound_read_barrier(
        BoundReadPrimitive::Open,
        1,
        move || {
            fs::remove_file(&record).expect("record removes");
            let listener = UnixListener::bind(&record).expect("socket binds");
            drop(listener);
        },
        || read_record(&directory),
    );

    assert!(fired, "bound read barrier fires");
    assert!(result.is_err());
}
