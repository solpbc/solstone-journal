// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../src/payload_inventory.rs"]
mod payload_inventory;

use payload_inventory::{declared_paths, inventory_diff, payload_set, tracked_paths};

const PAYLOAD_GIT_PATHS: &[&str] = &[
    "solstone/talent",
    "solstone/think/templates",
    "solstone/think/services/spp_attest/roots",
    "solstone/think/contract/layout.json",
    "solstone/apps",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn format_named_list(label: &str, names: &BTreeSet<Vec<u8>>) -> String {
    let mut lines = vec![format!("{label}:")];
    lines.extend(names.iter().map(|name| format!("  {}", display_path(name))));
    lines.join("\n")
}

fn display_path(path: &[u8]) -> String {
    match std::str::from_utf8(path) {
        Ok(path) => path.to_owned(),
        Err(_) => path.iter().map(|byte| format!("\\x{byte:02x}")).collect(),
    }
}

#[test]
fn payload_txt_matches_git_tracked_inventory() {
    let root = repository_root();
    let mut arguments = vec!["ls-files", "-z", "--"];
    arguments.extend(PAYLOAD_GIT_PATHS);
    let output = Command::new("git")
        .args(arguments)
        .current_dir(&root)
        .output()
        .expect("run git ls-files payload inventory");
    assert!(
        output.status.success(),
        "git ls-files payload inventory failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let declared = declared_paths(
        &fs::read_to_string(root.join("core/distribution/payload.txt")).expect("read payload.txt"),
    )
    .into_iter()
    .map(String::into_bytes)
    .collect::<BTreeSet<_>>();
    let tracked_bytes =
        payload_set(tracked_paths(&output.stdout).expect("parse NUL-delimited Git paths"));

    assert!(
        tracked_bytes.contains(b"solstone/talent/daily_schedule.md".as_slice()),
        "tracked inventory must see a known talent positive"
    );
    assert!(
        tracked_bytes.contains(b"solstone/think/contract/layout.json".as_slice()),
        "tracked inventory must see the layout contract anchor"
    );

    let (missing, unexpected) = inventory_diff(&declared, &tracked_bytes);
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{}\n{}",
        format_named_list("tracked but undeclared", &missing),
        format_named_list("declared but untracked", &unexpected)
    );
}
