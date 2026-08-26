// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

mod common;

// Before the authoritative open, replacing the requested root or its symlink target is accepted.
// After it, replacing any canonical component that has not yet been opened is rejected.

use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use common::{TempDir, entry, valid_four_root_journal, write};
use solstone_core_journal_archive::ArchiveSource;

#[test]
#[allow(clippy::disallowed_methods)]
fn replacement_before_open_is_the_current_requested_journal() {
    let temporary = TempDir::new("replacement-before-open");
    let root = common::journal(&temporary);
    write(&root, "imports/import-1/source.bin", b"first");
    fs::remove_dir_all(&root).expect("remove first journal before open");
    fs::create_dir(&root).expect("create replacement journal");
    write(&root, "imports/import-1/source.bin", b"second");

    let source = ArchiveSource::open(&root).expect("open replacement journal");
    let inventory_entry = entry(&source, "imports/import-1/source.bin");
    let mut bytes = Vec::new();
    source
        .open_file(inventory_entry)
        .expect("open replacement member")
        .into_file()
        .read_to_end(&mut bytes)
        .expect("read replacement member");
    assert_eq!(bytes, b"second");
}

#[test]
#[allow(clippy::disallowed_methods)]
fn retained_root_descriptor_survives_namespace_rename() {
    let temporary = TempDir::new("rooted-open");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");
    let inventory_entry = entry(&source, "imports/import-1/source.bin");
    let moved = temporary.path().join("journal-moved");
    fs::rename(&root, &moved).expect("rename journal namespace entry");
    fs::create_dir(&root).expect("create replacement root");
    write(&root, "imports/import-1/source.bin", b"replacement");

    source.revalidate().expect("validate retained root");
    let opened = source
        .open_file(inventory_entry)
        .expect("open through retained root");
    assert_eq!(opened.inventoried_size(), 6);
    let mut bytes = Vec::new();
    opened
        .into_file()
        .read_to_end(&mut bytes)
        .expect("read retained file");
    assert_eq!(bytes, b"source");
}

#[test]
#[allow(clippy::disallowed_methods)]
fn canonical_source_matches_verified_path_for_plain_root() {
    let temporary = TempDir::new("canonical-plain");
    let root = valid_four_root_journal(&temporary);
    let expected = fs::canonicalize(&root).expect("canonicalize journal root");

    let source = ArchiveSource::open(&root).expect("open source");

    assert_eq!(source.canonical_source(), expected);
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn canonical_source_matches_verified_path_for_symlink_root() {
    let temporary = TempDir::new("canonical-symlink");
    let target = temporary.path().join("target");
    fs::create_dir(&target).expect("create target journal");
    write(&target, "imports/import-1/source.bin", b"source");
    let requested = temporary.path().join("requested");
    symlink(&target, &requested).expect("create requested root symlink");
    let expected = fs::canonicalize(&target).expect("canonicalize target journal");

    let source = ArchiveSource::open(&requested).expect("open symlink source");

    assert_eq!(source.canonical_source(), expected);
}

#[test]
#[allow(clippy::disallowed_methods)]
fn canonical_source_survives_later_ancestor_rename() {
    let temporary = TempDir::new("canonical-rename");
    let ancestor = temporary.path().join("ancestor");
    let root = ancestor.join("journal");
    fs::create_dir(&ancestor).expect("create journal ancestor");
    fs::create_dir(&root).expect("create journal root");
    write(&root, "imports/import-1/source.bin", b"source");
    let expected = fs::canonicalize(&root).expect("canonicalize source root");
    let source = ArchiveSource::open(&root).expect("open source");
    let moved = temporary.path().join("ancestor-moved");

    fs::rename(&ancestor, &moved).expect("rename source ancestor");

    assert_eq!(source.canonical_source(), expected);
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn replacement_of_root_symlink_target_before_open_is_accepted() {
    let temporary = TempDir::new("symlink-target-before-open");
    let target = temporary.path().join("target");
    fs::create_dir(&target).expect("create initial target journal");
    write(&target, "imports/import-1/source.bin", b"first");
    let requested = temporary.path().join("requested");
    symlink(&target, &requested).expect("create requested root symlink");
    fs::remove_dir_all(&target).expect("remove initial target journal");
    fs::create_dir(&target).expect("create replacement target journal");
    write(&target, "imports/import-1/source.bin", b"second");

    let source = ArchiveSource::open(&requested).expect("open replacement symlink target");
    let inventory_entry = entry(&source, "imports/import-1/source.bin");
    let mut bytes = Vec::new();
    source
        .open_file(inventory_entry)
        .expect("open replacement member")
        .into_file()
        .read_to_end(&mut bytes)
        .expect("read replacement member");
    assert_eq!(bytes, b"second");
}
