// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep archive, backup, and ownership declarations aligned for endpoint key material.

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn backup_excludes_section(source: &str) -> &str {
    let start = source
        .find("pub const BACKUP_EXCLUDES")
        .expect("BACKUP_EXCLUDES declaration exists");
    let after_start = &source[start..];
    let end = after_start
        .find("];\n")
        .expect("BACKUP_EXCLUDES declaration closes");
    &after_start[..end]
}

#[test]
fn endpoint_exclusions_and_ownership_remain_coherent() {
    let archive = read_repo_file("core/crates/solstone-core-journal-archive/src/deny.rs");
    assert!(
        archive.contains("\"mcp-endpoint/\""),
        "PORTABLE_DENY must prune the top-level mcp-endpoint/ tree"
    );

    let backup = read_repo_file("core/crates/solstone-core-backup-runtime/src/engine.rs");
    assert!(
        backup.contains("resolved_journal.join(\"mcp-endpoint\")"),
        "backup_args must derive an mcp-endpoint exclusion from the resolved journal path"
    );
    assert!(
        backup.contains("\"--exclude\""),
        "backup_args must pass the resolved mcp-endpoint path as a restic exclusion"
    );
    assert!(
        !backup_excludes_section(&backup).contains("\"mcp-endpoint\""),
        "BACKUP_EXCLUDES must not use a bare mcp-endpoint basename exclusion"
    );

    let ownership = read_repo_file("AGENTS.md");
    let row = ownership
        .lines()
        .find(|line| line.contains("mcp-endpoint/**"))
        .expect("AGENTS.md must declare mcp-endpoint/** ownership");
    assert!(
        row.contains("solstone-core-mcp-endpoint"),
        "mcp-endpoint ownership row must name solstone-core-mcp-endpoint: {row}"
    );
}
