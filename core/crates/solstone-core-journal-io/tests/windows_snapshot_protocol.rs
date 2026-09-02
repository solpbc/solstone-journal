// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::collections::BTreeMap;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use solstone_core_journal_io::{
    JournalSnapshot, SnapshotDirectory, SnapshotError, SnapshotFile, capture_snapshot,
    restore_snapshot,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObservedEntry {
    File(Vec<u8>),
    Directory,
    Reparse {
        attributes: u32,
        is_file: bool,
        is_dir: bool,
        is_symlink: bool,
    },
    Other,
}

#[derive(Debug, Clone, Copy)]
enum Operation {
    Capture,
    Restore,
}

#[derive(Debug, Clone, Copy)]
enum ReparseKind {
    FileSymlink,
    DirectoryJunction,
}

#[derive(Debug, Clone, Copy)]
enum Position {
    Root,
    Descendant,
}

fn temporary(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(label).tempdir().unwrap()
}

fn observe_tree(root: &Path) -> BTreeMap<String, ObservedEntry> {
    let mut tree = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        observe_entry(&entry.path(), &name, &mut tree);
    }
    tree
}

fn observe_entry(path: &Path, rel: &str, tree: &mut BTreeMap<String, ObservedEntry>) {
    let metadata = fs::symlink_metadata(path).unwrap();
    let file_type = metadata.file_type();
    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        tree.insert(
            rel.to_owned(),
            ObservedEntry::Reparse {
                attributes,
                is_file: file_type.is_file(),
                is_dir: file_type.is_dir(),
                is_symlink: file_type.is_symlink(),
            },
        );
        return;
    }
    if file_type.is_file() {
        tree.insert(rel.to_owned(), ObservedEntry::File(fs::read(path).unwrap()));
        return;
    }
    if file_type.is_dir() {
        tree.insert(rel.to_owned(), ObservedEntry::Directory);
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            observe_entry(&entry.path(), &format!("{rel}/{name}"), tree);
        }
        return;
    }
    tree.insert(rel.to_owned(), ObservedEntry::Other);
}

fn assert_reparse(path: &Path) {
    let metadata = fs::symlink_metadata(path).unwrap();
    assert_ne!(
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
        0,
        "fixture is not a reparse point: {}",
        path.display()
    );
}

