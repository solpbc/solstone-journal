// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]

//! Shared assertions for replaying the recorded Body convey corpus.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

const PLACEHOLDER_ROOT: &str = "<journal-root>";

/// Returns the recorded JSON payload for one corpus phase and route rule.
pub(crate) fn recorded(phase: &str, rule: &str) -> Value {
    recorded_case(phase, rule)["json"].clone()
}

/// Reports the first structural difference between two JSON values.
pub(crate) fn first_difference(left: &Value, right: &Value, path: &str) -> Option<String> {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            let keys = left.keys().chain(right.keys()).collect::<BTreeSet<_>>();
            keys.into_iter()
                .find_map(|key| match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        first_difference(left, right, &format!("{path}.{key}"))
                    }
                    (Some(_), None) => Some(format!("{path}.{key}: key missing on right")),
                    (None, Some(_)) => Some(format!("{path}.{key}: key missing on left")),
                    (None, None) => None,
                })
        }
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => left
            .iter()
            .zip(right)
            .enumerate()
            .find_map(|(index, (left, right))| {
                first_difference(left, right, &format!("{path}[{index}]"))
            }),
        _ if left == right => None,
        _ => Some(format!("{path}: left={left}; right={right}")),
    }
}

/// Compares a native JSON payload with its recorded structural and hash contracts.
pub(crate) fn assert_recorded_payload(
    phase: &str,
    rule: &str,
    journal_root: &Path,
    payload: &Value,
) {
    let case = recorded_case(phase, rule);
    let path = case["path"].as_str().expect("recorded path");
    let mut actual = redact_journal_root(payload, journal_root);
    let mut omitted = Vec::new();
    if path.split('?').next() == Some("/app/body/api/status")
        && actual
            .as_object_mut()
            .and_then(|object| object.remove("freshness"))
            .is_some()
    {
        omitted.push("freshness");
    }
    assert_eq!(
        omitted,
        case["body_omitted_fields"]
            .as_array()
            .map(|values| values
                .iter()
                .map(|value| value.as_str().expect("omitted field"))
                .collect::<Vec<_>>())
            .unwrap_or_default(),
        "recorded omitted fields"
    );

    let mut normalized_fields = BTreeSet::new();
    normalize(
        &mut actual,
        "",
        path.split('?').next().unwrap_or(path),
        &mut normalized_fields,
    );
    assert_eq!(
        normalized_fields.into_iter().collect::<Vec<_>>(),
        case["normalized_fields"]
            .as_array()
            .expect("recorded normalized fields")
            .iter()
            .map(|value| value.as_str().expect("normalized field").to_owned())
            .collect::<Vec<_>>(),
        "recorded normalized fields"
    );

    let expected = &case["json"];
    if let Some(difference) = first_difference(&actual, expected, "$") {
        panic!("recorded corpus structural mismatch: {difference}");
    }
    let canonical = canonical_json(&actual);
    assert_eq!(
        format!("{:x}", Sha256::digest(canonical.as_bytes())),
        case["body_sha256"].as_str().expect("recorded body hash"),
        "recorded corpus normalized-json SHA-256"
    );
    assert_eq!(
        canonical.len(),
        case["body_bytes"].as_u64().expect("recorded body bytes") as usize,
        "recorded corpus normalized-json byte length"
    );
}

fn recorded_case(phase: &str, rule: &str) -> Value {
    let corpus: Value =
        serde_json::from_str(include_str!("../../../fixtures/convey_body_corpus.json"))
            .expect("corpus parses");
    corpus["cases"][phase]
        .as_array()
        .expect("phase is cases")
        .iter()
        .find(|case| case["rule"] == rule)
        .expect("recorded rule exists")
        .clone()
}

fn redact_journal_root(payload: &Value, journal_root: &Path) -> Value {
    let encoded = serde_json::to_string(payload).expect("payload serializes");
    let root = journal_root.to_string_lossy();
    serde_json::from_str(&encoded.replace(root.as_ref(), PLACEHOLDER_ROOT))
        .expect("redacted payload parses")
}

fn normalize(
    value: &mut Value,
    field_path: &str,
    request_path: &str,
    found: &mut BTreeSet<String>,
) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let path = if field_path.is_empty() {
                    key.to_owned()
                } else {
                    format!("{field_path}.{key}")
                };
                normalize(value, &path, request_path, found);
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize(value, &format!("{field_path}[]"), request_path, found);
            }
        }
        Value::String(text)
            if request_path == "/app/body/api/trends"
                && field_path == "generated_at_day"
                && text.len() == 8
                && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            *text = "<generated-at-day>".to_owned();
            found.insert(field_path.to_owned());
        }
        _ => {}
    }
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|key| format!(
                        "{}:{}",
                        python_json_string(key),
                        canonical_json(&object[key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::String(value) => python_json_string(value),
        _ => serde_json::to_string(value).expect("JSON value serializes"),
    }
}

fn python_json_string(value: &str) -> String {
    let mut encoded = String::from("\"");
    for character in value.chars() {
        match character {
            '\"' => encoded.push_str("\\\""),
            '\\' => encoded.push_str("\\\\"),
            '\u{08}' => encoded.push_str("\\b"),
            '\u{0C}' => encoded.push_str("\\f"),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character if character <= '\u{1F}' => {
                encoded.push_str(&format!("\\u{:04x}", character as u32));
            }
            character if character <= '\u{7F}' => encoded.push(character),
            character if (character as u32) <= 0xFFFF => {
                encoded.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => {
                let scalar = character as u32 - 0x1_0000;
                let high = 0xD800 + (scalar >> 10);
                let low = 0xDC00 + (scalar & 0x3FF);
                encoded.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
        }
    }
    encoded.push('\"');
    encoded
}
