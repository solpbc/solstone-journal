// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native saved-folder resolution inside the existing retained importer worker.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use solstone_core_import::{
    BackendName, SyncStateRead, WorkStop, permit_runtime_refresh, read_sync_state_bound,
    write_sync_state_bound,
};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::foreground::{ImportContext, ImportSeamFactory, PreparedImport};

/// Private grant material is supplied only to the native provider, never status.
pub struct SavedFolderRequest<'a> {
    pub backend: BackendName,
    pub source_identity: &'a str,
    pub access_material: &'a str,
    pub stop: &'a WorkStop,
}

pub enum SourceCopyError {
    AccessLost,
    Unavailable,
}

/// Native connection changes contain private grant material and are never
/// serialized to the owner document. OS permission ownership stays with the
/// native caller, under its original preparation and keepalive.
pub enum FolderConnectionChange {
    Begin {
        folder_name: String,
        access_material: String,
    },
    Reauthorize {
        expected_identity: String,
        folder_name: String,
        access_material: String,
    },
    Replace {
        expected_identity: String,
        folder_name: String,
        access_material: String,
    },
    Forget {
        expected_identity: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceConnectionError {
    Busy,
    Refused,
    /// Publication was attempted. Retain native grants until the real state
    /// can be read; this outcome does not authorize releasing either grant.
    Unconfirmed,
}

/// Shares the very same source exclusion used by the callable importer.
/// Keep this guard through native grant retirement as well as state mutation.
pub struct SourceConnectionGuard {
    backend: BackendName,
    _active: OwnedMutexGuard<()>,
}

impl SourceConnectionGuard {
    /// Called only by the native owner, with the original root supplied by its
    /// retained preparation. No attempt, factory, schedule or provider runs.
    /// Unreadable state refuses a configuration write instead of erasing it.
    pub fn update(
        &mut self,
        root: &solstone_core_journal_io::JournalRoot,
        stop: &WorkStop,
        change: FolderConnectionChange,
    ) -> Result<solstone_core_import::PublicConnection, SourceConnectionError> {
        use solstone_core_import::{
            SyncState, begin_folder, public_connection, public_connection_status, reauthorize,
            replace_folder,
        };
        if stop.is_requested() || root.revalidate_canonical_binding().is_err() {
            return Err(SourceConnectionError::Refused);
        }
        let mut state = match read_sync_state_bound(root, self.backend) {
            SyncStateRead::Absent => SyncState::empty(self.backend),
            SyncStateRead::Loaded(state) => state,
            SyncStateRead::Unreadable { .. } => return Err(SourceConnectionError::Refused),
        };
        let current =
            public_connection_status(&state).map_err(|_| SourceConnectionError::Refused)?;
        let expected = match &change {
            FolderConnectionChange::Begin { .. } => None,
            FolderConnectionChange::Reauthorize {
                expected_identity, ..
            }
            | FolderConnectionChange::Replace {
                expected_identity, ..
            }
            | FolderConnectionChange::Forget { expected_identity } => Some(expected_identity),
        };
        if expected.is_some_and(|expected| {
            current.as_ref().map(|value| &value.source_identity) != Some(expected)
        }) {
            return Err(SourceConnectionError::Refused);
        }
        let updated = match change {
            FolderConnectionChange::Begin {
                folder_name,
                access_material,
            } => begin_folder(&mut state, &folder_name, true, Some(&access_material)).map(|_| ()),
            FolderConnectionChange::Reauthorize {
                folder_name,
                access_material,
                ..
            } => reauthorize(&mut state, &folder_name, true, Some(&access_material)),
            FolderConnectionChange::Replace {
                folder_name,
                access_material,
                ..
            } => replace_folder(&mut state, &folder_name, true, Some(&access_material)).map(|_| ()),
            FolderConnectionChange::Forget { .. } => {
                let folder = current.as_ref().ok_or(SourceConnectionError::Refused)?;
                reauthorize(&mut state, &folder.folder_name, false, None)
            }
        };
        updated.map_err(|_| SourceConnectionError::Refused)?;
        if stop.is_requested() {
            return Err(SourceConnectionError::Refused);
        }
        let public = public_connection(&state).ok_or(SourceConnectionError::Refused)?;
        write_sync_state_bound(root, &state, stop)
            .map_err(|_| SourceConnectionError::Unconfirmed)?;
        Ok(public)
    }
}

/// Copy synchronously on the retained worker; close all provider resources
/// before returning, including on Stop. The destination is exclusively owned.
pub trait SavedFolderProvider: Send + Sync {
    fn copy(
        &self,
        request: SavedFolderRequest<'_>,
        destination: &Path,
    ) -> Result<(), SourceCopyError>;
}

/// One native owner's source execution. Its source guard remains held through
/// provider return, joined import execution and actual temporary-copy cleanup.
pub struct ConfiguredSources {
    cache: PathBuf,
    provider: Arc<dyn SavedFolderProvider>,
    obsidian: Arc<Mutex<()>>,
    audio: Arc<Mutex<()>>,
    plaud: Arc<Mutex<()>>,
}

impl ConfiguredSources {
    pub fn new(cache: PathBuf, provider: Arc<dyn SavedFolderProvider>) -> Self {
        Self {
            cache,
            provider,
            obsidian: Arc::new(Mutex::new(())),
            audio: Arc::new(Mutex::new(())),
            plaud: Arc::new(Mutex::new(())),
        }
    }

    /// Folder configuration does not admit Plaud or the held Body connection.
    /// The guard is shared with source execution, so a busy sync refuses an
    /// owner mutation before reading or changing its catalogue.
    pub fn connection_guard(
        &self,
        backend: BackendName,
    ) -> Result<SourceConnectionGuard, SourceConnectionError> {
        let lock = match backend {
            BackendName::Obsidian => &self.obsidian,
            BackendName::Audio => &self.audio,
            BackendName::Plaud | BackendName::Oura => return Err(SourceConnectionError::Refused),
        };
        let active = Arc::clone(lock)
            .try_lock_owned()
            .map_err(|_| SourceConnectionError::Busy)?;
        Ok(SourceConnectionGuard {
            backend,
            _active: active,
        })
    }

    fn acquire(&self, source: &str) -> Result<OwnedMutexGuard<()>, String> {
        let lock = match source {
            "obsidian" => &self.obsidian,
            "audio" => &self.audio,
            "plaud" => &self.plaud,
            _ => return Err(unavailable()),
        };
        Arc::clone(lock).try_lock_owned().map_err(|_| unavailable())
    }

    fn prepare(&self, context: ImportContext, library: &Path) -> Result<PreparedImport, String> {
        let mut args = context.arguments().to_vec();
        let class = context.classification();
        let Some(source) = class.sync_source.as_deref() else {
            return super::packaged_import_factory(library.to_path_buf())(context);
        };
        // Body remains under its existing handwritten gate and never enters
        // ordinary source configuration or the folder provider.
        if source == "oura" {
            return super::packaged_import_factory(library.to_path_buf())(context);
        }
        let active = self.acquire(source)?;
        let mut temporary = None;
        if class.explicit_path.is_none() && matches!(source, "obsidian" | "audio") {
            if context.stop().is_requested() {
                return Err(unavailable());
            }
            let backend = if source == "obsidian" {
                BackendName::Obsidian
            } else {
                BackendName::Audio
            };
            let SyncStateRead::Loaded(mut state) = read_sync_state_bound(context.root(), backend)
            else {
                return Err(unavailable());
            };
            let permit = permit_runtime_refresh(&state).map_err(|_| unavailable())?;
            let material = state
                .root()
                .get("access_material")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(unavailable)?;
            if context.stop().is_requested() {
                return Err(unavailable());
            }
            let copy = SourceCopy(
                crate::owner_bridge::allocate_copy(&self.cache).map_err(|_| unavailable())?,
            );
            if context.stop().is_requested() {
                return Err(unavailable());
            }
            let copied = self.provider.copy(
                SavedFolderRequest {
                    backend,
                    source_identity: &permit.source_identity,
                    access_material: material,
                    stop: context.stop(),
                },
                &copy.0.directory(),
            );
            if context.stop().is_requested() {
                return Err(unavailable());
            }
            match copied {
                Ok(()) => (),
                Err(SourceCopyError::AccessLost) => {
                    solstone_core_import::mark_access_lost(&mut state)
                        .map_err(|_| unavailable())?;
                    write_sync_state_bound(context.root(), &state, context.stop())
                        .map_err(|_| unavailable())?;
                    return Err(unavailable());
                }
                Err(SourceCopyError::Unavailable) => return Err(unavailable()),
            }
            // Keep the original argv intact, including an end-of-options marker.
            args.splice(
                0..0,
                [
                    "--path".to_owned(),
                    copy.0
                        .directory()
                        .to_str()
                        .ok_or_else(unavailable)?
                        .to_owned(),
                ],
            );
            temporary = Some(copy);
        }
        if context.stop().is_requested() || !library.is_file() {
            return Err(unavailable());
        }
        let seams = context.seams_with_pdf(Box::new(
            solstone_core_import_host::pdf_in_process::InProcessPdfWorker::new(
                library.to_path_buf(),
            ),
        ));
        Ok(PreparedImport::new(args, seams).with_resources((temporary, active)))
    }
}

struct SourceCopy(crate::owner_bridge::OwnedCopy);
impl Drop for SourceCopy {
    fn drop(&mut self) {
        if crate::owner_bridge::release_copy(&self.0).is_err() {
            log::warn!("could not retire a temporary picked-folder copy");
        }
    }
}

fn unavailable() -> String {
    "source connection is unavailable".to_owned()
}

pub fn packaged_import_factory_with_sources(
    library: PathBuf,
    sources: Arc<ConfiguredSources>,
) -> Arc<ImportSeamFactory> {
    Arc::new(move |context| sources.prepare(context, &library))
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "import_sources_tests.rs"]
mod tests;