fn create_junction(link: &Path, target: &Path) {
    let output = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("cmd must launch to create the directory junction fixture");
    assert!(
        output.status.success(),
        "directory junction fixture creation failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn install_reparse_fixture(
    journal: &Path,
    kind: ReparseKind,
    position: Position,
) -> (&'static str, PathBuf) {
    fs::create_dir_all(journal.join("targets/directory")).unwrap();
    fs::write(journal.join("targets/file.bin"), b"target-file").unwrap();
    fs::write(
        journal.join("targets/directory/child.bin"),
        b"target-directory-child",
    )
    .unwrap();
    fs::write(journal.join("sentinel.bin"), b"outside-sentinel").unwrap();

    let (rel, link) = match position {
        Position::Root => ("observed", journal.join("observed")),
        Position::Descendant => {
            fs::create_dir(journal.join("observed")).unwrap();
            fs::write(journal.join("observed/sibling.bin"), b"ordinary-sibling").unwrap();
            ("observed", journal.join("observed/link"))
        }
    };
    match kind {
        ReparseKind::FileSymlink => {
            std::os::windows::fs::symlink_file(journal.join("targets/file.bin"), &link)
                .expect("file-symlink fixture creation must succeed");
        }
        ReparseKind::DirectoryJunction => {
            create_junction(&link, &journal.join("targets/directory"));
        }
    }
    (rel, link)
}

fn assert_unsupported_capture(result: Result<JournalSnapshot, SnapshotError>, expected: &Path) {
    match result {
        Err(SnapshotError::UnsupportedFileType { path }) => assert_eq!(path, expected),
        other => panic!(
            "expected capture UnsupportedFileType at {}, got {other:?}",
            expected.display()
        ),
    }
}

fn assert_unsupported_restore(result: Result<(), SnapshotError>, expected: &Path) {
    match result {
        Err(SnapshotError::UnsupportedFileType { path }) => assert_eq!(path, expected),
        other => panic!(
            "expected restore UnsupportedFileType at {}, got {other:?}",
            expected.display()
        ),
    }
}

fn assert_public_snapshot_round_trips() {
    let temporary = temporary("snapshot-public-");
    let journal = temporary.path();
    fs::write(journal.join("sentinel.bin"), b"sentinel").unwrap();

    let missing = capture_snapshot(journal, "missing").unwrap();
    assert_eq!(
        missing,
        JournalSnapshot::Missing {
            path: "missing".to_owned()
        }
    );
    fs::write(journal.join("missing"), b"remove-me").unwrap();
    restore_snapshot(journal, &missing).unwrap();
    assert!(!journal.join("missing").exists());

    fs::write(journal.join("file.bin"), b"captured-file").unwrap();
    let file = capture_snapshot(journal, "file.bin").unwrap();
    assert_eq!(
        file,
        JournalSnapshot::File(SnapshotFile {
            path: "file.bin".to_owned(),
            bytes: b"captured-file".to_vec(),
            mode: 0,
        })
    );
    fs::remove_file(journal.join("file.bin")).unwrap();
    fs::create_dir(journal.join("file.bin")).unwrap();
    restore_snapshot(journal, &file).unwrap();
    assert_eq!(
        fs::read(journal.join("file.bin")).unwrap(),
        b"captured-file"
    );

    fs::create_dir_all(journal.join("tree/nested")).unwrap();
    fs::write(journal.join("tree/z.bin"), b"z").unwrap();
    fs::write(journal.join("tree/nested/a.bin"), b"a").unwrap();
    let directory = capture_snapshot(journal, "tree").unwrap();
    assert_eq!(
        directory,
        JournalSnapshot::Directory(SnapshotDirectory {
            path: "tree".to_owned(),
            entries: vec![
                JournalSnapshot::Directory(SnapshotDirectory {
                    path: "tree/nested".to_owned(),
                    entries: vec![JournalSnapshot::File(SnapshotFile {
                        path: "tree/nested/a.bin".to_owned(),
                        bytes: b"a".to_vec(),
                        mode: 0,
                    })],
                }),
                JournalSnapshot::File(SnapshotFile {
                    path: "tree/z.bin".to_owned(),
                    bytes: b"z".to_vec(),
                    mode: 0,
                }),
            ],
        })
    );
    fs::remove_dir_all(journal.join("tree")).unwrap();
    fs::write(journal.join("tree"), b"replace-file-with-directory").unwrap();
    restore_snapshot(journal, &directory).unwrap();
    assert_eq!(fs::read(journal.join("tree/z.bin")).unwrap(), b"z");
    assert_eq!(fs::read(journal.join("tree/nested/a.bin")).unwrap(), b"a");
    assert_eq!(fs::read(journal.join("sentinel.bin")).unwrap(), b"sentinel");
}

#[test]
fn windows_snapshot_protocol() {
    assert_public_snapshot_round_trips();

    for operation in [Operation::Capture, Operation::Restore] {
        for kind in [ReparseKind::FileSymlink, ReparseKind::DirectoryJunction] {
            for position in [Position::Root, Position::Descendant] {
                let temporary = temporary("snapshot-reparse-");
                let journal = temporary.path();
                let (rel, link) = install_reparse_fixture(journal, kind, position);
                assert_reparse(&link);
                let before = observe_tree(journal);
                let target_before = observe_tree(&journal.join("targets"));

                match operation {
                    Operation::Capture => {
                        assert_unsupported_capture(capture_snapshot(journal, rel), &link);
                    }
                    Operation::Restore => {
                        assert_unsupported_restore(
                            restore_snapshot(
                                journal,
                                &JournalSnapshot::Missing {
                                    path: rel.to_owned(),
                                },
                            ),
                            &link,
                        );
                    }
                }

                assert_reparse(&link);
                assert_eq!(observe_tree(journal), before);
                assert_eq!(observe_tree(&journal.join("targets")), target_before);
            }
        }
    }

    println!("JOURNAL_WIN_CI_SNAPSHOT=capture/restore/reparse/pass");
}
