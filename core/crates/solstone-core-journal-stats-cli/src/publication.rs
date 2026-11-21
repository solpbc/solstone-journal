// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde::Serialize;
use solstone_core_journal_io::{AtomicWriteError, JournalRoot, JsonWriteOptions};

use crate::JournalStatsError;

/// Retained at a checked leaf's entry, before its payload is serialized.
pub(crate) struct StatsPublication {
    path: PathBuf,
    parent: JournalRoot,
}

impl StatsPublication {
    pub(crate) fn open(
        path: &Path,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<Self, JournalStatsError> {
        checkpoint().map_err(JournalStatsError::Validation)?;
        let parent = path.parent().expect("statistics output has a parent");
        std::fs::create_dir_all(parent).map_err(|error| JournalStatsError::io(path, error))?;
        checkpoint().map_err(JournalStatsError::Validation)?;
        let parent = JournalRoot::open(parent)
            .map_err(|error| JournalStatsError::io(path, std::io::Error::other(error)))?;
        Ok(Self {
            path: path.to_path_buf(),
            parent,
        })
    }

    pub(crate) fn json<T: Serialize>(
        &self,
        value: &T,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), JournalStatsError> {
        let bytes =
            solstone_core_journal_io::serialize_json_bytes(value, JsonWriteOptions::default())
                .map_err(|error| AtomicWriteError::Io {
                    path: self.path.clone(),
                    source: std::io::Error::other(error),
                })?;
        self.bytes(&bytes, checkpoint)
    }

    pub(crate) fn bytes(
        &self,
        bytes: &[u8],
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), JournalStatsError> {
        let admit = || {
            checkpoint()?;
            self.parent
                .revalidate_canonical_binding()
                .map_err(|error| error.to_string())
        };
        #[cfg(unix)]
        {
            use solstone_core_journal_io::BoundAtomicOutcome;
            let outcome = solstone_core_journal_io::atomic_replace_bound_checked(
                &self.parent,
                self.path.file_name().expect("statistics output has a name"),
                bytes,
                0o600,
                &admit,
            )
            .map_err(|error| AtomicWriteError::Io {
                path: self.path.clone(),
                source: std::io::Error::new(error.source.kind(), error.to_string()),
            })?;
            match outcome {
                BoundAtomicOutcome::Published { .. } => Ok(()),
                BoundAtomicOutcome::PublishedObservationUncertain { source, .. } => {
                    Err(AtomicWriteError::Io {
                        path: self.path.clone(),
                        source: std::io::Error::new(
                            source.kind(),
                            solstone_core_cli_copy::display_for(
                                solstone_core_cli_copy::ENGLISH,
                                "journal.cli.stats.publication.observed",
                                [("source", &source)],
                            ),
                        ),
                    }
                    .into())
                }
                BoundAtomicOutcome::PublishedDurabilityUncertain { source, .. } => {
                    Err(AtomicWriteError::Io {
                        path: self.path.clone(),
                        source: std::io::Error::new(
                            source.kind(),
                            solstone_core_cli_copy::display_for(
                                solstone_core_cli_copy::ENGLISH,
                                "journal.cli.stats.publication.durable",
                                [("source", &source)],
                            ),
                        ),
                    }
                    .into())
                }
            }
        }
        #[cfg(not(unix))]
        {
            solstone_core_journal_io::atomic_replace_checked(
                &self.path,
                bytes,
                solstone_core_journal_io::AtomicWriteOptions::default(),
                &admit,
            )?;
            Ok(())
        }
    }
}
