// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const INVENTORY: &str = "core/distribution/inventory.toml";
const WORKSPACE: &str = "core/Cargo.toml";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn format_named_list(label: &str, names: &BTreeSet<String>) -> String {
    let mut lines = vec![format!("{label}:")];
    for name in names {
        lines.push(format!("  {name}"));
    }
    lines.join("\n")
}

fn workspace_members(workspace: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut in_members = false;
    for line in workspace.lines() {
        let trimmed = line.trim();
        if trimmed == "members = [" {
            in_members = true;
            continue;
        }
        if in_members && trimmed == "]" {
            break;
        }
        if in_members {
            let value = trimmed.trim_end_matches(',').trim_matches('"');
            if !value.is_empty() {
                members.push(value.to_owned());
            }
        }
    }
    members
}

fn explicit_binary_names(manifest: &str) -> BTreeSet<String> {
    let mut in_bin = false;
    let mut names = BTreeSet::new();
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_bin = trimmed == "[[bin]]";
            continue;
        }
        if in_bin && let Some(value) = trimmed.strip_prefix("name = ") {
            names.insert(value.trim().trim_matches('"').to_owned());
        }
    }
    names
}

fn inventory_bins(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut kind_bin = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[[") {
            kind_bin = false;
            continue;
        }
        if trimmed == "kind = \"bin\"" {
            kind_bin = true;
            continue;
        }
        if kind_bin && let Some(value) = trimmed.strip_prefix("bin = ") {
            names.insert(value.trim().trim_matches('"').to_owned());
        }
        if trimmed.starts_with("[[deny]]") {
            kind_bin = false;
        }
    }
    names
}

fn inventory_denies(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut in_deny = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[[") {
            in_deny = trimmed == "[[deny]]";
            continue;
        }
        if in_deny && let Some(value) = trimmed.strip_prefix("bin = ") {
            names.insert(value.trim().trim_matches('"').to_owned());
        }
    }
    names
}

fn collect_workspace_bins(root: &Path) -> BTreeSet<String> {
    let workspace = fs::read_to_string(root.join(WORKSPACE)).expect("read workspace manifest");
    let mut names = BTreeSet::new();
    for member in workspace_members(&workspace) {
        let manifest_path = root.join("core").join(member).join("Cargo.toml");
        let Ok(manifest) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        names.extend(explicit_binary_names(&manifest));
    }
    names
}

#[test]
fn every_workspace_bin_is_inventory_entry_or_deny() {
    let root = repository_root();
    let inventory = fs::read_to_string(root.join(INVENTORY)).expect("read inventory");
    let admitted = inventory_bins(&inventory);
    let denied = inventory_denies(&inventory);
    let covered = admitted.union(&denied).cloned().collect::<BTreeSet<_>>();
    let workspace_bins = collect_workspace_bins(&root);
    let missing = workspace_bins
        .difference(&covered)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(
        missing.is_empty(),
        "{}",
        format_named_list("missing required", &missing)
    );
}
