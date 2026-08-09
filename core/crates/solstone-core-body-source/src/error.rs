// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;

use crate::Coordinate;
use crate::bundle_id::BundleId;
use crate::envelope_ledger::LEDGER_PATH;
use crate::manifest_binding::BODY_BUNDLE_REF_VALUE;
use crate::manifest_known_key::{
    BODY_BUNDLE_REF_KEY, BODY_BUNDLE_SHA256_KEY, BODY_SOURCE_SCHEMA_KEY, DAYS_AFFECTED_KEY,
    ENTRY_COUNT_KEY, IMPORT_ID_KEY, RAW_RETENTION_KEY, SOURCE_HASH_KEY, SOURCE_TYPE_KEY,
};

const MANIFEST_FIELD_KEY: &str = "manifest";
const INVALID_BUNDLE_PLACEHOLDER: &str = "<invalid>";

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

    /// Returns the checked bundle identifier this error is bound to.
    pub fn bundle(&self) -> &BundleId {
        &self.bundle
    }

    /// Returns this error's failure code.
    pub fn code(&self) -> ManifestBindingErrorCode {
        self.code
    }

    /// Returns the manifest field this error concerns.
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

/// The closed set of body-envelope failure codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EnvelopeErrorCode {
    InputTooLarge,
    MalformedJson,
    NoncanonicalJson,
    MissingField,
    UnknownField,
    WrongType,
    InvalidField,
    IncompatibleField,
    CountMismatch,
    ManifestMismatch,
}

impl EnvelopeErrorCode {
    /// Returns this code's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InputTooLarge => "input_too_large",
            Self::MalformedJson => "malformed_json",
            Self::NoncanonicalJson => "noncanonical_json",
            Self::MissingField => "missing_field",
            Self::UnknownField => "unknown_field",
            Self::WrongType => "wrong_type",
            Self::InvalidField => "invalid_field",
            Self::IncompatibleField => "incompatible_field",
            Self::CountMismatch => "count_mismatch",
            Self::ManifestMismatch => "manifest_mismatch",
        }
    }

    pub const ALL: [Self; 10] = [
        Self::InputTooLarge,
        Self::MalformedJson,
        Self::NoncanonicalJson,
        Self::MissingField,
        Self::UnknownField,
        Self::WrongType,
        Self::InvalidField,
        Self::IncompatibleField,
        Self::CountMismatch,
        Self::ManifestMismatch,
    ];
}

/// The closed set of body-envelope failure fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EnvelopeErrorField {
    Envelope,
    Schema,
    BundleId,
    SourceFamily,
    SourceHash,
    RawRetention,
    RowCount,
    Days,
    Shards,
    ShardPath,
    ShardBytes,
    ShardRows,
    ShardSha256,
    Ledger,
    LedgerPath,
    LedgerBytes,
    LedgerEvents,
    LedgerSha256,
    SummaryPlan,
    SummarySchema,
    SummaryDays,
    ManifestBinding,
}

impl EnvelopeErrorField {
    /// Returns this field's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Envelope => "envelope",
            Self::Schema => "schema",
            Self::BundleId => "bundle_id",
            Self::SourceFamily => "source_family",
            Self::SourceHash => "source_hash",
            Self::RawRetention => "raw_retention",
            Self::RowCount => "row_count",
            Self::Days => "days",
            Self::Shards => "shards",
            Self::ShardPath => "shard_path",
            Self::ShardBytes => "shard_bytes",
            Self::ShardRows => "shard_rows",
            Self::ShardSha256 => "shard_sha256",
            Self::Ledger => "ledger",
            Self::LedgerPath => "ledger_path",
            Self::LedgerBytes => "ledger_bytes",
            Self::LedgerEvents => "ledger_events",
            Self::LedgerSha256 => "ledger_sha256",
            Self::SummaryPlan => "summary_plan",
            Self::SummarySchema => "summary_schema",
            Self::SummaryDays => "summary_days",
            Self::ManifestBinding => "manifest_binding",
        }
    }

    pub const ALL: [Self; 22] = [
        Self::Envelope,
        Self::Schema,
        Self::BundleId,
        Self::SourceFamily,
        Self::SourceHash,
        Self::RawRetention,
        Self::RowCount,
        Self::Days,
        Self::Shards,
        Self::ShardPath,
        Self::ShardBytes,
        Self::ShardRows,
        Self::ShardSha256,
        Self::Ledger,
        Self::LedgerPath,
        Self::LedgerBytes,
        Self::LedgerEvents,
        Self::LedgerSha256,
        Self::SummaryPlan,
        Self::SummarySchema,
        Self::SummaryDays,
        Self::ManifestBinding,
    ];
}

