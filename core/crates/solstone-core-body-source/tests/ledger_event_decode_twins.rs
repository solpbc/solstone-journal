// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};
use solstone_core_body_source::{
    BodyDay, BodyDigest, BodyEnvelope, BodyRawRetention, BodySourceFamily, BodySourceHash,
    EnvelopeLedger, EnvelopeShard, LedgerEventErrorCode, LedgerEventErrorField,
    decode_body_envelope, decode_body_ledger_event, encode_body_ledger_event,
};

mod support;

use support::{build_ledger_event, native_bundle_fixture};

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

fn native_context(index: usize) -> (BodyEnvelope, Value, Value) {
    let case = &native_bundle_fixture()["cases"][index];
    (
        decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope frame")
                .as_bytes(),
        )
        .expect("fixture envelope decodes"),
        serde_json::from_str(
            case["expected_normalized_jsonl"]
                .as_str()
                .expect("normalized row"),
        )
        .expect("normalized row parses"),
        serde_json::from_str(
            case["expected_ledger_jsonl"]
                .as_str()
                .expect("ledger frame"),
        )
        .expect("ledger frame parses"),
    )
}

fn round_trip(envelope: &BodyEnvelope, event: solstone_core_body_source::BodyLedgerEvent) {
    let frame = encode_body_ledger_event(&event).expect("event encodes");
    let untouched_frame = frame.clone();
    let untouched_envelope = envelope.clone();
    let decoded =
        decode_body_ledger_event(&frame, envelope, event.sequence()).expect("event decodes");
    assert_eq!(decoded, event);
    assert_eq!(frame, untouched_frame);
    assert_eq!(*envelope, untouched_envelope);
}

#[test]
fn both_native_families_and_all_nullable_combinations_round_trip_purely() {
    for index in 0..2 {
        let (envelope, row, expected) = native_context(index);
        let event = build_ledger_event(
            &envelope,
            &serde_json::to_string(&row).unwrap(),
            0,
            1,
            1,
            None,
            digest(expected["value_hash"].as_str().expect("value hash")),
        );
        round_trip(&envelope, event);
    }

    let (envelope, Value::Object(row), expected) = native_context(1) else {
        unreachable!("native row is object")
    };
    for mask in 0..8 {
        let mut row = row.clone();
        for (bit, (field, present)) in [
            (0, ("end_date", json!("2026-01-03"))),
            (
                1,
                (
                    "raw_ref",
                    json!(format!(
                        "imports/{}/raw/oura/nullable",
                        envelope.bundle_id().as_str()
                    )),
                ),
            ),
            (2, ("source_record_id", json!("nullable-source-record"))),
        ] {
            row.insert(
                field.to_owned(),
                if mask & (1 << bit) == 0 {
                    Value::Null
                } else {
                    present
                },
            );
        }
        let event = build_ledger_event(
            &envelope,
            &serde_json::to_string(&Value::Object(row)).unwrap(),
            0,
            1,
            1,
            None,
            digest(expected["value_hash"].as_str().expect("value hash")),
        );
        round_trip(&envelope, event);
    }
}

#[test]
fn caller_digests_and_string_code_points_round_trip_exactly() {
    let (envelope, row, expected) = native_context(1);
    let row_text = serde_json::to_string(&row).expect("row serializes");
    for (row_sha256, value_hash) in [
        (
            digest("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            digest("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ),
        (
            digest("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            digest("sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
        ),
    ] {
        let event = build_ledger_event(
            &envelope,
            &row_text,
            0,
            1,
            1,
            Some(row_sha256.clone()),
            value_hash.clone(),
        );
        round_trip(&envelope, event.clone());
        assert_eq!(event.row_sha256(), &row_sha256);
        assert_eq!(event.value_hash(), &value_hash);
    }
    for escaped in [
        "\\u001c",
        "\\u0085",
        "\\u3000",
        "\\u4e00",
        "\\ud83e\\udde0",
        "\\ud800",
    ] {
        let row = row_with_string_suffix(
            &row_with_string_suffix(&row_text, "record_type", escaped),
            "raw_ref",
            escaped,
        );
        let event = build_ledger_event(
            &envelope,
            &row,
            0,
            1,
            1,
            None,
            digest(expected["value_hash"].as_str().expect("value hash")),
        );
        round_trip(&envelope, event);
    }
}

#[test]
fn maximum_and_adjacent_unsigned_locations_remain_distinct_and_decoding_is_pure_on_failure() {
    for value in [u64::MAX - 1, u64::MAX] {
        let (envelope, event) = maximum_event(value);
        let frame = encode_body_ledger_event(&event).expect("event encodes");
        let untouched_frame = frame.clone();
        let untouched_envelope = envelope.clone();
        let decoded = decode_body_ledger_event(&frame, &envelope, value).expect("event decodes");
        assert_eq!(decoded.sequence(), value);
        assert_eq!(decoded.line(), value);
        assert_eq!(frame, untouched_frame);
        assert_eq!(envelope, untouched_envelope);
    }

    let (envelope, _) = maximum_event(u64::MAX);
    let frame = b"{".to_vec();
    let untouched_frame = frame.clone();
    let untouched_envelope = envelope.clone();
    let error = decode_body_ledger_event(&frame, &envelope, u64::MAX).expect_err("malformed");
    assert_eq!(error.code(), LedgerEventErrorCode::MalformedJson);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(frame, untouched_frame);
    assert_eq!(envelope, untouched_envelope);
}

fn maximum_event(value: u64) -> (BodyEnvelope, solstone_core_body_source::BodyLedgerEvent) {
    let case = &native_bundle_fixture()["cases"][1];
    let bundle =
        solstone_core_body_source::BundleId::from_bytes(b"body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z")
            .unwrap();
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
    let mut row: Value =
        serde_json::from_str(case["expected_normalized_jsonl"].as_str().unwrap()).unwrap();
    row["normalized_ref"] = json!(format!(
        "imports/{}/normalized/2026-01.jsonl#L{value}",
        bundle.as_str()
    ));
    let expected: Value =
        serde_json::from_str(case["expected_ledger_jsonl"].as_str().unwrap()).unwrap();
    let event = build_ledger_event(
        &envelope,
        &serde_json::to_string(&row).unwrap(),
        0,
        value,
        value,
        None,
        digest(expected["value_hash"].as_str().unwrap()),
    );
    (envelope, event)
}

fn row_with_string_suffix(row: &str, field: &str, suffix: &str) -> String {
    let marker = format!("\"{field}\":\"");
    let start = row.find(&marker).expect("field marker") + marker.len();
    let end = row[start..].find('"').expect("field terminator") + start;
    format!("{}{}{}", &row[..end], suffix, &row[end..])
}
