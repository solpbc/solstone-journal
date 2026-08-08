// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod common;

// Root resolution intentionally canonicalizes once and may follow a root symlink.
// Only traversal beneath that acquired root must remain swap-proof.

use std::fs;
use std::io::Read;

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

    source
        .revalidate(inventory_entry)
        .expect("validate retained root");
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
