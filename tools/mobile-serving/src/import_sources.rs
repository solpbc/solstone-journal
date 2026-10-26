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
