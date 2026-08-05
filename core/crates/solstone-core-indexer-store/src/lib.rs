// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub mod db;
pub mod merge;
pub mod scan;

use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum StoreError {
    AggregateIncomplete {
        segment: String,
        warnings: Vec<String>,
    },
    Discovery(solstone_core_indexer::discovery::DiscoveryError),
    Edge(solstone_core_indexer::edges::EdgeError),
    EdgeFileFailed(String),
    Io(io::Error),
    Path(solstone_core_indexer::paths::JournalPathError),
    Sql(rusqlite::Error),
    OutsideJournal(PathBuf),
    NonUtf8Path(PathBuf),
    MissingFile(PathBuf),
    EdgeRebuildFailed(scan::EdgeRebuildReport),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::AggregateIncomplete { segment, warnings } => {
                if warnings.is_empty() {
                    write!(formatter, "segment aggregate incomplete for {segment}")
                } else {
                    write!(
                        formatter,
                        "segment aggregate incomplete for {segment}: {}",
                        warnings.join("; ")
                    )
                }
            }
            StoreError::Discovery(error) => write!(formatter, "{error}"),
            StoreError::Edge(error) => write!(formatter, "{error}"),
            StoreError::EdgeFileFailed(message) => write!(formatter, "{message}"),
            StoreError::Io(error) => write!(formatter, "{error}"),
            StoreError::Path(error) => write!(formatter, "{error}"),
            StoreError::Sql(error) => write!(formatter, "{error}"),
            StoreError::OutsideJournal(path) => {
                write!(
                    formatter,
                    "file is outside journal directory: {}",
                    path.display()
                )
            }
            StoreError::NonUtf8Path(path) => {
                write!(formatter, "path is not valid UTF-8: {}", path.display())
            }
            StoreError::MissingFile(path) => {
                write!(formatter, "file not found: {}", path.display())
            }
            StoreError::EdgeRebuildFailed(report) => {
                write!(formatter, "edge rebuild failed: {report:?}")
            }
        }
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

impl From<solstone_core_indexer::paths::JournalPathError> for StoreError {
    fn from(error: solstone_core_indexer::paths::JournalPathError) -> Self {
        StoreError::Path(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Sql(error)
    }
}
