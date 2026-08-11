// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) struct MaturinLeaf {
    pub(crate) crate_name: String,
    pub(crate) binary_name: String,
    pub(crate) pyproject: PathBuf,
    #[allow(dead_code)] // Used by warm.rs; ci_gate_purity compiles this shared module separately.
    pub(crate) target_family: Option<String>,
}

pub(crate) fn package_name(manifest: &str) -> String {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package && let Some(value) = trimmed.strip_prefix("name = ") {
            return value.trim().trim_matches('"').to_owned();
        }
    }
    panic!("manifest has no package name")
}

fn default_main_binary(manifest: &str) -> String {
    let mut in_bin = false;
    let mut name = None;
    let mut path = None;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_bin && path.as_deref() == Some("src/main.rs") {
                return name.expect("src/main.rs binary must have a name");
            }
            in_bin = trimmed == "[[bin]]";
            name = None;
            path = None;
            continue;
        }
        if !in_bin {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("name = ") {
            name = Some(value.trim().trim_matches('"').to_owned());
        } else if let Some(value) = trimmed.strip_prefix("path = ") {
            path = Some(value.trim().trim_matches('"').to_owned());
        }
    }
    if in_bin && path.as_deref() == Some("src/main.rs") {
        return name.expect("src/main.rs binary must have a name");
    }
    package_name(manifest)
}

fn toml_string(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&format!("{key} = "))
            .map(|value| value.trim().trim_matches('"').to_owned())
    })
}

fn release_target_family(text: &str) -> Option<String> {
    let mut in_release = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_release = trimmed == "[tool.solstone-release]";
            continue;
        }
        if in_release && let Some(value) = trimmed.strip_prefix("target-family = ") {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

pub(crate) fn host_packaged_leaves(root: &Path) -> Vec<MaturinLeaf> {
    let mut leaves = Vec::new();
    for entry in fs::read_dir(root.join("packages")).expect("read packages directory") {
        let pyproject = entry
            .expect("read package entry")
            .path()
            .join("pyproject.toml");
        if !pyproject.is_file() {
            continue;
        }
        let package = fs::read_to_string(&pyproject).expect("read package pyproject");
        if toml_string(&package, "build-backend").as_deref() != Some("maturin")
            || toml_string(&package, "bindings").as_deref() != Some("bin")
        {
            continue;
        }
        let manifest = pyproject
            .parent()
            .expect("package directory")
            .join(toml_string(&package, "manifest-path").expect("maturin manifest path"))
            .canonicalize()
            .expect("canonical Cargo manifest path");
        let manifest_text = fs::read_to_string(manifest).expect("read packaged Cargo manifest");
        leaves.push(MaturinLeaf {
            crate_name: package_name(&manifest_text),
            binary_name: default_main_binary(&manifest_text),
            pyproject,
            target_family: release_target_family(&package),
        });
    }
    leaves.sort_by(|left, right| left.pyproject.cmp(&right.pyproject));
    leaves
}

pub(crate) fn host_packaged_binaries(root: &Path) -> BTreeSet<(String, String)> {
    host_packaged_leaves(root)
        .into_iter()
        .map(|leaf| (leaf.crate_name, leaf.binary_name))
        .collect()
}

#[allow(dead_code)] // Used by warm.rs; ci_gate_purity compiles this shared module separately.
pub(crate) fn macos_native_target_families(root: &Path) -> BTreeSet<String> {
    let source = root.join("scripts/release_package_inventory.py");
    let text = fs::read_to_string(&source).unwrap_or_else(|error| {
        panic!(
            "{}: read macOS package inventory: {error}",
            source.display()
        )
    });
    let header =
        "    @property\n    def macos_native_packages(self) -> tuple[NativePackage, ...]:\n";
    let mut getters = text.match_indices(header);
    let Some((start, _)) = getters.next() else {
        panic!("{}: missing macos_native_packages getter", source.display());
    };
    assert!(
        getters.next().is_none(),
        "{}: macos_native_packages getter must be unique",
        source.display()
    );
    let body_start = start + header.len();
    let body_end = text[body_start..]
        .find("\n\ndef ")
        .map(|offset| body_start + offset)
        .unwrap_or_else(|| {
            panic!(
                "{}: macos_native_packages getter has no closing boundary",
                source.display()
            )
        });
    let body = &text[body_start..body_end];
    let lines = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert!(
        lines.len() == 6
            && lines[0].starts_with("\"\"\"")
            && lines[0].ends_with("\"\"\"")
            && lines[1] == "return tuple("
            && lines[2] == "package"
            && lines[3] == "for package in self.native_packages"
            && lines[5] == ")",
        "{}: macos_native_packages getter must be the direct self.native_packages tuple comprehension",
        source.display()
    );
    let predicate = lines[4];
    let prefix = "if package.target_family in {";
    let literal = predicate
        .strip_prefix(prefix)
        .and_then(|line| line.strip_suffix('}'))
        .unwrap_or_else(|| {
            panic!(
                "{}: macos_native_packages getter must contain exactly one direct `if package.target_family in {{...}}` predicate",
                source.display()
            )
        });
    assert!(
        !literal.is_empty(),
        "{}: macos_native_packages target-family set must not be empty",
        source.display()
    );
    literal
        .split(',')
        .map(|item| item.trim())
        .map(|item| {
            item.strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    panic!(
                        "{}: macos_native_packages target-family set must contain only quoted strings",
                        source.display()
                    )
                })
        })
        .collect()
}
