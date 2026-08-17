// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared vocabulary and lenient predicates for `_solstone_processing` records.

pub mod media;
pub mod predicate;
pub mod reentry;
pub mod vocab;

#[cfg(test)]
mod test_support;

pub use media::{MediaKind, analysis_row_key, expected_handler, is_media_extension, media_kind};
pub use predicate::{
    TerminalProofOutcome, evaluate_terminal_proof, is_failure_exhausted, record_attempts,
};
pub use reentry::{
    jsonl_has_row_with_key, read_processing_record_header, should_reenter_analysis_output,
};
