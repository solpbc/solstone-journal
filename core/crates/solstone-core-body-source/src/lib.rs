// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible JSON source decoding and canonicalization for body import data.

mod body_day;
mod body_month;
mod body_raw_retention;
mod body_source_family;
mod body_source_hash;
mod bundle_id;
mod calendar;
mod candidate;
mod canonicalize;
mod coordinate;
mod digest;
mod error;
mod health_hash;
mod integer;
mod manifest_binding;
mod manifest_known_key;
mod manifest_scan;
mod manifest_signal;
mod parser;
mod presentation;
mod string;
mod value;
mod whitespace;

pub use body_day::BodyDay;
pub use body_month::BodyMonth;
pub use body_raw_retention::BodyRawRetention;
pub use body_source_family::BodySourceFamily;
pub use body_source_hash::BodySourceHash;
pub use bundle_id::BundleId;
pub use candidate::{FieldState, LedgerCandidate, LedgerSchema, ValueState, project};
pub use canonicalize::canonicalize;
pub use coordinate::Coordinate;
pub use digest::BodyDigest;
pub use error::{
    BodyCalendarError, BodyCalendarField, BodyHashError, BodySourceHashError,
    BodySourcePolicyError, BodySourcePolicyField, BodyWireIdentityError, BodyWireIdentityField,
    CandidateError, CandidateErrorCode, CandidateErrorField, CanonicalizeError, IdentityField,
    ManifestBindingError, ManifestBindingErrorCode, ManifestBindingErrorField, ManifestScanError,
    ParseError,
};
pub use health_hash::{
    HealthRecordIdentity, health_hash, health_record_dedupe_key, health_value_hash,
};
pub use integer::BodyInteger;
pub use manifest_binding::BodyManifestBinding;
pub use manifest_known_key::{
    BODY_BUNDLE_REF_KEY, BODY_BUNDLE_SHA256_KEY, BODY_SOURCE_SCHEMA_KEY, DAYS_AFFECTED_KEY,
    ENTRY_COUNT_KEY, IMPORT_ID_KEY, ManifestKnownKey, RAW_RETENTION_KEY, SOURCE_HASH_KEY,
    SOURCE_TYPE_KEY,
};
pub use manifest_scan::{ScannedBodyManifest, scan_body_manifest};
pub use manifest_signal::{ManifestKeySignal, inspect_body_manifest_signal};
pub use parser::parse;
pub use presentation::PresentationRow;
pub use string::BodyString;
pub use value::{BodyObject, BodyValue};
