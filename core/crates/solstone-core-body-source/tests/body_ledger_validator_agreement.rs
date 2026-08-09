// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerValidator, EnvelopeLedger, LedgerEventErrorCode,
    LedgerEventErrorField, decode_body_envelope,
};

mod support;

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
        validator.push(
            case["expected_ledger_jsonl"]
                .as_str()
                .expect("ledger")
                .as_bytes(),
        );
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
    validator.push(data);
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
    one_frame.push(&data[..first_end]);
    assert_error(
        one_frame.finish(),
        &envelope,
        LedgerEventErrorCode::CountMismatch,
        2,
    );

    let mut two_frames = BodyLedgerValidator::new(&envelope);
    two_frames.push(&data[..second_end]);
    assert_error(
        two_frames.finish(),
        &envelope,
        LedgerEventErrorCode::CountMismatch,
        3,
    );

    for bytes in [2492_u64, 2494] {
        let twin = with_ledger(&envelope, bytes, envelope.ledger().sha256().clone());
        let mut validator = BodyLedgerValidator::new(&twin);
        validator.push(data);
        assert_error(
            validator.finish(),
            &twin,
            LedgerEventErrorCode::CountMismatch,
            3,
        );
    }

    let alternate = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("alternate digest is valid");
    let digest_twin = with_ledger(&envelope, 2493, alternate.clone());
    let mut digest_validator = BodyLedgerValidator::new(&digest_twin);
    digest_validator.push(data);
    assert_error(
        digest_validator.finish(),
        &digest_twin,
        LedgerEventErrorCode::ReferenceMismatch,
        3,
    );

    let combined_twin = with_ledger(&envelope, 2492, alternate);
    let mut combined_validator = BodyLedgerValidator::new(&combined_twin);
    combined_validator.push(data);
    assert_error(
        combined_validator.finish(),
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
    validator.push(b"");
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
    validator.push(frame);
    validator.push(b"{");
    assert_error(
        validator.finish(),
        &envelope,
        LedgerEventErrorCode::CountMismatch,
        2,
    );
}
