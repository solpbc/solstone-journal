// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{
    BodyDigest, BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField, EnvelopeLedger,
};

const BUNDLE: &str = "body-00000000000000000000000000";
const NONEMPTY_SHA256: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EMPTY_CONTENT_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn bundle() -> BundleId {
    BundleId::from_bytes(BUNDLE.as_bytes()).expect("test bundle is valid")
}

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

fn assert_error(
    error: &EnvelopeError,
    bundle: &BundleId,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
) {
    assert_eq!(error.bundle(), Some(bundle));
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
    assert_eq!(error.index(), None);

    let display = error.to_string();
    assert!(display.contains(bundle.as_str()));
    assert!(display.contains(field.as_str()));
    assert!(display.len() <= 122);
    assert_eq!(format!("{error:?}"), display);
    assert!(Error::source(error).is_none());
}

#[test]
fn envelope_ledger_precedence_count_parity_over_digest_state() {
    let bundle = bundle();
    for (bytes, events) in [(0, 1), (1, 0)] {
        for sha256 in [NONEMPTY_SHA256, EMPTY_CONTENT_SHA256] {
            let error = EnvelopeLedger::new(&bundle, bytes, events, digest(sha256))
                .expect_err("count parity mismatch must refuse");
            assert_error(
                &error,
                &bundle,
                EnvelopeErrorCode::IncompatibleField,
                EnvelopeErrorField::LedgerBytes,
            );
        }
    }
}

#[test]
fn envelope_ledger_precedence_count_parity_over_excess_events() {
    let bundle = bundle();
    let error = EnvelopeLedger::new(&bundle, 0, 5, digest(NONEMPTY_SHA256))
        .expect_err("count parity mismatch must take precedence");
    assert_error(
        &error,
        &bundle,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::LedgerBytes,
    );
}

#[test]
fn envelope_ledger_precedence_excess_events_over_digest_state() {
    let bundle = bundle();
    let error = EnvelopeLedger::new(&bundle, 1, 2, digest(EMPTY_CONTENT_SHA256))
        .expect_err("excess events must take precedence");
    assert_error(
        &error,
        &bundle,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::LedgerEvents,
    );
}

#[test]
fn envelope_ledger_rejects_empty_digest_for_positive_ledger() {
    let bundle = bundle();
    let error = EnvelopeLedger::new(&bundle, 1, 1, digest(EMPTY_CONTENT_SHA256))
        .expect_err("empty digest must refuse a positive ledger");
    assert_error(
        &error,
        &bundle,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::LedgerSha256,
    );
}

#[test]
fn envelope_ledger_rejects_nonempty_digest_for_empty_ledger() {
    let bundle = bundle();
    let error = EnvelopeLedger::new(&bundle, 0, 0, digest(NONEMPTY_SHA256))
        .expect_err("nonempty digest must refuse an empty ledger");
    assert_error(
        &error,
        &bundle,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::LedgerSha256,
    );
}

#[test]
fn envelope_ledger_failure_does_not_mutate_bundle_input() {
    let bundle = bundle();
    let before = bundle.clone();
    let error = EnvelopeLedger::new(&bundle, 0, 1, digest(NONEMPTY_SHA256))
        .expect_err("count parity mismatch must refuse");
    let after = bundle.clone();

    assert_eq!(before, bundle);
    assert_eq!(bundle, after);
    assert_eq!(error.bundle(), Some(&bundle));
}