/// A bounded body-envelope failure.
#[derive(Clone, PartialEq, Eq)]
pub struct EnvelopeError {
    bundle: Option<BundleId>,
    code: EnvelopeErrorCode,
    field: EnvelopeErrorField,
    index: Option<u64>,
}

impl EnvelopeError {
    /// Builds a body-envelope failure.
    pub(crate) fn new(
        bundle: Option<BundleId>,
        code: EnvelopeErrorCode,
        field: EnvelopeErrorField,
        index: Option<u64>,
    ) -> Self {
        Self {
            bundle,
            code,
            field,
            index,
        }
    }

    /// Returns the checked bundle identifier this error is bound to, if available.
    pub fn bundle(&self) -> Option<&BundleId> {
        self.bundle.as_ref()
    }

    /// Returns this error's failure code.
    pub fn code(&self) -> EnvelopeErrorCode {
        self.code
    }

    /// Returns the envelope field this error concerns.
    pub fn field(&self) -> EnvelopeErrorField {
        self.field
    }

    /// Returns the zero-based array-element index for the named field, when this failure
    /// concerns a specific element rather than a whole collection or envelope.
    pub fn index(&self) -> Option<u64> {
        self.index
    }
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bundle = self
            .bundle
            .as_ref()
            .map(BundleId::as_str)
            .unwrap_or(INVALID_BUNDLE_PLACEHOLDER);
        match self.index {
            Some(index) => write!(
                formatter,
                "body-envelope[{bundle}]/{BODY_BUNDLE_REF_VALUE}[{index}] {}: {}",
                self.code.as_str(),
                self.field.as_str()
            ),
            None => write!(
                formatter,
                "body-envelope[{bundle}]/{BODY_BUNDLE_REF_VALUE} {}: {}",
                self.code.as_str(),
                self.field.as_str()
            ),
        }
    }
}

impl fmt::Debug for EnvelopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for EnvelopeError {}

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

/// The closed set of body-ledger event failure codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LedgerEventErrorCode {
    InputTooLarge,
    MalformedJson,
    NoncanonicalJson,
    MissingField,
    UnknownField,
    WrongType,
    InvalidField,
    IncompatibleField,
    InvalidSequence,
    ReferenceMismatch,
    CountMismatch,
}

impl LedgerEventErrorCode {
    /// Returns this code's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::InputTooLarge => "input_too_large",
            Self::MalformedJson => "malformed_json",
            Self::NoncanonicalJson => "noncanonical_json",
            Self::MissingField => "missing_field",
            Self::UnknownField => "unknown_field",
            Self::WrongType => "wrong_type",
            Self::InvalidField => "invalid_field",
            Self::IncompatibleField => "incompatible_field",
            Self::InvalidSequence => "invalid_sequence",
            Self::ReferenceMismatch => "reference_mismatch",
            Self::CountMismatch => "count_mismatch",
        }
    }

    pub const ALL: [Self; 11] = [
        Self::InputTooLarge,
        Self::MalformedJson,
        Self::NoncanonicalJson,
        Self::MissingField,
        Self::UnknownField,
        Self::WrongType,
        Self::InvalidField,
        Self::IncompatibleField,
        Self::InvalidSequence,
        Self::ReferenceMismatch,
        Self::CountMismatch,
    ];
}

/// The closed set of body-ledger event failure fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LedgerEventErrorField {
    Ledger,
    Schema,
    BundleId,
    Sequence,
    RowSchema,
    Shard,
    Line,
    NormalizedRef,
    RowSha256,
    DedupeKey,
    SourceFamily,
    SourceRecordId,
    RecordType,
    StartTime,
    EndTime,
    Day,
    ValueHash,
    RawRef,
}

