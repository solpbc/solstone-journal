// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

mod common;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::sync::mpsc;
use std::time::Duration;

use common::{TempDir, entry, valid_four_root_journal, write};
use solstone_core_journal_archive::{ArchiveError, ArchiveSource};

#[test]
#[allow(clippy::disallowed_methods)]
fn replacement_file_and_nested_directory_hard_link_are_detected() {
    let temporary = TempDir::new("proof-barriers");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");

    fs::remove_file(root.join("chronicle/20260101/a.txt")).expect("remove file");
    write(&root, "chronicle/20260101/a.txt", b"replacement");
    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));

    let nested_temporary = TempDir::new("nested-proof-barrier");
    let nested_root = valid_four_root_journal(&nested_temporary);
    let nested_source = ArchiveSource::open(&nested_root).expect("open nested source");
    let nested_entry = entry(&nested_source, "chronicle/20260101/nested/b.txt");
    let keeper = nested_temporary.path().join("original-child");
    let _original_nested_directory = fs::File::open(nested_root.join("chronicle/20260101/nested"))
        .expect("hold original nested directory inode");
    fs::hard_link(nested_root.join("chronicle/20260101/nested/b.txt"), &keeper)
        .expect("preserve original child inode");
    fs::remove_dir_all(nested_root.join("chronicle/20260101/nested"))
        .expect("replace nested directory");
    fs::create_dir(nested_root.join("chronicle/20260101/nested"))
        .expect("create replacement directory");
    fs::hard_link(&keeper, nested_root.join("chronicle/20260101/nested/b.txt"))
        .expect("hard-link original inode into replacement directory");

    assert!(matches!(
        nested_source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
    assert!(matches!(
        nested_source.open_file(nested_entry),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn replacement_symlink_is_detected_without_following_it() {
    let temporary = TempDir::new("symlink-barrier");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");
    let member = "imports/import-1/source.bin";
    let inventory_entry = entry(&source, member);
    let outside = temporary.path().join("outside");
    fs::write(&outside, b"outside").expect("write outside marker");
    fs::remove_file(root.join(member)).expect("remove original");
    symlink(&outside, root.join(member)).expect("replace with symlink");

    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
    assert!(matches!(
        source.open_file(inventory_entry),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[test]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
fn in_place_size_changes_keep_inode_but_invalidate_proof() {
    let temporary = TempDir::new("size-barrier");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");
    let member = "imports/import-1/source.bin";
    let inventory_entry = entry(&source, member);
    let path = root.join(member);
    let inode = fs::metadata(&path).expect("stat original").ino();
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open original for in-place changes");
    file.set_len(2).expect("shrink original in place");
    assert_eq!(
        fs::metadata(&path).expect("stat shrunken file").ino(),
        inode
    );
    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
    assert!(matches!(
        source.open_file(inventory_entry),
        Err(ArchiveError::SourceChanged { .. })
    ));

    file.set_len(9).expect("grow original in place");
    assert_eq!(fs::metadata(&path).expect("stat grown file").ino(), inode);
    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
    assert!(matches!(
        source.open_file(inventory_entry),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn removing_empty_counted_day_invalidates_revalidation_but_open_file_stays_reachable() {
    let temporary = TempDir::new("empty-day-barrier");
    let root = common::journal(&temporary);
    write(&root, "chronicle/20260101/a.txt", b"a");
    common::directory(&root, "chronicle/20260102");
    let source = ArchiveSource::open(&root).expect("open source");
    let inventory_entry = entry(&source, "chronicle/20260101/a.txt");
    assert_eq!(source.inventory().day_count(), 2);

    fs::remove_dir(root.join("chronicle/20260102")).expect("remove empty counted day");

    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
    source
        .open_file(inventory_entry)
        .expect("open unrelated entry");
    assert_eq!(source.inventory().day_count(), 2);
}

#[test]
#[allow(clippy::disallowed_methods)]
fn removing_empty_portable_root_invalidates_revalidation() {
    let temporary = TempDir::new("empty-portable-root-barrier");
    let root = common::journal(&temporary);
    common::directory(&root, "imports");
    let source = ArchiveSource::open(&root).expect("open source");

    fs::remove_dir(root.join("imports")).expect("remove empty portable root");

    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn removing_empty_uncounted_nested_directory_invalidates_revalidation() {
    let temporary = TempDir::new("empty-nested-directory-barrier");
    let root = common::journal(&temporary);
    write(&root, "chronicle/20260101/a.txt", b"a");
    common::directory(&root, "chronicle/20260101/empty");
    let source = ArchiveSource::open(&root).expect("open source");

    fs::remove_dir(root.join("chronicle/20260101/empty")).expect("remove empty nested directory");

    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn removing_nonempty_directory_invalidates_revalidation() {
    let temporary = TempDir::new("nonempty-directory-barrier");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");

    fs::remove_dir_all(root.join("chronicle/20260101/nested")).expect("remove nonempty directory");

    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn removing_regular_file_invalidates_revalidation() {
    let temporary = TempDir::new("removed-file-barrier");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");

    fs::remove_file(root.join("imports/import-1/source.bin")).expect("remove regular file");

    assert!(matches!(
        source.revalidate(),
        Err(ArchiveError::SourceChanged { .. })
    ));
}

#[test]
#[allow(clippy::disallowed_methods)]
fn late_added_file_and_directory_are_ignored_by_revalidation() {
    let temporary = TempDir::new("late-additions-barrier");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");

    write(&root, "imports/import-2/later.bin", b"later");
    common::directory(&root, "chronicle/20260102");

    source.revalidate().expect("revalidate frozen inventory");
    assert_eq!(source.inventory().entries().len(), 5);
    assert_eq!(source.inventory().day_count(), 1);
    assert_eq!(source.inventory().entity_count(), 1);
    assert_eq!(source.inventory().facet_count(), 1);
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn replacement_fifo_is_rejected_promptly_without_opening_it() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    let temporary = TempDir::new("fifo-barrier");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");
    let member = "imports/import-1/source.bin";
    fs::remove_file(root.join(member)).expect("remove original");
    mkfifo(&root.join(member), Mode::S_IRUSR | Mode::S_IWUSR).expect("create replacement fifo");

    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            sender
                .send(source.revalidate())
                .expect("send revalidation result");
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("FIFO revalidation must not block");
        assert!(matches!(result, Err(ArchiveError::SourceChanged { .. })));
    });
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn replacement_socket_is_rejected_promptly_without_opening_it() {
    use std::os::unix::net::UnixListener;

    let temporary = TempDir::new("socket-barrier");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");
    let member = "imports/import-1/source.bin";
    let inventory_entry = entry(&source, member);
    fs::remove_file(root.join(member)).expect("remove original");
    let _socket = UnixListener::bind(root.join(member)).expect("bind replacement socket");

    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            sender
                .send((
                    source.revalidate(),
                    source.open_file(inventory_entry).map(|_| ()),
                ))
                .expect("send socket barrier results");
        });
        let (revalidated, opened) = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("socket classification must not block");
        assert!(matches!(
            revalidated,
            Err(ArchiveError::SourceChanged { .. })
        ));
        assert!(matches!(opened, Err(ArchiveError::SourceChanged { .. })));
    });
}
