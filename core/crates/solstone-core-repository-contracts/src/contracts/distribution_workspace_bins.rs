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

fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && let Some(value) = trimmed.strip_prefix("name = ") {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

fn autobins_enabled(manifest: &str) -> bool {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && let Some(value) = trimmed.strip_prefix("autobins") {
            return !value
                .split('=')
                .nth(1)
                .is_some_and(|item| item.contains("false"));
        }
    }
    true
}

struct ExplicitBin {
    name: String,
    path: Option<String>,
}

fn explicit_bins(manifest: &str) -> Vec<ExplicitBin> {
    let mut bins = Vec::new();
    let mut in_bin = false;
    let mut name = None;
    let mut path = None;
    let flush =
        |bins: &mut Vec<ExplicitBin>, name: &mut Option<String>, path: &mut Option<String>| {
            if let Some(name) = name.take() {
                bins.push(ExplicitBin {
                    name,
                    path: path.take(),
                });
            } else {
                path.take();
            }
        };
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_bin {
                flush(&mut bins, &mut name, &mut path);
            }
            in_bin = trimmed == "[[bin]]";
            continue;
        }
        if !in_bin {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("name = ") {
            name = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("path = ") {
            path = Some(value.trim().trim_matches('"').replace('\\', "/"));
        }
    }
    if in_bin {
        flush(&mut bins, &mut name, &mut path);
    }
    bins
}

fn package_bins(member_dir: &Path, manifest: &str) -> BTreeSet<String> {
    let explicit = explicit_bins(manifest);
    let mut names = explicit
        .iter()
        .map(|bin| bin.name.clone())
        .collect::<BTreeSet<_>>();
    if !autobins_enabled(manifest) {
        return names;
    }
    let claimed = explicit
        .iter()
        .filter_map(|bin| bin.path.clone())
        .collect::<BTreeSet<_>>();
    if member_dir.join("src/main.rs").is_file() && !claimed.contains("src/main.rs") {
        if let Some(name) = package_name(manifest) {
            names.insert(name);
        }
    }
    let bin_dir = member_dir.join("src/bin");
    let Ok(entries) = fs::read_dir(&bin_dir) else {
        return names;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            let relative = format!(
                "src/bin/{}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            if claimed.contains(&relative) {
                continue;
            }
            if let Some(stem) = path.file_stem() {
                names.insert(stem.to_string_lossy().into_owned());
            }
        } else if path.is_dir() && path.join("main.rs").is_file() {
            let relative = format!(
                "src/bin/{}/main.rs",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            if !claimed.contains(&relative) {
                names.insert(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
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
        let manifest_path = root.join("core").join(&member).join("Cargo.toml");
        let Ok(manifest) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        names.extend(package_bins(&root.join("core").join(member), &manifest));
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
    for required in [
        "solstone-core",
        "solstone-core-depict",
        "solstone-core-describe",
        "solstone-core-speakers-analyze",
        "solstone-core-vad-analyze",
    ] {
        assert!(
            workspace_bins.contains(required),
            "implicit bin {required} must be visible to the allow-list"
        );
    }
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

#[test]
fn planted_implicit_bin_is_detected() {
    let root = tempfile::tempdir().expect("planted member");
    let member = root.path();
    fs::create_dir_all(member.join("src")).expect("src");
    fs::write(member.join("src/main.rs"), "fn main() {}\n").expect("main");
    let manifest = "[package]\nname = \"solstone-audit-planted\"\n";
    let bins = package_bins(member, manifest);
    assert!(
        bins.contains("solstone-audit-planted"),
        "src/main.rs must count as the package bin"
    );
    let inventory = fs::read_to_string(repository_root().join(INVENTORY)).expect("inventory");
    let admitted = inventory_bins(&inventory);
    let denied = inventory_denies(&inventory);
    let covered = admitted.union(&denied).cloned().collect::<BTreeSet<_>>();
    assert!(
        !covered.contains("solstone-audit-planted"),
        "planted implicit bin must be absent from the allow-list"
    );
    let missing = bins.difference(&covered).cloned().collect::<BTreeSet<_>>();
    assert!(missing.contains("solstone-audit-planted"));
}
