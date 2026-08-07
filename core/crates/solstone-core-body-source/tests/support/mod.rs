// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/fixtures/body_source_python_json_vectors.json")
}

pub fn vectors() -> Value {
    serde_json::from_str(&std::fs::read_to_string(fixture_path()).expect("fixture should read"))
        .expect("fixture should parse")
}

pub fn expand(pattern: &Value) -> String {
    let prefix = pattern
        .get("prefix")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let repeat = pattern
        .get("repeat")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let count = pattern["repeat_count"].as_u64().expect("repeat count") as usize;
    let suffix = pattern
        .get("suffix")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("{prefix}{}{suffix}", repeat.repeat(count))
}

pub fn codec_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/fixtures/body_source_codec_rows.json")
}

pub fn codec_rows() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(codec_fixture_path()).expect("codec fixture should read"),
    )
    .expect("codec fixture should parse")
}
