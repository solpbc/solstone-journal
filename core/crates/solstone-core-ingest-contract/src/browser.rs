// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::sync::LazyLock;

use jsonschema::error::ValidationErrorKind;
use jsonschema::{Draft, options};
use serde_json::Value;
use sha2::{Digest, Sha256};

const SCHEMA_BYTES: &[u8] = include_bytes!("browser.schema.json");

static SCHEMA_VALUE: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_slice(SCHEMA_BYTES).expect("browser schema is valid JSON"));

static SCHEMA_ID: LazyLock<String> = LazyLock::new(|| {
    SCHEMA_VALUE
        .get("$id")
        .and_then(Value::as_str)
        .expect("browser schema has $id string")
        .to_owned()
});

static SCHEMA_DIGEST: LazyLock<String> =
    LazyLock::new(|| format!("{:x}", Sha256::digest(SCHEMA_BYTES)));

static COMPILED_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    options()
        .with_draft(Draft::Draft202012)
        .build(&SCHEMA_VALUE)
        .expect("browser schema compiles")
});

static SNAPSHOT_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let mut schema = SCHEMA_VALUE.clone();
    schema["$ref"] = serde_json::json!("#/$defs/snapshot");
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("oneOf");
    }
    options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .expect("snapshot schema compiles")
});

static ADD_UPDATE_DELTA_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let mut schema = SCHEMA_VALUE.clone();
    schema["$ref"] = serde_json::json!("#/$defs/addUpdateDelta");
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("oneOf");
    }
    options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .expect("addUpdateDelta schema compiles")
});

static REMOVE_DELTA_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let mut schema = SCHEMA_VALUE.clone();
    schema["$ref"] = serde_json::json!("#/$defs/removeDelta");
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("oneOf");
    }
    options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .expect("removeDelta schema compiles")
});

static DELTA_SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let mut schema = SCHEMA_VALUE.clone();
    schema["$ref"] = serde_json::json!("#/$defs/delta");
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("oneOf");
    }
    options()
        .with_draft(Draft::Draft202012)
        .build(&schema)
        .expect("delta schema compiles")
});

#[cfg(test)]
static BLOCKS_MAX_ITEMS: LazyLock<usize> = LazyLock::new(|| {
    SCHEMA_VALUE
        .get("$defs")
        .and_then(|defs| defs.get("snapshot"))
        .and_then(|snap| snap.get("properties"))
        .and_then(|props| props.get("blocks"))
        .and_then(|blocks| blocks.get("maxItems"))
        .and_then(Value::as_u64)
        .expect("snapshot blocks maxItems keyword must be present in schema") as usize
});

/// Return the canonical schema bytes for `browser-jsonl`.
pub fn browser_schema_bytes() -> &'static [u8] {
    SCHEMA_BYTES
}

/// Return the `$id` defined in the canonical browser schema.
pub fn browser_schema_id() -> &'static str {
    &SCHEMA_ID
}

/// Return the lowercase hex-encoded SHA-256 digest of the canonical browser schema bytes.
pub fn browser_schema_digest() -> String {
    SCHEMA_DIGEST.clone()
}

/// Error returned when browser record or JSONL validation fails.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrowserRecordError {
    pub row: usize,
    pub field: String,
    pub cause: &'static str,
}

impl fmt::Display for BrowserRecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "row={} field={} cause={}",
            self.row, self.field, self.cause
        )
    }
}

impl std::error::Error for BrowserRecordError {}

/// Validate a single browser JSON record.
pub fn validate_browser_record(value: &Value) -> Result<(), BrowserRecordError> {
    validate_record_inner(1, value)
}

