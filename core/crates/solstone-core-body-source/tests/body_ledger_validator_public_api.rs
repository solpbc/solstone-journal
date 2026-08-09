// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDigest, BodyEnvelope, BodyLedgerValidator, EnvelopeLedger, LedgerEventErrorCode,
    LedgerEventErrorField, ValidatedBodyLedger, decode_body_envelope,
};

mod support;

use support::native_bundle_fixture;

fn assert_traits<T: Clone + std::fmt::Debug + PartialEq + Eq>() {}

fn fixture_envelope(index: usize) -> (BodyEnvelope, Vec<u8>) {
    let case = &native_bundle_fixture()["cases"][index];
    (
        decode_body_envelope(
            case["expected_envelope_jsonl"]
                .as_str()
                .expect("envelope")
                .as_bytes(),
        )
        .expect("fixture envelope decodes"),
        case["expected_ledger_jsonl"]
            .as_str()
            .expect("ledger")
            .as_bytes()
            .to_vec(),
    )
}

fn validate_chunks(envelope: &BodyEnvelope, chunks: &[&[u8]]) -> ValidatedBodyLedger {
    let mut validator = BodyLedgerValidator::new(envelope);
    for chunk in chunks {
        validator.push(chunk).expect("fixture chunk validates");
    }
    validator.finish().expect("fixture validates")
}

fn with_digest(envelope: &BodyEnvelope, digest: BodyDigest) -> BodyEnvelope {
    BodyEnvelope::new(
        envelope.bundle_id().clone(),
        envelope.source_family(),
        envelope.source_hash().clone(),
        envelope.raw_retention(),
        envelope.row_count(),
        envelope.days().to_vec(),
        envelope.shards().to_vec(),
        EnvelopeLedger::new(
            envelope.bundle_id(),
            envelope.ledger().bytes(),
            envelope.ledger().events(),
            digest,
        )
        .expect("replacement descriptor is valid"),
        envelope.summary_plan().cloned(),
    )
    .expect("replacement envelope is valid")
}

#[test]
fn public_validator_handles_chunked_receipts_and_structured_errors() {
    assert_traits::<ValidatedBodyLedger>();

    let (envelope, data) = fixture_envelope(1);
    let mut validator = BodyLedgerValidator::new(&envelope);
    let first = data.len() / 3;
    let second = data.len() * 2 / 3;
    let unit: () = validator
        .push(&data[..first])
        .expect("successful push returns a unit payload");
    assert_eq!(unit, ());
    validator
        .push(&data[first..second])
        .expect("middle chunk validates");
    validator
        .push(&data[second..])
        .expect("final chunk validates");
    let receipt = validator.finish().expect("fixture validates");
    let clone = receipt.clone();

    assert_eq!(receipt, clone);
    assert_eq!(receipt.bundle_id(), envelope.bundle_id());
    assert_eq!(receipt.bytes(), 801);
    assert_eq!(receipt.events(), 1);
    assert_eq!(receipt.sha256(), envelope.ledger().sha256());

    let mut malformed = BodyLedgerValidator::new(&envelope);
    malformed
        .push(b"{")
        .expect("bounded partial frame is buffered until finish");
    let error = malformed.finish().expect_err("malformed frame refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::MalformedJson);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), 1);

    let empty = BodyLedgerValidator::new(&envelope);
    let error = empty.finish().expect_err("missing event refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::CountMismatch);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), 1);

    let digest = BodyDigest::from_bytes(
        b"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("digest is valid");
    let digest_envelope = with_digest(&envelope, digest);
    let mut digest_validator = BodyLedgerValidator::new(&digest_envelope);
    digest_validator
        .push(&data)
        .expect("digest twin frame validates before finish");
    let error = digest_validator
        .finish()
        .expect_err("digest mismatch refuses");
    assert_eq!(error.code(), LedgerEventErrorCode::ReferenceMismatch);
    assert_eq!(error.field(), LedgerEventErrorField::Ledger);
    assert_eq!(error.line(), 1);
}

#[test]
fn independently_validated_receipts_remain_equality_sensitive() {
    let (apple_empty_envelope, apple_empty_data) = fixture_envelope(2);
    let (oura_empty_envelope, oura_empty_data) = fixture_envelope(3);
    let apple_empty = validate_chunks(&apple_empty_envelope, &[&apple_empty_data]);
    let oura_empty = validate_chunks(&oura_empty_envelope, &[&oura_empty_data]);
    assert_ne!(apple_empty, oura_empty);

    let (apple_envelope, apple_data) = fixture_envelope(0);
    let (oura_envelope, oura_data) = fixture_envelope(1);
    let apple = validate_chunks(&apple_envelope, &[&apple_data]);
    let oura = validate_chunks(&oura_envelope, &[&oura_data]);
    assert_ne!(apple, oura);
}
