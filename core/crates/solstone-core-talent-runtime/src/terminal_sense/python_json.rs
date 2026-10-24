// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DumpOptions {
    pub ensure_ascii: bool,
    pub compact: bool,
    pub sort_keys: bool,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            ensure_ascii: true,
            compact: false,
            sort_keys: false,
        }
    }
}

pub(crate) fn python_dumps(value: &Value, options: DumpOptions) -> String {
    let mut out = String::new();
    write_value(value, options, &mut out);
    out
}

fn write_value(value: &Value, options: DumpOptions, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_f64() {
                let text = format!("{:?}", n.as_f64().unwrap());
                if let Some((mantissa, exponent)) = text.split_once('e') {
                    let exponent = exponent.parse::<i32>().unwrap();
                    let _ = write!(out, "{mantissa}e{exponent:+03}");
                } else {
                    out.push_str(&text);
                }
            } else {
                let _ = write!(out, "{n}");
            }
        }
        Value::String(s) => write_string(s, options.ensure_ascii, out),
        Value::Array(items) => {
            out.push('[');
            let sep = if options.compact { "," } else { ", " };
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(sep);
                }
                write_value(item, options, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let item_sep = if options.compact { "," } else { ", " };
            let kv_sep = if options.compact { ":" } else { ": " };
            if options.sort_keys {
                let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                entries.sort_by_key(|(a, _)| *a);
                for (i, (k, v)) in entries.into_iter().enumerate() {
                    if i > 0 {
                        out.push_str(item_sep);
                    }
                    write_string(k, options.ensure_ascii, out);
                    out.push_str(kv_sep);
                    write_value(v, options, out);
                }
            } else {
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push_str(item_sep);
                    }
                    write_string(k, options.ensure_ascii, out);
                    out.push_str(kv_sep);
                    write_value(v, options, out);
                }
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, ensure_ascii: bool, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ensure_ascii && (c as u32) > 0x7F => {
                let cp = c as u32;
                if cp <= 0xFFFF {
                    let _ = write!(out, "\\u{:04x}", cp);
                } else {
                    let u = cp - 0x10000;
                    let hi = 0xD800 + (u >> 10);
                    let lo = 0xDC00 + (u & 0x3FF);
                    let _ = write!(out, "\\u{:04x}\\u{:04x}", hi, lo);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_python_dumps_options() {
        let val = json!({"b": "text\n", "a": [1, "❯", "😊"]});
        // Default ASCII
        let s1 = python_dumps(&val, DumpOptions::default());
        assert!(s1.contains("\\u276f"));
        assert!(s1.contains("\\ud83d\\ude0a"));
        assert!(s1.contains(": "));
        assert!(s1.contains(", "));

        // Non-ASCII
        let s2 = python_dumps(
            &val,
            DumpOptions {
                ensure_ascii: false,
                compact: true,
                sort_keys: true,
            },
        );
        assert_eq!(s2, "{\"a\":[1,\"❯\",\"😊\"],\"b\":\"text\\n\"}");
    }
}
