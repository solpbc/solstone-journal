// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const FIXTURE: &str = include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");

#[test]
fn storage_ops_grammar_fixture_has_expected_blocks() {
    let blocks = FIXTURE
        .lines()
        .filter_map(|line| line.strip_prefix("=== "))
        .collect::<Vec<_>>();
    let expected = [
        "streams --help",
        "segment --help",
        "segment list --help",
        "segment inspect --help",
        "segment verify --help",
        "segment move --help",
        "journal-stats --help",
        "reprocess --help",
        "reprocess (missing day)",
        "backfill-processing-records --help",
        "misuse exit codes",
    ];

    assert_eq!(blocks, expected, "unexpected fixture blocks: {blocks:?}");
}
