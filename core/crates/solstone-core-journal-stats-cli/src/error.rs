// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use solstone_core_cli_copy as copy;
use thiserror::Error;

/// Failure that prevents a per-day statistics result from being computed.
#[derive(Debug, Error)]
pub enum JournalStatsError {
    InvalidDay(String),
    JournalPath(#[from] solstone_core_journal_io::PathError),
    JournalRead(#[from] solstone_core_journal_io::ReadError),
    JournalWrite(#[from] solstone_core_journal_io::AtomicWriteError),
    FacetStore(#[from] solstone_core_facets::FacetStoreError),
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    Validation(String),
    /// A day that failed to scan and whose inputs have not changed since; the
    /// cause is the original failure's text, repeated rather than re-derived.
    UnchangedUnreadable(String),
    Remembered {
        cause: String,
        copy: StatsCause,
    },
    SchemaVersion {
        actual: u32,
        expected: u32,
    },
    MissingGeneratedAt,
}

impl JournalStatsError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Typed facts saved beside the original English cause for older journal readers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "code", content = "arguments", rename_all = "snake_case")]
pub enum StatsCause {
    InvalidDay { day: String },
    JournalPath { detail: String },
    JournalRead { detail: String },
    JournalWrite { detail: String },
    FacetStore { detail: String },
    Io { path: String, detail: String },
    Validation { detail: String },
    SchemaVersion { actual: u32, expected: u32 },
    MissingGeneratedAt,
}

impl StatsCause {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::InvalidDay { day } => copy::display_for(
                locale,
                "journal.cli.stats.cause.invalid.day",
                [("day", day)],
            ),
            Self::JournalPath { detail } => {
                copy::display_for(locale, "journal.cli.stats.cause.path", [("detail", detail)])
            }
            Self::JournalRead { detail } => {
                copy::display_for(locale, "journal.cli.stats.cause.read", [("detail", detail)])
            }
            Self::JournalWrite { detail } => copy::display_for(
                locale,
                "journal.cli.stats.cause.write",
                [("detail", detail)],
            ),
            Self::FacetStore { detail } => copy::display_for(
                locale,
                "journal.cli.stats.cause.facet",
                [("detail", detail)],
            ),
            Self::Io { path, detail } => copy::display_for(
                locale,
                "journal.cli.stats.cause.io",
                [("path", path), ("detail", detail)],
            ),
            Self::Validation { detail } => copy::display_for(
                locale,
                "journal.cli.stats.cause.validation",
                [("detail", detail)],
            ),
            Self::SchemaVersion { actual, expected } => copy::display_for(
                locale,
                "journal.cli.stats.cause.schema",
                [("actual", actual), ("expected", expected)],
            ),
            Self::MissingGeneratedAt => {
                copy::display_for(locale, "journal.cli.stats.cause.timestamp", [])
            }
        }
    }
}

impl JournalStatsError {
    pub(crate) fn stored_cause(&self) -> Option<StatsCause> {
        Some(match self {
            Self::InvalidDay(day) => StatsCause::InvalidDay { day: day.clone() },
            Self::JournalPath(error) => StatsCause::JournalPath {
                detail: error.to_string(),
            },
            Self::JournalRead(error) => StatsCause::JournalRead {
                detail: error.to_string(),
            },
            Self::JournalWrite(error) => StatsCause::JournalWrite {
                detail: error.to_string(),
            },
            Self::FacetStore(error) => StatsCause::FacetStore {
                detail: error.to_string(),
            },
            Self::Io { path, source } => StatsCause::Io {
                path: path.display().to_string(),
                detail: source.to_string(),
            },
            Self::Validation(message) => StatsCause::Validation {
                detail: message.clone(),
            },
            Self::SchemaVersion { actual, expected } => StatsCause::SchemaVersion {
                actual: *actual,
                expected: *expected,
            },
            Self::MissingGeneratedAt => StatsCause::MissingGeneratedAt,
            Self::Remembered { copy, .. } => copy.clone(),
            Self::UnchangedUnreadable(_) => return None,
        })
    }

    pub(crate) fn render(&self, locale: &str) -> String {
        match self {
            Self::UnchangedUnreadable(cause) => cause.clone(),
            Self::Remembered { cause, .. } if locale == copy::ENGLISH => cause.clone(),
            _ => self
                .stored_cause()
                .expect("typed statistics error")
                .render(locale),
        }
    }
}

impl std::fmt::Display for JournalStatsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render(copy::ENGLISH))
    }
}

/// Use the original text when a saved cause predates this contract or comes from a newer writer.
pub fn display_saved_cause<'a>(
    source: &'a str,
    metadata: Option<&serde_json::Value>,
    locale: &str,
) -> std::borrow::Cow<'a, str> {
    if locale != copy::ENGLISH
        && let Some(value) = metadata
        && let Ok(cause) = StatsCause::deserialize(value)
    {
        return cause.render(locale).into();
    }
    source.into()
}
