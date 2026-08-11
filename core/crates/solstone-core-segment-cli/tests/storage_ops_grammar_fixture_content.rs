// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use solstone_core_segment_cli::run_cli_with;
use tempfile::TempDir;

const FIXTURE: &str = include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn fixture_block(name: &str) -> String {
    let header = format!("=== {name}\n");
    let start = FIXTURE.find(&header).expect("segment fixture block exists") + header.len();
    let rest = &FIXTURE[start..];
    rest[..rest.find("\n=== ").unwrap_or(rest.len())].to_owned()
}

fn help(root: &Path, values: &[&str]) -> String {
    run_cli_with(&arguments(values), root, |_| None, || false).stdout
}

#[test]
fn segment_help_blocks_match_the_storage_operations_fixture_content() {
    let root = TempDir::new().unwrap();
    for (arguments, block) in [
        (["--help"].as_slice(), "segment --help"),
        (["list", "--help"].as_slice(), "segment list --help"),
        (["inspect", "--help"].as_slice(), "segment inspect --help"),
        (["verify", "--help"].as_slice(), "segment verify --help"),
        (["move", "--help"].as_slice(), "segment move --help"),
    ] {
        assert_eq!(help(root.path(), arguments), fixture_block(block));
    }
}