impl LedgerEventErrorField {
    /// Returns this field's stable wire spelling.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ledger => "ledger",
            Self::Schema => "schema",
            Self::BundleId => "bundle_id",
            Self::Sequence => "sequence",
            Self::RowSchema => "row_schema",
            Self::Shard => "shard",
            Self::Line => "line",
            Self::NormalizedRef => "normalized_ref",
            Self::RowSha256 => "row_sha256",
            Self::DedupeKey => "dedupe_key",
            Self::SourceFamily => "source_family",
            Self::SourceRecordId => "source_record_id",
            Self::RecordType => "record_type",
            Self::StartTime => "start_time",
            Self::EndTime => "end_time",
            Self::Day => "day",
            Self::ValueHash => "value_hash",
            Self::RawRef => "raw_ref",
        }
    }

    pub const ALL: [Self; 18] = [
        Self::Ledger,
        Self::Schema,
        Self::BundleId,
        Self::Sequence,
        Self::RowSchema,
        Self::Shard,
        Self::Line,
        Self::NormalizedRef,
        Self::RowSha256,
        Self::DedupeKey,
        Self::SourceFamily,
        Self::SourceRecordId,
        Self::RecordType,
        Self::StartTime,
        Self::EndTime,
        Self::Day,
        Self::ValueHash,
        Self::RawRef,
    ];
}

/// A bounded body-ledger event failure.
#[derive(Clone, PartialEq, Eq)]
pub struct LedgerEventError {
    bundle: Option<BundleId>,
    code: LedgerEventErrorCode,
    field: LedgerEventErrorField,
    line: u64,
}

impl LedgerEventError {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by B1h2 ledger event model")
    )]
    pub(crate) fn new(
        bundle: Option<BundleId>,
        code: LedgerEventErrorCode,
        field: LedgerEventErrorField,
        line: u64,
    ) -> Self {
        Self {
            bundle,
            code,
            field,
            line,
        }
    }

    /// Returns the checked bundle identifier this error is bound to, if available.
    pub fn bundle(&self) -> Option<&BundleId> {
        self.bundle.as_ref()
    }

    /// Returns this error's failure code.
    pub fn code(&self) -> LedgerEventErrorCode {
        self.code
    }

    /// Returns the ledger event field this error concerns.
    pub fn field(&self) -> LedgerEventErrorField {
        self.field
    }

    /// Returns the ledger line this error concerns.
    pub fn line(&self) -> u64 {
        self.line
    }
}

impl fmt::Display for LedgerEventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bundle = self
            .bundle
            .as_ref()
            .map(BundleId::as_str)
            .unwrap_or(INVALID_BUNDLE_PLACEHOLDER);
        write!(
            formatter,
            "body-ledger[{bundle}]/{LEDGER_PATH}#L{} {}: {}",
            self.line,
            self.code.as_str(),
            self.field.as_str()
        )
    }
}

