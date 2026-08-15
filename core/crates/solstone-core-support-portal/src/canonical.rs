// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible canonical operation bytes and keyed derivations.

use std::fmt::Write;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

/// Versioned canonical namespace included in every HMAC input.
pub const CANONICAL_NAMESPACE: &str = "solstone.support.operation-key.v1";

const VERB_FIELDS: &[(&str, &[&str])] = &[
    (
        "create",
        &[
            "product",
            "subject",
            "description",
            "severity",
            "category",
            "user_email",
            "user_context",
            "anonymous",
        ],
    ),
    ("reply", &["ticket_id", "content"]),
    (
        "attach",
        &[
            "ticket_id",
            "filename",
            "content_type",
            "byte_size",
            "content_sha256",
        ],
    ),
    (
        "feedback",
        &["product", "body", "user_email", "user_context", "anonymous"],
    ),
    ("close", &["ticket_id"]),
    ("resolved", &["ticket_id"]),
    ("still_need_help", &["ticket_id"]),
];

/// Return the permitted field names for a support operation verb.
pub fn verb_fields(verb: &str) -> Option<&'static [&'static str]> {
    VERB_FIELDS
        .iter()
        .find_map(|(candidate, fields)| (*candidate == verb).then_some(*fields))
}

/// Derive the length-prefixed child action ID used as the on-disk record name.
pub fn derive_child_action_id(parent_action_id: &str, verb: &str, index: u64) -> String {
    let mut input = Vec::new();
    for part in [
        normalize(parent_action_id),
        normalize(verb),
        index.to_string(),
    ] {
        let bytes = part.as_bytes();
        input.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        input.extend_from_slice(bytes);
    }
    let digest = Sha256::digest(input);
    format!("sact1_{}", URL_SAFE_NO_PAD.encode(digest))
}

/// Emit strict canonical JSON bytes without delegating HMAC bytes to serde_json.
pub fn canonicalize_operation(
    verb: &str,
    fields: &Map<String, Value>,
    principal: &str,
    child_action_id: &str,
) -> Result<Vec<u8>, String> {
    let field_order =
        verb_fields(verb).ok_or_else(|| format!("unsupported support operation verb: {verb}"))?;
    if !(principal == "anonymous"
        || principal
            .strip_prefix("jkt:")
            .is_some_and(|tag| !tag.is_empty()))
    {
        return Err("principal must be anonymous or a jkt thumbprint".to_owned());
    }
    if let Some(unknown) = fields
        .keys()
        .find(|name| !field_order.contains(&name.as_str()))
    {
        return Err(format!("unsupported operation fields: {unknown:?}"));
    }

    let mut output = String::new();
    output.push_str("{\"namespace\":");
    emit_string(&mut output, CANONICAL_NAMESPACE);
    output.push_str(",\"principal\":");
    emit_string(&mut output, &normalize(principal));
    output.push_str(",\"verb\":");
    emit_string(&mut output, &normalize(verb));
    output.push_str(",\"child_action_id\":");
    emit_string(&mut output, &normalize(child_action_id));
    output.push_str(",\"fields\":[");
    for (index, field) in field_order.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str("{\"name\":");
        emit_string(&mut output, field);
        output.push_str(",\"value\":{\"present\":");
        if let Some(value) = fields.get(*field) {
            output.push_str("true,\"value\":");
            emit_value(&mut output, value)?;
        } else {
            output.push_str("false");
        }
        output.push_str("}}");
    }
    output.push_str("]}");
    Ok(output.into_bytes())
}

/// Derive the keyed canonical fingerprint retained in the record.
pub fn canonical_fingerprint(key: &[u8], canonical: &[u8]) -> String {
    hmac_hex(key, b"portal-fingerprint\0", canonical)
}

/// Derive the keyed principal tag retained in the record.
pub fn principal_tag(key: &[u8], principal: &str) -> String {
    hmac_hex(key, b"portal-principal\0", normalize(principal).as_bytes())
}

/// Derive the server idempotency key, which is never persisted.
pub fn operation_key(key: &[u8], canonical: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(b"portal-key\0");
    mac.update(canonical);
    format!(
        "spk1_{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

fn hmac_hex(key: &[u8], prefix: &[u8], payload: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(prefix);
    mac.update(payload);
    hex(&mac.finalize().into_bytes())
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn emit_value(output: &mut String, value: &Value) -> Result<(), String> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => emit_number(output, value)?,
        Value::String(value) => emit_string(output, &normalize(value)),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                emit_value(output, value)?;
            }
            output.push(']');
        }
        Value::Object(values) => emit_object(output, values)?,
    }
    Ok(())
}

