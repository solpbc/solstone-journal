// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

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

pub(crate) fn host_packaged_binaries(root: &Path) -> BTreeSet<(String, String)> {
    let mut binaries = BTreeSet::new();
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
        binaries.insert((
            package_name(&manifest_text),
            default_main_binary(&manifest_text),
        ));
    }
    binaries
}
