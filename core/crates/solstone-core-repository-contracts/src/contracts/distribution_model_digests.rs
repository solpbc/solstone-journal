// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
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

fn format_named_list(label: &str, names: &BTreeSet<String>) -> String {
    let mut lines = vec![format!("{label}:")];
    for name in names {
        lines.push(format!("  {name}"));
    }
    lines.join("\n")
}

fn inventory_model_asset_digest_consts(text: &str) -> BTreeMap<String, BTreeSet<String>> {
    let document = text
        .parse::<toml_edit::DocumentMut>()
        .expect("parse distribution inventory");
    let entries = document["entry"]
        .as_array_of_tables()
        .expect("inventory entries are an array of tables");
    let mut by_source = BTreeMap::new();

    for entry in entries {
        if entry.get("kind").and_then(toml_edit::Item::as_str) != Some("model-asset") {
            continue;
        }
        let digest_source = entry
            .get("digest_source")
            .and_then(toml_edit::Item::as_str)
            .expect("model asset has digest_source");
        let digest_const = entry
            .get("digest_const")
            .and_then(toml_edit::Item::as_str)
            .expect("model asset has digest_const");
        by_source
            .entry(digest_source.to_owned())
            .or_insert_with(BTreeSet::new)
            .insert(digest_const.to_owned());
    }

    by_source
}

fn rust_const_hex(text: &str) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut pending: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("pub const ") {
            if let Some((name, after)) = rest.split_once(':')
                && after.contains("&str")
            {
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
fn inventory_digest_consts_bind_their_declared_source_hex_literals() {
    let root = repository_root();
    let inventory = fs::read_to_string(root.join(INVENTORY)).expect("read inventory");
    for (source, required) in inventory_model_asset_digest_consts(&inventory) {
        let assets = fs::read_to_string(root.join(&source))
            .unwrap_or_else(|error| panic!("read digest source {source}: {error}"));
        let literals = rust_const_hex(&assets);
        let missing = required
            .iter()
            .filter(|name| !literals.contains_key(*name))
            .cloned()
            .collect::<BTreeSet<_>>();
        assert!(
            missing.is_empty(),
            "{} in {source}",
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
}
