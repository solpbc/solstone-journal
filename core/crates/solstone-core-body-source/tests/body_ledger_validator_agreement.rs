// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerValidator, EnvelopeLedger, LedgerEventError,
    LedgerEventErrorCode, LedgerEventErrorField, decode_body_envelope,
};

use crate::support;

use support::{ledger_events_fixture, native_bundle_fixture};

fn assert_error(
    result: Result<
        solstone_core_body_source::ValidatedBodyLedger,
        solstone_core_body_source::LedgerEventError,
    >,
    envelope: &BodyEnvelope,
    code: LedgerEventErrorCode,
    line: u64,
) {
    let error = result.expect_err("validator refuses");
    assert_structured_error(&error, envelope, code, line);
}

fn assert_structured_error(
    error: &LedgerEventError,
    envelope: &BodyEnvelope,
    code: LedgerEventErrorCode,
    line: u64,
) {
    assert_eq!(error.bundle(), Some(envelope.bundle_id()));
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), line);
}

fn with_ledger(envelope: &BodyEnvelope, bytes: u64, sha256: BodyDigest) -> BodyEnvelope {
    let ledger = EnvelopeLedger::new(
        envelope.bundle_id(),
        bytes,
        envelope.ledger().events(),
        sha256,
    )
    .expect("replacement descriptor is self-consistent");
    BodyEnvelope::new(
        envelope.bundle_id().clone(),
        envelope.source_family(),
        envelope.source_hash().clone(),
        envelope.raw_retention(),
        envelope.row_count(),
        envelope.days().to_vec(),
        envelope.shards().to_vec(),
        ledger,
        envelope.summary_plan().cloned(),
    )
    .expect("replacement envelope is checked")
}

#[test]
fn fixture_receipts_agree_with_checked_ledger_descriptors() {
    for case in native_bundle_fixture()["cases"]
        .as_array()
        .expect("native cases")
    {
        let envelope = decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope")
                .as_bytes(),
        )
        .expect("fixture envelope decodes");
        let mut validator = BodyLedgerValidator::new(&envelope);
        validator
            .push(
                case["expected_ledger_jsonl"]
                    .as_str()
                    .expect("ledger")
                    .as_bytes(),
            )
            .expect("fixture ledger push validates");
        let receipt = validator.finish().expect("fixture validates");
        assert_eq!(receipt.bytes(), envelope.ledger().bytes());
        assert_eq!(receipt.events(), envelope.ledger().events());
        assert_eq!(receipt.sha256(), envelope.ledger().sha256());
    }

    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let data = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes();
    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(data).expect("fixture ledger push validates");
    let receipt = validator.finish().expect("fixture validates");
    assert_eq!(receipt.bytes(), 2493);
    assert_eq!(receipt.events(), 3);
    assert_eq!(
        receipt.sha256().as_str(),
        "sha256:3a3bccf5f2049f113f05123cf3adc6f90416e17505754b4f7bf586b5ebfc8ae2"
    );
}

