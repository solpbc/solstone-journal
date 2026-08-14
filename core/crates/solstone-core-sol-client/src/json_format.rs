// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_format::{
    ensure_ascii, json_compact_ascii as format_json_compact_ascii,
    json_compact_utf8 as format_json_compact_utf8,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonFormat {
    pub sort_keys: bool,
    pub ensure_ascii: bool,
}

#[must_use]
pub fn sorted_json_pretty_ascii(value: &Value) -> String {
    json_pretty(
        value,
        JsonFormat {
            sort_keys: true,
            ensure_ascii: true,
        },
    )
}

#[must_use]
pub fn json_pretty_ascii(value: &Value) -> String {
    json_pretty(
        value,
        JsonFormat {
            sort_keys: false,
            ensure_ascii: true,
        },
    )
}

#[must_use]
pub fn json_pretty_utf8(value: &Value) -> String {
    json_pretty(
        value,
        JsonFormat {
            sort_keys: false,
            ensure_ascii: false,
        },
    )
}

#[must_use]
pub fn json_pretty(value: &Value, format: JsonFormat) -> String {
    let formatted = if format.sort_keys {
        let sorted = sort_json(value);
        serde_json::to_string_pretty(&sorted).expect("JSON output should serialize")
    } else {
        serde_json::to_string_pretty(value).expect("JSON output should serialize")
    };
    if format.ensure_ascii {
        ensure_ascii(&formatted)
    } else {
        formatted
    }
}

#[must_use]
pub fn json_compact_ascii(value: &Value) -> String {
    format_json_compact_ascii(value)
}

#[must_use]
pub fn sorted_json_compact_ascii(value: &Value) -> String {
    format_json_compact_ascii(&sort_json(value))
}

#[must_use]
pub fn json_compact_utf8(value: &Value) -> String {
    format_json_compact_utf8(value)
}

fn sort_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sort_json).collect()),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), sort_json(&object[key]));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pretty_prints_objects_with_sorted_keys_and_ascii_escapes() {
        assert_eq!(
            sorted_json_pretty_ascii(&json!({"b": "é", "a": {"d": 2, "c": 1}})),
            "{\n  \"a\": {\n    \"c\": 1,\n    \"d\": 2\n  },\n  \"b\": \"\\u00e9\"\n}"
        );
    }

    #[test]
    fn pretty_prints_objects_preserving_key_order_with_ascii_escapes() {
        assert_eq!(
            json_pretty_ascii(&json!({"b": "é", "a": {"d": 2, "c": 1}})),
            "{\n  \"b\": \"\\u00e9\",\n  \"a\": {\n    \"d\": 2,\n    \"c\": 1\n  }\n}"
        );
    }

    #[test]
    fn pretty_prints_objects_preserving_key_order_and_utf8() {
        assert_eq!(
            json_pretty_utf8(&json!({"b": "é", "a": {"d": 2, "c": 1}})),
            "{\n  \"b\": \"é\",\n  \"a\": {\n    \"d\": 2,\n    \"c\": 1\n  }\n}"
        );
    }

    #[test]
    fn compact_prints_objects_like_python_default_json_dumps() {
        assert_eq!(
            json_compact_ascii(&json!({"b": "é", "a": [1, true, null]})),
            "{\"b\": \"\\u00e9\", \"a\": [1, true, null]}"
        );
    }

    #[test]
    fn compact_prints_objects_with_sorted_keys() {
        assert_eq!(
            sorted_json_compact_ascii(&json!({"b": 2, "a": {"d": 4, "c": 3}})),
            "{\"a\": {\"c\": 3, \"d\": 4}, \"b\": 2}"
        );
    }

    #[test]
    fn compact_prints_objects_with_utf8_when_requested() {
        assert_eq!(
            json_compact_utf8(&json!({"b": "é", "a": [1, true, null]})),
            "{\"b\": \"é\", \"a\": [1, true, null]}"
        );
    }
}
