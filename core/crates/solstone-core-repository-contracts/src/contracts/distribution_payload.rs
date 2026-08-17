// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--",
            "solstone/talent",
            "solstone/think/templates",
            "solstone/think/services/spp_attest/roots",
            "solstone/think/contract/layout.json",
            "solstone/apps",
        ])
        .current_dir(root)
        .output()
        .expect("run git ls-files payload oracle");
    assert!(
        output.status.success(),
        "git ls-files payload oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git payload paths are UTF-8")
        .split('\0')
        .filter(|path| !path.is_empty() && !path.ends_with(".py"))
        .filter(|path| is_payload_path(path))
        .map(str::to_owned)
        .collect()
}

fn is_payload_path(path: &str) -> bool {
    path == "solstone/think/contract/layout.json"
        || path.starts_with("solstone/talent/")
        || path.starts_with("solstone/think/templates/")
        || path.starts_with("solstone/think/services/spp_attest/roots/")
        || path
            .strip_prefix("solstone/apps/")
            .and_then(|relative| relative.split_once('/'))
            .is_some_and(|(_, child)| child.starts_with("talent/"))
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
    assert!(
        oracle.contains("solstone/talent/daily_schedule.md"),
        "payload oracle must see a known talent positive"
    );
    assert!(
        oracle.contains("solstone/think/contract/layout.json"),
        "payload oracle must see the layout contract anchor"
    );
    let missing = oracle.difference(&listed).cloned().collect::<BTreeSet<_>>();
    let unexpected = listed.difference(&oracle).cloned().collect::<BTreeSet<_>>();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{}\n{}",
        format_named_list("missing", &missing),
        format_named_list("unexpected", &unexpected)
    );
}
