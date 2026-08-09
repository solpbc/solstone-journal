// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyLedgerValidator, LedgerEventErrorCode, LedgerEventErrorField, decode_body_envelope,
};

mod support;

use support::{ledger_events_fixture, native_bundle_fixture};

fn assert_error(validator: BodyLedgerValidator<'_>, code: LedgerEventErrorCode, line: u64) {
    let error = validator.finish().expect_err("framing refuses");
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), line);
}

#[test]
fn missing_final_lf_uses_the_shared_decoder_path() {
    for case in [
        &native_bundle_fixture()["cases"][0],
        &ledger_events_fixture()["cases"][0],
    ] {
        let envelope = decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope")
                .as_bytes(),
        )
        .expect("fixture envelope decodes");
        let frame = case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger")
            .as_bytes();
        let mut validator = BodyLedgerValidator::new(&envelope);
        validator.push(&frame[..frame.len() - 1]);
        assert_error(
            validator,
            LedgerEventErrorCode::NoncanonicalJson,
            frame.iter().filter(|byte| **byte == b'\n').count() as u64,
        );
    }
}

#[test]
fn blank_and_partial_frames_are_checked_as_first_ledger_event() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");

    for frame in [b"\n".as_slice(), b"{".as_slice()] {
        let mut validator = BodyLedgerValidator::new(&envelope);
        validator.push(frame);
        assert_error(validator, LedgerEventErrorCode::MalformedJson, 1);
    }

    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(b"{}\n");
    let error = validator.finish().expect_err("missing schema refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::MissingField);
    assert_eq!(error.field(), LedgerEventErrorField::Schema);
    assert_eq!(error.line(), 1);
}

#[test]
fn frame_cap_is_checked_before_scanning_or_buffering() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");

    let mut exact_cap = vec![b'x'; 65_536];
    exact_cap.push(b'\n');
    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(&exact_cap);
    assert_error(validator, LedgerEventErrorCode::MalformedJson, 1);

    let mut over_cap = vec![b'x'; 65_537];
    over_cap.push(b'\n');
    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(&over_cap);
    assert_error(validator, LedgerEventErrorCode::InputTooLarge, 1);
}
