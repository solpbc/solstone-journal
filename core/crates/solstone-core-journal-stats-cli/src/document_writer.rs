// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use crate::{JournalStatsError, StatsDocument};

/// Publishes a complete journal statistics document.
pub trait DocumentWriter {
    fn write_document(&self, path: &Path, payload: &StatsDocument)
    -> Result<(), JournalStatsError>;
}

/// Production document writer using journal-io's atomic JSON publication.
#[derive(Debug, Default, Clone, Copy)]
pub struct FilesystemDocumentWriter;

impl DocumentWriter for FilesystemDocumentWriter {
    fn write_document(
        &self,
        path: &Path,
        payload: &StatsDocument,
    ) -> Result<(), JournalStatsError> {
        solstone_core_journal_io::write_json(
            path,
            payload,
            solstone_core_journal_io::JsonWriteOptions::default(),
        )?;
        Ok(())
    }
}
