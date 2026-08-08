// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible JSON source decoding and canonicalization for body import data.

mod bundle_id;
mod candidate;
mod canonicalize;
mod coordinate;
mod digest;
mod error;
mod health_hash;
mod integer;
mod parser;
mod presentation;
mod string;
mod value;
mod whitespace;

pub use bundle_id::BundleId;
pub use candidate::{FieldState, LedgerCandidate, LedgerSchema, ValueState, project};
pub use canonicalize::canonicalize;
pub use coordinate::Coordinate;
pub use digest::BodyDigest;
pub use error::{
    BodyHashError, BodyWireIdentityError, BodyWireIdentityField, CandidateError,
    CandidateErrorCode, CandidateErrorField, CanonicalizeError, IdentityField, ParseError,
};
pub use health_hash::{
    HealthRecordIdentity, health_hash, health_record_dedupe_key, health_value_hash,
};
pub use integer::BodyInteger;
pub use parser::parse;
pub use presentation::PresentationRow;
pub use string::BodyString;
pub use value::{BodyObject, BodyValue};
