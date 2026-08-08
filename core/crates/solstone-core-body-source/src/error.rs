// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use crate::Coordinate;
use crate::bundle_id::BundleId;
use crate::manifest_known_key::{
    BODY_BUNDLE_REF_KEY, BODY_BUNDLE_SHA256_KEY, BODY_SOURCE_SCHEMA_KEY, DAYS_AFFECTED_KEY,
    ENTRY_COUNT_KEY, IMPORT_ID_KEY, RAW_RETENTION_KEY, SOURCE_HASH_KEY, SOURCE_TYPE_KEY,
};

const MANIFEST_FIELD_KEY: &str = "manifest";

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

/// The closed set of native body-wire identity fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyWireIdentityField {
    BundleId,
    Digest,
}

impl BodyWireIdentityField {
    /// Returns this field's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::BundleId => "bundle_id",
            Self::Digest => "digest",
        }
    }
}

/// A bounded native body-wire identity failure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BodyWireIdentityError {
    InvalidFormat(BodyWireIdentityField),
}

impl fmt::Display for BodyWireIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(field) => {
                write!(formatter, "body-wire invalid_format: {}", field.as_str())
            }
        }
    }
}

impl fmt::Debug for BodyWireIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyWireIdentityError {}

/// A bounded native body-source hash failure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BodySourceHashError {
    InvalidFormat,
}

impl fmt::Display for BodySourceHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat => {
                write!(formatter, "body-source-hash invalid_format: source_hash")
            }
        }
    }
}

impl fmt::Debug for BodySourceHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodySourceHashError {}

/// The closed set of body-manifest binding failure codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManifestBindingErrorCode {
    InputTooLarge,
    MalformedManifest,
    DuplicateField,
    UnknownField,
    MissingField,
    WrongType,
    InvalidField,
    IncompatibleField,
}

impl ManifestBindingErrorCode {
    /// Returns this code's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InputTooLarge => "input_too_large",
            Self::MalformedManifest => "malformed_manifest",
            Self::DuplicateField => "duplicate_field",
            Self::UnknownField => "unknown_field",
            Self::MissingField => "missing_field",
            Self::WrongType => "wrong_type",
            Self::InvalidField => "invalid_field",
            Self::IncompatibleField => "incompatible_field",
        }
    }

    pub const ALL: [Self; 8] = [
        Self::InputTooLarge,
        Self::MalformedManifest,
        Self::DuplicateField,
        Self::UnknownField,
        Self::MissingField,
        Self::WrongType,
        Self::InvalidField,
        Self::IncompatibleField,
    ];
}

/// The closed set of body-manifest binding failure fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ManifestBindingErrorField {
    Manifest,
    BodySourceSchema,
    BodyBundleRef,
    BodyBundleSha256,
    ImportId,
    SourceType,
    SourceHash,
    EntryCount,
    DaysAffected,
    RawRetention,
}

impl ManifestBindingErrorField {
    /// Returns this field's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Manifest => MANIFEST_FIELD_KEY,
            Self::BodySourceSchema => BODY_SOURCE_SCHEMA_KEY,
            Self::BodyBundleRef => BODY_BUNDLE_REF_KEY,
            Self::BodyBundleSha256 => BODY_BUNDLE_SHA256_KEY,
            Self::ImportId => IMPORT_ID_KEY,
            Self::SourceType => SOURCE_TYPE_KEY,
            Self::SourceHash => SOURCE_HASH_KEY,
            Self::EntryCount => ENTRY_COUNT_KEY,
            Self::DaysAffected => DAYS_AFFECTED_KEY,
            Self::RawRetention => RAW_RETENTION_KEY,
        }
    }

    pub const ALL: [Self; 10] = [
        Self::Manifest,
        Self::BodySourceSchema,
        Self::BodyBundleRef,
        Self::BodyBundleSha256,
        Self::ImportId,
        Self::SourceType,
        Self::SourceHash,
        Self::EntryCount,
        Self::DaysAffected,
        Self::RawRetention,
    ];
}

