// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::PathBuf;

use solstone_core_journal_io::JournalRootError;

/// One outcome/error vocabulary for the convergence store.
#[derive(Debug)]
pub enum ConvergenceError {
    InvalidJournal {
        root: PathBuf,
        reason: &'static str,
    },
    UnsupportedJournal {
        root: PathBuf,
        reason: &'static str,
    },
    Changed {
        what: ChangedWhat,
    },
    Unknown {
        role: DurableRole,
    },
    PreservedPrior {
        operation: &'static str,
        source: io::Error,
    },
    Refused(Refusal),
    Io {
        operation: &'static str,
        role: DurableRole,
        source: io::Error,
    },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ChangedWhat {
    Root,
    LockNamespace,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DurableRole {
    RootWitness,
    Allocator,
    Adoption,
    EverWitness,
    Head,
    RevisionWitness,
    Record,
    DayLock,
    TopologyLock,
    Directory,
    ClaimRevision,
    ClaimHead,
    Intent,
    Active,
    Terminal,
    ClearanceMember,
    ClearanceBarrier,
    #[allow(dead_code)]
    ConsumptionWitness,
    StreamUpdated,
    DailyUpdated,
    ChronicleHealth,
}

#[derive(Debug)]
pub enum Refusal {
    UnknownField {
        field: String,
    },
    MissingSerial,
    FutureSerial {
        observed: u64,
        next: u64,
    },
    RevisionRollback {
        observed: u64,
        current: u64,
    },
    GenerationRollback {
        observed: u64,
        current: u64,
    },
    PersistedZeroRevision,
    PersistedZeroDirtyGeneration,
    PersistedZeroSerial,
    CompletedExceedsDirty,
    WrongLineage,
    WrongDay {
        expected: String,
        observed: String,
    },
    Exhausted,
    Uninitialized,
    AlreadyInitialized,
    NonCanonicalDays,
    DuplicateDays,
    StaleLease,
    ReusedAuthority,
    Busy,
    NoPermit,
    IntentMismatch,
    IntentDigestMismatch,
    ChangedPredecessor,
    ChangedProjection,
    ClaimAncestry,
    NotVirgin,
    CleanupOnly,
    ConflictingProjection,
    ConflictingTerminal,
    WrongGenerationMarker,
    OldAuthorMarker,
    OldProjectionDigest,
    ProjectionByteMismatch,
    WrongOutcome,
    OppositeTerminal,
    GenericRejection,
    #[allow(dead_code)]
    DaySetChanged,
    #[allow(dead_code)]
    ClaimSwapped,
    #[allow(dead_code)]
    IncompleteEvidence,
    StaleEvidence,
    #[allow(dead_code)]
    MixedEvidence,
    Superseded,
}

impl std::fmt::Display for ConvergenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJournal { root, reason } => {
                write!(
                    formatter,
                    "invalid journal root {}: {reason}",
                    root.display()
                )
            }
            Self::UnsupportedJournal { root, reason } => {
                write!(
                    formatter,
                    "unsupported journal root {}: {reason}",
                    root.display()
                )
            }
            Self::Changed { what } => write!(formatter, "convergence source changed: {what:?}"),
            Self::Unknown { role } => write!(formatter, "unknown convergence state for {role:?}"),
            Self::PreservedPrior { operation, source } => {
                write!(formatter, "{operation}: prior preserved: {source}")
            }
            Self::Refused(refusal) => write!(formatter, "refused: {refusal:?}"),
            Self::Io {
                operation,
                role,
                source,
            } => write!(formatter, "{operation} ({role:?}): {source}"),
        }
    }
}

pub(crate) fn map_root_error(error: JournalRootError) -> ConvergenceError {
    match error {
        JournalRootError::Invalid { root, reason } => {
            ConvergenceError::InvalidJournal { root, reason }
        }
        JournalRootError::Unsupported { root, reason } => {
            ConvergenceError::UnsupportedJournal { root, reason }
        }
        JournalRootError::Io {
            operation, source, ..
        } => ConvergenceError::Io {
            operation,
            role: DurableRole::Directory,
            source,
        },
        JournalRootError::Changed => ConvergenceError::Changed {
            what: ChangedWhat::Root,
        },
    }
}

pub(crate) fn random_hex() -> Result<String, ConvergenceError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| ConvergenceError::Io {
        operation: "read csprng",
        role: DurableRole::Directory,
        source: io::Error::other(error.to_string()),
    })?;
    Ok(crate::digest::hex_encode(&bytes))
}

impl std::error::Error for ConvergenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PreservedPrior { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
