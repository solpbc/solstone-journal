// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Remove, then tell the index — proven against a real index, end to end.
//!
//! The unit tests establish that a notification cannot name a path a removal did
//! not produce. This establishes the composition: a real segment, a real SQLite
//! index holding rows for it, a real removal, and then the notification — with the
//! index checked at each step.
//!
//! ⚠ This is the seam the earlier waves could not test: one wave built the outcome
//! model, another the verb, another the index prune. A defect in how they compose
//! would pass every one of their own suites.

#![allow(
    clippy::disallowed_methods,
    reason = "bed teardown; the crate-wide ban exists so only the door reaches a removal \
              primitive in production code, and it fired here, which is it working"
)]

use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_indexer_store::RetentionIndex;
use solstone_core_indexer_store::db::open_index;
use solstone_core_retention::door::{notify_index, remove_segments};
use solstone_core_retention::notify::PruneCounts;
use solstone_core_retention::receipt::Target;
use solstone_core_retention::tombstone::RemovalReason;

fn bed(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "retention-index-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("a bed");
    root
}

fn indexed_row(journal: &Path, rel: &str) {
    let conn = open_index(journal).expect("an index");
    conn.execute("INSERT INTO files(path, mtime) VALUES (?1, 1)", [rel])
        .expect("a file row");
    conn.execute(
        "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
         VALUES ('text', ?1, '20260805', '', '', 'field.audio', 0, '')",
        [rel],
    )
    .expect("a chunk row");
}

fn indexed_paths(journal: &Path) -> Vec<String> {
    let conn = open_index(journal).expect("an index");
    let mut statement = conn
        .prepare("SELECT path FROM files ORDER BY path")
        .expect("a query");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("rows");
    rows.map(|row| row.expect("a path")).collect()
}

/// The whole composition, with the index observed between the steps.
#[test]
fn a_removal_leaves_the_index_stale_until_it_is_told_and_never_before() {
    let journal = bed("ordering");
    let segment = journal.join("chronicle/20260805/field.audio/070000_17");
    fs::create_dir_all(&segment).expect("a segment");
    fs::write(segment.join("audio.flac"), b"raw").expect("raw");
    fs::write(segment.join("audio.jsonl"), b"{}").expect("derived");

    let derived = "chronicle/20260805/field.audio/070000_17/audio.jsonl";
    let sibling = "chronicle/20260805/field.audio/070100_17/audio.jsonl";
    indexed_row(&journal, derived);
    indexed_row(&journal, sibling);
    assert_eq!(indexed_paths(&journal).len(), 2);

    let target = Target {
        day: "20260805".to_owned(),
        stream: "field.audio".to_owned(),
        dir: "070000_17".to_owned(),
    };
    let outcome = remove_segments(
        &journal,
        std::slice::from_ref(&target),
        "2026-08-05T22:00:00Z",
        RemovalReason::OwnerSegmentDelete,
        "sha256:abc",
    );
    assert!(
        outcome.targets[0].not_removed.is_empty(),
        "the removal must succeed: {outcome:?}"
    );

    // 🔴 The index is STALE here, and that is correct. The files are gone and the
    // rows remain; search returns the hit text from the index without opening
    // the file, and a day discovery found nothing in keeps those rows until a full
    // rescan. Telling the index first would hide content that is still on disk.
    assert_eq!(
        indexed_paths(&journal).len(),
        2,
        "nothing may touch the index before the removal is complete"
    );

    let index = RetentionIndex::new(&journal);
    let counts = notify_index(&index, &outcome).expect("the index accepts the notification");
    assert!(counts.files > 0, "the notification cleared file rows");

    let remaining = indexed_paths(&journal);
    assert_eq!(
        remaining,
        vec![sibling.to_owned()],
        "exactly the removed segment left the index"
    );

    fs::remove_dir_all(&journal).expect("teardown");
}

