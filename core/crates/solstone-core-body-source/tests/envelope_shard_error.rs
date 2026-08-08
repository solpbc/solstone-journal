// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;

use solstone_core_body_source::{
    BodyDigest, BodyMonth, BundleId, EnvelopeError, EnvelopeErrorCode, EnvelopeErrorField,
    EnvelopeShard,
};

const BUNDLE: &str = "body-00000000000000000000000000";
const NONEMPTY_SHA256: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EMPTY_CONTENT_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn bundle() -> BundleId {
    BundleId::from_bytes(BUNDLE.as_bytes()).expect("test bundle is valid")
}

fn month() -> BodyMonth {
    BodyMonth::from_bytes(b"2026-01").expect("test month is valid")
}

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("test digest is valid")
}

fn assert_error(
    error: &EnvelopeError,
    bundle: &BundleId,
    index: u64,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
) {
    assert_eq!(error.bundle(), Some(bundle));
    assert_eq!(error.code(), code);
    assert_eq!(error.field(), field);
    assert_eq!(error.index(), Some(index));

    let display = error.to_string();
    assert!(display.contains(bundle.as_str()));
    assert!(display.contains(field.as_str()));
    assert!(display.len() <= 122);
    assert_eq!(format!("{error:?}"), display);
    assert!(Error::source(error).is_none());
}

#[test]
fn envelope_shard_precedence_bytes_over_zero_rows_and_empty_digest() {
    let bundle = bundle();
    let error = EnvelopeShard::new(&bundle, 0, month(), 0, 0, digest(EMPTY_CONTENT_SHA256))
        .expect_err("zero bytes must take precedence");
    assert_error(
        &error,
        &bundle,
        0,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardBytes,
    );
}

#[test]
fn envelope_shard_precedence_bytes_over_excess_rows_and_empty_digest() {
    let bundle = bundle();
    let error = EnvelopeShard::new(&bundle, 1, month(), 0, 1, digest(EMPTY_CONTENT_SHA256))
        .expect_err("zero bytes must take precedence");
    assert_error(
        &error,
        &bundle,
        1,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardBytes,
    );
}

#[test]
fn envelope_shard_precedence_zero_rows_over_empty_digest() {
    let bundle = bundle();
    let error = EnvelopeShard::new(&bundle, 2, month(), 1, 0, digest(EMPTY_CONTENT_SHA256))
        .expect_err("zero rows must take precedence");
    assert_error(
        &error,
        &bundle,
        2,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardRows,
    );
}

#[test]
fn envelope_shard_precedence_excess_rows_over_empty_digest() {
    let bundle = bundle();
    let error = EnvelopeShard::new(&bundle, 3, month(), 1, 2, digest(EMPTY_CONTENT_SHA256))
        .expect_err("excess rows must take precedence");
    assert_error(
        &error,
        &bundle,
        3,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::ShardRows,
    );
}

#[test]
fn envelope_shard_failure_does_not_mutate_bundle_input() {
    let bundle = bundle();
    let before = bundle.clone();
    let error = EnvelopeShard::new(&bundle, 4, month(), 0, 1, digest(NONEMPTY_SHA256))
        .expect_err("zero bytes must refuse");
    let after = bundle.clone();

    assert_eq!(before, bundle);
    assert_eq!(bundle, after);
    assert_eq!(error.bundle(), Some(&bundle));
}
