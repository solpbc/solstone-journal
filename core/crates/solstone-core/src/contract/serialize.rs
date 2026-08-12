// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write;

use serde_json::Value;

pub(crate) fn render(value: &Value) -> String {
    let mut output = String::new();
    write_value(&mut output, value, 0);
    output.push('\n');
    output
}

fn write_value(output: &mut String, value: &Value, depth: usize) {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&python_style_number(value)),
        Value::String(value) => write_string(output, value),
        Value::Array(values) => {
            if values.is_empty() {
                output.push_str("[]");
                return;
            }
            output.push_str("[\n");
            for (index, value) in values.iter().enumerate() {
                indent(output, depth + 1);
                write_value(output, value, depth + 1);
                if index + 1 != values.len() {
                    output.push(',');
                }
                output.push('\n');
            }
            indent(output, depth);
            output.push(']');
        }
        Value::Object(values) => {
            if values.is_empty() {
                output.push_str("{}");
                return;
            }
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            output.push_str("{\n");
            for (index, (key, value)) in entries.iter().enumerate() {
                indent(output, depth + 1);
                write_string(output, key);
                output.push_str(": ");
                write_value(output, value, depth + 1);
                if index + 1 != entries.len() {
                    output.push(',');
                }
                output.push('\n');
            }
            indent(output, depth);
            output.push('}');
        }
    }
}

fn indent(output: &mut String, depth: usize) {
    for _ in 0..depth {
        output.push_str("  ");
    }
}

fn write_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{09}' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{0c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            character if character.is_ascii() && !character.is_control() => output.push(character),
            character if (character as u32) <= 0xffff => {
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => {
                let scalar = character as u32 - 0x1_0000;
                let high = 0xd800 + (scalar >> 10);
                let low = 0xdc00 + (scalar & 0x3ff);
                let _ = write!(output, "\\u{high:04x}\\u{low:04x}");
            }
        }
    }
    output.push('"');
}

fn python_style_number(number: &serde_json::Number) -> String {
    let rendered = number.to_string();
    let Some((mantissa, exponent)) = rendered.split_once(['e', 'E']) else {
        return rendered;
    };
    let exponent = exponent.parse::<i32>().unwrap_or_default();
    format!("{mantissa}e{exponent:+}")
}

#[cfg(test)]
mod tests {
    use super::render;
    use serde_json::json;

    #[test]
    fn sorts_objects_preserves_arrays_and_ascii_escapes() {
        let value = json!({"z": ["second", "first"], "a": {"é": "😀\u{2028}\u{7f}"}});
        assert_eq!(
            render(&value),
            "{\n  \"a\": {\n    \"\\u00e9\": \"\\ud83d\\ude00\\u2028\\u007f\"\n  },\n  \"z\": [\n    \"second\",\n    \"first\"\n  ]\n}\n"
        );
    }

    #[test]
    fn native_number_fallback_is_pinned_without_arbitrary_precision() {
        let value: serde_json::Value =
            serde_json::from_str("{\"large\":10000000000000000000000000000,\"float\":1e30}")
                .unwrap();
        // AC2 deliberately pins serde_json's default f64 fallback. Do not add
        // arbitrary_precision: other workspace crates rely on this feature set.
        assert_eq!(
            render(&value),
            "{\n  \"float\": 1e+30,\n  \"large\": 1e+28\n}\n"
        );
    }
}
