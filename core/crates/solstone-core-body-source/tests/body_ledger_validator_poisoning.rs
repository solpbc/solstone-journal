// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyLedgerValidator, LedgerEventErrorCode, LedgerEventErrorField, decode_body_envelope,
};

mod support;

use support::native_bundle_fixture;

#[test]
fn first_malformed_frame_poisoning_wins_over_later_data() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let valid = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes();
    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(&[b"{\n".as_slice(), valid].concat());
    validator.push(valid);
    validator.push(b"extra");

    let error = validator
        .finish()
        .expect_err("malformed first frame refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::MalformedJson);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), 1);
}

#[test]
fn overrun_in_the_same_chunk_is_not_inspected_as_a_frame() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let valid = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes();
    let mut input = valid.to_vec();
    input.extend_from_slice(b"{");

    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(&input);
    validator.push(b"\n");
    let error = validator.finish().expect_err("overrun refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::CountMismatch);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), 2);
}

#[test]
fn empty_pushes_do_not_change_poisoned_or_unpoisoned_state() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(b"");
    validator.push(b"{\n");
    validator.push(b"");
    let error = validator.finish().expect_err("malformed frame refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::MalformedJson);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), 1);
}
