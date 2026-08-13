// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use solstone_core_body_source::{
    BodyDigest, BodyLedgerEvent, LedgerEventErrorCode, LedgerEventErrorField, decode_body_envelope,
    encode_body_ledger_event,
};

use crate::support;

use support::{build_ledger_event, native_bundle_fixture};

const MAX_LEDGER_EVENT_OBJECT_BYTES: usize = 65_536;
const MAX_LEDGER_EVENT_FRAME_BYTES: usize = 65_537;
const MAX_PEAK_BYTES: u64 = 150_000;
const MAX_SMALL_PEAK_BYTES: u64 = 20_000;

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("fixture digest is valid")
}

#[test]
fn terminal_framing_accepts_an_object_at_the_object_cap() {
    let event = boundary_event(MAX_LEDGER_EVENT_OBJECT_BYTES);
    let encoded = encode_body_ledger_event(&event).expect("boundary event encodes");
    assert_eq!(canonical_len(&event), MAX_LEDGER_EVENT_OBJECT_BYTES);
    assert_eq!(encoded.len(), MAX_LEDGER_EVENT_FRAME_BYTES);
    assert_eq!(encoded.last(), Some(&b'\n'));
}

#[test]
fn object_one_byte_over_the_cap_refuses_before_terminal_framing() {
    let event = boundary_event(MAX_LEDGER_EVENT_OBJECT_BYTES + 1);
    assert_eq!(canonical_len(&event), MAX_LEDGER_EVENT_OBJECT_BYTES + 1);
    assert_overflow(encode_body_ledger_event(&event), &event);
}

#[test]
fn megabyte_body_string_refuses_with_bounded_encoder_peak() {
    let event = event_with_raw_suffix(1_048_576);
    assert_overflow(encode_body_ledger_event(&event), &event);
    let info = allocation_counter::measure(|| {
        drop(encode_body_ledger_event(&event));
    });
    assert!(
        info.bytes_max <= MAX_PEAK_BYTES,
        "peak was {} bytes",
        info.bytes_max
    );
}

#[test]
fn small_event_does_not_preallocate_the_full_cap() {
    let event = event_with_raw_suffix(1);
    let info = allocation_counter::measure(|| {
        assert!(encode_body_ledger_event(&event).is_ok());
    });
    assert!(
        info.bytes_max <= MAX_SMALL_PEAK_BYTES,
        "small-event peak was {} bytes",
        info.bytes_max
    );
}

#[test]
fn every_escape_width_uses_emitted_bytes_at_the_exact_boundary() {
    let baseline = event_with_raw_suffix(1);
    let remaining = MAX_LEDGER_EVENT_OBJECT_BYTES - canonical_len(&baseline);
    let ascii = event_with_raw_suffix(1 + remaining);
    let prefix = format!("imports/{}/raw/oura/", ascii.bundle_id().as_str());

    assert_eq!(canonical_len(&ascii), MAX_LEDGER_EVENT_OBJECT_BYTES);
    assert_boundary_frame(&ascii);
    for (tail, emitted_bytes) in [
        ("\"", 2),
        ("\\", 2),
        ("\u{0001}", 6),
        ("\u{001c}", 6),
        ("\u{0100}", 6),
        ("🧠", 12),
    ] {
        let escaped = event_with_raw_ref(format!(
            "{}{}{}",
            prefix,
            "a".repeat(1 + remaining - emitted_bytes),
            tail
        ));
        assert_boundary_frame(&escaped);
        let escaped_over = event_with_raw_ref(format!(
            "{}{}{}",
            prefix,
            "a".repeat(2 + remaining - emitted_bytes),
            tail
        ));
        assert_eq!(
            canonical_len(&escaped_over),
            MAX_LEDGER_EVENT_OBJECT_BYTES + 1
        );
        assert_overflow(encode_body_ledger_event(&escaped_over), &escaped_over);
    }

    let lone_surrogate = event_with_raw_json_tail(1 + remaining - 6, "\\ud800");
    assert_eq!(
        lone_surrogate.raw_ref().unwrap().code_points().last(),
        Some(&0xd800)
    );
    assert_boundary_frame(&lone_surrogate);
    let lone_surrogate_over = event_with_raw_json_tail(2 + remaining - 6, "\\ud800");
    assert_eq!(
        canonical_len(&lone_surrogate_over),
        MAX_LEDGER_EVENT_OBJECT_BYTES + 1
    );
    assert_overflow(
        encode_body_ledger_event(&lone_surrogate_over),
        &lone_surrogate_over,
    );
}

fn assert_boundary_frame(event: &BodyLedgerEvent) {
    assert_eq!(canonical_len(event), MAX_LEDGER_EVENT_OBJECT_BYTES);
    assert_eq!(
        encode_body_ledger_event(event).unwrap().len(),
        MAX_LEDGER_EVENT_FRAME_BYTES
    );
}

fn boundary_event(target: usize) -> BodyLedgerEvent {
    let baseline = event_with_raw_suffix(1);
    let remaining = target - canonical_len(&baseline);
    let event = event_with_raw_suffix(1 + remaining);
    assert_eq!(canonical_len(&event), target);
    event
}

