// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Python-compatible JSON source decoding and canonicalization for body import data.

mod candidate;
mod canonicalize;
mod coordinate;
mod error;
mod integer;
mod parser;
mod presentation;
mod string;
mod value;

pub use candidate::{FieldState, LedgerCandidate, LedgerSchema, ValueState, project};
pub use canonicalize::canonicalize;
pub use coordinate::Coordinate;
pub use error::{
    CandidateError, CandidateErrorCode, CandidateErrorField, CanonicalizeError, ParseError,
};
pub use integer::BodyInteger;
pub use parser::parse;
pub use presentation::PresentationRow;
pub use string::BodyString;
pub use value::{BodyObject, BodyValue};
