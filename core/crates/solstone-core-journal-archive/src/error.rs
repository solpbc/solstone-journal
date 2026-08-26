// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use solstone_core_journal_io::{JournalEntryKind, JournalRootError};

use crate::ArchiveMemberName;

/// Failure while acquiring or reading a portable archive source.
#[derive(Debug)]
pub enum ArchiveError {
    /// The requested journal root is not a usable absolute directory.
    InvalidJournal { root: PathBuf, reason: &'static str },
    /// The current backend cannot retain a handle for this journal root.
    UnsupportedJournal { root: PathBuf, reason: &'static str },
    /// A stable unsafe object was found while initially inventorying a root.
    UnsafeJournalEntry {
        member: ArchiveMemberName,
        kind: JournalEntryKind,
    },
    /// A source operation failed without evidence of a replacement race.
    SourceIo {
        operation: &'static str,
        member: Option<ArchiveMemberName>,
        source: io::Error,
    },
    /// The acquired source or a frozen route changed after observation.
    SourceChanged { member: Option<ArchiveMemberName> },
}

impl fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
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
            Self::UnsafeJournalEntry { member, kind } => {
                write!(
                    formatter,
                    "unsafe journal entry {}: {kind:?}",
                    member.as_str()
                )
            }
            Self::SourceIo {
                operation,
                member: Some(member),
                source,
            } => write!(formatter, "{operation} {}: {source}", member.as_str()),
            Self::SourceIo {
                operation,
                member: None,
                source,
            } => write!(formatter, "{operation}: {source}"),
            Self::SourceChanged {
                member: Some(member),
            } => write!(formatter, "journal source changed at {}", member.as_str()),
            Self::SourceChanged { member: None } => {
                formatter.write_str("journal source changed during acquisition")
            }
        }
    }
}

impl Error for ArchiveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceIo { source, .. } => Some(source),
            Self::InvalidJournal { .. }
            | Self::UnsupportedJournal { .. }
            | Self::UnsafeJournalEntry { .. }
            | Self::SourceChanged { .. } => None,
        }
    }
}

pub(crate) fn map_root_error(error: JournalRootError) -> ArchiveError {
    match error {
        JournalRootError::Invalid { root, reason, .. } => {
            ArchiveError::InvalidJournal { root, reason }
        }
        JournalRootError::Unsupported { root, reason, .. } => {
            ArchiveError::UnsupportedJournal { root, reason }
        }
        JournalRootError::Io {
            operation, source, ..
        } => ArchiveError::SourceIo {
            operation,
            member: None,
            source,
        },
        JournalRootError::Changed => ArchiveError::SourceChanged { member: None },
    }
}