/// A bounded body-manifest binding failure.
#[derive(Clone, PartialEq, Eq)]
pub struct ManifestBindingError {
    bundle: BundleId,
    code: ManifestBindingErrorCode,
    field: ManifestBindingErrorField,
}

impl ManifestBindingError {
    /// Builds a body-manifest binding failure.
    pub fn new(
        bundle: BundleId,
        code: ManifestBindingErrorCode,
        field: ManifestBindingErrorField,
    ) -> Self {
        Self {
            bundle,
            code,
            field,
        }
    }

    pub fn bundle(&self) -> &BundleId {
        &self.bundle
    }

    pub fn code(&self) -> ManifestBindingErrorCode {
        self.code
    }

    pub fn field(&self) -> ManifestBindingErrorField {
        self.field
    }
}

impl fmt::Display for ManifestBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "body-manifest[{}] {}: {}",
            self.bundle.as_str(),
            self.code.as_str(),
            self.field.as_str()
        )
    }
}

impl fmt::Debug for ManifestBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ManifestBindingError {}

/// A bounded body-manifest scan failure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ManifestScanError {
    InputTooLarge,
    MalformedManifest,
}

impl fmt::Display for ManifestScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputTooLarge => {
                write!(formatter, "body-manifest-scan input_too_large: manifest")
            }
            Self::MalformedManifest => {
                write!(formatter, "body-manifest-scan malformed_manifest: manifest")
            }
        }
    }
}

impl fmt::Debug for ManifestScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for ManifestScanError {}

/// The closed set of native body-source policy fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodySourcePolicyField {
    SourceFamily,
    RawRetention,
}

impl BodySourcePolicyField {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SourceFamily => "source_family",
            Self::RawRetention => "raw_retention",
        }
    }
}

/// A bounded native body-source policy failure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BodySourcePolicyError {
    InvalidFormat(BodySourcePolicyField),
    Incompatible(BodySourcePolicyField),
}

impl fmt::Display for BodySourcePolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(field) => {
                write!(
                    formatter,
                    "body-source-policy invalid_format: {}",
                    field.as_str()
                )
            }
            Self::Incompatible(field) => {
                write!(
                    formatter,
                    "body-source-policy incompatible: {}",
                    field.as_str()
                )
            }
        }
    }
}

impl fmt::Debug for BodySourcePolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodySourcePolicyError {}

/// The closed set of native body-calendar fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyCalendarField {
    Day,
    Month,
}

impl BodyCalendarField {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Month => "month",
        }
    }
}

/// A bounded native body-calendar failure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BodyCalendarError {
    InvalidFormat(BodyCalendarField),
}

impl fmt::Display for BodyCalendarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(field) => {
                write!(
                    formatter,
                    "body-calendar invalid_format: {}",
                    field.as_str()
                )
            }
        }
    }
}

impl fmt::Debug for BodyCalendarError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyCalendarError {}

/// The closed set of required source identity fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityField {
    SourceFamily,
    RecordType,
    StartTime,
}

impl IdentityField {
    /// Returns this field's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SourceFamily => "source_family",
            Self::RecordType => "record_type",
            Self::StartTime => "start_time",
        }
    }
}

/// A bounded health-record identity or value-hash failure.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BodyHashError {
    InvalidIdentity(IdentityField),
    ValueTooDeep,
}

impl fmt::Display for BodyHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity(field) => {
                write!(
                    formatter,
                    "body-identity invalid_identity: {}",
                    field.as_str()
                )
            }
            Self::ValueTooDeep => write!(formatter, "body-hash value_too_deep"),
        }
    }
}

impl fmt::Debug for BodyHashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for BodyHashError {}

/// The closed set of normalized-row construction and projection failure codes.
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

/// The closed set of normalized-row construction and projection failure locations.
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

/// A bounded, redacting normalized-row construction or projection failure.
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
