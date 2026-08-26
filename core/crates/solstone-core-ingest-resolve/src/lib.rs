// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resolution and application of native cid/source ingest requests onto journal segments.
//!
//! This crate separates exploratory candidate evaluation from the later write
//! phase. `resolve` deliberately contains no journal write primitive; its
//! structural test keeps that property local and reviewable.

mod apply;
mod held;
mod manifest;
mod notify;
mod quarantine;
mod resolve;
mod terminal_proof;

pub use apply::{
    AppliedDisposition, AppliedFile, ApplyError, ApplyFailure, ApplyResult, apply_plan,
};
pub use manifest::write_ingest_manifest;
pub use notify::{IngestNotice, IngestNotifier};
pub use quarantine::{QuarantineReceipt, quarantine_failed};
pub use resolve::{
    ApplyPlan, ConflictPlan, FailedPlan, FileDisposition, HeldEvidence, IngestFile,
    MAX_INGEST_SEGMENT_ATTEMPTS, MissingWriteReason, PlanStatus, PlannedFile, Resolution,
    ResolveError, UnwrittenReason, resolve_ingest,
};
pub use solstone_core_journal_io::bump_stream_marker;
pub use terminal_proof::SegmentTerminalProof;

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod architecture_tests {
    #[test]
    fn resolve_has_no_journal_write_primitive() {
        let source = include_str!("resolve.rs");
        for primitive in [
            "write_bytes_exclusive",
            "hold_lock",
            "write_json",
            "append_jsonl",
            "atomic_replace",
            "write_content",
            "append_event",
            "std::fs::write",
            "File::create",
            "OpenOptions::new",
            ".write_all(",
        ] {
            assert!(
                !source.contains(primitive),
                "forbidden resolution primitive {primitive}"
            );
        }
    }
}
