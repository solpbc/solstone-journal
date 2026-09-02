// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::process::Command;

use solstone_core_journal_io::{AtomicWriteError, AtomicWriteOptions, install_file};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

const PAYLOAD: &[u8] = b"install-file-payload";
const PREVIOUS: &[u8] = b"previous-destination";
const SIBLING: &[u8] = b"unrelated-sibling";

fn temporary(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(label).tempdir().unwrap()
}

fn io_kind(error: &AtomicWriteError) -> io::ErrorKind {
    match error {
        AtomicWriteError::Io { source, .. } => source.kind(),
        AtomicWriteError::PublicationUncertain { .. } => {
            panic!("expected a pre-publication I/O error, got {error}")
        }
    }
}

fn io_display(error: &AtomicWriteError) -> String {
    match error {
        AtomicWriteError::Io { source, .. } => source.to_string(),
        AtomicWriteError::PublicationUncertain { .. } => {
            panic!("expected a pre-publication I/O error, got {error}")
        }
    }
}

fn is_reparse_point(path: &Path) -> bool {
    fs::symlink_metadata(path).unwrap().file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn try_directory_junction(link: &Path, target: &Path) -> bool {
    Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn try_file_symlink(link: &Path, target: &Path) -> bool {
    Command::new("cmd")
        .args(["/d", "/c", "mklink"])
        .arg(link)
        .arg(target)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[test]
fn same_parent_source_replaces_the_destination() {
    let temporary = temporary("install-same-parent-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");
    fs::write(&source, PAYLOAD).unwrap();

    install_file(&source, &destination, AtomicWriteOptions::default()).unwrap();

    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert!(!source.exists());
}

#[test]
fn different_parent_save_tmp_shape_publishes_on_the_same_volume() {
    let temporary = temporary("install-save-tmp-");
    let save_tmp = temporary.path().join("imports").join(".save-tmp");
    let dated = temporary.path().join("imports").join("20240101");
    fs::create_dir_all(&save_tmp).unwrap();
    fs::create_dir_all(&dated).unwrap();
    let source = save_tmp.join("staged.bin");
    let destination = dated.join("upload.bin");
    fs::write(&source, PAYLOAD).unwrap();

    install_file(&source, &destination, AtomicWriteOptions::default()).unwrap();

    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert!(!source.exists());
}

#[test]
fn existing_regular_destination_is_replaced() {
    let temporary = temporary("install-replace-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");
    let sibling = temporary.path().join("sibling.bin");
    fs::write(&source, PAYLOAD).unwrap();
    fs::write(&destination, PREVIOUS).unwrap();
    fs::write(&sibling, SIBLING).unwrap();

    install_file(&source, &destination, AtomicWriteOptions::default()).unwrap();

    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
    assert_eq!(fs::read(&sibling).unwrap(), SIBLING);
    assert!(!source.exists());
}

#[test]
fn zero_byte_source_publishes() {
    let temporary = temporary("install-zero-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");
    fs::write(&source, b"").unwrap();

    install_file(&source, &destination, AtomicWriteOptions::default()).unwrap();

    assert_eq!(fs::read(&destination).unwrap(), b"");
    assert!(!source.exists());
}

#[test]
fn multi_megabyte_source_publishes() {
    let temporary = temporary("install-large-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");
    let payload = vec![0x5A; 2 * 1024 * 1024];
    fs::write(&source, &payload).unwrap();

    install_file(&source, &destination, AtomicWriteOptions::default()).unwrap();

    assert_eq!(fs::read(&destination).unwrap(), payload);
    assert!(!source.exists());
}

#[test]
fn missing_source_is_a_not_found_refusal() {
    let temporary = temporary("install-missing-source-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("missing source must refuse");
    assert_eq!(io_kind(&error), io::ErrorKind::NotFound);
    assert!(io_display(&error).contains("does not exist"), "{error}");
    assert!(!destination.exists());
}

#[test]
fn missing_source_ancestor_is_a_not_found_refusal() {
    let temporary = temporary("install-missing-ancestor-");
    let source = temporary.path().join("absent").join("source.bin");
    let destination = temporary.path().join("destination.bin");

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("missing source ancestor must refuse");
    assert_eq!(io_kind(&error), io::ErrorKind::NotFound);
    assert!(io_display(&error).contains("does not exist"), "{error}");
    assert!(!destination.exists());
    assert!(!temporary.path().join("absent").exists());
}

#[test]
fn not_directory_source_ancestor_is_refused() {
    let temporary = temporary("install-file-ancestor-");
    let blocked = temporary.path().join("blocked");
    fs::write(&blocked, b"not-a-directory").unwrap();
    let source = blocked.join("source.bin");
    let destination = temporary.path().join("destination.bin");

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("file ancestor must refuse");
    assert!(io_display(&error).contains("is not a directory"), "{error}");
    assert!(!destination.exists());
    assert_eq!(fs::read(&blocked).unwrap(), b"not-a-directory");
}

#[test]
fn reparse_source_ancestor_is_refused() {
    let temporary = temporary("install-reparse-ancestor-");
    let real = temporary.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("source.bin"), PAYLOAD).unwrap();
    let link = temporary.path().join("link");
    if !try_directory_junction(&link, &real) {
        eprintln!("skipping reparse source ancestor test: directory junction is not viable here");
        return;
    }
    let source = link.join("source.bin");
    let destination = temporary.path().join("destination.bin");

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("reparse ancestor must refuse");
    assert!(io_display(&error).contains("reparse point"), "{error}");
    assert!(!destination.exists());
    assert_eq!(fs::read(real.join("source.bin")).unwrap(), PAYLOAD);
}

#[test]
fn missing_source_leaf_is_a_not_found_refusal() {
    let temporary = temporary("install-missing-leaf-");
    let nested = temporary.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let source = nested.join("source.bin");
    let destination = temporary.path().join("destination.bin");

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("missing source leaf must refuse");
    assert_eq!(io_kind(&error), io::ErrorKind::NotFound);
    assert!(io_display(&error).contains("does not exist"), "{error}");
    assert!(!destination.exists());
}

#[test]
fn not_regular_source_leaf_is_refused() {
    let temporary = temporary("install-dir-leaf-");
    let source = temporary.path().join("source.bin");
    fs::create_dir(&source).unwrap();
    let destination = temporary.path().join("destination.bin");

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("directory source leaf must refuse");
    assert!(
        io_display(&error).contains("is not a regular file"),
        "{error}"
    );
    assert!(source.is_dir());
    assert!(!destination.exists());
}

#[test]
fn reparse_source_leaf_is_refused() {
    let temporary = temporary("install-reparse-leaf-");
    let target = temporary.path().join("target.bin");
    fs::write(&target, PAYLOAD).unwrap();
    let source = temporary.path().join("source.bin");
    if !try_file_symlink(&source, &target) {
        eprintln!("skipping reparse source leaf test: file symlink is not viable here");
        return;
    }
    let destination = temporary.path().join("destination.bin");

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("reparse source leaf must refuse");
    assert!(io_display(&error).contains("reparse point"), "{error}");
    assert_eq!(fs::read(&target).unwrap(), PAYLOAD);
    assert!(is_reparse_point(&source));
    assert!(!destination.exists());
}

#[test]
fn destination_directory_is_refused_and_only_source_is_removed() {
    let temporary = temporary("install-dest-dir-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");
    let sibling = temporary.path().join("sibling.bin");
    fs::write(&source, PAYLOAD).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(&sibling, SIBLING).unwrap();

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("directory destination must refuse");
    assert!(
        io_display(&error).contains("is not a regular file"),
        "{error}"
    );
    assert!(!source.exists());
    assert!(destination.is_dir());
    assert_eq!(fs::read(&sibling).unwrap(), SIBLING);
}

#[test]
fn destination_reparse_is_refused_and_only_source_is_removed() {
    let temporary = temporary("install-dest-reparse-");
    let target = temporary.path().join("target.bin");
    fs::write(&target, PREVIOUS).unwrap();
    let destination = temporary.path().join("destination.bin");
    if !try_file_symlink(&destination, &target) {
        eprintln!("skipping dest reparse test: file symlink is not viable here");
        return;
    }
    let source = temporary.path().join("source.bin");
    let sibling = temporary.path().join("sibling.bin");
    fs::write(&source, PAYLOAD).unwrap();
    fs::write(&sibling, SIBLING).unwrap();

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("reparse destination must refuse");
    assert!(io_display(&error).contains("reparse point"), "{error}");
    assert!(!source.exists());
    assert!(is_reparse_point(&destination));
    assert_eq!(fs::read(&target).unwrap(), PREVIOUS);
    assert_eq!(fs::read(&sibling).unwrap(), SIBLING);
}

#[test]
fn alias_is_refused_without_deleting_the_source() {
    let temporary = temporary("install-alias-");
    let path = temporary.path().join("same.bin");
    fs::write(&path, PAYLOAD).unwrap();

    let error =
        install_file(&path, &path, AtomicWriteOptions::default()).expect_err("alias must refuse");
    assert_eq!(io_kind(&error), io::ErrorKind::InvalidInput);
    assert!(
        io_display(&error).contains("source and destination name the same file"),
        "{error}"
    );
    assert_eq!(fs::read(&path).unwrap(), PAYLOAD);
}

#[test]
fn substitution_is_refused_without_deleting_the_source() {
    let temporary = temporary("install-substitution-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");
    fs::write(&source, PAYLOAD).unwrap();
    if let Err(error) = fs::hard_link(&source, &destination) {
        eprintln!("skipping substitution test: hard link is not viable here ({error})");
        return;
    }

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("substitution must refuse");
    assert_eq!(io_kind(&error), io::ErrorKind::InvalidInput);
    assert!(
        io_display(&error).contains("destination already identifies the source file"),
        "{error}"
    );
    assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
    assert_eq!(fs::read(&destination).unwrap(), PAYLOAD);
}

#[test]
fn oversized_mode_is_refused_with_zero_mutation() {
    let temporary = temporary("install-mode-");
    let parent = temporary.path().join("missing");
    let source = temporary.path().join("source.bin");
    let destination = parent.join("destination.bin");
    fs::write(&source, PAYLOAD).unwrap();

    let error = install_file(
        &source,
        &destination,
        AtomicWriteOptions { mode: Some(0o1000) },
    )
    .expect_err("mode above 0o777 must refuse");
    assert_eq!(io_kind(&error), io::ErrorKind::InvalidInput);
    assert!(io_display(&error).contains("mode exceeds 0o777"), "{error}");
    assert_eq!(fs::read(&source).unwrap(), PAYLOAD);
    assert!(!parent.exists());
    assert!(!destination.exists());
}

#[test]
fn pre_move_destination_directory_cleans_only_the_source() {
    let temporary = temporary("install-cleanup-dir-");
    let source = temporary.path().join("source.bin");
    let destination = temporary.path().join("destination.bin");
    let sibling = temporary.path().join("keep.bin");
    fs::write(&source, PAYLOAD).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("inside.bin"), b"inside").unwrap();
    fs::write(&sibling, SIBLING).unwrap();

    let error = install_file(&source, &destination, AtomicWriteOptions::default())
        .expect_err("directory destination must refuse");
    assert!(
        io_display(&error).contains("is not a regular file"),
        "{error}"
    );
    assert!(!source.exists());
    assert_eq!(fs::read(destination.join("inside.bin")).unwrap(), b"inside");
    assert_eq!(fs::read(&sibling).unwrap(), SIBLING);
}
