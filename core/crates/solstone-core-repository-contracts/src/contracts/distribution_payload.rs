// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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

fn collect_payload_roots(root: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    collect_files(root, &root.join("solstone/talent"), &mut found);
    collect_files(root, &root.join("solstone/think/templates"), &mut found);
    collect_files(
        root,
        &root.join("solstone/think/services/spp_attest/roots"),
        &mut found,
    );
    let apps = root.join("solstone/apps");
    if let Ok(entries) = fs::read_dir(&apps) {
        for entry in entries.flatten() {
            collect_files(root, &entry.path().join("talent"), &mut found);
        }
    }
    found
}

fn collect_files(root: &Path, dir: &Path, found: &mut BTreeSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, found);
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(".py") {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(root) {
            found.insert(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn payload_txt_matches_git_ls_files_oracle() {
    let root = repository_root();
    let listed = fs::read_to_string(root.join("core/distribution/payload.txt"))
        .expect("read payload.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let oracle = collect_payload_roots(&root);
    let missing = oracle.difference(&listed).cloned().collect::<BTreeSet<_>>();
    let unexpected = listed.difference(&oracle).cloned().collect::<BTreeSet<_>>();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{}\n{}",
        format_named_list("missing", &missing),
        format_named_list("unexpected", &unexpected)
    );
}
