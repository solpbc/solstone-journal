// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    LedgerEventErrorCode, LedgerEventErrorField, decode_body_envelope, decode_body_ledger_event,
    encode_body_ledger_event,
};

use crate::support;

use support::native_bundle_fixture;

#[test]
fn public_decoder_returns_a_readable_reencodable_checked_event() {
    let case = &native_bundle_fixture()["cases"][1];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope frame")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let frame = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger frame");
    let event = decode_body_ledger_event(frame.as_bytes(), &envelope, 1).expect("event decodes");

    let _ = (
        event.schema(),
        event.bundle_id(),
        event.sequence(),
        event.row_schema(),
        event.shard(),
        event.line(),
        event.normalized_ref(),
        event.row_sha256(),
        event.dedupe_key(),
        event.source_family(),
        event.source_record_id(),
        event.record_type(),
        event.start_time(),
        event.end_time(),
        event.day(),
        event.value_hash(),
        event.raw_ref(),
    );
    assert_eq!(encode_body_ledger_event(&event).unwrap(), frame.as_bytes());
}

#[test]
fn public_decoder_exposes_structured_scan_field_and_reference_errors() {
    let case = &native_bundle_fixture()["cases"][1];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope frame")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    for (frame, code, field) in [
        (
            b"{".as_slice(),
            LedgerEventErrorCode::MalformedJson,
            LedgerEventErrorField::Ledger,
        ),
        (
            b"{}\n",
            LedgerEventErrorCode::MissingField,
            LedgerEventErrorField::Schema,
        ),
    ] {
        let error = decode_body_ledger_event(frame, &envelope, 1).expect_err("refuses");
        assert_eq!(error.bundle(), Some(envelope.bundle_id()));
        assert_eq!(error.line(), 1);
        assert_eq!(error.code(), code);
        assert_eq!(error.field(), field);
    }
    let frame = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger frame")
        .replace("\"sequence\":1", "\"sequence\":2");
    let error = decode_body_ledger_event(frame.as_bytes(), &envelope, 1).expect_err("refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::InvalidSequence);
    assert_eq!(error.field(), LedgerEventErrorField::Sequence);
    assert_eq!(error.bundle(), Some(envelope.bundle_id()));
    assert_eq!(error.line(), 1);
}
