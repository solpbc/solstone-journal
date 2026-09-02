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
        let dependencies = package
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

fn collect_rust_files(path: &Path, files: &mut Vec<PathBuf>) {
    for entry in
        fs::read_dir(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            files.push(path);
        }
    }
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

    for excluded in [
        "solstone-core-speakers",
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
fn speaker_onnx_has_one_production_session_builder_site() {
    let source_root = repository_root().join("core/crates/solstone-core-speakers-onnx/src");
    let mut files = Vec::new();
    collect_rust_files(&source_root, &mut files);
    files.sort();
    let count = files
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
                .matches("Session::builder()?")
                .count()
        })
        .sum::<usize>();

    assert!(
        files.len() >= 4,
        "source census visited only {} files",
        files.len()
    );
    assert_eq!(count, 1, "speaker ONNX Session::builder site census");
}

#[test]
fn vad_and_transcribe_share_the_silero_digest() {
    let vad = read_repo_file("core/crates/solstone-core-vad-analyze/src/lib.rs");
    let transcribe = read_repo_file("core/crates/solstone-core-transcribe/src/model_assets.rs");

    assert_eq!(
        rust_string_const(&vad, "SILERO_VAD_V6_SHA256"),
        rust_string_const(&transcribe, "SILERO_VAD_V6_SHA256")
    );
}

#[test]
fn vad_session_options_match_the_reference_configuration() {
    let source = read_repo_file("core/crates/solstone-core-vad-analyze/src/lib.rs");
    for call in [
        "CPU::default().with_arena_allocator(false).build()",
        ".with_intra_threads(1)",
        ".with_inter_threads(1)",
    ] {
        assert!(
            source.contains(call),
            "SileroVadSession::open no longer applies {call}"
        );
    }
}
