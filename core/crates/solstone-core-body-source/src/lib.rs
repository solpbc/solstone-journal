// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible JSON source decoding and canonicalization for body import data.

mod apple_summary_plan;
mod authority;
mod body_day;
mod body_envelope;
mod body_envelope_decode;
mod body_envelope_encode;
mod body_envelope_manifest_binding;
mod body_envelope_projection;
mod body_envelope_scan;
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
mod envelope_ledger;
mod envelope_shard;
mod error;
mod health_hash;
mod integer;
mod ledger_event;
mod ledger_event_decode;
mod ledger_event_encode;
mod ledger_event_projection;
mod ledger_event_scan;
mod manifest_binding;
mod manifest_decode;
mod manifest_known_key;
mod manifest_projection;
mod manifest_scan;
mod manifest_signal;
mod parser;
mod presentation;
mod string;
mod value;
mod whitespace;

pub use apple_summary_plan::AppleSummaryPlan;
pub use authority::{
    AuthorityError, BundleClass, DirectoryObservation, NativeAuthority, authorize_native_bundle,
    classify_bundle_directory,
};
pub use body_day::BodyDay;
pub use body_envelope::BodyEnvelope;
pub use body_envelope_decode::decode_body_envelope;
pub use body_envelope_encode::encode_body_envelope;
pub use body_envelope_manifest_binding::decode_body_envelope_with_manifest;
pub use body_month::BodyMonth;
pub use body_raw_retention::BodyRawRetention;
pub use body_source_family::BodySourceFamily;
pub use body_source_hash::BodySourceHash;
pub use bundle_id::BundleId;
pub use candidate::{FieldState, LedgerCandidate, LedgerSchema, ValueState, project};
pub use canonicalize::canonicalize;
pub use coordinate::Coordinate;
pub use digest::BodyDigest;
pub use envelope_ledger::EnvelopeLedger;
pub use envelope_shard::EnvelopeShard;
pub use error::{
    BodyCalendarError, BodyCalendarField, BodyHashError, BodySourceHashError,
    BodySourcePolicyError, BodySourcePolicyField, BodyWireIdentityError, BodyWireIdentityField,
    CandidateError, CandidateErrorCode, CandidateErrorField, CanonicalizeError, EnvelopeError,
    EnvelopeErrorCode, EnvelopeErrorField, IdentityField, LedgerEventError, LedgerEventErrorCode,
    LedgerEventErrorField, ManifestBindingError, ManifestBindingErrorCode,
    ManifestBindingErrorField, ManifestScanError, ParseError,
};
pub use health_hash::{
    HealthRecordIdentity, health_hash, health_record_dedupe_key, health_value_hash,
};
pub use integer::BodyInteger;
pub use ledger_event::BodyLedgerEvent;
pub use ledger_event_decode::decode_body_ledger_event;
pub use ledger_event_encode::encode_body_ledger_event;
pub use manifest_binding::BodyManifestBinding;
pub use manifest_decode::decode_body_manifest;
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
