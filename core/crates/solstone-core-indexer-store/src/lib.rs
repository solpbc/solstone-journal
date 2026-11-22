// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod activity_provenance;
pub mod calendar;
pub mod calendar_schedule;
pub mod chunk_sources;
pub mod classification;
pub mod classification_batch;
mod cli_detail;
pub mod db;
pub mod edge_refresh;
pub mod generation;
mod hide;
pub mod merge;
pub mod migrations;
pub mod reconcile;
pub mod recovery;
pub mod retention;
pub mod scan;
mod screen_text;
pub mod status;
pub mod todo;
pub(crate) mod writer_admission;

pub use chunk_sources::{PathLookupStatus, apply_path_lookup, inspect_path_lookup};
pub use classification_batch::{
    ClassificationInitialization, ClassificationStatus, apply_classification_batch,
    drain_classifications, inspect_classifications,
};
pub use db::open_index_reader;
pub use retention::RetentionIndex;
pub use scan::{SavedPublicationAttempt, SavedPublicationOutcome, attempt_saved_publication};

pub use screen_text::{
    screen_text_current_digest, screen_text_pinned_bytes_match, screen_text_pinned_page,
    screen_text_source_current,
};

#[cfg(test)]
mod hide_tests;
#[cfg(test)]
mod test_support;

use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum StoreError {
    Discovery(solstone_core_indexer::discovery::DiscoveryError),
    Edge(solstone_core_indexer::edges::EdgeError),
    EdgeFileFailed(solstone_core_cli_copy::MessageDetail),
    Io(io::Error),
    Path(solstone_core_format::paths::JournalPathError),
    Sql(rusqlite::Error),
    OutsideJournal(PathBuf),
    NonUtf8Path(PathBuf),
    MissingFile(PathBuf),
    MemoryOriginalUnavailable {
        path: PathBuf,
        detail: String,
    },
    PathLookupRequired {
        cause: Option<String>,
    },
    WriterBusy {
        operation: &'static str,
        cause: String,
    },
    WriterAdmission {
        operation: &'static str,
        cause: String,
    },
    IndexGenerationNewer {
        found: i64,
        known: i64,
    },
}

impl StoreError {
    /// Whether SQLite itself says the index file is damaged. Busy, locked, full,
    /// read-only and I/O errors say nothing about the file and don't count.
    #[must_use]
    pub fn is_corruption(&self) -> bool {
        matches!(
            self,
            Self::Sql(rusqlite::Error::SqliteFailure(failure, _))
                if matches!(
                    failure.code,
                    rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
                )
        )
    }
}

impl StoreError {
    /// Whether this is worth asking for a check of the file. Wider than
    /// [`Self::is_corruption`] on purpose: indexing a file keeps only the text of
    /// a SQL error. It can only start a check, and only the check can empty anything.
    #[must_use]
    pub fn suggests_damage(&self) -> bool {
        self.is_corruption()
            || matches!(self, Self::Io(error) if recovery::text_suggests_damage(&error.to_string()))
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.diagnostic(), formatter)
    }
}

impl std::error::Error for StoreError {}

impl From<solstone_core_indexer::discovery::DiscoveryError> for StoreError {
    fn from(error: solstone_core_indexer::discovery::DiscoveryError) -> Self {
        StoreError::Discovery(error)
    }
}

impl From<solstone_core_indexer::edges::EdgeError> for StoreError {
    fn from(error: solstone_core_indexer::edges::EdgeError) -> Self {
        StoreError::Edge(error)
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        StoreError::Io(error)
    }
}

impl From<solstone_core_format::paths::JournalPathError> for StoreError {
    fn from(error: solstone_core_format::paths::JournalPathError) -> Self {
        StoreError::Path(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Sql(error)
    }
}
