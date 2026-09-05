// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashSet};
use std::hash::Hash;

use solstone_core_body_source::{LedgerEventErrorCode, LedgerEventErrorField};

fn code_spelling(code: LedgerEventErrorCode) -> &'static str {
    match code {
        LedgerEventErrorCode::InputTooLarge => "input_too_large",
        LedgerEventErrorCode::MalformedJson => "malformed_json",
        LedgerEventErrorCode::NoncanonicalJson => "noncanonical_json",
        LedgerEventErrorCode::MissingField => "missing_field",
        LedgerEventErrorCode::UnknownField => "unknown_field",
        LedgerEventErrorCode::WrongType => "wrong_type",
        LedgerEventErrorCode::InvalidField => "invalid_field",
        LedgerEventErrorCode::IncompatibleField => "incompatible_field",
        LedgerEventErrorCode::InvalidSequence => "invalid_sequence",
        LedgerEventErrorCode::ReferenceMismatch => "reference_mismatch",
        LedgerEventErrorCode::CountMismatch => "count_mismatch",
    }
}

fn field_spelling(field: LedgerEventErrorField) -> &'static str {
    match field {
        LedgerEventErrorField::Ledger => "ledger",
        LedgerEventErrorField::Schema => "schema",
        LedgerEventErrorField::BundleId => "bundle_id",
        LedgerEventErrorField::Sequence => "sequence",
        LedgerEventErrorField::RowSchema => "row_schema",
        LedgerEventErrorField::Shard => "shard",
        LedgerEventErrorField::Line => "line",
        LedgerEventErrorField::NormalizedRef => "normalized_ref",
        LedgerEventErrorField::RowSha256 => "row_sha256",
        LedgerEventErrorField::DedupeKey => "dedupe_key",
        LedgerEventErrorField::SourceFamily => "source_family",
        LedgerEventErrorField::SourceRecordId => "source_record_id",
        LedgerEventErrorField::RecordType => "record_type",
        LedgerEventErrorField::StartTime => "start_time",
        LedgerEventErrorField::EndTime => "end_time",
        LedgerEventErrorField::Day => "day",
        LedgerEventErrorField::ValueHash => "value_hash",
        LedgerEventErrorField::RawRef => "raw_ref",
    }
}

fn assert_traits<T: Copy + Eq + Ord + Hash>() {}

#[test]
fn ledger_event_error_vocabulary_is_canonical_exhaustive_copyable_ordered_and_hashable() {
    let expected_codes = [
        (LedgerEventErrorCode::InputTooLarge, "input_too_large"),
        (LedgerEventErrorCode::MalformedJson, "malformed_json"),
        (LedgerEventErrorCode::NoncanonicalJson, "noncanonical_json"),
        (LedgerEventErrorCode::MissingField, "missing_field"),
        (LedgerEventErrorCode::UnknownField, "unknown_field"),
        (LedgerEventErrorCode::WrongType, "wrong_type"),
        (LedgerEventErrorCode::InvalidField, "invalid_field"),
        (
            LedgerEventErrorCode::IncompatibleField,
            "incompatible_field",
        ),
        (LedgerEventErrorCode::InvalidSequence, "invalid_sequence"),
        (
            LedgerEventErrorCode::ReferenceMismatch,
            "reference_mismatch",
        ),
        (LedgerEventErrorCode::CountMismatch, "count_mismatch"),
    ];
    let expected_fields = [
        (LedgerEventErrorField::Ledger, "ledger"),
        (LedgerEventErrorField::Schema, "schema"),
        (LedgerEventErrorField::BundleId, "bundle_id"),
        (LedgerEventErrorField::Sequence, "sequence"),
        (LedgerEventErrorField::RowSchema, "row_schema"),
        (LedgerEventErrorField::Shard, "shard"),
        (LedgerEventErrorField::Line, "line"),
        (LedgerEventErrorField::NormalizedRef, "normalized_ref"),
        (LedgerEventErrorField::RowSha256, "row_sha256"),
        (LedgerEventErrorField::DedupeKey, "dedupe_key"),
        (LedgerEventErrorField::SourceFamily, "source_family"),
        (LedgerEventErrorField::SourceRecordId, "source_record_id"),
        (LedgerEventErrorField::RecordType, "record_type"),
        (LedgerEventErrorField::StartTime, "start_time"),
        (LedgerEventErrorField::EndTime, "end_time"),
        (LedgerEventErrorField::Day, "day"),
        (LedgerEventErrorField::ValueHash, "value_hash"),
        (LedgerEventErrorField::RawRef, "raw_ref"),
    ];

    assert_traits::<LedgerEventErrorCode>();
    assert_traits::<LedgerEventErrorField>();
    assert_eq!(LedgerEventErrorCode::ALL.len(), 11);
    assert_eq!(LedgerEventErrorField::ALL.len(), 18);
    assert_eq!(
        LedgerEventErrorCode::ALL
            .iter()
            .map(LedgerEventErrorCode::as_str)
            .collect::<Vec<_>>(),
        expected_codes
            .iter()
            .map(|(_, spelling)| *spelling)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        LedgerEventErrorField::ALL
            .iter()
            .map(LedgerEventErrorField::as_str)
            .collect::<Vec<_>>(),
        expected_fields
            .iter()
            .map(|(_, spelling)| *spelling)
            .collect::<Vec<_>>(),
    );

    for (actual, (expected_variant, expected_spelling)) in
        LedgerEventErrorCode::ALL.iter().zip(expected_codes)
    {
        assert_eq!(*actual, expected_variant);
        assert_eq!(actual.as_str(), expected_spelling);
        assert_eq!(code_spelling(*actual), expected_spelling);
    }
    for (actual, (expected_variant, expected_spelling)) in
        LedgerEventErrorField::ALL.iter().zip(expected_fields)
    {
        assert_eq!(*actual, expected_variant);
        assert_eq!(actual.as_str(), expected_spelling);
        assert_eq!(field_spelling(*actual), expected_spelling);
    }

    let mut ordered_codes = BTreeSet::new();
    let mut hashed_codes = HashSet::new();
    for code in LedgerEventErrorCode::ALL {
        assert!(ordered_codes.insert(code));
        assert!(hashed_codes.insert(code));
    }
    assert_eq!(
        ordered_codes.into_iter().collect::<Vec<_>>(),
        LedgerEventErrorCode::ALL
    );
    assert_eq!(hashed_codes.len(), 11);

    let mut ordered_fields = BTreeSet::new();
    let mut hashed_fields = HashSet::new();
    for field in LedgerEventErrorField::ALL {
        assert!(ordered_fields.insert(field));
        assert!(hashed_fields.insert(field));
    }
    assert_eq!(
        ordered_fields.into_iter().collect::<Vec<_>>(),
        LedgerEventErrorField::ALL
    );
    assert_eq!(hashed_fields.len(), 18);
}