/// Validate an entire browser JSONL file.
pub fn validate_browser_jsonl(bytes: &[u8]) -> Result<(), BrowserRecordError> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => {
            return Err(BrowserRecordError {
                row: 0,
                field: String::new(),
                cause: "utf8",
            });
        }
    };

    let mut non_empty_lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .peekable();

    if non_empty_lines.peek().is_none() {
        return Err(BrowserRecordError {
            row: 0,
            field: String::new(),
            cause: "empty",
        });
    }

    for (index, line) in non_empty_lines.enumerate() {
        let row = index + 1;
        let value = match serde_json::from_str::<Value>(line) {
            Ok(value) => value,
            Err(_) => {
                return Err(BrowserRecordError {
                    row,
                    field: String::new(),
                    cause: "json",
                });
            }
        };

        if !value.is_object() {
            return Err(BrowserRecordError {
                row,
                field: String::new(),
                cause: "type",
            });
        }

        if row == 1 {
            let t = value.get("t").and_then(Value::as_str);
            if t == Some("delta") {
                return Err(BrowserRecordError {
                    row: 1,
                    field: "t".to_owned(),
                    cause: "delta_first",
                });
            }
            if t != Some("segment_start") {
                return Err(BrowserRecordError {
                    row: 1,
                    field: "t".to_owned(),
                    cause: "variant",
                });
            }
        }

        validate_record_inner(row, &value)?;
    }

    Ok(())
}

