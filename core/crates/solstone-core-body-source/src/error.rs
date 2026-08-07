// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use crate::Coordinate;

/// A token-free body-source parse failure with a raw UTF-8 byte offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    MalformedJson { byte_offset: usize },
    NumberTooLong { byte_offset: usize },
}

impl ParseError {
    pub(crate) const fn malformed(byte_offset: usize) -> Self {
        Self::MalformedJson { byte_offset }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedJson { byte_offset } => {
                write!(formatter, "malformed_json at byte offset {byte_offset}")
            }
            Self::NumberTooLong { byte_offset } => {
                write!(formatter, "number_too_long at byte offset {byte_offset}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// A bounded body-value canonicalization failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalizeError {
    ValueTooDeep { depth: usize },
}

impl fmt::Display for CanonicalizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ValueTooDeep { depth } => {
                write!(
                    formatter,
                    "value nesting exceeds maximum depth at container depth {depth}"
                )
            }
        }
    }
}

impl std::error::Error for CanonicalizeError {}

/// The closed set of normalized-row projection failure codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateErrorCode {
    UnsupportedSchema,
    MissingField,
    WrongType,
    BlankField,
    IncompatibleField,
}

impl CandidateErrorCode {
    /// Returns this code's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "unsupported_schema",
            Self::MissingField => "missing_field",
            Self::WrongType => "wrong_type",
            Self::BlankField => "blank_field",
            Self::IncompatibleField => "incompatible_field",
        }
    }
}

/// The closed set of normalized-row projection failure locations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateErrorField {
    Row,
    Schema,
    SourceFamily,
    RecordType,
    DedupeKey,
    StartDate,
    Day,
    Kind,
    ImportId,
    Month,
    EndDate,
    SourceRecordId,
    SourceName,
    SourceVersion,
    Unit,
    NormalizedRef,
    RawRef,
    Metadata,
}

impl CandidateErrorField {
    /// Returns this field's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Row => "row",
            Self::Schema => "schema",
            Self::SourceFamily => "source_family",
            Self::RecordType => "record_type",
            Self::DedupeKey => "dedupe_key",
            Self::StartDate => "start_date",
            Self::Day => "day",
            Self::Kind => "kind",
            Self::ImportId => "import_id",
            Self::Month => "month",
            Self::EndDate => "end_date",
            Self::SourceRecordId => "source_record_id",
            Self::SourceName => "source_name",
            Self::SourceVersion => "source_version",
            Self::Unit => "unit",
            Self::NormalizedRef => "normalized_ref",
            Self::RawRef => "raw_ref",
            Self::Metadata => "metadata",
        }
    }
}

/// A bounded, redacting normalized-row projection failure.
#[derive(Clone, PartialEq)]
pub struct CandidateError {
    pub coordinate: Coordinate,
    pub code: CandidateErrorCode,
    pub field: CandidateErrorField,
}

impl CandidateError {
    pub(crate) fn new(
        coordinate: &Coordinate,
        code: CandidateErrorCode,
        field: CandidateErrorField,
    ) -> Self {
        Self {
            coordinate: coordinate.clone(),
            code,
            field,
        }
    }
}

impl fmt::Display for CandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "body-row[{}] {}: {}",
            self.coordinate,
            self.code.as_str(),
            self.field.as_str()
        )
    }
}

impl fmt::Debug for CandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for CandidateError {}
