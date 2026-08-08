// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_body_source::{
    BodyDigest, BodyMonth, BundleId, EnvelopeErrorCode, EnvelopeErrorField, EnvelopeShard,
};

const MIN_BUNDLE: &str = "body-00000000000000000000000000";
const MAX_BUNDLE: &str = "body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
const MONTHS: [&str; 2] = ["0001-01", "9999-12"];
const BUNDLES: [&str; 2] = [MIN_BUNDLE, MAX_BUNDLE];
const INDEXES: [u64; 3] = [0, 1, u64::MAX];
const VALID_PAIRS: [(u64, u64); 3] = [(1, 1), (u64::MAX, 1), (u64::MAX, u64::MAX)];
const NONEMPTY_SHA256: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const EMPTY_CONTENT_SHA256: &str =
    "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn bundle(value: &str) -> BundleId {
    BundleId::from_bytes(value.as_bytes()).expect("boundary bundle is valid")
}

fn month(value: &str) -> BodyMonth {
    BodyMonth::from_bytes(value.as_bytes()).expect("boundary month is valid")
}

fn digest(value: &str) -> BodyDigest {
    BodyDigest::from_bytes(value.as_bytes()).expect("boundary digest is valid")
}

#[test]
fn envelope_shard_crosses_every_valid_coordinate_with_nonempty_digest() {
    let mut combinations = 0;
    for month_text in MONTHS {
        for bundle_text in BUNDLES {
            for index in INDEXES {
                for (bytes, rows) in VALID_PAIRS {
                    let bundle = bundle(bundle_text);
                    let shard = EnvelopeShard::new(
                        &bundle,
                        index,
                        month(month_text),
                        bytes,
                        rows,
                        digest(NONEMPTY_SHA256),
                    )
                    .unwrap_or_else(|error| {
                        panic!(
                            "{month_text} {bundle_text} {index} {bytes}/{rows} should bind: {error}"
                        )
                    });
                    assert_eq!(shard.path(), format!("normalized/{month_text}.jsonl"));
                    assert_eq!(shard.month().as_str(), month_text);
                    assert_eq!(shard.bytes(), bytes);
                    assert_eq!(shard.rows(), rows);
                    assert_eq!(shard.sha256().as_str(), NONEMPTY_SHA256);
                    combinations += 1;
                }
            }
        }
    }
    assert_eq!(combinations, 36);
}

#[test]
fn envelope_shard_crosses_every_valid_coordinate_with_empty_digest() {
    let mut combinations = 0;
    for month_text in MONTHS {
        for bundle_text in BUNDLES {
            for index in INDEXES {
                for (bytes, rows) in VALID_PAIRS {
                    let bundle = bundle(bundle_text);
                    let error = EnvelopeShard::new(
                        &bundle,
                        index,
                        month(month_text),
                        bytes,
                        rows,
                        digest(EMPTY_CONTENT_SHA256),
                    )
                    .expect_err("empty content digest must refuse");
                    assert_eq!(error.code(), EnvelopeErrorCode::IncompatibleField);
                    assert_eq!(error.field(), EnvelopeErrorField::ShardSha256);
                    assert_eq!(error.bundle(), Some(&bundle));
                    assert_eq!(error.index(), Some(index));
                    combinations += 1;
                }
            }
        }
    }
    assert_eq!(combinations, 36);
}

fn assert_boundary_error(
    bytes: u64,
    rows: u64,
    expected_code: EnvelopeErrorCode,
    expected_field: EnvelopeErrorField,
) {
    let bundle = bundle(MIN_BUNDLE);
    let error = EnvelopeShard::new(
        &bundle,
        0,
        month("0001-01"),
        bytes,
        rows,
        digest(NONEMPTY_SHA256),
    )
    .expect_err("invalid boundary must refuse");
    assert_eq!(error.code(), expected_code);
    assert_eq!(error.field(), expected_field);
}

#[test]
fn envelope_shard_rejects_zero_bytes_boundary() {
    assert_boundary_error(
        0,
        1,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardBytes,
    );
}

#[test]
fn envelope_shard_rejects_zero_rows_boundary() {
    assert_boundary_error(
        1,
        0,
        EnvelopeErrorCode::InvalidField,
        EnvelopeErrorField::ShardRows,
    );
}

#[test]
fn envelope_shard_rejects_adjacent_rows_greater_than_bytes_boundary() {
    assert_boundary_error(
        1,
        2,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::ShardRows,
    );
}

#[test]
fn envelope_shard_rejects_wide_rows_greater_than_bytes_boundary() {
    assert_boundary_error(
        1,
        u64::MAX,
        EnvelopeErrorCode::IncompatibleField,
        EnvelopeErrorField::ShardRows,
    );
}
