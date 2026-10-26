// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Embedded and desktop entry for the maintained journal importer.
//!
//! [`preview`] and [`start`] are the embedded API. They do not probe the
//! supervisor, look up a sibling executable, or call `require_solstone`.
//! The desktop binary probes, builds the sibling PDF worker and Generate
//! client, and calls [`execute_desktop`] on the same grammar.
//!
//! [`start`] runs the attempt on a fresh thread named `solstone-import`.
//! That thread has no Tokio runtime. Generic audio builds its current-thread
//! runtime there and refuses when a runtime is already present. The caller
//! of [`start`] may already be on a runtime or UI thread.
//! [`ImportHandle::join`] blocks until native PDF, FFmpeg, and model calls
//! have returned and the terminal facts are settled. A UI thread must not
//! call [`ImportHandle::join`] or drop the handle: drop joins too.
//! [`ImportHandle::cancel`] records stopping intent and returns without
//! waiting. Cancellation is not reported while a native call is still inside
//! its owner.
//!
//! Apple Health import, Oura sync, and Oura connect stay on the desktop
//! importer. This entry refuses them and does not call those writers.
//!
//! In-process PDF is the optional `in-process-pdf` feature. The caller
//! passes an explicit library path: Android `nativeLibraryDir/libpdfium.so`,
//! or iOS `Frameworks/PDFium.framework/PDFium` in the host app and in
//! JournalWorker. The in-process call does not enforce the helper's 2,048
//! MiB address-space budget, 60 CPU seconds, or 90-second timeout, and the
//! extractor has no page-count, metadata-allocation, or bitmap-byte budget.
//! See `execute_in_process` in `solstone-core-pdf`.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

use solstone_core_import::cli_render::CliRun;
use solstone_core_import::{WorkStop, WorkStopGuard};

use crate::cli_argv::{preview_arguments_with, run_library};

/// Seams an embedded or desktop caller supplies for one attempt.
///
/// The bundle is the existing text, image, document-model, PDF, and archive
/// reindex boundaries. It is not a plugin host and not a command string.
pub struct ImportSeams {
    pub text: Box<dyn solstone_core_import::text::WireClient + Send>,
    pub image: Box<dyn solstone_core_import_sources::image::WireClient + Send>,
    pub model: Box<dyn solstone_core_import_sources::document::DocumentModelClient + Send>,
    pub pdf: Box<dyn solstone_core_import_sources::document::PdfWorker + Send>,
    pub reindex: Box<dyn solstone_core_import_sources::archive::FullReindexRequester + Send>,
}

/// Live attempt. [`ImportHandle::cancel`] does not wait. [`ImportHandle::join`]
/// and drop both block until the worker thread has returned.
#[must_use]
pub struct ImportHandle {
    stop: WorkStop,
    worker: Option<JoinHandle<(CliRun, Option<String>)>>,
}

impl ImportHandle {
    /// Record stopping intent and return. Native work that has already
    /// entered PDF, FFmpeg, or a model call stays owned until that call
    /// returns. No later source, member, document, page, model call,
    /// source-output install, or publication starts after the check that
    /// observes this flag.
    pub fn cancel(&self) {
        self.stop.request();
    }

