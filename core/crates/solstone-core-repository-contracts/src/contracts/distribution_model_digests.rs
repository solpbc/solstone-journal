// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

const INVENTORY: &str = "core/distribution/inventory.toml";
const ASSETS: &str = "core/crates/solstone-core-transcribe/src/model_assets.rs";

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

fn inventory_digest_consts(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("digest_const = ")
                .map(|value| value.trim().trim_matches('"').to_owned())
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn rust_const_hex(text: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub const ") {
            if let Some((name, after)) = rest.split_once(':') {
                if after.contains("&str") {
                    if let Some((_, literal)) = trimmed.split_once('=') {
                        let hex = literal
                            .trim()
                            .trim_end_matches(';')
                            .trim()
                            .trim_matches('"');
                        if hex.len() == 64 {
                            found.insert(name.trim().to_owned(), hex.to_owned());
                            pending = None;
                            continue;
                        }
                    }
                    pending = Some(name.trim().to_owned());
                }
            }
            continue;
        }
        if let Some(name) = pending.take() {
            let hex = trimmed.trim_end_matches(';').trim().trim_matches('"');
            if hex.len() == 64 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                found.insert(name, hex.to_owned());
            }
        }
    }
    found
}

#[test]
fn inventory_digest_consts_bind_transcribe_hex_literals() {
    let root = repository_root();
    let inventory = fs::read_to_string(root.join(INVENTORY)).expect("read inventory");
    let assets = fs::read_to_string(root.join(ASSETS)).expect("read model asset constants");
    let required = inventory_digest_consts(&inventory);
    let literals = rust_const_hex(&assets);
    let missing = required
        .iter()
        .filter(|name| !literals.contains_key(*name))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(
        missing.is_empty(),
        "{}",
        format_named_list("missing required", &missing)
    );
    for name in &required {
        let hex = literals.get(name).expect("bound hex");
        assert_eq!(hex.len(), 64, "{name}");
        assert!(
            hex.chars().all(|ch| ch.is_ascii_hexdigit()),
            "{name} is not hex"
        );
    }
}
