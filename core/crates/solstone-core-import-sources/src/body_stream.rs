// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The names a body source's windows are written under.
//!
//! One writer merges every body source's rows into windows. What differs between
//! sources is where their files go, what their rows and errors are called, and how
//! a later import's span meets a stored one. That is all this holds.

use solstone_core_import::RegistrySource;

/// How an experience's span meets the one already stored under its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanPolicy {
    /// Nothing shrinks: a stored end later than this import's is kept. For a source
    /// whose export can omit the end of an experience an earlier one held.
    Union,
    /// The span is the one this import states, for a source that states each
    /// experience whole every time.
    Document,
}

/// A body source's stream, its row and file names, and its span policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodySource {
    /// The stream its windows are in, under each day.
    pub stream: &'static str,
    /// What its rows name as their source, and what its summary calls it.
    pub source: &'static str,
    pub registry: RegistrySource,
    /// Held for a whole run, so merging into a window can't race another import of
    /// this source.
    pub lock_rel: &'static str,
    /// A run's own folder: its state and its staging.
    pub run_dir_rel: &'static str,
    /// The zone fixed at this source's first import.
    pub zone_rel: &'static str,
    pub span: SpanPolicy,
    /// The error when another run holds the lock.
    pub lock_unavailable: &'static str,
    /// The error when the fixed zone can't be read.
    pub zone_unrecognised: &'static str,
}

pub const APPLE: BodySource = BodySource {
    stream: "import.apple_health",
    source: "apple_health",
    registry: RegistrySource::AppleHealth,
    lock_rel: "imports/.apple_health",
    run_dir_rel: "imports/.apple_health-run",
    zone_rel: "imports/.apple_health-zone.json",
    span: SpanPolicy::Union,
    lock_unavailable: "apple_health_lock_unavailable",
    zone_unrecognised: "apple_health_zone_unrecognised",
};