    /// Block until native work has returned and the terminal facts are settled.
    ///
    /// Do not call this on a UI thread.
    #[must_use]
    pub fn join(mut self) -> (CliRun, Option<String>) {
        let worker = self.worker.take().expect("import handle joined once");
        match worker.join() {
            Ok(outcome) => outcome,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

impl Drop for ImportHandle {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Preview on the caller's thread.
///
/// A non-dry-run import is shaped into its dry-run arguments first. Preview
/// does not admit an attempt, allocate an id, or publish. It does not probe
/// the supervisor and does not build a Tokio runtime.
#[must_use]
pub fn preview(args: &[String], journal: &Path, seams: &ImportSeams) -> CliRun {
    match preview_arguments_with(args, journal, || Ok(())) {
        Ok(Some(preview_args)) => run_library(&preview_args, journal, seams, true).0,
        Ok(None) => run_library(args, journal, seams, true).0,
        Err(run) => run,
    }
}

/// Start one attempt on a fresh `solstone-import` thread and return its handle.
///
/// The new thread has no Tokio runtime. Generic audio builds one there.
/// `cancel` on the returned handle does not wait. `join` blocks the caller.
/// A UI thread must not call `join` or drop the handle.
///
/// # Errors
/// Returns the thread builder's error when the operating system refuses the
/// thread. The attempt has not started.
pub fn start(
    args: Vec<String>,
    journal: PathBuf,
    seams: ImportSeams,
    stop: WorkStop,
) -> io::Result<ImportHandle> {
    let worker_stop = stop.clone();
    let worker = thread::Builder::new()
        .name("solstone-import".to_owned())
        .spawn(move || {
            let _guard = WorkStopGuard::install(&worker_stop);
            run_library(&args, &journal, &seams, true)
        })?;
    Ok(ImportHandle {
        stop,
        worker: Some(worker),
    })
}

/// Run the grammar on the caller's thread for the desktop binary.
///
/// This is not the embedded API. The desktop adapter has already probed the
/// supervisor and built the sibling adapters. Apple Health and Oura keep
/// their existing desktop gates. The calling thread must not already have a
/// Tokio runtime when the import is generic audio: this function builds that
/// runtime on the caller, the same way the desktop importer always has.
/// It does not probe the supervisor.
#[must_use]
pub fn execute_desktop(
    args: &[String],
    journal: &Path,
    seams: &ImportSeams,
) -> (CliRun, Option<String>) {
    run_library(args, journal, seams, false)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::mpsc::{self, TryRecvError};

    use solstone_core_import_sources::document::{PdfWorker, PdfWorkerRequest, WorkerFailure};

    use super::*;

    struct IdleText;

    impl solstone_core_import::text::WireClient for IdleText {
        fn execute(
            &self,
            _request: &solstone_core_generate::GenerateRequest,
        ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError>
        {
            Ok(solstone_core_generate::GenerateResponse::Cancelled)
        }
    }

    fn idle_seams(journal: &Path) -> ImportSeams {
        ImportSeams {
            text: Box::new(IdleText),
            image: Box::new(solstone_core_import_sources::NullWireClient),
            model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
            pdf: Box::new(solstone_core_import_sources::NullPdfWorker),
            reindex: Box::new(crate::registry::IndexScan {
                journal: journal.to_path_buf(),
            }),
        }
    }

    #[test]
    fn embedded_list_does_not_probe_the_supervisor() {
        let journal = tempfile::tempdir().unwrap();
        let seams = idle_seams(journal.path());
        let run = preview(&["--list-importers".to_owned()], journal.path(), &seams);
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stdout.contains("document"));
    }

    #[test]
    fn embedded_entry_refuses_body_imports_without_calling_a_writer() {
        let journal = tempfile::tempdir().unwrap();
        let media = journal.path().join("export.zip");
        std::fs::write(&media, b"not a zip").unwrap();
        let seams = idle_seams(journal.path());
        for args in [
            vec!["--connect".to_owned(), "oura".to_owned()],
            vec!["--sync".to_owned(), "oura".to_owned()],
            vec![
                "--source".to_owned(),
                "apple_health".to_owned(),
                media.display().to_string(),
            ],
        ] {
            let (run, id) = run_library(&args, journal.path(), &seams, true);
            assert_eq!(run.exit_code, 1, "{args:?} {}", run.stderr);
            assert!(id.is_none(), "{args:?}");
            assert!(
                run.stderr.contains("desktop importer"),
                "{args:?} {}",
                run.stderr
            );
        }
    }

    struct BlockingPdf {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }

    impl PdfWorker for BlockingPdf {
        fn execute(&self, _request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
            self.entered.send(()).unwrap();
            let _ = self.release.recv();
            Err(WorkerFailure::Cancelled)
        }
    }

    use solstone_core_import_sources::document::PdfPayload;

    #[test]
    fn join_waits_until_the_native_pdf_call_returns() {
        let journal = tempfile::tempdir().unwrap();
        let pdf = journal.path().join("notes.pdf");
        std::fs::write(&pdf, b"%PDF-1.1\n").unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let stop = WorkStop::new();
        let seams = ImportSeams {
            text: Box::new(IdleText),
            image: Box::new(solstone_core_import_sources::NullWireClient),
            model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
            pdf: Box::new(BlockingPdf {
                entered: entered_tx,
                release: release_rx,
            }),
            reindex: Box::new(crate::registry::IndexScan {
                journal: journal.path().to_path_buf(),
            }),
        };
        let handle = start(
            vec![
                "--source".to_owned(),
                "document".to_owned(),
                pdf.display().to_string(),
                "--timestamp".to_owned(),
                "20260809_090000".to_owned(),
            ],
            journal.path().to_path_buf(),
            seams,
            stop.clone(),
        )
        .unwrap();
        entered_rx.recv().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            let outcome = handle.join();
            done_tx.send(outcome).unwrap();
        });
        assert_eq!(done_rx.try_recv().unwrap_err(), TryRecvError::Empty);
        stop.request();
        assert_eq!(done_rx.try_recv().unwrap_err(), TryRecvError::Empty);
        release_tx.send(()).unwrap();
        let (run, _) = done_rx.recv().unwrap();
        assert_ne!(run.exit_code, 0);
    }
}
