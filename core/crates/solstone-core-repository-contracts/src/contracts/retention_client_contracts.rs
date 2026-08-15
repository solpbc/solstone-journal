// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const CLIENT: &str = "solstone-core-retention-client";
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
const ALLOWED_DEPENDENTS: &[&str] = &["solstone-core-home-web"];

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

fn metadata(root: &Path) -> Value {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            "core/Cargo.toml",
            "--locked",
            "--format-version",
            "1",
            "--no-deps",
        ])
        .current_dir(root)
        .output()
        .expect("locked workspace cargo metadata runs");
    assert!(
        output.status.success(),
        "locked workspace cargo metadata must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("workspace cargo metadata parses")
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
fn retention_client_has_no_unapproved_workspace_dependents() {
    let root = repository_root();
    let metadata = metadata(&root);
    let workspace_members = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members array")
        .iter()
        .map(|member| member.as_str().expect("workspace member id"))
        .collect::<BTreeSet<_>>();
    let dependents = metadata["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .filter(|package| {
            workspace_members.contains(package["id"].as_str().expect("package id"))
                && package["dependencies"]
                    .as_array()
                    .expect("package dependencies array")
                    .iter()
                    .any(|dependency| dependency["name"] == CLIENT)
        })
        .map(|package| package["name"].as_str().expect("package name").to_owned())
        .collect::<BTreeSet<_>>();
    let allowed = ALLOWED_DEPENDENTS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let unexpected = dependents.difference(&allowed).cloned().collect::<Vec<_>>();
    assert!(
        unexpected.is_empty(),
        "retention client has workspace dependents outside its allowlist: {unexpected:?}"
    );

    let tree = Command::new("cargo")
        .args([
            "tree",
            "--manifest-path",
            "core/Cargo.toml",
            "--locked",
            "-p",
            CLIENT,
            "--edges",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(root)
        .output()
        .expect("locked workspace cargo tree runs");
    assert!(
        tree.status.success(),
        "locked workspace cargo tree must succeed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
}