fn validate_record_inner(row: usize, value: &Value) -> Result<(), BrowserRecordError> {
    let Some(obj) = value.as_object() else {
        return Err(BrowserRecordError {
            row,
            field: String::new(),
            cause: "type",
        });
    };

    let validator = match obj.get("t").and_then(Value::as_str) {
        Some("segment_start") => &*SNAPSHOT_SCHEMA,
        Some("delta") => match obj.get("op").and_then(Value::as_str) {
            Some("add" | "update") => &*ADD_UPDATE_DELTA_SCHEMA,
            Some("remove") => &*REMOVE_DELTA_SCHEMA,
            _ => &*DELTA_SCHEMA,
        },
        _ => &*COMPILED_SCHEMA,
    };

    let mapped_errors = validator.iter_errors(value).map(|error| {
        let path = error
            .instance_path()
            .as_str()
            .trim_start_matches('/')
            .replace('/', ".");

        let cause = match error.kind() {
            ValidationErrorKind::MaxItems { .. }
            | ValidationErrorKind::MaxLength { .. }
            | ValidationErrorKind::Maximum { .. } => "limit",
            ValidationErrorKind::Minimum { .. } if path == "ts" => "ts_negative",
            ValidationErrorKind::Type { .. } if path == "ts" && value["ts"].is_number() => {
                "ts_fractional"
            }
            ValidationErrorKind::Type { .. } => "type",
            ValidationErrorKind::Required { .. }
            | ValidationErrorKind::Enum { .. }
            | ValidationErrorKind::OneOfNotValid { .. }
            | ValidationErrorKind::AnyOf { .. }
            | ValidationErrorKind::AdditionalProperties { .. } => "variant",
            _ => "variant",
        };

        (path, cause)
    });

    if let Some((field, cause)) =
        mapped_errors.min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
    {
        return Err(BrowserRecordError { row, field, cause });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_constants_and_digest() {
        assert_eq!(browser_schema_id(), "solstone-journal-format:browser-jsonl");
        let digest = browser_schema_digest();
        assert_eq!(digest.len(), 64);
        assert_eq!(
            digest,
            format!("{:x}", Sha256::digest(browser_schema_bytes()))
        );
    }

    #[test]
    fn root_validator_accepts_valid_snapshot_and_deltas() {
        let snapshot = json!({
            "t": "segment_start",
            "ts": 1783046501000i64,
            "site": "mail.google.com",
            "url": "https://mail.google.com/mail/u/0/#inbox",
            "title": "Inbox - Gmail",
            "adapter": "gmail",
            "inst": "inst-123",
            "ctx": "inbox",
            "rel": 0,
            "n": 2,
            "blocks": [
                {"type": "heading", "text": "Inbox", "id": "b1", "attrs": {"label": "mail"}},
                {"type": "row", "text": "Message row"}
            ],
            "extra_field": "allowed"
        });
        assert!(validate_browser_record(&snapshot).is_ok());

        let add_delta = json!({
            "t": "delta",
            "ts": 1783046509120i64,
            "op": "add",
            "inst": "inst-123",
            "block": {"type": "row", "text": "New message", "id": "b2"},
            "extra_delta": 42
        });
        assert!(validate_browser_record(&add_delta).is_ok());

        let update_delta = json!({
            "t": "delta",
            "ts": 1783046523440i64,
            "op": "update",
            "block": {"type": "row", "text": "Updated message"}
        });
        assert!(validate_browser_record(&update_delta).is_ok());

        let remove_delta = json!({
            "t": "delta",
            "ts": 1783046530100i64,
            "op": "remove",
            "block": {"id": "b1", "extra": "allowed_in_remove"}
        });
        assert!(validate_browser_record(&remove_delta).is_ok());

        let remove_with_text = json!({
            "t": "delta",
            "ts": 1783046530100i64,
            "op": "remove",
            "block": {"id": "b1", "text": "optional text in remove"}
        });
        assert!(validate_browser_record(&remove_with_text).is_ok());
    }

    #[test]
    fn root_validator_rejects_invalid_records() {
        assert!(validate_browser_record(&json!({})).is_err());
        assert!(validate_browser_record(&json!({"t": "unknown", "ts": 100})).is_err());
        assert!(validate_browser_record(&json!({"t": "segment_start", "ts": 100})).is_err()); // missing blocks
        assert!(validate_browser_record(&json!({"t": "delta", "ts": 100})).is_err()); // missing op/block
        assert!(
            validate_browser_record(
                &json!({"t": "delta", "ts": 100, "op": "remove", "block": {"text": "no_id"}})
            )
            .is_err()
        ); // remove without id
        assert!(
            validate_browser_record(
                &json!({"t": "delta", "ts": 100, "op": "add", "block": {"id": "b1"}})
            )
            .is_err()
        ); // add without text
    }

    #[test]
    fn codepoint_length_distinguishes_unicode_planes_and_escapes() {
        // 2001 ASCII code points
        let text_2001_ascii = "a".repeat(2001);
        let text_2002_ascii = "a".repeat(2002);

        let delta_2001 = json!({
            "t": "delta",
            "ts": 100,
            "op": "add",
            "block": {"text": text_2001_ascii}
        });
        assert!(validate_browser_record(&delta_2001).is_ok());

        let delta_2002 = json!({
            "t": "delta",
            "ts": 100,
            "op": "add",
            "block": {"text": text_2002_ascii}
        });
        let err = validate_browser_record(&delta_2002).unwrap_err();
        assert_eq!(err.cause, "limit");

        // 2001 non-ASCII BMP code points (U+00E9 'é' is 2 UTF-8 bytes each)
        let text_2001_bmp = "é".repeat(2001);
        assert_eq!(text_2001_bmp.len(), 4002); // 4002 bytes, 2001 code points
        let delta_2001_bmp = json!({
            "t": "delta",
            "ts": 100,
            "op": "add",
            "block": {"text": text_2001_bmp}
        });
        assert!(validate_browser_record(&delta_2001_bmp).is_ok());

        // 2001 supplementary-plane code points (U+1F600 '😀' is 4 UTF-8 bytes and 2 UTF-16 units each)
        let text_2001_supp = "😀".repeat(2001);
        assert_eq!(text_2001_supp.len(), 8004); // 8004 bytes, 2001 code points
        let delta_2001_supp = json!({
            "t": "delta",
            "ts": 100,
            "op": "add",
            "block": {"text": text_2001_supp}
        });
        assert!(validate_browser_record(&delta_2001_supp).is_ok());

        // 2002 supplementary-plane code points
        let text_2002_supp = "😀".repeat(2002);
        let delta_2002_supp = json!({
            "t": "delta",
            "ts": 100,
            "op": "add",
            "block": {"text": text_2002_supp}
        });
        assert_eq!(
            validate_browser_record(&delta_2002_supp).unwrap_err().cause,
            "limit"
        );
    }

    #[test]
    fn blocks_limit_and_n_limit_from_schema() {
        let max_items = *BLOCKS_MAX_ITEMS;
        assert_eq!(max_items, 1500);

        let mut blocks = Vec::with_capacity(max_items);
        for _ in 0..max_items {
            blocks.push(json!({"text": "row"}));
        }

        let snap_ok = json!({
            "t": "segment_start",
            "ts": 100,
            "n": max_items,
            "blocks": blocks
        });
        assert!(validate_browser_record(&snap_ok).is_ok());

        let snap_n_too_high = json!({
            "t": "segment_start",
            "ts": 100,
            "n": max_items + 1,
            "blocks": []
        });
        let err = validate_browser_record(&snap_n_too_high).unwrap_err();
        assert_eq!(err.cause, "limit");
        assert_eq!(err.field, "n");

        let mut blocks_too_many = Vec::with_capacity(max_items + 1);
        for _ in 0..=max_items {
            blocks_too_many.push(json!({"text": "row"}));
        }
        let snap_blocks_too_many = json!({
            "t": "segment_start",
            "ts": 100,
            "blocks": blocks_too_many
        });
        let err = validate_browser_record(&snap_blocks_too_many).unwrap_err();
        assert_eq!(err.cause, "limit");
        assert_eq!(err.field, "blocks");
    }

    #[test]
    fn jsonl_validation_enforces_lf_crlf_and_nonblank_start() {
        // Two segment_start rows with LF and CRLF
        let jsonl = b"{\"t\":\"segment_start\",\"ts\":100,\"blocks\":[{\"text\":\"first\"}]}\r\n{\"t\":\"segment_start\",\"ts\":200,\"blocks\":[{\"text\":\"second\"}]}\n";
        assert!(validate_browser_jsonl(jsonl).is_ok());

        // Malformed row 2 reports row=2
        let jsonl_bad_2 = b"{\"t\":\"segment_start\",\"ts\":100,\"blocks\":[{\"text\":\"first\"}]}\n{\"t\":\"delta\",\"ts\":200}\n";
        let err = validate_browser_jsonl(jsonl_bad_2).unwrap_err();
        assert_eq!(err.row, 2);

        // Unicode line separator characters stay inside a line
        let jsonl_unicode_separators = "{\"t\":\"segment_start\",\"ts\":100,\"title\":\"Line\u{0085}with\u{2028}separators\u{2029}\",\"blocks\":[{\"text\":\"content\"}]}\n";
        assert!(validate_browser_jsonl(jsonl_unicode_separators.as_bytes()).is_ok());

        // Empty and whitespace-only
        assert_eq!(validate_browser_jsonl(b"").unwrap_err().cause, "empty");
        assert_eq!(
            validate_browser_jsonl(b"   \n\r\n   ").unwrap_err().cause,
            "empty"
        );

        // Delta first
        let delta_first =
            b"{\"t\":\"delta\",\"ts\":100,\"op\":\"add\",\"block\":{\"text\":\"a\"}}\n";
        let err = validate_browser_jsonl(delta_first).unwrap_err();
        assert_eq!(err.row, 1);
        assert_eq!(err.cause, "delta_first");
    }

    #[test]
    fn timestamp_classification_rejects_fractional_negative_and_overflow() {
        let neg = json!({"t": "segment_start", "ts": -1, "blocks": []});
        assert_eq!(
            validate_browser_record(&neg).unwrap_err().cause,
            "ts_negative"
        );

        let frac = json!({"t": "segment_start", "ts": 1.5, "blocks": []});
        assert_eq!(
            validate_browser_record(&frac).unwrap_err().cause,
            "ts_fractional"
        );

        let overflow = json!({"t": "segment_start", "ts": u64::MAX, "blocks": []});
        assert_eq!(
            validate_browser_record(&overflow).unwrap_err().cause,
            "limit"
        );
    }

    #[test]
    fn raw_schema_and_admission_agree_on_numeric_representations() {
        for (raw, expected) in [
            (r#"{"t":"segment_start","ts":0,"blocks":[]}"#, true),
            (r#"{"t":"segment_start","ts":100.0,"blocks":[]}"#, true),
            (r#"{"t":"segment_start","ts":1e3,"blocks":[]}"#, true),
            (
                r#"{"t":"segment_start","ts":9007199254740991,"blocks":[]}"#,
                true,
            ),
            (
                r#"{"t":"segment_start","ts":9007199254740991.0,"blocks":[]}"#,
                true,
            ),
            (
                r#"{"t":"segment_start","ts":9007199254740992,"blocks":[]}"#,
                false,
            ),
            (
                r#"{"t":"segment_start","ts":9223372036854775808,"blocks":[]}"#,
                false,
            ),
            (r#"{"t":"segment_start","ts":-1,"blocks":[]}"#, false),
            (r#"{"t":"segment_start","ts":1.5,"blocks":[]}"#, false),
            (
                r#"{"t":"segment_start","ts":100,"n":1500.0,"blocks":[]}"#,
                true,
            ),
            (
                r#"{"t":"segment_start","ts":100,"n":1501,"blocks":[]}"#,
                false,
            ),
            (
                r#"{"t":"segment_start","ts":100,"n":1501.0,"blocks":[]}"#,
                false,
            ),
        ] {
            let value: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(COMPILED_SCHEMA.is_valid(&value), expected);
            assert_eq!(validate_browser_record(&value).is_ok(), expected);
            assert_eq!(validate_browser_jsonl(raw.as_bytes()).is_ok(), expected);
        }
    }

    #[test]
    fn metadata_bounds_are_owned_by_the_schema() {
        for (pointer, maximum) in [
            ("/title", 8192),
            ("/url", 32768),
            ("/site", 512),
            ("/adapter", 64),
            ("/ctx", 256),
            ("/inst", 128),
            ("/blocks/0/id", 256),
            ("/blocks/0/type", 64),
            ("/blocks/0/attrs/label", 300),
            ("/blocks/0/attrs/level", 16),
            ("/blocks/0/attrs/linkHost", 512),
        ] {
            for (length, expected) in [(maximum, true), (maximum + 1, false)] {
                let mut value = json!({
                    "t":"segment_start", "ts":100, "title":"", "url":"",
                    "site":"", "adapter":"", "ctx":"", "inst":"",
                    "blocks":[{"text":"text", "id":"", "type":"",
                        "attrs":{"label":"", "level":"", "linkHost":""}}]
                });
                *value.pointer_mut(pointer).unwrap() = json!("😀".repeat(length));
                assert_eq!(COMPILED_SCHEMA.is_valid(&value), expected, "{pointer}");
                assert_eq!(
                    validate_browser_record(&value).is_ok(),
                    expected,
                    "{pointer}"
                );
            }
        }
        for (depth, expected) in [
            (json!(0), true),
            (json!(4096), true),
            (json!(4097), false),
            (json!(-1), false),
            (json!(0.5), false),
            (json!("1"), false),
        ] {
            let value = json!({"t":"delta", "ts":100, "op":"add",
                "block":{"id":"x", "text":"text", "depth":depth}});
            assert_eq!(COMPILED_SCHEMA.is_valid(&value), expected);
            assert_eq!(validate_browser_record(&value).is_ok(), expected);
        }
    }

    #[test]
    fn display_omits_planted_marker_values() {
        let secret_marker = "SECRET_MARKER_DO_NOT_LEAK";
        let invalid = json!({
            "t": "delta",
            "ts": 100,
            "op": "add",
            "block": {"type": secret_marker} // missing required "text"
        });
        let err = validate_browser_record(&invalid).unwrap_err();
        let display = err.to_string();
        assert!(!display.contains(secret_marker));
        assert_eq!(display, "row=1 field=block cause=variant");
    }
}