fn emit_object(output: &mut String, values: &Map<String, Value>) -> Result<(), String> {
    let mut normalized = Vec::with_capacity(values.len());
    for (key, value) in values {
        let key = normalize(key);
        normalized.push((key, value));
    }
    normalized.sort_unstable_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    if normalized.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("operation map keys collide after normalization".to_owned());
    }
    output.push('{');
    for (index, (key, value)) in normalized.into_iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        emit_string(output, &key);
        output.push(':');
        emit_value(output, value)?;
    }
    output.push('}');
    Ok(())
}

fn emit_number(output: &mut String, value: &Number) -> Result<(), String> {
    if value.is_i64() || value.is_u64() {
        output.push_str(&value.to_string());
        return Ok(());
    }
    let value = value
        .as_f64()
        .ok_or_else(|| "unsupported operation number".to_owned())?;
    if !value.is_finite() {
        return Err("operation values cannot contain non-finite floats".to_owned());
    }
    output.push_str(&python_float(value));
    Ok(())
}

/// Render an IEEE-754 `f64` using Python `repr(float)` JSON notation rules.
fn python_float(value: f64) -> String {
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("Rust scientific f64 formatting contains an exponent");
    let exponent = exponent
        .parse::<i32>()
        .expect("Rust scientific f64 exponent is numeric");
    if (-4..16).contains(&exponent) {
        fixed_from_scientific(mantissa, exponent)
    } else {
        let sign = if exponent >= 0 { '+' } else { '-' };
        format!("{mantissa}e{sign}{:02}", exponent.unsigned_abs())
    }
}

fn fixed_from_scientific(mantissa: &str, exponent: i32) -> String {
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |rest| ("-", rest));
    let mut digits = mantissa.replace('.', "");
    let decimal = 1 + exponent;
    let body = if decimal <= 0 {
        format!(
            "0.{}{}",
            "0".repeat(decimal.unsigned_abs() as usize),
            digits
        )
    } else if decimal as usize >= digits.len() {
        digits.push_str(&"0".repeat(decimal as usize - digits.len()));
        format!("{digits}.0")
    } else {
        let point = decimal as usize;
        format!("{}.{}", &digits[..point], &digits[point..])
    };
    format!("{sign}{body}")
}

