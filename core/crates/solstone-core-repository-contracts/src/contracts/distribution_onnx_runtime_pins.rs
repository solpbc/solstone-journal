// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

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
fn distribution_source_still_pins_the_three_cpu_wheels() {
    let root = repository_root();
    let rust = fs::read_to_string(root.join(RUST_SOURCE)).expect("read rust pin table");
    let present = pin_values(&quoted_strings(&rust));
    let required = BTreeSet::from([
        "https://files.pythonhosted.org/packages/a9/1b/d681878f227513917d8620e4ea504af5eb3313fc01f8aea7b19a976c65db/onnxruntime-1.25.0-cp312-cp312-manylinux_2_27_x86_64.manylinux_2_28_x86_64.whl"
            .to_owned(),
        "https://files.pythonhosted.org/packages/5a/c6/19c5bfbc60396791e975652f982bcff9ff4b27947c8e2bf0064ac5d5727b/onnxruntime-1.25.0-cp312-cp312-manylinux_2_27_aarch64.manylinux_2_28_aarch64.whl"
            .to_owned(),
        "https://files.pythonhosted.org/packages/7a/69/f98c6bda4c34ac382b70c36033a989ceffd1caf5afba47bd2ef26535850f/onnxruntime-1.25.0-cp312-cp312-macosx_14_0_arm64.whl"
            .to_owned(),
        "be93baa694ef8e5831fcb7b542da21f502b122918b5b9612d9f02972e043ee01".to_owned(),
        "9c99238d20bfa80ac68c7b03c2c936d389189ae40997f78a30d151570d7e18bf".to_owned(),
        "8ecd3362de3fb496fb3e2d055a95d5acab611cf759a27609c6d99704c9d8f184".to_owned(),
    ]);
    let missing = required
        .difference(&present)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(
        missing.is_empty(),
        "{}",
        format_named_list("missing required", &missing)
    );
}
