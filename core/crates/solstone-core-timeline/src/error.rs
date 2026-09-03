// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::schema::TimelineKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineCurationStage {
    Day,
    Master,
}

impl std::fmt::Display for TimelineCurationStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Day => "day",
            Self::Master => "master",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidSelectionReason {
    OutOfRange,
    Duplicate,
}

impl std::fmt::Display for InvalidSelectionReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::OutOfRange => "out of range",
            Self::Duplicate => "duplicate",
        })
    }
}

#[derive(Debug, Error)]
pub enum TimelineError {
    #[error("ambiguous segment {day}/{segment} across streams {streams:?}")]
    AmbiguousSegment {
        day: String,
        segment: String,
        streams: Vec<String>,
    },
    #[error("segment {day}/{segment} was not found in stream {stream:?}")]
    SegmentNotFound {
        day: String,
        segment: String,
        stream: Option<String>,
    },
    #[error(
        "invalid {stage} model selection index {index} for {candidate_count} candidates: {reason}"
    )]
    InvalidModelSelection {
        stage: TimelineCurationStage,
        index: usize,
        candidate_count: usize,
        reason: InvalidSelectionReason,
    },
    #[error("timeline schema version mismatch: expected {expected}, got {actual}")]
    SchemaVersionMismatch { expected: u32, actual: u32 },
    #[error("timeline schema kind mismatch: expected {expected:?}, got {actual:?}")]
    SchemaKindMismatch {
        expected: TimelineKind,
        actual: TimelineKind,
    },
    #[error("malformed segment binding ({day:?}, {stream:?}, {segment:?})")]
    MalformedBinding {
        day: String,
        stream: String,
        segment: String,
    },
    #[error("invalid segment identity: {detail}")]
    InvalidSegmentIdentity { detail: String },
    #[error("timeline lock contention: {detail}")]
    LockContention { detail: String },
    #[error("timeline digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error("timeline curation failed: {detail}")]
    CurationFailed { detail: String },
    #[error("timeline publication durability is uncertain for {}: {detail}", path.display())]
    DurabilityUncertain { path: PathBuf, detail: String },
    #[error("timeline publication did not complete for {}: {detail}", path.display())]
    PublicationNotConfirmed { path: PathBuf, detail: String },
    #[error("timeline publication failed ({primary}); terminal state write also failed ({state})")]
    TerminalStateWriteFailed { primary: String, state: String },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    #[error(transparent)]
    Fingerprint(#[from] solstone_core_brain::FingerprintError),
    #[error(transparent)]
    JournalPath(#[from] solstone_core_journal_io::PathError),
    #[error(transparent)]
    Atomic(#[from] solstone_core_journal_io::DetailedAtomicError),
}

impl TimelineError {
    pub(crate) fn segment_not_found(day: &str, segment: &str, stream: Option<&str>) -> Self {
        let stream = stream.map(ToOwned::to_owned);
        Self::SegmentNotFound {
            day: day.to_owned(),
            segment: segment.to_owned(),
            stream,
        }
    }
}
