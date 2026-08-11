// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Failure that prevents a per-day statistics result from being computed.
#[derive(Debug, Error)]
pub enum JournalStatsError {
    #[error("invalid day {0}; expected YYYYMMDD calendar date")]
    InvalidDay(String),
    #[error("journal path error: {0}")]
    JournalPath(#[from] solstone_core_journal_io::PathError),
    #[error("journal read error: {0}")]
    JournalRead(#[from] solstone_core_journal_io::ReadError),
    #[error("journal atomic-write error: {0}")]
    JournalWrite(#[from] solstone_core_journal_io::AtomicWriteError),
    #[error("facet store error: {0}")]
    FacetStore(#[from] solstone_core_facets::FacetStoreError),
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid talent configuration {path}: {message}")]
    TalentConfig { path: PathBuf, message: String },
}

impl JournalStatsError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn json(path: impl Into<PathBuf>, source: serde_json::Error) -> Self {
        Self::Json {
            path: path.into(),
            source,
        }
    }
}
