// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! This repository is public. The names of private operator build and
//! automation hosts must never appear in it. The names are assembled from
//! pieces here so this file does not itself carry them.

use std::fs;
use std::path::{Path, PathBuf};

const PRIVATE_HOST_NAMES: [&[&str]; 4] = [
    &["pr", "o5", "e"],
    &["spark", "-", "a8", "a6"],
    &["tm", "ux", "-", "run"],
    &["automation", ":", "build", "-"],
];

/// Build outputs, environments and version-control metadata are not source.
const SKIPPED_DIRECTORIES: [&str; 9] = [
    ".git",
    "target",
    ".venv",
    "node_modules",
    "__pycache__",
    "dist",
    ".wrangler",
    ".hypothesis",
    ".mypy_cache",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("repository directory is readable") {
        let entry = entry.expect("repository directory entry is readable");
        let path = entry.path();
        let file_type = entry
            .file_type()
            .expect("repository entry type is readable");
        if file_type.is_dir() {
            let skipped = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| SKIPPED_DIRECTORIES.contains(&name));
            if !skipped {
                collect_files(&path, files);
            }
        } else if file_type.is_file() {
            files.push(path);
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn private_host_names_stay_out_of_the_public_repository() {
    let root = repository_root();
    let names: Vec<String> = PRIVATE_HOST_NAMES
        .iter()
        .map(|pieces| pieces.concat())
        .collect();
    let mut files = Vec::new();
    collect_files(&root, &mut files);
    files.sort();
    assert!(
        files.len() > 1_000,
        "the scan must read the repository, found {} files",
        files.len()
    );

    let mut findings = Vec::new();
    for path in &files {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        for name in &names {
            if contains(&bytes, name.as_bytes()) {
                findings.push(format!(
                    "{}: names private host {name:?}",
                    path.strip_prefix(&root).unwrap_or(path).display()
                ));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "private host names in the public repository:\n{}",
        findings.join("\n")
    );
}
