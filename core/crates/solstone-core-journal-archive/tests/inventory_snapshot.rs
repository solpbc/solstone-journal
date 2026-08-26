// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

mod common;

use common::{TempDir, valid_four_root_journal, write};
use solstone_core_journal_archive::ArchiveSource;

#[test]
fn empty_journal_has_an_empty_frozen_inventory() {
    let temporary = TempDir::new("empty-snapshot");
    let root = common::journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open empty source");

    assert!(source.inventory().entries().is_empty());
    assert!(source.inventory().skipped_root_names().is_empty());
    assert_eq!(source.inventory().day_count(), 0);
    assert_eq!(source.inventory().entity_count(), 0);
    assert_eq!(source.inventory().facet_count(), 0);
}

#[test]
fn inventory_is_frozen_and_has_fixed_root_then_member_order() {
    let temporary = TempDir::new("snapshot");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");

    let members: Vec<(&str, u64)> = source
        .inventory()
        .entries()
        .iter()
        .map(|entry| (entry.member_name().as_str(), entry.size()))
        .collect();
    assert_eq!(
        members,
        vec![
            ("chronicle/20260101/a.txt", 1),
            ("chronicle/20260101/nested/b.txt", 2),
            ("entities/alice/entity.json", 2),
            ("facets/work/facet.json", 2),
            ("imports/import-1/source.bin", 6),
        ]
    );
    assert_eq!(source.inventory().day_count(), 1);
    assert_eq!(source.inventory().entity_count(), 1);
    assert_eq!(source.inventory().facet_count(), 1);
    assert_eq!(
        source
            .inventory()
            .skipped_root_names()
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>(),
        vec!["config"]
    );
    assert_eq!(
        source
            .inventory()
            .included_root_names()
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>(),
        vec!["chronicle", "entities", "facets", "imports"]
    );

    write(&root, "chronicle/20260102/later.txt", b"later");
    write(&root, "imports/import-2/later.bin", b"later");
    assert_eq!(source.inventory().entries().len(), 5);
    assert_eq!(source.inventory().day_count(), 1);
}

#[test]
fn health_durable_files_are_inventoried_and_brain_key_is_not() {
    let temporary = TempDir::new("health-snapshot");
    let root = valid_four_root_journal(&temporary);
    write(&root, "health/pruning-runs/x.jsonl", b"audit");
    write(&root, "health/brain.json", b"{}");
    write(&root, "apps/observer/x.json", b"{}");
    let source = ArchiveSource::open(&root).expect("open source");
    let members: Vec<&str> = source
        .inventory()
        .entries()
        .iter()
        .map(|entry| entry.member_name().as_str())
        .collect();
    assert!(members.contains(&"health/pruning-runs/x.jsonl"));
    assert!(!members.contains(&"health/brain.json"));
    assert!(!members.iter().any(|name| name.starts_with("apps/")));
    assert!(
        source
            .inventory()
            .skipped_root_names()
            .iter()
            .any(|name| name.as_str() == "apps")
    );
}
