// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! This test rebuilds every byte input and expected native output from the
//! immutable Python-reference fixture.  It deliberately does not copy its rows.

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_steward_prune::{Disposition, WholeNoopReason, classify_prune};

const ORACLE: &str = include_str!("../../../fixtures/steward-prune-reference.json");

fn decode_base64(value: &str) -> Vec<u8> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let mut output = Vec::new();
    for chunk in value.as_bytes().chunks(4) {
        assert_eq!(chunk.len(), 4, "fixture base64 is padded");
        let first = digit(chunk[0]).expect("valid base64");
        let second = digit(chunk[1]).expect("valid base64");
        output.push((first << 2) | (second >> 4));
        if chunk[2] != b'=' {
            let third = digit(chunk[2]).expect("valid base64");
            output.push((second << 4) | (third >> 2));
            if chunk[3] != b'=' {
                output.push((third << 6) | digit(chunk[3]).expect("valid base64"));
            }
        }
    }
    output
}

fn recipe_bytes(recipe: &Value) -> Vec<u8> {
    let mut output = Vec::new();
    for chunk in recipe.as_array().expect("recipe is an array") {
        if let Some(base64) = chunk.get("b64").and_then(Value::as_str) {
            output.extend(decode_base64(base64));
        } else {
            let byte = u8::from_str_radix(
                chunk
                    .get("byte_hex")
                    .and_then(Value::as_str)
                    .expect("hex byte"),
                16,
            )
            .expect("a byte");
            let repeat = chunk.get("repeat").and_then(Value::as_u64).expect("repeat") as usize;
            output.extend(std::iter::repeat_n(byte, repeat));
        }
    }
    output
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fixture_counts(case: &Value) -> (u64, u64) {
    case.get("logger_calls")
        .and_then(Value::as_array)
        .and_then(|calls| calls.first())
        .and_then(|call| call.get("args"))
        .and_then(Value::as_array)
        .filter(|args| args.len() == 2)
        .map(|args| {
            (
                args[0].as_u64().expect("aged count"),
                args[1].as_u64().expect("malformed count"),
            )
        })
        .unwrap_or((0, 0))
}

// The fixture records Python's log counters and native bytes, but compatibility
// bookkeeping is native-only. These are exactly its compatibility-bearing rows.
fn fixture_compatibility(id: &str) -> u64 {
    match id {
        "nan-row-local-keep" => 1,
        "lone-surrogate-timestamp-row-local-keep" | "lone-surrogate-nontimestamp-json-valid" => 2,
        "nested-timestamp-depth-128-row-local-keep"
        | "nested-timestamp-depth-129-row-local-keep"
        | "nested-timestamp-depth-1100-row-local-keep" => 1,
        _ => 0,
    }
}

fn noop_reason(id: &str) -> Option<WholeNoopReason> {
    Some(match id {
        "positive-infinity-whole-noop"
        | "negative-infinity-whole-noop"
        | "finite-exponent-overflow-whole-noop" => WholeNoopReason::NumericOverflow,
        "json-literal-4301-digits-whole-noop" => WholeNoopReason::IntegerDigitLimit,
        "invalid-utf8-whole-noop" => WholeNoopReason::InvalidUtf8,
        "nested-timestamp-python-recursion-whole-noop" => WholeNoopReason::RecursionLimit,
        _ => return None,
    })
}

#[test]
fn immutable_oracle_is_reproduced_byte_for_byte() {
    assert_eq!(
        hex(&Sha256::digest(ORACLE.as_bytes())),
        "81f95489052c79e3eaaacd5d969e30a01f5c9f39f9ba3abdca0436be5cdab297"
    );
    let document: Value = serde_json::from_str(ORACLE).expect("fixture is JSON");
    let now_ms = document
        .get("now_ms")
        .and_then(Value::as_i64)
        .expect("now_ms");
    let cases = document
        .get("cases")
        .and_then(Value::as_array)
        .expect("cases");
    assert_eq!(cases.len(), 18);

    for case in cases {
        let id = case.get("id").and_then(Value::as_str).expect("case id");
        let input = recipe_bytes(case.get("input_recipe").expect("input recipe"));
        let expected = recipe_bytes(case.get("native_expected_recipe").expect("native recipe"));
        assert_eq!(
            hex(&Sha256::digest(&input)),
            case.get("input_sha256")
                .and_then(Value::as_str)
                .expect("input digest"),
            "{id}"
        );
        assert_eq!(
            hex(&Sha256::digest(&expected)),
            case.get("native_expected_sha256")
                .and_then(Value::as_str)
                .expect("output digest"),
            "{id}"
        );

        let result = classify_prune(&input, now_ms);
        assert_eq!(result.output, expected, "{id}: output");
        let (aged, malformed) = fixture_counts(case);
        assert_eq!(
            (result.aged, result.malformed),
            (aged, malformed),
            "{id}: Python counters"
        );
        assert_eq!(
            result.compatibility_kept,
            fixture_compatibility(id),
            "{id}: compatibility counter"
        );
        match noop_reason(id) {
            Some(reason) => assert_eq!(result.disposition, Disposition::WholeNoop(reason), "{id}"),
            None if aged + malformed == 0 => {
                assert_eq!(result.disposition, Disposition::NoChange, "{id}")
            }
            None => assert_eq!(
                result.disposition,
                Disposition::Rewrite {
                    dropped: aged + malformed
                },
                "{id}"
            ),
        }
    }
}