#[test]
fn count_and_descriptor_mismatches_follow_declared_precedence() {
    let case = &ledger_events_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let data = case["expected_ledger_jsonl"]
        .as_str()
        .expect("ledger")
        .as_bytes();
    let first_end = data.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    let second_end = data[first_end..]
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap()
        + first_end
        + 1;

    let mut one_frame = BodyLedgerValidator::new(&envelope);
    one_frame
        .push(&data[..first_end])
        .expect("first frame validates");
    assert_error(
        one_frame.finish(),
        &envelope,
        LedgerEventErrorCode::CountMismatch,
        2,
    );

    let mut two_frames = BodyLedgerValidator::new(&envelope);
    two_frames
        .push(&data[..second_end])
        .expect("first two frames validate");
    assert_error(
        two_frames.finish(),
        &envelope,
        LedgerEventErrorCode::CountMismatch,
        3,
    );

    let short_twin = with_ledger(&envelope, 2492, envelope.ledger().sha256().clone());
    let mut short_validator = BodyLedgerValidator::new(&short_twin);
    let short_error = short_validator
        .push(data)
        .expect_err("declared byte boundary refuses the final byte");
    assert_structured_error(
        &short_error,
        &short_twin,
        LedgerEventErrorCode::CountMismatch,
        3,
    );
    assert_eq!(
        short_validator
            .finish()
            .expect_err("poison remains terminal"),
        short_error
    );

    let long_twin = with_ledger(&envelope, 2494, envelope.ledger().sha256().clone());
    let mut long_validator = BodyLedgerValidator::new(&long_twin);
    long_validator
        .push(data)
        .expect("all supplied bytes are within the declared boundary");
    assert_error(
        long_validator.finish(),
        &long_twin,
        LedgerEventErrorCode::CountMismatch,
        3,
    );

    let alternate = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("alternate digest is valid");
    let digest_twin = with_ledger(&envelope, 2493, alternate.clone());
    let mut digest_validator = BodyLedgerValidator::new(&digest_twin);
    digest_validator
        .push(data)
        .expect("digest twin bytes and count validate");
    assert_error(
        digest_validator.finish(),
        &digest_twin,
        LedgerEventErrorCode::ReferenceMismatch,
        3,
    );

    let combined_twin = with_ledger(&envelope, 2492, alternate);
    let mut combined_validator = BodyLedgerValidator::new(&combined_twin);
    let combined_error = combined_validator
        .push(data)
        .expect_err("byte mismatch precedes digest mismatch");
    assert_structured_error(
        &combined_error,
        &combined_twin,
        LedgerEventErrorCode::CountMismatch,
        3,
    );
}

#[test]
fn zero_bytes_against_a_positive_envelope_is_an_initial_count_mismatch() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let mut validator = BodyLedgerValidator::new(&envelope);
    validator.push(b"").expect("empty push is a no-op");
    assert_error(
        validator.finish(),
        &envelope,
        LedgerEventErrorCode::CountMismatch,
        1,
    );
}

#[test]
fn additional_bytes_after_a_complete_native_ledger_are_count_mismatch() {
    let case = &native_bundle_fixture()["cases"][0];
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
    validator.push(frame).expect("declared ledger validates");
    let first = validator
        .push(b"{")
        .expect_err("additional byte refuses immediately");
    assert_structured_error(&first, &envelope, LedgerEventErrorCode::CountMismatch, 2);
    assert_eq!(
        validator.finish().expect_err("poison remains terminal"),
        first
    );
}

#[test]
fn declared_byte_boundary_is_enforced_before_lf_scanning() {
    let case = &native_bundle_fixture()["cases"][0];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("fixture envelope decodes");
    let alternate = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("alternate digest is valid");
    let one_byte = with_ledger(&envelope, 1, alternate);
    let mut validator = BodyLedgerValidator::new(&one_byte);
    let error = validator
        .push(b"{\n")
        .expect_err("the LF is beyond the declared byte boundary");
    assert_structured_error(&error, &one_byte, LedgerEventErrorCode::CountMismatch, 1);

    let mut split = BodyLedgerValidator::new(&one_byte);
    split
        .push(b"{")
        .expect("the exact declared byte remains buffered");
    let split_error = split
        .push(b"\n")
        .expect_err("the next chunk crosses the declared byte boundary");
    assert_eq!(split_error, error);
}

#[test]
fn empty_authority_refuses_any_nonempty_input_immediately() {
    let case = &native_bundle_fixture()["cases"][2];
    let envelope = decode_body_envelope(
        case["expected_envelope_jsonl"]
            .as_str()
            .expect("envelope")
            .as_bytes(),
    )
    .expect("empty fixture envelope decodes");
    let mut validator = BodyLedgerValidator::new(&envelope);
    let error = validator
        .push(b"x")
        .expect_err("empty authority refuses any byte");
    assert_structured_error(&error, &envelope, LedgerEventErrorCode::CountMismatch, 1);
}
