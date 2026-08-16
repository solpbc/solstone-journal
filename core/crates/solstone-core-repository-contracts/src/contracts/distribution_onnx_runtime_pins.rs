// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const PYTHON_SCRIPT: &str = "scripts/stage_speakers_analyze_runtime.py";
const RUST_SOURCE: &str = "core/crates/solstone-core-distribution/src/onnx_runtime.rs";

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

fn quoted_strings(text: &str) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut value = String::new();
        while let Some(next) = chars.next() {
            if next == '\\' {
                if let Some(escaped) = chars.next() {
                    value.push(escaped);
                }
                continue;
            }
            if next == '"' {
                break;
            }
            value.push(next);
        }
        if !value.is_empty() {
            values.insert(value);
        }
    }
    values
}

const MEMBER_PREFIX: &str = "onnxruntime";
const STAGED_PREFIX: &str = "libonnxruntime";
const GPU_NEEDLE: &str = "providers_";

fn pin_values(quotes: &BTreeSet<String>) -> BTreeSet<String> {
    quotes
        .iter()
        .filter(|value| {
            value.starts_with("https://")
                || (value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
                || value.starts_with(&format!("{MEMBER_PREFIX}/"))
                || value.starts_with(&format!("{MEMBER_PREFIX}-"))
                || value.starts_with(STAGED_PREFIX)
                || value.contains(GPU_NEEDLE)
        })
        .cloned()
        .collect()
}

#[test]
fn python_runtime_pins_appear_in_distribution_source() {
    let root = repository_root();
    let python = fs::read_to_string(root.join(PYTHON_SCRIPT)).expect("read python pin table");
    let rust = fs::read_to_string(root.join(RUST_SOURCE)).expect("read rust pin table");
    let required = pin_values(&quoted_strings(&python));
    let present = quoted_strings(&rust);
    let missing = required
        .iter()
        .filter(|value| !present.contains(*value) && !rust.contains(value.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(
        missing.is_empty(),
        "{}",
        format_named_list("missing required", &missing)
    );
}
