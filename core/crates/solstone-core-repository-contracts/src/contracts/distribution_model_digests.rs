// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

const INVENTORY: &str = "core/distribution/inventory.toml";

/// Model digests that a second crate re-declares under its own name.
///
/// The inventory names one owning source per model asset, and the contract
/// below binds that. It cannot see a mirror: `solstone-core-distribution`
/// cannot depend on `solstone-core-transcribe` — that would pull the FFmpeg
/// build into a crate the Windows gate deliberately builds before the FFmpeg
/// toolchain is staged — so the Windows ONNX staging carries its own copy.
/// This pairs them by name so a repin has to move both.
///
/// ⛔ Not a pinned value: repinning a model changes both literals and this
/// stays green. What it refuses is changing one of them.
const MIRRORED_MODEL_DIGESTS: &[(&str, &str, &str)] = &[
    (
        "core/crates/solstone-core-transcribe/src/model_assets.rs",
        "WESPEAKER_RESNET34_SHA256",
        "WESPEAKER_SHA256",
    ),
    (
        "core/crates/solstone-core-transcribe/src/model_assets.rs",
        "PYANNOTE_SEGMENTATION_SHA256",
        "PYANNOTE_SHA256",
    ),
    (
        "core/crates/solstone-core-transcribe/src/model_assets.rs",
        "SILERO_VAD_V6_SHA256",
        "SILERO_VAD_SHA256",
    ),
];

const MIRROR_SOURCE: &str = "core/crates/solstone-core-distribution/src/onnx_windows.rs";

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

#[test]
fn a_model_digest_re_declared_in_a_second_crate_agrees_with_its_owner() {
    let root = repository_root();
    let mirror = fs::read_to_string(root.join(MIRROR_SOURCE)).expect("read mirror source");
    let mirror_literals = rust_const_hex(&mirror);
    for (owner_source, owner_const, mirror_const) in MIRRORED_MODEL_DIGESTS {
        let owner = fs::read_to_string(root.join(owner_source))
            .unwrap_or_else(|error| panic!("read digest source {owner_source}: {error}"));
        let owner_hex = rust_const_hex(&owner)
            .get(*owner_const)
            .unwrap_or_else(|| panic!("{owner_const} is declared in {owner_source}"))
            .clone();
        let mirror_hex = mirror_literals
            .get(*mirror_const)
            .unwrap_or_else(|| panic!("{mirror_const} is declared in {MIRROR_SOURCE}"));
        assert_eq!(
            &owner_hex, mirror_hex,
            "{mirror_const} in {MIRROR_SOURCE} disagrees with {owner_const} in {owner_source}; \
             a repin has to move both"
        );
    }
}
