// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Indexed audio archive library.
//!
//! Provides planning, claim indexing, atomic tile publication, and frontier inspection
//! for audio archive imports.

use std::fmt;
use std::path::PathBuf;

pub mod claims;
pub mod frontier;
pub mod identity;
pub mod plan;
pub mod publish;
pub mod record;

pub use claims::{
    DEFAULT_WRITE_BUSY_TIMEOUT, archive_day_has_pending, claim_db_path, find_held_members,
    insert_tile_claims, read_matching_archive_run,
};
pub use frontier::{
    AudioArchiveFrontier, FrontierPart, FrontierTile, FrontierUnavailablePart, PendingArchiveBatch,
    finish_archive_part, finish_archive_part_checked, update_archive_frontier,
    update_pending_archive_tiles, update_pending_archive_tiles_checked,
};
pub use identity::{MemberIdentity, MemberStartEvidence, MemberTimeAuthority, ResolvedOccurrence};
pub use plan::{
    AUDIO_ARCHIVE_LEN_SECONDS, AUDIO_ARCHIVE_WINDOW_MS, AudioArchiveMemberInput, AudioArchivePlan,
    AudioArchiveSettings, AudioArchiveSize, DurationInput, IntervalConflictPayload, LeftOutReason,
    MemberDisposition, PartOutcome, PlannedMember, PlannedPart, PlannedTile, SimulatedPart,
    SourceRejection, apply_held_members, plan_audio_archive,
};
#[cfg(unix)]
pub use publish::read_published_part_receipt_bound;
pub use publish::{
    ArchiveFault, ArchiveProgressFn, ArchiveProgressPhase, AudioArchiveMaterializer,
    AudioArchiveProgress, AudioArchivePublishOptions, AudioArchivePublishResult,
    AudioArchivePublishSeams, EmitObservedFn, prepare_audio_archive_plan, publish_audio_archive,
};
pub use record::{
    AUDIO_ARCHIVE_RECORD_FILENAME, AUDIO_ARCHIVE_RECORD_VERSION, ArchiveStore, AudioArchiveRecord,
    AudioArchiveRecordMember, AudioArchiveRecordPart, AudioArchiveRecordSettings,
    AudioArchiveStatus, read_audio_archive, save_audio_archive_record, validate_resume_settings,
};

/// Dedicated error type for all audio archive operations.
#[derive(Debug)]
pub enum AudioArchiveError {
    SettingsMismatch(String),
    MissingRunDirectory(PathBuf),
    InvalidStreamName(String),
    ClaimDatabaseCorrupt(String),
    ClaimDatabaseBusy(String),
    MemberHeldByLivingRun { import_id: String },
    ChronicleSegmentRefused(String),
    HashMismatch { expected: String, actual: String },
    SourceRejected(SourceRejection),
    Fault(ArchiveFault),
    Io(std::io::Error),
    Json(serde_json::Error),
    Sqlite(rusqlite::Error),
    Other(String),
}

impl fmt::Display for AudioArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SettingsMismatch(msg) => write!(f, "settings mismatch: {msg}"),
            Self::MissingRunDirectory(path) => {
                write!(f, "missing run directory: {}", path.display())
            }
            Self::InvalidStreamName(msg) => write!(f, "invalid stream name: {msg}"),
            Self::ClaimDatabaseCorrupt(msg) => write!(f, "claim database corrupt: {msg}"),
            Self::ClaimDatabaseBusy(msg) => write!(f, "claim database busy: {msg}"),
            Self::MemberHeldByLivingRun { import_id } => {
                write!(f, "member held by living run: {import_id}")
            }
            Self::ChronicleSegmentRefused(msg) => {
                write!(f, "chronicle segment refused: {msg}")
            }
            Self::HashMismatch { expected, actual } => {
                write!(f, "hash mismatch: expected {expected}, actual {actual}")
            }
            Self::Fault(fault) => write!(f, "archive fault injected: {fault:?}"),
            Self::SourceRejected(cause) => write!(f, "archive source rejected: {cause:?}"),
            Self::Io(err) => write!(f, "i/o error: {err}"),
            Self::Json(err) => write!(f, "json error: {err}"),
            Self::Sqlite(err) => write!(f, "sqlite error: {err}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for AudioArchiveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::Sqlite(err) => Some(err),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for AudioArchiveError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}
impl From<serde_json::Error> for AudioArchiveError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl From<std::io::Error> for AudioArchiveError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests;
