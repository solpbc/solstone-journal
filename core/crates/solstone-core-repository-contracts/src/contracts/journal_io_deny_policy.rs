// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const JOURNAL_IO: &str = "solstone-core-journal-io";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn declared_journal_io_parents(root: &Path) -> BTreeSet<String> {
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
    let metadata: Value =
        serde_json::from_slice(&output.stdout).expect("workspace cargo metadata parses");
    let workspace_members = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members array")
        .iter()
        .map(|member| member.as_str().expect("workspace member id"))
        .collect::<BTreeSet<_>>();
    assert!(!workspace_members.is_empty(), "workspace must have members");

    metadata["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .filter(|package| {
            workspace_members.contains(package["id"].as_str().expect("package id"))
                && package["dependencies"]
                    .as_array()
                    .expect("package dependencies array")
                    .iter()
                    .any(|dependency| dependency["name"] == JOURNAL_IO)
        })
        .map(|package| package["name"].as_str().expect("package name").to_owned())
        .collect()
}

fn configured_journal_io_wrappers(root: &Path) -> BTreeSet<String> {
    let deny = fs::read_to_string(root.join("core/deny.toml")).expect("core deny policy reads");
    let marker = format!("{{ name = \"{JOURNAL_IO}\", wrappers = [");
    assert_eq!(
        deny.matches(&marker).count(),
        1,
        "deny policy must contain exactly one journal-io wrapper entry"
    );
    let tail = deny
        .split_once(&marker)
        .map(|(_, tail)| tail)
        .expect("journal-io wrapper entry starts");
    let wrapper_block = tail
        .split_once("], reason =")
        .map(|(wrappers, _)| wrappers)
        .expect("journal-io wrapper entry ends before its reason");
    let wrappers = wrapper_block
        .split('"')
        .filter(|field| *field == "solstone-core" || field.starts_with("solstone-core-"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert!(
        !wrappers.is_empty(),
        "journal-io wrappers must not be empty"
    );
    wrappers
}

#[test]
fn journal_io_deny_wrappers_exactly_match_declared_workspace_parents() {
    let root = repository_root();
    let declared = declared_journal_io_parents(&root);
    let configured = configured_journal_io_wrappers(&root);
    let missing = declared
        .difference(&configured)
        .cloned()
        .collect::<Vec<_>>();
    let stale = configured
        .difference(&declared)
        .cloned()
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty() && stale.is_empty(),
        "journal-io wrappers must exactly match declared workspace parents\nmissing: {missing:?}\nstale: {stale:?}"
    );
}
