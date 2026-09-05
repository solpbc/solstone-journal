// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const CLIENT: &str = "solstone-core-retention-client";
const HOME: &str = "solstone-core-home-web";
const JOURNAL_IO: &str = "solstone-core-journal-io";
const RETENTION: &str = "solstone-core-retention";
const ALLOWED_DEPENDENTS: &[&str] = &["solstone-core-home-web"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
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
fn journal_io_deny_wrappers_exactly_match_declared_workspace_parents() {
    let root = repository_root();
    let metadata = metadata(&root);
    let workspace_members = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members array")
        .iter()
        .map(|member| member.as_str().expect("workspace member id"))
        .collect::<BTreeSet<_>>();
    assert!(!workspace_members.is_empty(), "workspace must have members");
    let declared = metadata["packages"]
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
        .collect::<BTreeSet<_>>();

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
    let configured = wrapper_block
        .split('"')
        .filter(|field| *field == "solstone-core" || field.starts_with("solstone-core-"))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert!(
        !configured.is_empty(),
        "journal-io wrappers must not be empty"
    );

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
    assert_eq!(
        dependents, allowed,
        "retention client workspace dependents must exactly match its allowlist"
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

#[test]
fn home_uses_the_bounded_client_without_a_direct_retention_edge() {
    let metadata = metadata(&repository_root());
    let home = metadata["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|package| package["name"] == HOME)
        .expect("home package");
    let dependencies = home["dependencies"].as_array().expect("home dependencies");
    assert!(
        dependencies
            .iter()
            .any(|dependency| dependency["name"] == CLIENT),
        "home must use the bounded retention client"
    );
    assert!(
        dependencies
            .iter()
            .all(|dependency| dependency["name"] != RETENTION),
        "home must not declare retention directly, including for tests"
    );
}

#[test]
fn ac16_settings_adds_no_native_linkage() {
    let root = repository_root();
    let manifest = root.join("core/Cargo.toml");
    let tree = Command::new("cargo")
        .args([
            "tree",
            "--manifest-path",
            manifest.to_str().expect("manifest path is UTF-8"),
            "-p",
            "solstone-core",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--locked",
        ])
        .current_dir(&root)
        .output()
        .expect("cargo tree runs");
    assert!(
        tree.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let closure = String::from_utf8(tree.stdout)
        .expect("cargo tree output is UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let metadata = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            manifest.to_str().expect("manifest path is UTF-8"),
            "--locked",
            "--format-version",
            "1",
        ])
        .current_dir(&root)
        .output()
        .expect("cargo metadata runs");
    assert!(
        metadata.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: Value = serde_json::from_slice(&metadata.stdout).expect("metadata JSON parses");
    let linked = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .filter_map(|package| {
            let name = package["name"].as_str()?;
            (closure.contains(name) && !package["links"].is_null()).then_some(name.to_owned())
        })
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "ffmpeg-sys-next".to_owned(),
        "libsqlite3-sys".to_owned(),
        "ring".to_owned(),
    ]);
    assert_eq!(linked, expected);
}
