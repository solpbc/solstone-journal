// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The names a body source's windows are written under.
//!
//! One writer merges every body source's rows into windows. What differs between
//! sources is where their files go, what their rows and errors are called, and how
//! a later import's span meets a stored one. That is all this holds, with the
//! streams the activity record writer reads, Strava's among them.

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

/// How an experience's tiles are named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileIds {
    /// `<id>-<index>`: a tile is its place in the span.
    Index,
    /// `<id>@<YYYYMMDD>/<window key>`, the window before any step around older
    /// content: a tile is its window, so one that stays in its window when the
    /// span moves is the same row, edited.
    Window,
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
    pub tiles: TileIds,
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
    tiles: TileIds::Index,
    lock_unavailable: "apple_health_lock_unavailable",
    zone_unrecognised: "apple_health_zone_unrecognised",
};

pub const OURA: BodySource = BodySource {
    stream: "import.oura",
    source: "oura",
    registry: RegistrySource::Oura,
    lock_rel: "imports/.oura",
    run_dir_rel: "imports/.oura-run",
    zone_rel: "imports/.oura-zone.json",
    span: SpanPolicy::Document,
    tiles: TileIds::Window,
    lock_unavailable: "oura_lock_unavailable",
    zone_unrecognised: "oura_zone_unrecognised",
};

/// The file a stream's windows hold, as the activity record writer reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowFile {
    /// `body.jsonl`, the rows the shared writer merged.
    BodyRecords,
    /// A Strava tile's `workout.json`, read as body rows through the Strava
    /// adapter.
    StravaTile,
}

/// A stream the activity record writer runs over: its windows, the lock its
/// import holds and the file its windows hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordSource {
    pub stream: &'static str,
    /// Held by the import that writes this stream's windows; the record writer
    /// runs under it.
    pub lock_rel: &'static str,
    pub file: WindowFile,
}

impl BodySource {
    /// This source's windows as the activity record writer reads them.
    #[must_use]
    pub const fn records(&self) -> RecordSource {
        RecordSource {
            stream: self.stream,
            lock_rel: self.lock_rel,
            file: WindowFile::BodyRecords,
        }
    }
}

/// Strava's tiles. Strava keeps its own writer and its own tiles, so it has no
/// [`BodySource`]; its records are written over its tiles as they are.
pub const STRAVA: RecordSource = RecordSource {
    stream: solstone_core_format::content::strava_body::STRAVA_STREAM,
    lock_rel: "imports/.strava",
    file: WindowFile::StravaTile,
};
