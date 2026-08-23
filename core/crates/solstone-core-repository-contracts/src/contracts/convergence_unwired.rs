// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! AC10: no non-test production target references the convergence store.
//!
//! The next lode retires this check when a production crate takes a normal
//! dependency on `solstone-core-journal-convergence`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

const STORE: &str = "solstone-core-journal-convergence";
const HARNESS: &str = "solstone-core-journal-convergence-harness";
const RUST_NAME: &str = "solstone_core_journal_convergence";
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

fn allowed_source(relative: &str) -> bool {
    relative.starts_with("core/crates/solstone-core-journal-convergence/")
        || relative.starts_with("core/crates/solstone-core-journal-convergence-harness/")
        || relative.contains("/contracts/")
}

fn strip_tests_and_comments(text: &str) -> String {
    let without_line_comments = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let marker = "#[cfg(test)]";
    let mut kept = String::new();
    let mut rest = without_line_comments.as_str();
    while let Some(index) = rest.find(marker) {
        kept.push_str(&rest[..index]);
        rest = &rest[index + marker.len()..];
        if let Some(brace) = rest.find('{') {
            let mut depth = 0_i32;
            let mut end = None;
            for (offset, ch) in rest[brace..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(brace + offset + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = match end {
                Some(end) => &rest[end..],
                None => "",
            };
        }
    }
    kept.push_str(rest);
    kept
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, found);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            found.push(path);
        }
    }
}

fn scan_source(root: &Path) -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_files(&root.join("core/crates"), &mut files);
    let mut unexpected = BTreeSet::new();
    for file in files {
        let Ok(relative) = file.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if allowed_source(&relative) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let text = strip_tests_and_comments(&text);
        if text.contains(RUST_NAME) {
            let crate_name = relative
                .strip_prefix("core/crates/")
                .unwrap_or(&relative)
                .split('/')
                .next()
                .unwrap_or(&relative);
            unexpected.insert(format!(
                "{crate_name} names {RUST_NAME} in {relative}; AC10 retires this check when a production crate takes a normal dependency on {STORE}"
            ));
        }
    }
    unexpected
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<Dep>,
}

#[derive(Deserialize)]
struct Dep {
    pkg: String,
}

fn cargo_metadata(manifest: &Path) -> Result<Metadata, String> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--locked",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .map_err(|error| format!("cargo metadata could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed (exit {}): {}",
            output.status.code().unwrap_or(2),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata JSON did not parse: {error}"))
}

fn depending_crates(metadata: &Metadata) -> BTreeSet<String> {
    let members: BTreeSet<String> = metadata.workspace_members.iter().cloned().collect();
    let names: BTreeMap<&str, &str> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package.name.as_str()))
        .collect();
    let Some(store_id) = metadata
        .packages
        .iter()
        .find(|package| package.name == STORE && members.contains(&package.id))
        .map(|package| package.id.as_str())
    else {
        return BTreeSet::new();
    };
    let mut dependents = BTreeSet::new();
    for node in &metadata.resolve.nodes {
        if !members.contains(&node.id) {
            continue;
        }
        let Some(from) = names.get(node.id.as_str()) else {
            continue;
        };
        if *from == STORE {
            continue;
        }
        if node.deps.iter().any(|dep| dep.pkg == store_id) {
            dependents.insert((*from).to_owned());
        }
    }
    dependents
}

#[test]
fn only_the_harness_depends_on_the_convergence_store() {
    let root = repository_root();
    let metadata = cargo_metadata(&root.join(WORKSPACE)).expect("cargo metadata");
    let dependents = depending_crates(&metadata);
    let unexpected: BTreeSet<String> = dependents
        .into_iter()
        .filter(|name| name != HARNESS)
        .map(|name| {
            format!(
                "{name} depends on {STORE}; AC10 retires this check when a production crate takes a normal dependency on {STORE}"
            )
        })
        .collect();
    assert!(
        unexpected.is_empty(),
        "{}",
        format_named_list("unexpected store dependents", &unexpected)
    );
}

#[test]
fn production_source_does_not_name_the_convergence_store() {
    let unexpected = scan_source(&repository_root());
    assert!(
        unexpected.is_empty(),
        "{}",
        format_named_list("unexpected store references", &unexpected)
    );
}
