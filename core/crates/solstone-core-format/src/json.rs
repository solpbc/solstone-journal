// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! JSON formatting compatible with Python's default ``json.dumps`` layout.

use serde_json::Value;

#[must_use]
pub fn json_compact_ascii(value: &Value) -> String {
    ensure_ascii(&json_compact(value))
}

#[must_use]
pub fn json_compact_utf8(value: &Value) -> String {
    json_compact(value)
}

#[must_use]
pub fn ensure_ascii(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            let codepoint = ch as u32;
            if codepoint <= 0xFFFF {
                output.push_str(&format!("\\u{codepoint:04x}"));
            } else {
                let adjusted = codepoint - 0x1_0000;
                let high = 0xD800 + (adjusted >> 10);
                let low = 0xDC00 + (adjusted & 0x3FF);
                output.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
        }
    }
    output
}

fn json_compact(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("string JSON"),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(json_compact)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    serde_json::to_string(key).expect("key JSON"),
                    json_compact(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
