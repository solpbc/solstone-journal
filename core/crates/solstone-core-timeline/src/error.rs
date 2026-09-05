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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineCurationFailureReason {
    GenerateFailed,
    Refused,
    RequestBindingMismatch,
    NonStopFinish,
    BlankModel,
    MissingSchemaEvidence,
    InvalidSchemaEvidence,
    MalformedPayload,
    WrongPickCount,
    WrongPickType,
    BlankRationale,
    InvalidConcurrency,
    WorkerPanicked,
}

impl TimelineCurationFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GenerateFailed => "generate-failed",
            Self::Refused => "refused",
            Self::RequestBindingMismatch => "request-binding-mismatch",
            Self::NonStopFinish => "non-stop-finish",
            Self::BlankModel => "blank-model",
            Self::MissingSchemaEvidence => "missing-schema-evidence",
            Self::InvalidSchemaEvidence => "invalid-schema-evidence",
            Self::MalformedPayload => "malformed-payload",
            Self::WrongPickCount => "wrong-pick-count",
            Self::WrongPickType => "wrong-pick-type",
            Self::BlankRationale => "blank-rationale",
            Self::InvalidConcurrency => "invalid-concurrency",
            Self::WorkerPanicked => "worker-panicked",
        }
    }
}

impl std::fmt::Display for TimelineCurationFailureReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
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
    #[error("timeline state unavailable for {subject}: {detail}")]
    StateUnavailable { subject: String, detail: String },
    #[error("timeline conversion required for {subject}: {detail}")]
    ConversionRequired { subject: String, detail: String },
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
    #[error("invalid segment source evidence: {detail}")]
    InvalidSourceEvidence { detail: String },
    #[error("timeline lock contention: {detail}")]
    LockContention { detail: String },
    #[error("timeline digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },
    #[error("timeline curation failed ({reason}): {detail}")]
    CurationFailed {
        reason: TimelineCurationFailureReason,
        detail: String,
    },
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
