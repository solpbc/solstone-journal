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
//! # What this crate does not do yet
//!
//! This is the foundation: the outcome types and the path type that proves a
//! removal happened. Neither verb exists yet, so nothing here removes anything.
//! The removal surface arrives in a later wave, confined to one module and held
//! there by `tests/architecture.rs`.
//!
//! ⛔ **This crate has no production caller, deliberately.** The removal an owner
//! can reach today is still the Python implementation's.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]
// A panic destroys an outcome as surely as a lost return does, and the workspace
// forbids only `unsafe_code`, so nothing else stops one inside a verb.
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

pub mod content;
pub mod door;
pub mod eligibility;
pub mod notify;
pub mod policy;
pub mod receipt;
pub mod staging;
pub mod tombstone;

pub use content::{ContentName, HandlerRegistry, MediaClassifier};
pub use door::{EvidenceTally, release_raw};
pub use eligibility::{Blocker, Evidence, FoundContent, ProvenRaw, RawRelease, SidecarFacts};
pub use notify::{IndexNotify, NoIndex, NotifyError, PruneCounts};
pub use policy::{Anchor, Days, Eligibility, Policy, Rule, SegmentAge};
pub use receipt::{NotRemoved, Outcome, RemovedPath, RunHalt, Target, TargetOutcome};
pub use staging::{STAGED_PREFIX, original_name, staged_name};
pub use tombstone::{ExecutorStamp, RemovalReason, TombstoneBody, tombstone_bytes};
