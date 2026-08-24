// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HealthError {
    #[error("health-log source error: {0}")]
    Source(String),
    #[error("invalid day {0}; expected YYYYMMDD")]
    InvalidDay(String),
    #[error("journal I/O error: {0}")]
    JournalIo(#[from] solstone_core_journal_io::ReadError),
    #[error("journal path error: {0}")]
    JournalPath(#[from] solstone_core_journal_io::PathError),
    #[error("health marker error: {0}")]
    HealthMarker(#[from] solstone_core_journal_io::HealthMarkerError),
    #[error("cannot read directory {path}: {message}")]
    Directory { path: PathBuf, message: String },
    #[error("cannot read file metadata {path}: {message}")]
    Metadata { path: PathBuf, message: String },
    #[error("segment path is not UTF-8 representable: {}", path.display())]
    UnrepresentableSegment { path: PathBuf },
    #[error(
        "named stream directory \"_default\" cannot be spelled as a record identity: {}",
        path.display()
    )]
    AmbiguousNamedDefault { path: PathBuf },
    #[error(transparent)]
    Identity(solstone_core_journal_io::SegmentIdentityError),
}
