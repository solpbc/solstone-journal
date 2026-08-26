// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

mod common;

use std::fs;

use common::{TempDir, directory, journal, write};
use solstone_core_journal_archive::{ArchiveError, ArchiveSource, JournalEntryKind};

#[test]
fn counts_use_immediate_structural_declarations_only() {
    let temporary = TempDir::new("counts");
    let root = journal(&temporary);
    directory(&root, "chronicle/20260101");
    directory(&root, "chronicle/20261332");
    directory(&root, "chronicle/2026010x");
    directory(&root, "chronicle/202601011");
    write(&root, "entities/a/entity.json", b"{}");
    write(&root, "entities/a/nested/entity.json", b"{}");
    write(&root, "entities/b/not-entity.json", b"{}");
    write(&root, "facets/work/facet.json", b"{}");
    write(&root, "facets/work/nested/facet.json", b"{}");

    let source = ArchiveSource::open(&root).expect("open source");
    assert_eq!(source.inventory().day_count(), 2);
    assert_eq!(source.inventory().entity_count(), 1);
    assert_eq!(source.inventory().facet_count(), 1);
}

#[cfg(unix)]
#[test]
#[allow(clippy::disallowed_methods)]
fn special_day_candidate_is_rejected_as_an_unsafe_member() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    let temporary = TempDir::new("unsafe-day");
    let root = journal(&temporary);
    fs::create_dir(root.join("chronicle")).expect("create chronicle");
    mkfifo(&root.join("chronicle/20260101"), Mode::S_IRUSR).expect("create fifo");

    assert!(matches!(
        ArchiveSource::open(&root),
        Err(ArchiveError::UnsafeJournalEntry {
            kind: JournalEntryKind::Fifo,
            ..
        })
    ));
}
