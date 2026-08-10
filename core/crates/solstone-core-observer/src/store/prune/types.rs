// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use solstone_core_segment::ContentIdentity;

use super::marker::StreamMarker;

/// A structured refusal: prune stops rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub subject: String,
    pub gate: &'static str,
    pub file: Option<String>,
    pub resolution: String,
}

impl Refusal {
    pub fn new(
        subject: impl Into<String>,
        gate: &'static str,
        file: Option<impl Into<String>>,
        resolution: impl Into<String>,
    ) -> Self {
        Self {
            subject: subject.into(),
            gate,
            file: file.map(Into::into),
            resolution: resolution.into(),
        }
    }
}

/// One segment's analyzed identity, marker state and unknown files.
#[derive(Debug, Clone)]
pub struct SegmentAnalysis {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub path: PathBuf,
    pub marker: Option<StreamMarker>,
    pub marker_error: Option<String>,
    pub identity: Option<ContentIdentity>,
    pub identity_issue: Option<Refusal>,
    pub unknown_files: Vec<String>,
}

impl SegmentAnalysis {
    pub fn label(&self) -> String {
        format!("{}/{}/{}", self.day, self.stream, self.segment)
    }
}

/// A candidate pruneable segment inside a group.
#[derive(Debug, Clone)]
pub struct PruneCandidate {
    pub analysis: SegmentAnalysis,
    pub last_physical_copy: bool,
}

/// One canonical + its duplicate candidates, restricted to one `(day, stream,
/// start)` set (or, for cross-start, one server-proven origin cluster).
#[derive(Debug, Clone)]
pub struct PruneGroup {
    pub day: String,
    pub stream: String,
    pub start: String,
    pub canonical: SegmentAnalysis,
    pub candidates: Vec<PruneCandidate>,
}

/// The full plan or execution outcome.
#[derive(Debug, Clone, Default)]
pub struct PruneResult {
    pub execute: bool,
    pub groups: Vec<PruneGroup>,
    pub refusals: Vec<Refusal>,
    pub deleted: Vec<PruneCandidate>,
    pub index_errors: Vec<String>,
    pub crash_repaired: u64,
    pub chain_repaired: u64,
}

impl PruneResult {
    pub fn new(execute: bool) -> Self {
        Self {
            execute,
            ..Default::default()
        }
    }

    /// Counts over `deleted` when executing, else over every planned
    /// candidate -- correct in both dry-run and execute output.
    pub fn last_physical_copy_count(&self) -> usize {
        if self.execute {
            self.deleted
                .iter()
                .filter(|candidate| candidate.last_physical_copy)
                .count()
        } else {
            self.groups
                .iter()
                .flat_map(|group| &group.candidates)
                .filter(|candidate| candidate.last_physical_copy)
                .count()
        }
    }

    /// The observer prune CLI exit code: 2 when refusals are present, else 0.
    pub fn exit_code(&self) -> i32 {
        if self.refusals.is_empty() { 0 } else { 2 }
    }
}
