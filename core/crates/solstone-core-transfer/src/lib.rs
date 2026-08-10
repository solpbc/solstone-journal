// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native per-day journal transfer archives.

#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

mod export;
mod import;
mod manifest;
mod rescan;
mod send;

use std::path::PathBuf;

use thiserror::Error;

pub use export::export;
pub use import::import;
pub use rescan::{RescanOutcome, send_indexer_rescan};
pub use send::{
    RESERVED_SEGMENT_FILENAMES, ResolvedPeer, SendReport, SendRequest, SendTerminal, send,
};

/// Input to [`export`].
#[derive(Debug, Clone)]
pub struct ExportRequest {
    /// The chronicle day to archive.
    pub day: String,
    /// Required explicit destination. Native transfer has no Python checkout
    /// project-root from which to derive the reference command's default.
    pub output: PathBuf,
}

/// Successful archive construction details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Day stored in the archive.
    pub day: String,
    /// Number of segments included in the manifest.
    pub segments: usize,
    /// Number of archived regular files.
    pub files: usize,
    /// Final output path.
    pub output: PathBuf,
}

/// Input to [`import`].
#[derive(Debug, Clone)]
pub struct ImportRequest {
    /// Archive to decode.
    pub archive: PathBuf,
    /// Validate and plan without publishing journal directories.
    pub dry_run: bool,
}

/// Outcome for one source segment, in canonical archive-key order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentOutcome {
    /// Published under its original key.
    Landed { source: String, target: String },
    /// Published under a newly selected key.
    LandedDeconflicted { source: String, target: String },
    /// Existing manifest-listed files already have matching hashes.
    SkippedAlreadySynced { source: String },
    /// Publication failed after earlier segments may have landed.
    Failed { source: String, reason: String },
    /// Not tried after an earlier publication failure.
    NotAttempted { source: String },
}

/// Complete import result, including partial import state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// Manifest day.
    pub day: String,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Ordered outcomes.
    pub outcomes: Vec<SegmentOutcome>,
    /// Rescan request result.
    pub rescan: RescanOutcome,
}

impl ImportReport {
    /// Number of directories published.
    pub fn landed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| {
                matches!(
                    outcome,
                    SegmentOutcome::Landed { .. } | SegmentOutcome::LandedDeconflicted { .. }
                )
            })
            .count()
    }

    /// Number of already-synced segments.
    pub fn skipped(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SegmentOutcome::SkippedAlreadySynced { .. }))
            .count()
    }

    /// Number of successfully deconflicted segments.
    pub fn deconflicted(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SegmentOutcome::LandedDeconflicted { .. }))
            .count()
    }

    /// Number of failed segment publications.
    pub fn failed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SegmentOutcome::Failed { .. }))
            .count()
    }

    /// Number of segments not attempted after a failure.
    pub fn not_attempted(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| matches!(outcome, SegmentOutcome::NotAttempted { .. }))
            .count()
    }
}

/// Transfer validation, source, or publication error.
#[derive(Debug, Error)]
pub enum TransferError {
    /// The supplied day is invalid.
    #[error("day must be YYYYMMDD")]
    InvalidDay,
    #[error("refusing symlinked journal day directory: {0}")]
    PoisonedDayDirectory(PathBuf),
    /// The day directory does not exist.
    #[error("journal day {0} does not exist")]
    MissingDay(String),
    /// No transferable segments were found.
    #[error("journal day {0} has no segments")]
    NoSegments(String),
    /// No paired peers are available in this journal.
    #[error("no peers paired (run \"sol link join --as peer\" first)")]
    NoPeersPaired,
    /// No peer has the requested label.
    #[error("no peer with label \"{label}\"; available: {available}")]
    PeerNotFound { label: String, available: String },
    /// More than one peer has the requested label.
    #[error(
        "multiple peers with label \"{label}\": {instance_ids}; use <journal_root>/peers/<instance_id> directly"
    )]
    AmbiguousPeer { label: String, instance_ids: String },
    /// Paired-link identity files or configuration could not be loaded.
    #[error("paired-link credential load failed: {0}")]
    CredentialLoad(String),
    /// Carrier or loopback HTTP transport failed.
    #[error("paired-link transport failed: {0}")]
    Transport(String),
    /// The local paired-link bridge could not start or drain.
    #[error("paired-link bridge failed: {0}")]
    Bridge(String),
    /// The requested output parent is absent or not a directory.
    #[error("output parent {0} does not exist or is not a directory")]
    MissingOutputParent(PathBuf),
    /// A source entry cannot be represented in the archive.
    #[error("unsupported non-regular source member: {0}")]
    UnsupportedSource(PathBuf),
    /// Archive input or filesystem operation failed.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Journal path validation failed.
    #[error("{0}")]
    Path(#[from] solstone_core_journal_io::PathError),
    /// Segment deconfliction failed.
    #[error("{0}")]
    Deconflict(#[from] solstone_core_journal_io::SegmentDeconflictError),
    /// Manifest JSON or shape is invalid.
    #[error("invalid transfer manifest: {0}")]
    Manifest(String),
    /// A tar member is invalid for this archive format.
    #[error("invalid archive member: {0}")]
    ArchiveMember(String),
    /// A manifest-listed file does not match its expected content.
    #[error("content mismatch for {0}")]
    ContentMismatch(String),
    /// Staged destination publication failed.
    #[error("{0}")]
    Staged(#[from] solstone_core_journal_io::StagedWriteError),
}

/// Import failure. Partial failures retain their complete report.
#[derive(Debug, Error)]
pub enum ImportError {
    /// Validation or pre-publication failure.
    #[error(transparent)]
    Fatal(#[from] TransferError),
    /// A segment failed after earlier segments landed.
    #[error("partial import: {reason}")]
    Partial {
        report: ImportReport,
        reason: String,
    },
}
