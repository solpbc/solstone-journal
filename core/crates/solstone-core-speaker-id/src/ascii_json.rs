// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! JSON serialization that never emits non-ASCII bytes.

use serde_json::Value;

/// Serialize a JSON value with `ensure_ascii=True`-style string escaping.
pub(crate) fn to_string(value: &Value) -> String {
    let mut output = String::new();
    write_value(&mut output, value);
    output
}

fn write_value(output: &mut String, value: &Value) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => write_string(output, value),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_value(output, value);
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_string(output, key);
                output.push(':');
                write_value(output, value);
            }
            output.push('}');
        }
    }
}

fn write_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_ascii_control() => write_escape(output, character as u16),
            character if character.is_ascii() => output.push(character),
            character => {
                let mut buffer = [0_u16; 2];
                for unit in character.encode_utf16(&mut buffer) {
                    write_escape(output, *unit);
                }
            }
        }
    }
    output.push('"');
}

fn write_escape(output: &mut String, unit: u16) {
    use std::fmt::Write as _;

    let _ = write!(output, "\\u{unit:04x}");
}