fn event_with_raw_suffix(count: usize) -> BodyLedgerEvent {
    let bundle = "body-01J9ZK2F5M7Q8R3S4T6V0W1X2Z";
    event_with_raw_ref(format!("imports/{bundle}/raw/oura/{}", "a".repeat(count)))
}

fn event_with_raw_ref(raw_ref: String) -> BodyLedgerEvent {
    let case = &native_bundle_fixture()["cases"][1];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()).unwrap();
    let mut row =
        serde_json::from_str::<Value>(case["expected_normalized_jsonl"].as_str().unwrap()).unwrap();
    row["raw_ref"] = Value::String(raw_ref);
    let expected =
        serde_json::from_str::<Value>(case["expected_ledger_jsonl"].as_str().unwrap()).unwrap();
    build_ledger_event(
        &envelope,
        &serde_json::to_string(&row).unwrap(),
        0,
        1,
        1,
        None,
        digest(expected["value_hash"].as_str().unwrap()),
    )
}

fn event_with_raw_json_tail(ascii_count: usize, escaped_tail: &str) -> BodyLedgerEvent {
    let case = &native_bundle_fixture()["cases"][1];
    let envelope =
        decode_body_envelope(case["expected_envelope_jsonl"].as_str().unwrap().as_bytes()).unwrap();
    let row = case["expected_normalized_jsonl"]
        .as_str()
        .unwrap()
        .trim_end();
    let marker = "\"raw_ref\":\"";
    let start = row.find(marker).unwrap() + marker.len();
    let end = row[start..].find('"').unwrap() + start;
    let prefix = format!("imports/{}/raw/oura/", envelope.bundle_id().as_str());
    let raw_json = format!(
        "{}{}{}{}{}",
        &row[..start],
        prefix,
        "a".repeat(ascii_count),
        escaped_tail,
        &row[end..]
    );
    let expected =
        serde_json::from_str::<Value>(case["expected_ledger_jsonl"].as_str().unwrap()).unwrap();
    build_ledger_event(
        &envelope,
        &raw_json,
        0,
        1,
        1,
        None,
        digest(expected["value_hash"].as_str().unwrap()),
    )
}

fn canonical_len(event: &BodyLedgerEvent) -> usize {
    let fields = [
        field_len("bundle_id", quoted_ascii_len(event.bundle_id().as_str())),
        field_len("day", quoted_ascii_len(event.day().as_str())),
        field_len("dedupe_key", quoted_ascii_len(event.dedupe_key().as_str())),
        field_len("end_time", optional_body_string_len(event.end_time())),
        field_len("line", digits_len(event.line())),
        field_len("normalized_ref", body_string_len(event.normalized_ref())),
        field_len("raw_ref", optional_body_string_len(event.raw_ref())),
        field_len("record_type", body_string_len(event.record_type())),
        field_len("row_schema", quoted_ascii_len(event.row_schema().as_str())),
        field_len("row_sha256", quoted_ascii_len(event.row_sha256().as_str())),
        field_len("schema", quoted_ascii_len(event.schema())),
        field_len("sequence", digits_len(event.sequence())),
        field_len("shard", quoted_ascii_len(event.shard())),
        field_len(
            "source_family",
            quoted_ascii_len(event.source_family().as_str()),
        ),
        field_len(
            "source_record_id",
            optional_body_string_len(event.source_record_id()),
        ),
        field_len("start_time", body_string_len(event.start_time())),
        field_len("value_hash", quoted_ascii_len(event.value_hash().as_str())),
    ];
    2 + fields.into_iter().sum::<usize>() + fields.len() - 1
}

fn field_len(key: &str, value: usize) -> usize {
    quoted_ascii_len(key) + 1 + value
}

fn quoted_ascii_len(value: &str) -> usize {
    value.len() + 2
}

fn optional_body_string_len(value: Option<&solstone_core_body_source::BodyString>) -> usize {
    value.map_or(4, body_string_len)
}

fn body_string_len(value: &solstone_core_body_source::BodyString) -> usize {
    2 + value
        .code_points()
        .iter()
        .map(|code_point| match code_point {
            0x22 | 0x5c | 0x08 | 0x0c | 0x0a | 0x0d | 0x09 => 2,
            0x00..=0x1f | 0x7f..=0xffff => 6,
            0x20..=0x7e => 1,
            0x10000..=0x10ffff => 12,
            _ => unreachable!("BodyString stores only valid code points"),
        })
        .sum::<usize>()
}

fn digits_len(value: u64) -> usize {
    value.to_string().len()
}

fn assert_overflow(
    result: Result<Vec<u8>, solstone_core_body_source::LedgerEventError>,
    event: &BodyLedgerEvent,
) {
    let error = result.expect_err("oversized event must refuse");
    assert_eq!(error.bundle(), Some(event.bundle_id()));
    assert_eq!(error.code(), LedgerEventErrorCode::InputTooLarge);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), event.sequence());
}
