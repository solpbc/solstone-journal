// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Repository-owned contracts for the native speaker and VAD package boundaries.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn read_repo_file(relative: &str) -> String {
    let path = repository_root().join(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

pub(super) fn dependency_keys(manifest: &str) -> BTreeSet<String> {
    let manifest = manifest
        .parse::<toml_edit::DocumentMut>()
        .expect("parse Cargo manifest");
    let mut active = BTreeSet::new();
    let mut pending = vec!["default".to_owned()];
    while let Some(feature) = pending.pop() {
        if !active.insert(feature.clone()) {
            continue;
        }
        if let Some(members) = manifest
            .get("features")
            .and_then(|f| f.get(&feature))
            .and_then(toml_edit::Item::as_array)
        {
            pending.extend(
                members
                    .iter()
                    .map(|m| m.as_str().expect("feature member").to_owned()),
            );
        }
    }
    fn add(
        table: Option<&toml_edit::Item>,
        active: &BTreeSet<String>,
        keys: &mut BTreeSet<String>,
    ) {
        if let Some(table) = table.and_then(toml_edit::Item::as_table_like) {
            for (key, value) in table.iter() {
                let optional =
                    value.get("optional").and_then(toml_edit::Item::as_bool) == Some(true);
                if !optional
                    || active.contains(key)
                    || active.contains(&format!("dep:{key}"))
                    || active.iter().any(|f| f.starts_with(&format!("{key}/")))
                {
                    keys.insert(key.to_owned());
                }
            }
        }
    }
    let mut keys = BTreeSet::new();
    add(manifest.get("dependencies"), &active, &mut keys);
    if let Some(targets) = manifest
        .get("target")
        .and_then(toml_edit::Item::as_table_like)
    {
        for (_, target) in targets.iter() {
            add(target.get("dependencies"), &active, &mut keys);
        }
    }
    keys
}

#[test]
fn dependency_keys_respect_optional_default_features_and_target_tables() {
    let manifest = r#"
[features]
default = ["local"]
local = ["dep:enabled", "forwarded/mode"]
embedded = ["dep:disabled"]
[dependencies]
always = "1"
enabled = { version = "1", optional = true }
disabled = { version = "1", optional = true }
forwarded = { version = "1", optional = true }
[target.'cfg(unix)'.dependencies.extra]
version = "1"
[dev-dependencies]
not_normal = "1"
"#;
    assert_eq!(
        dependency_keys(manifest),
        ["always", "enabled", "extra", "forwarded"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
}

fn lock_dependencies() -> BTreeMap<String, Vec<String>> {
    let lock = read_repo_file("core/Cargo.lock")
        .parse::<toml_edit::DocumentMut>()
        .expect("parse Cargo.lock");
    let packages = lock["package"]
        .as_array_of_tables()
        .expect("Cargo.lock packages");
    let mut by_name = BTreeMap::new();
    for package in packages {
        let name = package["name"].as_str().expect("package name").to_owned();
        let dependencies: Vec<String> = package
            .get("dependencies")
            .and_then(toml_edit::Item::as_array)
            .into_iter()
            .flatten()
            .map(|dependency| {
                dependency
                    .as_str()
                    .expect("dependency string")
                    .split_whitespace()
                    .next()
                    .expect("dependency name")
                    .to_owned()
            })
            .collect();
        by_name.insert(name, dependencies);
    }
    // Lockfiles include every optional edge, even when its feature is disabled.
    // Remove disabled optional workspace edges before walking the default closure.
    for entry in fs::read_dir(repository_root().join("core/crates")).expect("read crates") {
        let path = entry.expect("crate entry").path().join("Cargo.toml");
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(path).expect("read manifest");
        let manifest = text
            .parse::<toml_edit::DocumentMut>()
            .expect("parse manifest");
        let enabled = dependency_keys(&text);
        let name = manifest["package"]["name"].as_str().expect("package name");
        let Some(dependencies) = by_name.get_mut(name) else {
            continue;
        };
        let mut tables = vec![manifest.get("dependencies")];
        if let Some(targets) = manifest
            .get("target")
            .and_then(toml_edit::Item::as_table_like)
        {
            tables.extend(targets.iter().map(|(_, target)| target.get("dependencies")));
        }
        for table in tables
            .into_iter()
            .flatten()
            .filter_map(toml_edit::Item::as_table_like)
        {
            for (key, dependency) in table.iter() {
                if dependency
                    .get("optional")
                    .and_then(toml_edit::Item::as_bool)
                    == Some(true)
                    && !enabled.contains(key)
                {
                    let package = dependency
                        .get("package")
                        .and_then(toml_edit::Item::as_str)
                        .unwrap_or(key);
                    dependencies.retain(|name| name != package);
                }
            }
        }
    }
    by_name
}

fn dependency_closure(root: &str, packages: &BTreeMap<String, Vec<String>>) -> BTreeSet<String> {
    assert!(packages.contains_key(root), "root package {root} not found");
    let mut visited = BTreeSet::new();
    let mut stack = vec![root.to_owned()];
    while let Some(name) = stack.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        let dependencies = packages
            .get(&name)
            .unwrap_or_else(|| panic!("dependency package {name} not found"));
        stack.extend(dependencies.iter().cloned());
    }
    visited
}

fn rust_string_const(source: &str, name: &str) -> String {
    let prefix = format!("pub const {name}: &str =");
    let lines = source.lines().collect::<Vec<_>>();
    let index = lines
        .iter()
        .position(|line| line.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("missing {name}"));
    let declaration = lines[index..=index.saturating_add(1).min(lines.len() - 1)].join(" ");
    declaration
        .split('"')
        .nth(1)
        .unwrap_or_else(|| panic!("{name} has no string literal"))
        .to_owned()
}

#[test]
fn shipping_closure_excludes_speaker_crates() {
    let packages = lock_dependencies();
    let closure = dependency_closure("solstone-core", &packages);

    // The pure speakers crate is in the default closure because sentence turn
    // evidence is validated there; the ONNX speaker crates stay out.
    for excluded in [
        "solstone-core-speakers-onnx",
        "solstone-core-speakers-analyze",
    ] {
        assert!(
            !closure.contains(excluded),
            "shipping closure contains {excluded}"
        );
    }
}

#[test]
fn vad_and_transcribe_share_the_firered_digest() {
    let vad = read_repo_file("core/crates/solstone-core-vad-analyze/src/lib.rs");
    let transcribe = read_repo_file("core/crates/solstone-core-transcribe/src/model_assets.rs");

    assert_eq!(
        rust_string_const(&vad, "FIRERED_VAD_SHA256"),
        rust_string_const(&transcribe, "FIRERED_VAD_SHA256")
    );
}