impl fmt::Debug for LedgerEventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for LedgerEventError {}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    const MIN_BUNDLE: &str = "body-00000000000000000000000000";
    const MAX_BUNDLE: &str = "body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ";

    fn bundles() -> [BundleId; 2] {
        [
            BundleId::from_bytes(MIN_BUNDLE.as_bytes()).expect("minimum bundle ID is valid"),
            BundleId::from_bytes(MAX_BUNDLE.as_bytes()).expect("maximum bundle ID is valid"),
        ]
    }

    fn expected_rendering(
        bundle: Option<&BundleId>,
        code: EnvelopeErrorCode,
        field: EnvelopeErrorField,
        index: Option<u64>,
    ) -> String {
        let bundle = bundle
            .map(BundleId::as_str)
            .unwrap_or(INVALID_BUNDLE_PLACEHOLDER);
        match index {
            Some(index) => format!(
                "body-envelope[{bundle}]/{BODY_BUNDLE_REF_VALUE}[{index}] {}: {}",
                code.as_str(),
                field.as_str()
            ),
            None => format!(
                "body-envelope[{bundle}]/{BODY_BUNDLE_REF_VALUE} {}: {}",
                code.as_str(),
                field.as_str()
            ),
        }
    }

    #[test]
    fn envelope_error_constructs_and_clones_every_combination() {
        let [minimum, maximum] = bundles();
        let bundle_options = [None, Some(minimum), Some(maximum)];
        let indexes = [None, Some(0), Some(1), Some(u64::MAX)];

        for bundle in bundle_options {
            for code in EnvelopeErrorCode::ALL {
                for field in EnvelopeErrorField::ALL {
                    for index in indexes {
                        let expected_bundle = bundle.as_ref().map(BundleId::as_str);
                        let error = EnvelopeError::new(bundle.clone(), code, field, index);
                        assert_eq!(error.bundle().map(BundleId::as_str), expected_bundle);
                        assert_eq!(error.code(), code);
                        assert_eq!(error.field(), field);
                        assert_eq!(error.index(), index);
                        assert_eq!(error.clone(), error);
                    }
                }
            }
        }
    }

    #[test]
    fn envelope_error_renders_bounded_checked_output() {
        let [_, maximum] = bundles();
        let bundle_options = [None, Some(maximum)];
        let indexes = [None, Some(0), Some(1), Some(u64::MAX)];

        for bundle in bundle_options {
            for code in EnvelopeErrorCode::ALL {
                for field in EnvelopeErrorField::ALL {
                    for index in indexes {
                        let error = EnvelopeError::new(bundle.clone(), code, field, index);
                        let expected = expected_rendering(bundle.as_ref(), code, field, index);
                        let display = error.to_string();
                        assert_eq!(display, expected);
                        assert_eq!(format!("{error:?}"), expected);
                        assert!(Error::source(&error).is_none());
                        assert!(display.len() <= 122);
                        assert!(display.len() <= 256);
                        assert!(
                            display.contains(
                                bundle
                                    .as_ref()
                                    .map(BundleId::as_str)
                                    .unwrap_or(INVALID_BUNDLE_PLACEHOLDER)
                            )
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn envelope_error_resists_vocabulary_rendering_and_equality_drift() {
        assert_eq!(
            EnvelopeErrorCode::ALL
                .iter()
                .map(EnvelopeErrorCode::as_str)
                .collect::<Vec<_>>(),
            vec![
                "input_too_large",
                "malformed_json",
                "noncanonical_json",
                "missing_field",
                "unknown_field",
                "wrong_type",
                "invalid_field",
                "incompatible_field",
                "count_mismatch",
                "manifest_mismatch",
            ]
        );
        assert_eq!(
            EnvelopeErrorField::ALL
                .iter()
                .map(EnvelopeErrorField::as_str)
                .collect::<Vec<_>>(),
            vec![
                "envelope",
                "schema",
                "bundle_id",
                "source_family",
                "source_hash",
                "raw_retention",
                "row_count",
                "days",
                "shards",
                "shard_path",
                "shard_bytes",
                "shard_rows",
                "shard_sha256",
                "ledger",
                "ledger_path",
                "ledger_bytes",
                "ledger_events",
                "ledger_sha256",
                "summary_plan",
                "summary_schema",
                "summary_days",
                "manifest_binding",
            ]
        );

        let [minimum, maximum] = bundles();
        let baseline = EnvelopeError::new(
            Some(minimum.clone()),
            EnvelopeErrorCode::MissingField,
            EnvelopeErrorField::BundleId,
            Some(1),
        );
        assert_ne!(
            baseline,
            EnvelopeError::new(
                Some(maximum.clone()),
                EnvelopeErrorCode::MissingField,
                EnvelopeErrorField::BundleId,
                Some(1),
            )
        );
        assert_ne!(
            baseline,
            EnvelopeError::new(
                Some(minimum.clone()),
                EnvelopeErrorCode::WrongType,
                EnvelopeErrorField::BundleId,
                Some(1),
            )
        );
        assert_ne!(
            baseline,
            EnvelopeError::new(
                Some(minimum.clone()),
                EnvelopeErrorCode::MissingField,
                EnvelopeErrorField::Schema,
                Some(1),
            )
        );
        assert_ne!(
            baseline,
            EnvelopeError::new(
                Some(minimum),
                EnvelopeErrorCode::MissingField,
                EnvelopeErrorField::BundleId,
                None,
            )
        );
        assert_eq!(
            baseline.to_string(),
            "body-envelope[body-00000000000000000000000000]/body-bundle.json[1] missing_field: bundle_id"
        );
        assert_eq!(
            format!("{baseline:?}"),
            "body-envelope[body-00000000000000000000000000]/body-bundle.json[1] missing_field: bundle_id"
        );

        let absent = EnvelopeError::new(
            None,
            EnvelopeErrorCode::MissingField,
            EnvelopeErrorField::BundleId,
            None,
        );
        assert_eq!(
            absent.to_string(),
            "body-envelope[<invalid>]/body-bundle.json missing_field: bundle_id"
        );
        assert!(!absent.to_string().contains(MIN_BUNDLE));
        assert!(!absent.to_string().contains(MAX_BUNDLE));

        let maximum = EnvelopeError::new(
            Some(maximum),
            EnvelopeErrorCode::IncompatibleField,
            EnvelopeErrorField::ManifestBinding,
            Some(u64::MAX),
        );
        let maximum_display = maximum.to_string();
        assert_eq!(
            maximum_display,
            "body-envelope[body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ]/body-bundle.json[18446744073709551615] incompatible_field: manifest_binding"
        );
        assert_eq!(maximum_display.len(), 122);
        assert!(maximum_display.len() <= 256);
    }

    fn ledger_event_expected_rendering(
        bundle: Option<&BundleId>,
        code: LedgerEventErrorCode,
        field: LedgerEventErrorField,
        line: u64,
    ) -> String {
        let bundle = bundle
            .map(BundleId::as_str)
            .unwrap_or(INVALID_BUNDLE_PLACEHOLDER);
        format!(
            "body-ledger[{bundle}]/{LEDGER_PATH}#L{line} {}: {}",
            code.as_str(),
            field.as_str()
        )
    }

    #[test]
    fn ledger_event_error_constructs_and_clones_every_combination() {
        let [minimum, maximum] = bundles();
        let bundle_options = [None, Some(minimum), Some(maximum)];
        let lines = [0, 1, u64::MAX];

        for bundle in bundle_options {
            for code in LedgerEventErrorCode::ALL {
                for field in LedgerEventErrorField::ALL {
                    for line in lines {
                        let expected_bundle = bundle.as_ref().map(BundleId::as_str);
                        let error = LedgerEventError::new(bundle.clone(), code, field, line);
                        assert_eq!(error.bundle().map(BundleId::as_str), expected_bundle);
                        assert_eq!(error.code(), code);
                        assert_eq!(error.field(), field);
                        assert_eq!(error.line(), line);
                        assert_eq!(error.clone(), error);
                    }
                }
            }
        }
    }

    #[test]
    fn ledger_event_error_renders_bounded_checked_output() {
        let [_, maximum] = bundles();
        let bundle_options = [None, Some(maximum.clone())];
        let lines = [0, 1, u64::MAX];

        for bundle in bundle_options {
            for code in LedgerEventErrorCode::ALL {
                for field in LedgerEventErrorField::ALL {
                    for line in lines {
                        let error = LedgerEventError::new(bundle.clone(), code, field, line);
                        let expected =
                            ledger_event_expected_rendering(bundle.as_ref(), code, field, line);
                        let display = error.to_string();
                        assert_eq!(display, expected);
                        assert_eq!(format!("{error:?}"), expected);
                        assert!(Error::source(&error).is_none());
                        assert!(display.is_ascii());
                        assert!(display.len() <= 256);
                    }
                }
            }
        }

        let maximum = LedgerEventError::new(
            Some(maximum),
            LedgerEventErrorCode::ReferenceMismatch,
            LedgerEventErrorField::SourceRecordId,
            u64::MAX,
        );
        let maximum_display = maximum.to_string();
        assert_eq!(
            maximum_display,
            "body-ledger[body-7ZZZZZZZZZZZZZZZZZZZZZZZZZ]/body-ledger.jsonl#L18446744073709551615 reference_mismatch: source_record_id"
        );
        assert_eq!(maximum_display.len(), 121);
    }

    #[test]
    fn ledger_event_error_resists_vocabulary_rendering_and_equality_drift() {
        assert_eq!(
            LedgerEventErrorCode::ALL
                .iter()
                .map(LedgerEventErrorCode::as_str)
                .collect::<Vec<_>>(),
            vec![
                "input_too_large",
                "malformed_json",
                "noncanonical_json",
                "missing_field",
                "unknown_field",
                "wrong_type",
                "invalid_field",
                "incompatible_field",
                "invalid_sequence",
                "reference_mismatch",
                "count_mismatch",
            ]
        );
        assert_eq!(
            LedgerEventErrorField::ALL
                .iter()
                .map(LedgerEventErrorField::as_str)
                .collect::<Vec<_>>(),
            vec![
                "ledger",
                "schema",
                "bundle_id",
                "sequence",
                "row_schema",
                "shard",
                "line",
                "normalized_ref",
                "row_sha256",
                "dedupe_key",
                "source_family",
                "source_record_id",
                "record_type",
                "start_time",
                "end_time",
                "day",
                "value_hash",
                "raw_ref",
            ]
        );

        let [minimum, maximum] = bundles();
        let baseline = LedgerEventError::new(
            Some(minimum.clone()),
            LedgerEventErrorCode::MissingField,
            LedgerEventErrorField::BundleId,
            1,
        );
        assert_ne!(
            baseline,
            LedgerEventError::new(
                Some(maximum.clone()),
                LedgerEventErrorCode::MissingField,
                LedgerEventErrorField::BundleId,
                1,
            )
        );
        assert_ne!(
            baseline,
            LedgerEventError::new(
                Some(minimum.clone()),
                LedgerEventErrorCode::WrongType,
                LedgerEventErrorField::BundleId,
                1,
            )
        );
        assert_ne!(
            baseline,
            LedgerEventError::new(
                Some(minimum.clone()),
                LedgerEventErrorCode::MissingField,
                LedgerEventErrorField::Schema,
                1,
            )
        );
        assert_ne!(
            baseline,
            LedgerEventError::new(
                Some(minimum),
                LedgerEventErrorCode::MissingField,
                LedgerEventErrorField::BundleId,
                2,
            )
        );

        let absent = LedgerEventError::new(
            None,
            LedgerEventErrorCode::MissingField,
            LedgerEventErrorField::BundleId,
            0,
        );
        assert_eq!(
            absent.to_string(),
            "body-ledger[<invalid>]/body-ledger.jsonl#L0 missing_field: bundle_id"
        );
        assert!(!absent.to_string().contains(MIN_BUNDLE));
        assert!(!absent.to_string().contains(MAX_BUNDLE));
    }

    #[test]
    fn ledger_event_error_all_is_declaration_ordered() {
        let mut sorted_codes = LedgerEventErrorCode::ALL.to_vec();
        sorted_codes.sort();
        assert_eq!(sorted_codes, LedgerEventErrorCode::ALL.to_vec());

        let mut sorted_fields = LedgerEventErrorField::ALL.to_vec();
        sorted_fields.sort();
        assert_eq!(sorted_fields, LedgerEventErrorField::ALL.to_vec());
    }

    #[test]
    fn ledger_event_error_forbidden_content_never_renders() {
        let raw_content = format!("SENTINEL_RAW_CONTENT_{}", "x".repeat(1_000_000));
        let path = format!("SENTINEL_PATH_{}", "/".repeat(1_000_000));
        let hash = format!("SENTINEL_HASH_{}", "a".repeat(1_000_000));
        let reference = format!("SENTINEL_REFERENCE_{}", "r".repeat(1_000_000));
        let root = format!("SENTINEL_ROOT_{}", "~".repeat(1_000_000));
        let sentinels = [&raw_content, &path, &hash, &reference, &root];

        let [_, maximum] = bundles();
        let bundle_options = [None, Some(maximum)];
        for bundle in bundle_options {
            for code in LedgerEventErrorCode::ALL {
                for field in LedgerEventErrorField::ALL {
                    for line in [0, u64::MAX] {
                        let display =
                            LedgerEventError::new(bundle.clone(), code, field, line).to_string();
                        for sentinel in sentinels {
                            assert!(!display.contains(sentinel));
                        }
                    }
                }
            }
        }
    }
}
