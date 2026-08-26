// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

mod common;

use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use common::{TempDir, directory, journal, write};
use solstone_core_journal_archive::{ArchiveError, ArchiveSource, JournalEntryKind};

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn unreadable_included_directory_returns_source_io() {
    use nix::unistd::Uid;

    if Uid::effective().is_root() {
        eprintln!("skipping permission-denied assertion as effective root");
        return;
    }

    let temporary = TempDir::new("permission-denied");
    let root = common::valid_four_root_journal(&temporary);
    let restricted = root.join("chronicle/20260101");
    fs::set_permissions(&restricted, fs::Permissions::from_mode(0o000))
        .expect("remove directory permissions");
    let result = ArchiveSource::open(&root);
    fs::set_permissions(&restricted, fs::Permissions::from_mode(0o755))
        .expect("restore directory permissions before temporary cleanup");

    assert!(matches!(result, Err(ArchiveError::SourceIo { .. })));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn rejects_relative_missing_and_file_roots() {
    let temporary = TempDir::new("invalid-roots");
    let file = temporary.path().join("file");
    let missing = temporary.path().join("missing");
    fs::write(&file, b"not a journal").expect("write file root");

    for root in [Path::new("relative"), missing.as_path(), file.as_path()] {
        assert!(matches!(
            ArchiveSource::open(root),
            Err(ArchiveError::InvalidJournal { .. })
        ));
    }
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn rejects_dangling_file_and_loop_symlink_roots() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new("invalid-symlink-roots");
    let missing = temporary.path().join("missing");
    let dangling = temporary.path().join("dangling");
    let file = temporary.path().join("file");
    let file_link = temporary.path().join("file-link");
    let loop_left = temporary.path().join("loop-left");
    let loop_right = temporary.path().join("loop-right");
    fs::write(&file, b"not a directory").expect("write file root");
    symlink(&missing, &dangling).expect("create dangling root symlink");
    symlink(&file, &file_link).expect("create file root symlink");
    symlink(&loop_right, &loop_left).expect("create first loop symlink");
    symlink(&loop_left, &loop_right).expect("create second loop symlink");

    for root in [&dangling, &file_link, &loop_left] {
        assert!(matches!(
            ArchiveSource::open(root),
            Err(ArchiveError::InvalidJournal { .. })
        ));
    }
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn rejects_symlink_fifo_and_socket_without_opening_special_members() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;

    let temporary = TempDir::new("special-members");
    let root = journal(&temporary);
    directory(&root, "chronicle/20260101");
    let outside = temporary.path().join("outside");
    fs::write(&outside, b"outside").expect("write outside marker");
    symlink(&outside, root.join("chronicle/20260101/link")).expect("create symlink");
    assert!(matches!(
        ArchiveSource::open(&root),
        Err(ArchiveError::UnsafeJournalEntry {
            kind: JournalEntryKind::Symlink,
            ..
        })
    ));
    fs::remove_file(root.join("chronicle/20260101/link")).expect("remove symlink");

    mkfifo(
        &root.join("chronicle/20260101/pipe"),
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .expect("create fifo");
    let root_for_thread = root.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        sender
            .send(ArchiveSource::open(&root_for_thread))
            .expect("send inventory result");
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("FIFO classification must not block");
    assert!(matches!(
        result,
        Err(ArchiveError::UnsafeJournalEntry {
            kind: JournalEntryKind::Fifo,
            ..
        })
    ));
    fs::remove_file(root.join("chronicle/20260101/pipe")).expect("remove fifo");

    let _socket = UnixListener::bind(root.join("chronicle/20260101/socket")).expect("bind socket");
    assert!(matches!(
        ArchiveSource::open(&root),
        Err(ArchiveError::UnsafeJournalEntry {
            kind: JournalEntryKind::Socket,
            ..
        })
    ));
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "APFS rejects non-UTF-8 path components before the product boundary"
)]
#[allow(clippy::disallowed_methods)]
fn rejects_distinct_non_utf8_member_names_without_lossy_normalization() {
    let temporary = TempDir::new("non-utf8-members");
    let root = journal(&temporary);
    directory(&root, "imports");
    for bytes in [vec![b'a', 0xff], vec![b'a', 0xfe]] {
        let name = std::ffi::OsString::from_vec(bytes);
        fs::write(root.join("imports").join(name), b"bad").expect("write invalid name");
        assert!(matches!(
            ArchiveSource::open(&root),
            Err(ArchiveError::UnsafeJournalEntry {
                kind: JournalEntryKind::Other,
                ..
            })
        ));
        fs::remove_dir_all(root.join("imports")).expect("clear invalid entry");
        fs::create_dir(root.join("imports")).expect("restore imports root");
    }
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "APFS rejects non-UTF-8 path components before the product boundary"
)]
#[allow(clippy::disallowed_methods)]
fn rejects_distinct_non_utf8_root_names_with_invalid_member_placeholder() {
    let temporary = TempDir::new("non-utf8-root-names");
    let root = journal(&temporary);
    for bytes in [vec![b'a', 0xff], vec![b'a', 0xfe]] {
        let name = std::ffi::OsString::from_vec(bytes);
        fs::write(root.join(&name), b"bad").expect("write invalid root name");
        assert!(matches!(
            ArchiveSource::open(&root),
            Err(ArchiveError::UnsafeJournalEntry {
                member,
                kind: JournalEntryKind::Other,
            }) if member.as_str() == "<invalid>"
        ));
        fs::remove_file(root.join(&name)).expect("remove invalid root name");
    }
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "APFS rejects non-UTF-8 path components before the product boundary"
)]
#[allow(clippy::disallowed_methods)]
fn rejects_non_utf8_canonical_root_ancestor() {
    let temporary = TempDir::new("non-utf8-ancestor");
    let ancestor = temporary
        .path()
        .join(std::ffi::OsString::from_vec(vec![0xff]));
    fs::create_dir(&ancestor).expect("create non-utf8 ancestor");
    let root = ancestor.join("journal");
    fs::create_dir(&root).expect("create journal");
    write(&root, "imports/file", b"x");
    assert!(matches!(
        ArchiveSource::open(&root),
        Err(ArchiveError::InvalidJournal { .. })
    ));
}
