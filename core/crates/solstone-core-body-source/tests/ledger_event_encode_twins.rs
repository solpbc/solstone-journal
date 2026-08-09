// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};
use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerEvent, decode_body_envelope, encode_body_ledger_event,
};

mod support;

use support::{build_ledger_event, native_bundle_fixture};

const KEYS: [&str; 17] = [
    "bundle_id",
    "day",
    "dedupe_key",
    "end_time",
    "line",
    "normalized_ref",
    "raw_ref",
    "record_type",
    "row_schema",
    "row_sha256",
    "schema",
    "sequence",
    "shard",
    "source_family",
    "source_record_id",
    "start_time",
    "value_hash",
];

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("fixture digest is valid")
}

fn context() -> (
    BodyEnvelope,
    Map<String, Value>,
    String,
    BodyDigest,
    BodyDigest,
) {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()).unwrap();
    let row = case["expected_normalized_jsonl"]
        .as_str()
        .unwrap()
        .trim_end()
        .to_owned();
    let Value::Object(object) = serde_json::from_str(&row).unwrap() else {
        unreachable!("fixture row is an object")
    };
    let expected =
        serde_json::from_str::<Value>(case["expected_ledger_jsonl"].as_str().unwrap()).unwrap();
    (
        envelope,
        object,
        row,
        digest(expected["row_sha256"].as_str().unwrap()),
        digest(expected["value_hash"].as_str().unwrap()),
    )
}

fn event(
    envelope: &BodyEnvelope,
    row: Map<String, Value>,
    row_sha256: BodyDigest,
    value_hash: BodyDigest,
) -> BodyLedgerEvent {
    build_ledger_event(
        envelope,
        &serde_json::to_string(&Value::Object(row)).unwrap(),
        0,
        1,
        1,
        Some(row_sha256),
        value_hash,
    )
}

fn object(frame: &[u8]) -> Value {
    assert_eq!(frame.last(), Some(&b'\n'));
    serde_json::from_slice(&frame[..frame.len() - 1]).unwrap()
}

fn field_bytes<'a>(frame: &'a [u8], key: &str) -> &'a [u8] {
    let prefix = format!("\"{key}\":");
    let start = frame
        .windows(prefix.len())
        .position(|window| window == prefix.as_bytes())
        .unwrap()
        + prefix.len();
    let tail = &frame[start..frame.len() - 2];
    let end = tail
        .iter()
        .position(|byte| *byte == b',')
        .unwrap_or(tail.len());
    &tail[..end]
}

fn assert_only_field_diff(baseline: &BodyLedgerEvent, twin: &BodyLedgerEvent, changed: &str) {
    let baseline = encode_body_ledger_event(baseline).unwrap();
    let twin = encode_body_ledger_event(twin).unwrap();
    let baseline_object = object(&baseline);
    let twin_object = object(&twin);
    for value in [&baseline_object, &twin_object] {
        assert_eq!(
            value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            KEYS.into_iter().collect(),
        );
    }
    for key in KEYS {
        if key == changed {
            assert_ne!(field_bytes(&baseline, key), field_bytes(&twin, key));
        } else {
            assert_eq!(field_bytes(&baseline, key), field_bytes(&twin, key));
            assert_eq!(baseline_object[key], twin_object[key]);
        }
    }
}

fn assert_only_field_bytes_diff(baseline: &BodyLedgerEvent, twin: &BodyLedgerEvent, changed: &str) {
    let baseline = encode_body_ledger_event(baseline).unwrap();
    let twin = encode_body_ledger_event(twin).unwrap();
    for key in KEYS {
        if key == changed {
            assert_ne!(field_bytes(&baseline, key), field_bytes(&twin, key));
        } else {
            assert_eq!(field_bytes(&baseline, key), field_bytes(&twin, key));
        }
    }
}

#[test]
fn optional_field_twins_change_only_their_canonical_value() {
    let (envelope, row, _, row_sha256, value_hash) = context();
    let baseline = event(
        &envelope,
        row.clone(),
        row_sha256.clone(),
        value_hash.clone(),
    );

    let mut no_end_time = row.clone();
    no_end_time.insert("end_date".into(), Value::Null);
    assert_only_field_diff(
        &baseline,
        &event(
            &envelope,
            no_end_time,
            row_sha256.clone(),
            value_hash.clone(),
        ),
        "end_time",
    );

    let mut no_raw_ref = row.clone();
    no_raw_ref.insert("raw_ref".into(), Value::Null);
    assert_only_field_diff(
        &baseline,
        &event(
            &envelope,
            no_raw_ref,
            row_sha256.clone(),
            value_hash.clone(),
        ),
        "raw_ref",
    );

    let mut source_record_id = row;
    source_record_id.insert("source_record_id".into(), json!("synthetic-record-1"));
    assert_only_field_diff(
        &baseline,
        &event(&envelope, source_record_id, row_sha256, value_hash),
        "source_record_id",
    );
}

#[test]
fn body_string_fields_encode_each_supported_code_point_class() {
    let (envelope, row, row_text, row_sha256, value_hash) = context();
    let baseline = event(&envelope, row, row_sha256.clone(), value_hash.clone());
    for (points, suffix, escaped) in [
        (vec![0xd800], "\\ud800", "\\ud800"),
        (vec![0x0001], "\\u0001", "\\u0001"),
        (vec![0x0080], "\\u0080", "\\u0080"),
        (vec![0x4e00], "\\u4e00", "\\u4e00"),
        (vec![0x1f9e0], "\\ud83e\\udde0", "\\ud83e\\udde0"),
    ] {
        let event = build_ledger_event(
            &envelope,
            &row_with_raw_ref_suffix(&row_text, suffix),
            0,
            1,
            1,
            Some(row_sha256.clone()),
            value_hash.clone(),
        );
        assert_eq!(event.raw_ref().unwrap().code_points().last(), points.last());
        let encoded = encode_body_ledger_event(&event).unwrap();
        assert!(
            field_bytes(&encoded, "raw_ref")
                .windows(escaped.len())
                .any(|window| window == escaped.as_bytes())
        );
        assert_only_field_bytes_diff(&baseline, &event, "raw_ref");
    }
}

fn row_with_raw_ref_suffix(row: &str, suffix: &str) -> String {
    let marker = "\"raw_ref\":\"";
    let start = row.find(marker).unwrap() + marker.len();
    let end = row[start..].find('"').unwrap() + start;
    format!("{}{}{}", &row[..end], suffix, &row[end..])
}
