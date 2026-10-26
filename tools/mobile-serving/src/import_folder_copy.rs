// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A picked provider folder is copied and saved on its original service worker.

use std::path::Path;

use solstone_core_import::{
    AdmittedFolder, FolderAdmitError, Timestamp, WorkStopGuard, admit_folder_bound,
};
use solstone_core_journal_io::JournalRoot;

use super::ImportPreparation;

#[derive(Debug)]
pub enum FolderCopyResult<T> {
    Registered {
        folder: AdmittedFolder,
        value: T,
    },
    RegistrationFailed {
        folder: AdmittedFolder,
        reason: String,
    },
}

#[derive(Debug)]
pub enum FolderCopyError {
    Refused(String),
    AdmissionUnconfirmed { reference: String, reason: String },
}

struct BridgeCopy(crate::owner_bridge::OwnedCopy);

impl Drop for BridgeCopy {
    fn drop(&mut self) {
        if crate::owner_bridge::release_copy(&self.0).is_err() {
            // A proven partial stays eligible for the maintained startup sweep.
            // An admitted original is already in imports and is never this root.
            log::warn!("could not retire a temporary picked-folder copy");
        }
    }
}

impl ImportPreparation {
    /// Copy from an OS provider, admit that copy, and register it under this
    /// preparation's original root and Stop. Call on the retained service worker.
    ///
    /// `cache` is the native app's cache directory. The copy callback receives
    /// only an exclusively created destination. It must return after all provider
    /// reads and native resources have retired, including cancellation. The
    /// registration callback uses the domain metadata writer; an error preserves
    /// already admitted custody. No importer attempt or factory runs here.
    pub fn with_folder_copy<T>(
        self,
        cache: &Path,
        reference: &str,
        source_timestamp: &Timestamp,
        copy: impl FnOnce(&Path) -> Result<(), String>,
        register: impl FnOnce(&JournalRoot, &AdmittedFolder) -> Result<T, String>,
    ) -> Result<FolderCopyResult<T>, FolderCopyError> {
        self.checkpoint().map_err(FolderCopyError::Refused)?;
        let _installed_stop = WorkStopGuard::install(&self.stop);
        let temporary = BridgeCopy(
            crate::owner_bridge::allocate_copy(cache)
                .map_err(|_| FolderCopyError::Refused(FolderAdmitError::Denied.to_string()))?,
        );
        let result = (|| {
            self.checkpoint().map_err(FolderCopyError::Refused)?;
            copy(&temporary.0.directory()).map_err(FolderCopyError::Refused)?;
            self.checkpoint().map_err(FolderCopyError::Refused)?;
            let admitted = admit_folder_bound(
                &self.root,
                &temporary.0.directory(),
                reference,
                source_timestamp,
            )
            .map_err(|error| FolderCopyError::AdmissionUnconfirmed {
                reference: reference.to_owned(),
                reason: error.to_string(),
            })?;
            // Once custody commits it stays committed, including a Stop arriving
            // before metadata registration. Complete that handoff under this use.
            Ok(match register(&self.root, &admitted) {
                Ok(value) => FolderCopyResult::Registered {
                    folder: admitted,
                    value,
                },
                Err(reason) => FolderCopyResult::RegistrationFailed {
                    folder: admitted,
                    reason,
                },
            })
        })();
        // This is real, synchronous cleanup on the original retained worker.
        // Keep the original root use until every provider read and cleanup ends.
        drop(temporary);
        drop(_installed_stop);
        drop(self);
        result
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "import_folder_copy_tests.rs"]
mod tests;