fn emit_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{0c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            character if character <= '\u{1f}' => {
                let _ = write!(output, "\\u{:04x}", character as u32);
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

fn normalize(value: &str) -> String {
    value.nfc().collect()
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::*;

    // Provenance (repo root): python3 -c 'import sys,hmac,hashlib;sys.path.insert(0,".");from solstone.apps.support import operations as o;k=bytes.fromhex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");c=bytes.fromhex("7b226e616d657370616365223a22766563746f72222c227061796c6f6164223a22617574686f726564227d");z=bytes([0]);print(o._digest(k,b"portal-fingerprint"+z,c));print(o._digest(k,b"portal-principal"+z,b"jkt:vector-principal"));print("spk1_"+o._b64url(hmac.new(k,b"portal-key"+z+c,hashlib.sha256).digest()))'
    // Reference: b0976d3a707f8545562085c8589bd907ec188f05
    #[test]
    fn hmac_derivations_match_reference_vector() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let canonical = b"{\"namespace\":\"vector\",\"payload\":\"authored\"}";
        assert_eq!(
            canonical_fingerprint(&key, canonical),
            "9a6f6681d8e5c7bc5d107651951ee3e350eede4df1050443c7b61b72224ffbf6"
        );
        assert_eq!(
            principal_tag(&key, "jkt:vector-principal"),
            "9f672a0003b959771600212e9839907272f58d87fec06b137f2ad042365e48c3"
        );
        assert_eq!(
            operation_key(&key, canonical),
            "spk1_9_iR-GMJFOMHRTFNv-puODdkHXq7olb-YIqfZ1kIZhk"
        );
    }

    // Provenance (repo root): python3 -c 'import sys;sys.path.insert(0,".");from solstone.apps.support.operations import derive_child_action_id as d;print(d("parent-with-dash","reply",3));print(d("a","bc",0));print(d("ab","c",0))'
    // Reference: b0976d3a707f8545562085c8589bd907ec188f05
    #[test]
    fn child_action_id_length_prefix_vectors_match_reference() {
        assert_eq!(
            derive_child_action_id("parent-with-dash", "reply", 3),
            "sact1_mhmeoe9A663QcOxfhiKB4C7udtgFwjX5pVWiepeCTtw"
        );
        assert_eq!(
            derive_child_action_id("a", "bc", 0),
            "sact1_a1ENOzUdcNB_M5rHYKSZ-aYG3Mx52ycUuq2YTkzUh2I"
        );
        assert_eq!(
            derive_child_action_id("ab", "c", 0),
            "sact1_-0yw7Z0c8vBb0nvr-i8-OY1ktoQ25tbrGS6jOt1bvv4"
        );
    }

    // Provenance (repo root): python3 -c 'import sys;sys.path.insert(0,".");from solstone.apps.support.operations import canonicalize_operation as c;f={"product":"desktop","subject":"subject","description":"description","severity":"low","category":"bug","user_email":"owner@example.test","user_context":{"中":"han","a":"ascii","e"+chr(0x301):"nfc"},"anonymous":False};print(c("create",f,principal="jkt:vector-principal",child_action_id="sact1_vector"));del f["severity"];print(c("create",f,principal="jkt:vector-principal",child_action_id="sact1_vector"));f["severity"]="low";f["subject"]="Cafe"+chr(0x301);print(c("create",f,principal="jkt:vector-principal",child_action_id="sact1_vector"))'
    // Reference: b0976d3a707f8545562085c8589bd907ec188f05
    #[test]
    fn canonical_bytes_match_reference_cases() {
        let mut fields = Map::new();
        fields.insert("product".into(), json!("desktop"));
        fields.insert("subject".into(), json!("subject"));
        fields.insert("description".into(), json!("description"));
        fields.insert("severity".into(), json!("low"));
        fields.insert("category".into(), json!("bug"));
        fields.insert("user_email".into(), json!("owner@example.test"));
        fields.insert(
            "user_context".into(),
            json!({"中": "han", "a": "ascii", "e\u{301}": "nfc"}),
        );
        fields.insert("anonymous".into(), json!(false));
        let full =
            canonicalize_operation("create", &fields, "jkt:vector-principal", "sact1_vector")
                .expect("valid vector");
        let expected = "{\"namespace\":\"solstone.support.operation-key.v1\",\"principal\":\"jkt:vector-principal\",\"verb\":\"create\",\"child_action_id\":\"sact1_vector\",\"fields\":[{\"name\":\"product\",\"value\":{\"present\":true,\"value\":\"desktop\"}},{\"name\":\"subject\",\"value\":{\"present\":true,\"value\":\"subject\"}},{\"name\":\"description\",\"value\":{\"present\":true,\"value\":\"description\"}},{\"name\":\"severity\",\"value\":{\"present\":true,\"value\":\"low\"}},{\"name\":\"category\",\"value\":{\"present\":true,\"value\":\"bug\"}},{\"name\":\"user_email\",\"value\":{\"present\":true,\"value\":\"owner@example.test\"}},{\"name\":\"user_context\",\"value\":{\"present\":true,\"value\":{\"a\":\"ascii\",\"é\":\"nfc\",\"中\":\"han\"}}},{\"name\":\"anonymous\",\"value\":{\"present\":true,\"value\":false}}]}";
        assert_eq!(full, expected.as_bytes());

        fields.remove("severity");
        let missing =
            canonicalize_operation("create", &fields, "jkt:vector-principal", "sact1_vector")
                .expect("valid missing-field vector");
        let missing_expected = expected.replace(
            "{\"name\":\"severity\",\"value\":{\"present\":true,\"value\":\"low\"}}",
            "{\"name\":\"severity\",\"value\":{\"present\":false}}",
        );
        assert_eq!(missing, missing_expected.as_bytes());
        fields.insert("severity".into(), json!("low"));
        fields.insert("subject".into(), json!("Cafe\u{301}"));
        let nfc = canonicalize_operation("create", &fields, "jkt:vector-principal", "sact1_vector")
            .expect("valid nfc vector");
        let nfc_expected = expected.replacen("\"value\":\"subject\"", "\"value\":\"Café\"", 1);
        assert_eq!(nfc, nfc_expected.as_bytes());
    }

    // Provenance (repo root): python3 -c 'import sys,hmac,hashlib;sys.path.insert(0,".");from solstone.apps.support.operations import canonicalize_operation as c,derive_child_action_id as d,_b64url;p="draft-portal-vector";child=d(p,"reply",0);raw=c("reply",{"ticket_id":"T-42","content":"reference vector"},principal="jkt:vector-thumbprint",child_action_id=child);k=bytes.fromhex("f0e0d0c0b0a090807060504030201000112233445566778899aabbccddeeff00");print(child);print(raw);print("spk1_"+_b64url(hmac.new(k,b"portal-key"+bytes([0])+raw,hashlib.sha256).digest()))'
    // Reference: b0976d3a707f8545562085c8589bd907ec188f05
    #[test]
    fn end_to_end_operation_key_matches_reference_vector() {
        let mut fields = Map::new();
        fields.insert("ticket_id".into(), Value::String("T-42".into()));
        fields.insert("content".into(), Value::String("reference vector".into()));
        let child = derive_child_action_id("draft-portal-vector", "reply", 0);
        let canonical = canonicalize_operation("reply", &fields, "jkt:vector-thumbprint", &child)
            .expect("valid reference vector");
        let key = [
            0xf0, 0xe0, 0xd0, 0xc0, 0xb0, 0xa0, 0x90, 0x80, 0x70, 0x60, 0x50, 0x40, 0x30, 0x20,
            0x10, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
            0xdd, 0xee, 0xff, 0x00,
        ];
        assert_eq!(child, "sact1_wMRTK84_vw40j7hc-Qcr7Q0ilWZyoy7gJfAneD1c1QU");
        assert_eq!(canonical, b"{\"namespace\":\"solstone.support.operation-key.v1\",\"principal\":\"jkt:vector-thumbprint\",\"verb\":\"reply\",\"child_action_id\":\"sact1_wMRTK84_vw40j7hc-Qcr7Q0ilWZyoy7gJfAneD1c1QU\",\"fields\":[{\"name\":\"ticket_id\",\"value\":{\"present\":true,\"value\":\"T-42\"}},{\"name\":\"content\",\"value\":{\"present\":true,\"value\":\"reference vector\"}}]}".to_vec());
        assert_eq!(
            operation_key(&key, &canonical),
            "spk1_T3BvYM9PPdlK6rN0ran8ZXekXFHNZJRadTWli8IrQ-0"
        );
    }

    // Provenance (repo root): python3 -c 'import json;v=(0.0,-0.0,1.0,-1.5,1e-4,1e-5,1e-6,1e-3,1e14,1e15,1e16,1e17,1e-7,5e-324,1.7976931348623157e308,0.1+0.2);print([(repr(x),json.dumps(x,separators=(",",":"))) for x in v])'
    // Reference: b0976d3a707f8545562085c8589bd907ec188f05
    #[test]
    fn python_float_rendering_vectors_match_reference() {
        let vectors = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (-1.5, "-1.5"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1e-6, "1e-06"),
            (1e-3, "0.001"),
            (1e14, "100000000000000.0"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1e17, "1e+17"),
            (1e-7, "1e-07"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (0.1 + 0.2, "0.30000000000000004"),
        ];
        for (value, expected) in vectors {
            assert_eq!(python_float(value), expected, "{value:?}");
        }
    }

    #[test]
    fn canonicalization_refuses_the_typed_invalid_inputs_the_reference_refuses() {
        let fields = Map::new();
        assert!(canonicalize_operation("unknown", &fields, "anonymous", "sact1_vector").is_err());

        let mut unknown_field = Map::new();
        unknown_field.insert("not_a_reply_field".to_owned(), Value::Null);
        assert!(
            canonicalize_operation("reply", &unknown_field, "anonymous", "sact1_vector").is_err()
        );

        assert!(canonicalize_operation("reply", &fields, "jkt:", "sact1_vector").is_err());

        let mut colliding_map = Map::new();
        colliding_map.insert("é".to_owned(), Value::Null);
        colliding_map.insert("e\u{301}".to_owned(), Value::Null);
        let mut colliding_fields = Map::new();
        colliding_fields.insert("ticket_id".to_owned(), Value::Object(colliding_map));
        assert!(
            canonicalize_operation("reply", &colliding_fields, "anonymous", "sact1_vector")
                .is_err()
        );
    }
}