/// Acceptance 2: both chronicle/ and chronicle-free paths, including nested talents/ files.
#[test]
fn a_removal_clears_both_chronicle_shapes_and_nested_talent_files_acceptance_2() {
    let journal = bed("acceptance-2");
    let segment = journal.join("chronicle/20260805/field.audio/070000_17");
    fs::create_dir_all(segment.join("talents")).expect("a segment talents dir");
    fs::write(segment.join("audio.flac"), b"raw").expect("raw");
    fs::write(segment.join("audio.jsonl"), b"{}").expect("derived");
    fs::write(segment.join("talents/audio.md"), b"# Talent").expect("talent");

    let target_rows = [
        "chronicle/20260805/field.audio/070000_17/audio.jsonl",
        "20260805/field.audio/070000_17/audio.jsonl",
        "chronicle/20260805/field.audio/070000_17/talents/audio.md",
        "20260805/field.audio/070000_17/talents/audio.md",
    ];
    let sibling_rows = [
        "chronicle/20260805/field.audio/070100_17/audio.jsonl",
        "20260805/field.audio/070100_17/audio.jsonl",
        "chronicle/20260805/field.audio/070100_17/talents/audio.md",
        "20260805/field.audio/070100_17/talents/audio.md",
    ];

    for row in &target_rows {
        indexed_row(&journal, row);
    }
    for row in &sibling_rows {
        indexed_row(&journal, row);
    }
    assert_eq!(indexed_paths(&journal).len(), 8);

    let target = Target {
        day: "20260805".to_owned(),
        stream: "field.audio".to_owned(),
        dir: "070000_17".to_owned(),
    };
    let outcome = remove_segments(
        &journal,
        std::slice::from_ref(&target),
        "2026-08-05T22:00:00Z",
        RemovalReason::OwnerSegmentDelete,
        "sha256:abc",
    );
    assert!(
        outcome.targets[0].not_removed.is_empty(),
        "removal succeeds: {outcome:?}"
    );

    let index = RetentionIndex::new(&journal);
    let counts = notify_index(&index, &outcome).expect("notify index");
    assert_eq!(counts.files, 4, "all 4 target rows cleared");

    let remaining = indexed_paths(&journal);
    let mut expected_siblings = sibling_rows.to_vec();
    expected_siblings.sort();
    assert_eq!(remaining, expected_siblings);

    fs::remove_dir_all(&journal).expect("teardown");
}

/// A journal with no index is not a failure.
#[test]
fn a_journal_without_an_index_accepts_a_notification_and_gains_none() {
    let journal = bed("no-index");
    let segment = journal.join("chronicle/20260805/field.audio/070000_17");
    fs::create_dir_all(&segment).expect("a segment");
    fs::write(segment.join("audio.flac"), b"raw").expect("raw");

    let outcome = remove_segments(
        &journal,
        &[Target {
            day: "20260805".to_owned(),
            stream: "field.audio".to_owned(),
            dir: "070000_17".to_owned(),
        }],
        "2026-08-05T22:00:00Z",
        RemovalReason::RetentionPolicy,
        "sha256:abc",
    );
    assert!(outcome.targets[0].not_removed.is_empty());

    let index = RetentionIndex::new(&journal);
    assert_eq!(
        notify_index(&index, &outcome).expect("no index is not an error"),
        PruneCounts::default()
    );
    assert!(
        !journal.join("indexer").exists(),
        "a notification must not bring an index into existence"
    );

    fs::remove_dir_all(&journal).expect("teardown");
}

/// A run that removed nothing does not reach the index at all.
#[test]
fn a_run_that_removed_nothing_makes_no_notification() {
    let journal = bed("nothing");
    fs::create_dir_all(journal.join("chronicle/20260805/field.audio")).expect("a stream");
    indexed_row(
        &journal,
        "chronicle/20260805/field.audio/070000_17/audio.jsonl",
    );

    let outcome = remove_segments(
        &journal,
        &[Target {
            day: "20260805".to_owned(),
            stream: "field.audio".to_owned(),
            dir: "nosuch_1".to_owned(),
        }],
        "2026-08-05T22:00:00Z",
        RemovalReason::OwnerSegmentDelete,
        "sha256:abc",
    );
    assert_eq!(outcome.targets[0].not_removed.len(), 1, "refused");

    let index = RetentionIndex::new(&journal);
    assert_eq!(
        notify_index(&index, &outcome).expect("an empty notification is legal"),
        PruneCounts::default()
    );
    assert_eq!(
        indexed_paths(&journal).len(),
        1,
        "a refused removal must not clear an index row"
    );

    fs::remove_dir_all(&journal).expect("teardown");
}
