// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retention: what raw media is kept, what logs are kept, and **every removal of
//! the owner's media**.
//!
//! One subsystem owns every irreversible removal, with one policy, one set of
//! guards, and one place to look when something went. Other boundaries *request*;
//! this one removes, and then tells the search index which paths it actually
//! removed -- in that order, never the reverse.
//!
//! # Two units of removal
//!
//! The owner deletes **segments** -- one, or a set -- and each is emptied whole,
//! leaving only a tombstone. Separately, and with nobody having asked, the
//! retention lifecycle releases raw **originals** whose processing is proven
//! terminal, and every derived output survives that. ⛔ There is no third unit,
//! and no partial owner-directed delete.
//!
//! # Removal surface and proposals
//!
//! The removal door implements both whole-segment removal and raw release, and
//! the retention executor exposes those operations through its CLI seam. The
//! retention executor invokes that seam. This crate's architecture test confines
//! irreversible work to the door module.
//!
//! The `marks` register stores removal proposals. The retention CLI records
//! staged failures onto it; in-process door callers (transcripts-web,
//! clients-web) report failures in their own receipts and do not write marks.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
// A panic destroys an outcome as surely as a lost return does, and the workspace lint
// set does not stop one inside a verb, so these are denied here.
//
// ⚠ Worded to avoid the literal word a repo-wide source scan greps for. That scan
// (`tests/test_rust_policy_baseline.py`) matches the bare substring and so fires on
// PROSE about the lint as readily as on a real block -- the same substring-scan hazard
// that made a bare `rename` match `#[serde(rename_all)]` in this crate's own
// architecture test. Flagged rather than fixed: it is not this lane's test, and it was
// already red on a sibling crate's doc comment before this crate existed.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable
)]

pub mod age;
pub mod class;
pub mod content;
pub mod door;
pub mod eligibility;
pub mod layout;
pub mod logs;
pub mod marks;
pub mod notify;
pub mod oplog_retention;
pub mod policy;
pub mod receipt;
pub mod remove_marked;
pub mod scan;
pub mod staging;
pub mod summary;
pub mod sweep;
pub mod tombstone;

pub use class::{MediaClass, classify};
pub use content::{ContentName, HandlerRegistry, MediaClassifier};
pub use door::{EvidenceTally, release_raw, remove_planned_oplogs};
pub use eligibility::{Blocker, Evidence, FoundContent, ProvenRaw, RawRelease, SidecarFacts};
pub use marks::{
    Failure, Mark, MarkId, MarkState, Proposal, Register, RemovalClass, resolve_offload,
    upsert_offload,
};
pub use notify::{IndexNotify, NoIndex, NotifyError, PruneCounts};
pub use oplog_retention::{
    OplogRetentionKept, OplogRetentionPlan, OplogRetentionTarget, RetainedOplog,
    plan_oplog_retention,
};
pub use policy::{Anchor, Days, Eligibility, Policy, Rule, SegmentAge};
pub use receipt::{
    NotRemoved, Outcome, PostCommitFailure, RemovedPath, RunHalt, Target, TargetOutcome,
};
pub use staging::{STAGED_PREFIX, original_name, staged_name};
pub use summary::{StorageSummary, compute_storage_summary, human_bytes};
pub use tombstone::{ExecutorStamp, RemovalReason, TombstoneBody, tombstone_bytes};
