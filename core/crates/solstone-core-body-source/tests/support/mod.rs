// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_body_source::BodyValue;

/// Asserts recursive body-value equality with floating-point bits compared exactly.
pub fn assert_body_value_bitwise_eq(actual: &BodyValue, expected: &BodyValue) {
    assert_body_value_at_path(actual, expected, "$");
}

fn assert_body_value_at_path(actual: &BodyValue, expected: &BodyValue, path: &str) {
    match (actual, expected) {
        (BodyValue::Number(actual), BodyValue::Number(expected)) => {
            assert_eq!(
                actual.to_bits(),
                expected.to_bits(),
                "number bits differ at {path}"
            );
        }
        (BodyValue::Array(actual), BodyValue::Array(expected)) => {
            assert_eq!(
                actual.len(),
                expected.len(),
                "array length differs at {path}"
            );
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert_body_value_at_path(actual, expected, &format!("{path}[{index}]"));
            }
        }
        (BodyValue::Object(actual), BodyValue::Object(expected)) => {
            assert_eq!(
                actual.len(),
                expected.len(),
                "object size differs at {path}"
            );
            for (key, actual) in actual {
                let expected = expected.get(key).unwrap_or_else(|| {
                    panic!("missing object key {:?} at {path}", key.code_points())
                });
                assert_body_value_at_path(
                    actual,
                    expected,
                    &format!("{path}[{:?}]", key.code_points()),
                );
            }
        }
        _ => assert_eq!(actual, expected, "value differs at {path}"),
    }
}

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

pub fn hash_vectors_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/fixtures/body_source_hash_vectors.json")
}

pub fn hash_vectors() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(hash_vectors_path()).expect("hash fixture should read"),
    )
    .expect("hash fixture should parse")
}
