// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashSet};
use std::hash::Hash;

use solstone_core_body_source::{EnvelopeErrorCode, EnvelopeErrorField};

fn code_spelling(code: EnvelopeErrorCode) -> &'static str {
    match code {
        EnvelopeErrorCode::InputTooLarge => "input_too_large",
        EnvelopeErrorCode::MalformedJson => "malformed_json",
        EnvelopeErrorCode::NoncanonicalJson => "noncanonical_json",
        EnvelopeErrorCode::MissingField => "missing_field",
        EnvelopeErrorCode::UnknownField => "unknown_field",
        EnvelopeErrorCode::WrongType => "wrong_type",
        EnvelopeErrorCode::InvalidField => "invalid_field",
        EnvelopeErrorCode::IncompatibleField => "incompatible_field",
        EnvelopeErrorCode::CountMismatch => "count_mismatch",
        EnvelopeErrorCode::ManifestMismatch => "manifest_mismatch",
    }
}

fn field_spelling(field: EnvelopeErrorField) -> &'static str {
    match field {
        EnvelopeErrorField::Envelope => "envelope",
        EnvelopeErrorField::Schema => "schema",
        EnvelopeErrorField::BundleId => "bundle_id",
        EnvelopeErrorField::SourceFamily => "source_family",
        EnvelopeErrorField::SourceHash => "source_hash",
        EnvelopeErrorField::RawRetention => "raw_retention",
        EnvelopeErrorField::RowCount => "row_count",
        EnvelopeErrorField::Days => "days",
        EnvelopeErrorField::Shards => "shards",
        EnvelopeErrorField::ShardPath => "shard_path",
        EnvelopeErrorField::ShardBytes => "shard_bytes",
        EnvelopeErrorField::ShardRows => "shard_rows",
        EnvelopeErrorField::ShardSha256 => "shard_sha256",
        EnvelopeErrorField::Ledger => "ledger",
        EnvelopeErrorField::LedgerPath => "ledger_path",
        EnvelopeErrorField::LedgerBytes => "ledger_bytes",
        EnvelopeErrorField::LedgerEvents => "ledger_events",
        EnvelopeErrorField::LedgerSha256 => "ledger_sha256",
        EnvelopeErrorField::SummaryPlan => "summary_plan",
        EnvelopeErrorField::SummarySchema => "summary_schema",
        EnvelopeErrorField::SummaryDays => "summary_days",
        EnvelopeErrorField::ManifestBinding => "manifest_binding",
    }
}

fn assert_traits<T: Copy + Eq + Ord + Hash>() {}

#[test]
fn envelope_error_vocabulary_is_canonical_exhaustive_copyable_ordered_and_hashable() {
    let expected_codes = [
        (EnvelopeErrorCode::InputTooLarge, "input_too_large"),
        (EnvelopeErrorCode::MalformedJson, "malformed_json"),
        (EnvelopeErrorCode::NoncanonicalJson, "noncanonical_json"),
        (EnvelopeErrorCode::MissingField, "missing_field"),
        (EnvelopeErrorCode::UnknownField, "unknown_field"),
        (EnvelopeErrorCode::WrongType, "wrong_type"),
        (EnvelopeErrorCode::InvalidField, "invalid_field"),
        (EnvelopeErrorCode::IncompatibleField, "incompatible_field"),
        (EnvelopeErrorCode::CountMismatch, "count_mismatch"),
        (EnvelopeErrorCode::ManifestMismatch, "manifest_mismatch"),
    ];
    let expected_fields = [
        (EnvelopeErrorField::Envelope, "envelope"),
        (EnvelopeErrorField::Schema, "schema"),
        (EnvelopeErrorField::BundleId, "bundle_id"),
        (EnvelopeErrorField::SourceFamily, "source_family"),
        (EnvelopeErrorField::SourceHash, "source_hash"),
        (EnvelopeErrorField::RawRetention, "raw_retention"),
        (EnvelopeErrorField::RowCount, "row_count"),
        (EnvelopeErrorField::Days, "days"),
        (EnvelopeErrorField::Shards, "shards"),
        (EnvelopeErrorField::ShardPath, "shard_path"),
        (EnvelopeErrorField::ShardBytes, "shard_bytes"),
        (EnvelopeErrorField::ShardRows, "shard_rows"),
        (EnvelopeErrorField::ShardSha256, "shard_sha256"),
        (EnvelopeErrorField::Ledger, "ledger"),
        (EnvelopeErrorField::LedgerPath, "ledger_path"),
        (EnvelopeErrorField::LedgerBytes, "ledger_bytes"),
        (EnvelopeErrorField::LedgerEvents, "ledger_events"),
        (EnvelopeErrorField::LedgerSha256, "ledger_sha256"),
        (EnvelopeErrorField::SummaryPlan, "summary_plan"),
        (EnvelopeErrorField::SummarySchema, "summary_schema"),
        (EnvelopeErrorField::SummaryDays, "summary_days"),
        (EnvelopeErrorField::ManifestBinding, "manifest_binding"),
    ];

    assert_traits::<EnvelopeErrorCode>();
    assert_traits::<EnvelopeErrorField>();
    assert_eq!(EnvelopeErrorCode::ALL.len(), 10);
    assert_eq!(EnvelopeErrorField::ALL.len(), 22);
    assert_eq!(
        EnvelopeErrorCode::ALL
            .iter()
            .map(EnvelopeErrorCode::as_str)
            .collect::<Vec<_>>(),
        expected_codes
            .iter()
            .map(|(_, spelling)| *spelling)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        EnvelopeErrorField::ALL
            .iter()
            .map(EnvelopeErrorField::as_str)
            .collect::<Vec<_>>(),
        expected_fields
            .iter()
            .map(|(_, spelling)| *spelling)
            .collect::<Vec<_>>(),
    );

    for (actual, (expected_variant, expected_spelling)) in
        EnvelopeErrorCode::ALL.iter().zip(expected_codes)
    {
        assert_eq!(*actual, expected_variant);
        assert_eq!(actual.as_str(), expected_spelling);
        assert_eq!(code_spelling(*actual), expected_spelling);
    }
    for (actual, (expected_variant, expected_spelling)) in
        EnvelopeErrorField::ALL.iter().zip(expected_fields)
    {
        assert_eq!(*actual, expected_variant);
        assert_eq!(actual.as_str(), expected_spelling);
        assert_eq!(field_spelling(*actual), expected_spelling);
    }

    let mut ordered_codes = BTreeSet::new();
    let mut hashed_codes = HashSet::new();
    for code in EnvelopeErrorCode::ALL {
        assert!(ordered_codes.insert(code));
        assert!(hashed_codes.insert(code));
    }
    assert_eq!(
        ordered_codes.into_iter().collect::<Vec<_>>(),
        EnvelopeErrorCode::ALL
    );
    assert_eq!(hashed_codes.len(), 10);

    let mut ordered_fields = BTreeSet::new();
    let mut hashed_fields = HashSet::new();
    for field in EnvelopeErrorField::ALL {
        assert!(ordered_fields.insert(field));
        assert!(hashed_fields.insert(field));
    }
    assert_eq!(
        ordered_fields.into_iter().collect::<Vec<_>>(),
        EnvelopeErrorField::ALL
    );
    assert_eq!(hashed_fields.len(), 22);
}
