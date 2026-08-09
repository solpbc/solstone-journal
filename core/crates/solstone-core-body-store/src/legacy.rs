// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use solstone_core_body_source::{
    BodyDigest, BodyMonth, BodySourceFamily, BodyString, FieldState, LedgerCandidate, LedgerSchema,
    health_value_hash,
};

use crate::row::BodyDedupeRow;
use crate::text::body_string_to_text;

/// The closed failure kinds for a pre-native normalized body row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LegacyBodyRowErrorKind {
    InvalidText,
    ReferenceMismatch,
    ValueTooDeep,
}

impl LegacyBodyRowErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidText => "invalid_text",
            Self::ReferenceMismatch => "reference_mismatch",
            Self::ValueTooDeep => "value_too_deep",
        }
    }
}

/// The closed fields which can prevent legacy-row reconstruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LegacyBodyRowErrorField {
    DedupeKey,
    SourceRecordId,
    RecordType,
    StartTime,
    EndTime,
    ImportId,
    Month,
    NormalizedRef,
    RawRef,
    ValueHash,
}

impl LegacyBodyRowErrorField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DedupeKey => "dedupe_key",
            Self::SourceRecordId => "source_record_id",
            Self::RecordType => "record_type",
            Self::StartTime => "start_time",
            Self::EndTime => "end_time",
            Self::ImportId => "import_id",
            Self::Month => "month",
            Self::NormalizedRef => "normalized_ref",
            Self::RawRef => "raw_ref",
            Self::ValueHash => "value_hash",
        }
    }
}

/// A bounded, value-redacted refusal while checking a pre-native row.
#[derive(Clone, PartialEq, Eq)]
pub struct LegacyBodyRowError {
    kind: LegacyBodyRowErrorKind,
    field: LegacyBodyRowErrorField,
}

impl LegacyBodyRowError {
    fn new(kind: LegacyBodyRowErrorKind, field: LegacyBodyRowErrorField) -> Self {
        Self { kind, field }
    }

    pub fn kind(&self) -> LegacyBodyRowErrorKind {
        self.kind
    }

    pub fn field(&self) -> LegacyBodyRowErrorField {
        self.field
    }
}

impl fmt::Display for LegacyBodyRowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "legacy-body-row {}: {}",
            self.kind.as_str(),
            self.field.as_str()
        )
    }
}

impl fmt::Debug for LegacyBodyRowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for LegacyBodyRowError {}

/// A pre-native normalized row checked for lossless SQLite reconstruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedLegacyBodyRow(BodyDedupeRow);

impl ValidatedLegacyBodyRow {
    pub fn row(&self) -> &BodyDedupeRow {
        &self.0
    }
}

