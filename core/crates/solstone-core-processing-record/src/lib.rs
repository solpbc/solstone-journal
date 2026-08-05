// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared vocabulary and lenient predicates for `_solstone_processing` records.

pub mod predicate;
pub mod vocab;

pub use predicate::{
    TerminalProofOutcome, evaluate_terminal_proof, is_failure_exhausted, record_attempts,
};
