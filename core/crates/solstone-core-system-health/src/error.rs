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
    #[error("cannot read health directory {path}: {message}")]
    Directory { path: PathBuf, message: String },
}
