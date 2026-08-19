// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

const INVENTORY: &str = "core/distribution/inventory.toml";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

struct InventoryEntry {
    kind: Option<String>,
    source: Option<String>,
    dest: Option<String>,
}

fn inventory_entries(text: &str) -> Vec<InventoryEntry> {
    let mut entries = Vec::new();
    let mut current: Option<InventoryEntry> = None;
    let flush = |entries: &mut Vec<InventoryEntry>, current: &mut Option<InventoryEntry>| {
        if let Some(entry) = current.take() {
            entries.push(entry);
        }
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "[[entry]]" {
            flush(&mut entries, &mut current);
            current = Some(InventoryEntry {
                kind: None,
                source: None,
                dest: None,
            });
            continue;
        }
        if trimmed.starts_with("[[") {
            flush(&mut entries, &mut current);
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        if let Some(value) = trimmed.strip_prefix("kind = ") {
            entry.kind = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("source = ") {
            entry.source = Some(value.trim().trim_matches('"').to_owned());
        }
        if let Some(value) = trimmed.strip_prefix("dest = ") {
            entry.dest = Some(value.trim().trim_matches('"').to_owned());
        }
    }
    flush(&mut entries, &mut current);
    entries
}

#[test]
fn inventory_has_no_sol_launcher_and_keeps_solstone() {
    let root = repository_root();
    let inventory = fs::read_to_string(root.join(INVENTORY)).expect("read inventory");
    let entries = inventory_entries(&inventory);
    assert!(
        entries
            .iter()
            .all(|entry| entry.dest.as_deref() != Some("bin/sol")),
        "inventory must not ship dest = \"bin/sol\""
    );
    assert!(
        entries.iter().any(|entry| {
            entry.kind.as_deref() == Some("launcher")
                && entry.dest.as_deref() == Some("bin/solstone")
        }),
        "inventory must keep a launcher at dest = \"bin/solstone\""
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.source.as_deref() != Some("scripts/root-launchers/sol")),
        "inventory must not reference scripts/root-launchers/sol"
    );

    for rel in [
        "pyproject.toml",
        "MANIFEST.in",
        "core/distribution/macos.sh",
        "core/distribution/cleanroom.sh",
    ] {
        let text = fs::read_to_string(root.join(rel))
            .unwrap_or_else(|error| panic!("read {rel}: {error}"));
        assert!(
            !text.contains("scripts/root-launchers/sol\""),
            "{rel} still names scripts/root-launchers/sol"
        );
        assert!(
            !text.contains("include scripts/root-launchers/sol\n"),
            "{rel} still includes scripts/root-launchers/sol"
        );
        assert!(
            !text.contains("journal sol solstone"),
            "{rel} still asserts a sol launcher beside journal and solstone"
        );
        assert!(
            !text.contains("command -v sol;"),
            "{rel} still probes command -v sol"
        );
    }
}
