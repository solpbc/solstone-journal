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
    "policy_from_journal_config",
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

#[test]
fn solstone_retention_binary_invocation_is_strictly_controlled() {
    let root = repository_root();
    let crates_dir = root.join("core/crates");
    let mut found_spawners = BTreeSet::new();

    for entry in fs::read_dir(&crates_dir).expect("read crates dir") {
        let entry = entry.expect("crate entry");
        let src_dir = entry.path().join("src");
        if !src_dir.is_dir() {
            continue;
        }
        for file in sources(&src_dir) {
            let filename = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if filename.ends_with("tests.rs") || filename.starts_with("test_") {
                continue;
            }
            let rel = file
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel.contains("/solstone-core-retention-cli/") {
                continue;
            }
            let content = fs::read_to_string(&file).expect("read file");
            if content.contains("\"solstone-retention\"") {
                found_spawners.insert(rel.clone());
                assert!(
                    !content.contains("release-raw"),
                    "{rel} must not invoke or name `release-raw`"
                );
                assert!(
                    !content.contains("sweep"),
                    "{rel} must not invoke or name `sweep`"
                );
            }
        }
    }

    let expected_spawners = BTreeSet::from([
        "core/crates/solstone-core-retention-client/src/lib.rs".to_owned(),
        "core/crates/solstone-core-settings-web/src/retention_executor.rs".to_owned(),
        "core/crates/solstone-core/src/warm.rs".to_owned(),
    ]);

    assert_eq!(
        found_spawners, expected_spawners,
        "solstone-retention binary references in production sources must match exact allowlist"
    );
}