/// Checks one projected row from a directory already classified as legacy.
///
/// Apple value hashes are intentionally left absent: some shipping Apple rows
/// contain post-identity enrichment, so their original value hash cannot be
/// reconstructed from normalized bytes. Oura rows retain the exact identity
/// payload and therefore regain their shipping value hash deterministically.
pub fn validate_legacy_body_row(
    candidate: &LedgerCandidate,
    physical_import_id: &str,
    physical_month: &BodyMonth,
    physical_line: u64,
) -> Result<ValidatedLegacyBodyRow, LegacyBodyRowError> {
    if !valid_import_id(physical_import_id) {
        return Err(LegacyBodyRowError::new(
            LegacyBodyRowErrorKind::InvalidText,
            LegacyBodyRowErrorField::ImportId,
        ));
    }
    if physical_line == 0 {
        return Err(LegacyBodyRowError::new(
            LegacyBodyRowErrorKind::InvalidText,
            LegacyBodyRowErrorField::NormalizedRef,
        ));
    }
    let normalized_ref = format!(
        "imports/{physical_import_id}/normalized/{}.jsonl#L{physical_line}",
        physical_month.as_str()
    );
    require_optional_reference(
        candidate.import_id(),
        physical_import_id,
        LegacyBodyRowErrorField::ImportId,
    )?;
    require_optional_reference(
        candidate.month(),
        physical_month.as_str(),
        LegacyBodyRowErrorField::Month,
    )?;
    require_optional_reference(
        candidate.normalized_ref(),
        &normalized_ref,
        LegacyBodyRowErrorField::NormalizedRef,
    )?;

    let dedupe_key = required_text(candidate.dedupe_key(), LegacyBodyRowErrorField::DedupeKey)?;
    let source_family = BodySourceFamily::from_body_string(candidate.source_family())
        .expect("ledger projection already checked the exact source family");
    let source_record_id = optional_text(
        candidate.source_record_id(),
        LegacyBodyRowErrorField::SourceRecordId,
    )?;
    let record_type = required_text(candidate.record_type(), LegacyBodyRowErrorField::RecordType)?;
    let start_time = required_text(candidate.start_date(), LegacyBodyRowErrorField::StartTime)?;
    let end_time = optional_text(candidate.end_date(), LegacyBodyRowErrorField::EndTime)?;
    let raw_ref = optional_text(candidate.raw_ref(), LegacyBodyRowErrorField::RawRef)?;
    if raw_ref
        .as_deref()
        .is_some_and(|value| !valid_raw_ref(value, physical_import_id))
    {
        return Err(LegacyBodyRowError::new(
            LegacyBodyRowErrorKind::ReferenceMismatch,
            LegacyBodyRowErrorField::RawRef,
        ));
    }

    let value_hash = match candidate.schema() {
        LedgerSchema::OuraV1 => {
            let value =
                health_value_hash(candidate.unit(), candidate.metadata(), candidate.value())
                    .map_err(|_| {
                        LegacyBodyRowError::new(
                            LegacyBodyRowErrorKind::ValueTooDeep,
                            LegacyBodyRowErrorField::ValueHash,
                        )
                    })?;
            Some(BodyDigest::from_bytes(value.as_bytes()).map_err(|_| {
                LegacyBodyRowError::new(
                    LegacyBodyRowErrorKind::InvalidText,
                    LegacyBodyRowErrorField::ValueHash,
                )
            })?)
        }
        LedgerSchema::AppleHealthV1 | LedgerSchema::NormalizedV1 => None,
    };

    Ok(ValidatedLegacyBodyRow(BodyDedupeRow::new(
        dedupe_key,
        source_family,
        source_record_id,
        record_type,
        start_time,
        end_time,
        value_hash,
        Some(physical_import_id.to_owned()),
        Some(physical_import_id.to_owned()),
        Some(normalized_ref),
        raw_ref,
    )))
}

fn required_text(
    value: &BodyString,
    field: LegacyBodyRowErrorField,
) -> Result<String, LegacyBodyRowError> {
    body_string_to_text(value)
        .ok_or_else(|| LegacyBodyRowError::new(LegacyBodyRowErrorKind::InvalidText, field))
}

fn optional_text(
    value: &FieldState<BodyString>,
    field: LegacyBodyRowErrorField,
) -> Result<Option<String>, LegacyBodyRowError> {
    match value {
        FieldState::Absent | FieldState::Null => Ok(None),
        FieldState::Present(value) => required_text(value, field).map(Some),
    }
}

fn require_optional_reference(
    value: &FieldState<BodyString>,
    expected: &str,
    field: LegacyBodyRowErrorField,
) -> Result<(), LegacyBodyRowError> {
    let FieldState::Present(value) = value else {
        return Ok(());
    };
    let actual = required_text(value, field)?;
    if actual != expected {
        return Err(LegacyBodyRowError::new(
            LegacyBodyRowErrorKind::ReferenceMismatch,
            field,
        ));
    }
    Ok(())
}

fn valid_import_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_raw_ref(value: &str, physical_import_id: &str) -> bool {
    let path = value.split_once('#').map_or(value, |(path, _)| path);
    if path.contains('\\') {
        return false;
    }
    let rooted = format!("imports/{physical_import_id}/raw/");
    let relative = "raw/";
    let Some(remainder) = path
        .strip_prefix(&rooted)
        .or_else(|| path.strip_prefix(relative))
    else {
        return false;
    };
    !remainder.is_empty()
        && remainder
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}
