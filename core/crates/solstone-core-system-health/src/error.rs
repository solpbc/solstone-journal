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

impl HealthError {
    pub fn render(&self, locale: &str) -> String {
        use solstone_core_journal_io::cli_diagnostics::{
            health_marker_detail, path_detail, read_detail, segment_identity,
        };
        match self {
            Self::Source(detail) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.health.source",
                [("detail", &(detail))],
            ),
            Self::InvalidDay(day) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.health.day.invalid",
                [("day", &(day))],
            ),
            Self::JournalIo(detail) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.health.read",
                [("detail", &(read_detail(locale, detail)))],
            ),
            Self::HealthMarker(detail) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.health.marker",
                [("detail", &(health_marker_detail(locale, detail)))],
            ),
            Self::Directory { path, message } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.health.directory",
                [("path", &(path.display())), ("detail", &(message))],
            ),
            Self::Metadata { path, message } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.health.metadata",
                [("path", &(path.display())), ("detail", &(message))],
            ),
            Self::JournalPath(detail) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.stats.cause.path",
                [("detail", &(path_detail(locale, detail)))],
            ),
            Self::UnrepresentableSegment { path } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.backfill.identity.encoding",
                [("path", &(path.display()))],
            ),
            Self::AmbiguousNamedDefault { path } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.backfill.identity.ambiguous",
                [("path", &(path.display()))],
            ),
            Self::Identity(error) => segment_identity(locale, error),
        }
    }
}
