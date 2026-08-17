// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
const CLIENT_SOURCE: &str = "core/crates/solstone-core-retention-client/src";
const ALLOWED_REEXPORTS: &[&str] = &[
    "Mark",
    "MarkState",
    "Policy",
    "Proposal",
    "RemovalClass",
    "Target",
    "human_bytes",
    "policy_from_retention",
    "policy_would_release",
    "stream_rel",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn sources(directory: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory).expect("retention client source directory reads") {
        let path = entry.expect("retention client source entry reads").path();
        if path.is_dir() {
            paths.extend(sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn client_source(root: &Path) -> String {
    sources(&root.join(CLIENT_SOURCE))
        .into_iter()
        .map(|path| fs::read_to_string(path).expect("retention client source reads"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn reexports(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub use "))
        .map(|line| {
            line.strip_suffix(';')
                .expect("each public reexport is one statement")
                .rsplit("::")
                .next()
                .expect("public reexport has a name")
                .to_owned()
        })
        .collect()
}

#[test]
fn retention_client_source_has_one_bounded_process_path() {
    let source = client_source(&repository_root());
    assert_eq!(
        source.matches("Command::new").count(),
        1,
        "retention client source must contain exactly one Command::new"
    );
    assert_eq!(
        source.matches(".spawn(").count(),
        1,
        "retention client source must contain exactly one child spawn"
    );
    assert_eq!(
        source.matches(".output(").count(),
        0,
        "retention client source must stream child output"
    );
    assert_eq!(
        source.matches("tokio::process").count(),
        0,
        "retention client source must use standard-library process control"
    );
}

#[test]
fn retention_client_reexports_only_its_allowlist() {
    let source = client_source(&repository_root());
    let actual = reexports(&source);
    let expected = ALLOWED_REEXPORTS
        .iter()
        .map(|item| (*item).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected,
        "retention client public reexports must exactly match the allowlist"
    );
}
