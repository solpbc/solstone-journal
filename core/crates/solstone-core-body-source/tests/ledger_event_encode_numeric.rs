// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyRawRetention, BodySourceFamily, BodySourceHash,
    BodyString, BodyValue, BundleId, EnvelopeLedger, EnvelopeShard, encode_body_ledger_event,
    parse,
};

use crate::support;

use support::{build_ledger_event, native_bundle_fixture};

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

#[test]
fn u64_max_sequence_and_line_use_exact_unsigned_decimal_spellings() {
    assert_numeric_encoding(u64::MAX, "18446744073709551615");
}

#[test]
fn adjacent_u64_sequence_and_line_use_exact_unsigned_decimal_spellings() {
    assert_numeric_encoding(u64::MAX - 1, "18446744073709551614");
}

fn assert_numeric_encoding(value: u64, expected: &str) {
    let frame = encode_body_ledger_event(&event(value)).unwrap();
    let encoded = String::from_utf8(frame.clone()).unwrap();
    let BodyValue::Object(object) = parse(&frame[..frame.len() - 1]).unwrap() else {
        unreachable!("encoded event is an object")
    };
    for field in ["line", "sequence"] {
        let needle = format!("\"{field}\":{expected}");
        assert_eq!(encoded.matches(&needle).count(), 1);
        assert!(!encoded.contains(&format!("\"{field}\":-{expected}")));
        assert!(!encoded.contains(&format!("\"{field}\":{expected}.")));
        assert!(!encoded.contains(&format!("\"{field}\":{expected}e")));
        assert!(!encoded.contains(&format!("\"{field}\":{expected}E")));
        let key = BodyString::from_code_points(field.bytes().map(u32::from).collect()).unwrap();
        let BodyValue::Integer(integer) = object.get(&key).unwrap() else {
            panic!("{field} must parse as an exact body integer")
        };
        assert!(!integer.is_negative());
        assert_eq!(integer.digits(), expected);
    }
}

fn event(value: u64) -> solstone_core_body_source::BodyLedgerEvent {
    let case = &native_bundle_fixture()["cases"][1];
    let bundle = BundleId::from_bytes(b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z").unwrap();
    let day = BodyDay::from_bytes(b"20260102").unwrap();
    let envelope_digest =
        digest("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let envelope = BodyEnvelope::new(
        bundle.clone(),
        BodySourceFamily::OuraApi,
        BodySourceHash::from_bytes_for_family(
            b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &BodySourceFamily::OuraApi,
        )
        .unwrap(),
        BodyRawRetention::RetainParsed,
        value,
        vec![day.clone()],
        vec![
            EnvelopeShard::new(
                &bundle,
                0,
                day.month(),
                value,
                value,
                envelope_digest.clone(),
            )
            .unwrap(),
        ],
        EnvelopeLedger::new(&bundle, value, value, envelope_digest).unwrap(),
        None,
    )
    .unwrap();
    let mut row =
        serde_json::from_str::<Value>(case["expected_normalized_jsonl"].as_str().unwrap()).unwrap();
    row["normalized_ref"] = json!(format!(
        "imports/{}/normalized/2026-01.jsonl#L{value}",
        bundle.as_str()
    ));
    let expected = serde_json::from_str::<Value>(
        case["expected_ledger_jsonl"]
            .as_str()
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    build_ledger_event(
        &envelope,
        &serde_json::to_string(&row).unwrap(),
        0,
        value,
        value,
        None,
        digest(expected["value_hash"].as_str().unwrap()),
    )
}
