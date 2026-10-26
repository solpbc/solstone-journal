// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retained import route for one foreground host.
//!
//! `prepare` calls `import_preparation` before final arguments exist and keeps
//! that preparation's `ImportKeepalive` until this admission drops. `bind`
//! consumes `ImportPreparation::bind` and moves a clone of the same keepalive
//! onto the captured request, so the use stays alive across later metadata.
//! Preview and run call `ImportRequest::prepare` on the import-web retained
//! worker. This module does not spawn another thread, does not admit a native
//! attempt, and does not take or release a recovery lease.
//!
//! `PreparedImport` resource guards are `Vec<Box<dyn Any>>`. They are
//! intentionally not `Send`. `ImportRequest::prepare` creates them on the
//! retained worker and drops them on that worker before `preview` or `run`
//! returns. This module never stores `PreparedImport`.

#[cfg(feature = "in-process-pdf")]
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

use solstone_core_import::WorkStop;
use solstone_core_import::cli_render::CliRun;
use solstone_core_import_web::{CapturedImport, ImportAdmission, RetainedImportRoute};
use solstone_core_journal_config::work_priority::WorkPriority;

use crate::foreground::{
    ForegroundProcessingHost, ImportKeepalive, ImportPreparation, ImportRequest,
};

#[cfg(all(unix, feature = "in-process-pdf"))]
#[path = "import_sources.rs"]
mod sources;
#[cfg(all(unix, feature = "in-process-pdf"))]
pub use sources::{
    ConfiguredSources, FolderConnectionChange, SavedFolderProvider, SavedFolderRequest,
    SourceConnectionError, SourceConnectionGuard, SourceCopyError,
    packaged_import_factory_with_sources,
};

#[cfg(feature = "in-process-pdf")]
use crate::foreground::{ImportSeamFactory, PreparedImport};

/// Admitted request plus the outer keepalive.
///
/// The mutex contains only `ImportRequest`. It never contains `PreparedImport`.
struct ForegroundCaptured {
    keepalive: ImportKeepalive,
    request: Mutex<Option<ImportRequest>>,
}

impl ForegroundCaptured {
    fn take_request(&self) -> Result<ImportRequest, String> {
        let _lease = &self.keepalive;
        self.request
            .lock()
            .map_err(|_| "importer unavailable".to_owned())?
            .take()
            .ok_or_else(|| "importer unavailable".to_owned())
    }
}

impl CapturedImport for ForegroundCaptured {
    fn preview(&self) -> Result<CliRun, String> {
        let request = self.take_request()?;
        let prepared = request.prepare()?;
        let run = prepared.preview();
        drop(prepared);
        Ok(run)
    }

    fn run(&self) -> Result<CliRun, String> {
        let request = self.take_request()?;
        let prepared = request.prepare()?;
        prepared
            .run()
            .map(|(run, _)| run)
            .map_err(|error| error.to_string())
    }
}

/// Preparation taken before final arguments exist.
///
/// The keepalive is the preparation's private target use. It outlives `bind`
/// on this value, and the captured request holds another clone.
struct ForegroundAdmission {
    preparation: Mutex<Option<ImportPreparation>>,
    keepalive: ImportKeepalive,
}

impl ImportAdmission for ForegroundAdmission {
    fn checkpoint(&self) -> Result<(), String> {
        self.preparation
            .lock()
            .map_err(|_| "importer unavailable".to_owned())?
            .as_ref()
            .ok_or_else(|| "importer unavailable".to_owned())?
            .checkpoint()
    }

    fn bind(self: Arc<Self>, args: Vec<String>) -> Result<Arc<dyn CapturedImport>, String> {
        let preparation = self
            .preparation
            .lock()
            .map_err(|_| "importer unavailable".to_owned())?
            .take()
            .ok_or_else(|| "importer unavailable".to_owned())?;
        let request = preparation.bind(args)?;
        Ok(Arc::new(ForegroundCaptured {
            keepalive: self.keepalive.clone(),
            request: Mutex::new(Some(request)),
        }))
    }
}

/// Calls the host import entry. Preparation stays with this route until bind.
pub struct ForegroundImportRoute {
    host: Weak<ForegroundProcessingHost>,
}

impl ForegroundImportRoute {
    pub fn new(host: Arc<ForegroundProcessingHost>) -> Self {
        Self {
            host: Arc::downgrade(&host),
        }
    }
}

impl RetainedImportRoute for ForegroundImportRoute {
    fn prepare(
        &self,
        stop: WorkStop,
        priority: WorkPriority,
    ) -> Result<Arc<dyn ImportAdmission>, String> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| "importer unavailable".to_owned())?;
        let preparation = host.import_preparation(stop, priority)?;
        let keepalive = preparation.keepalive();
        Ok(Arc::new(ForegroundAdmission {
            preparation: Mutex::new(Some(preparation)),
            keepalive,
        }))
    }
}

/// Seam factory for one caller-supplied PDFium library.
///
/// A path that is not a file fails when the factory runs. This does not
/// search `PATH`, the environment, or `pdfium_library_path`.
#[cfg(feature = "in-process-pdf")]
pub fn packaged_import_factory(library: PathBuf) -> Arc<ImportSeamFactory> {
    Arc::new(move |context| {
        if !library.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "packaged PDF runtime unavailable",
            )
            .to_string());
        }
        let seams = context.seams_with_pdf(Box::new(
            solstone_core_import_host::pdf_in_process::InProcessPdfWorker::new(library.clone()),
        ));
        Ok(
            PreparedImport::new(context.arguments().to_vec(), seams)
                .with_resources(library.clone()),
        )
    })
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "import_route_tests.rs"]
mod tests;
