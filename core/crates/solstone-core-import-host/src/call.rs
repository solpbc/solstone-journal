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
//! Apple Health import and Oura connect stay on the desktop importer. An
//! ordinary Oura sync does too. A stored Oura sync that already carries
//! `--confirm-body-save` or `--scheduled` keeps the existing handwritten
//! approval and is not given a new connect, card, or schedule entry.
//!
//! [`crate::cli_argv::admit_arguments`] parses the same grammar and touches
//! no journal, supervisor, or source bytes. [`classify_arguments`] returns
//! that parse: the sync backend, `--path` when present, and whether the
//! command is a held Body entry or a stored Oura sync. It does not load saved
//! Obsidian or audio settings. [`WorkStop::linked`] observes a
//! parent cancellation flag and a deadline at each check.
//! [`WorkStop::linked_with_checkpoint`] also calls the caller's retained
//! checkpoint, which already includes host stop and root binding. There is
//! no timer thread. [`ImportHandle::join`] still blocks until native work returns.
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

use crate::cli_argv::{library_preview_arguments, run_library};

pub use crate::cli_argv::{ArgumentClass, BodyAdmission, admit_arguments, classify_arguments};

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
/// A non-dry-run file import is shaped into its dry-run arguments. A sync
/// that carries `--save` runs as that source's catalog preview. A non-dry
/// audio archive runs as `--dry-run`. Connect does not start Oura
/// authorization. Preview does not admit an attempt, allocate an id, or
/// publish. It does not probe the supervisor and does not build a Tokio
/// runtime.
#[must_use]
pub fn preview(args: &[String], journal: &Path, seams: &ImportSeams) -> CliRun {
    match library_preview_arguments(args, journal) {
        Ok(preview_args) => run_library(&preview_args, journal, seams, true).0,
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
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc::{self, TryRecvError};

    use solstone_core_import_sources::document::{
        PdfPayload, PdfWorker, PdfWorkerRequest, WorkerFailure,
    };

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
        payload: Option<PdfPayload>,
    }

    impl PdfWorker for BlockingPdf {
        fn execute(&self, _request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
            self.entered.send(()).unwrap();
            let _ = self.release.recv();
            self.payload.clone().ok_or(WorkerFailure::Cancelled)
        }
    }

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
                payload: None,
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

    struct BlockingText {
        entered: mpsc::Sender<()>,
        release: std::sync::Mutex<mpsc::Receiver<solstone_core_generate::GenerateResponse>>,
    }

    impl solstone_core_import::text::WireClient for BlockingText {
        fn execute(
            &self,
            _request: &solstone_core_generate::GenerateRequest,
        ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError>
        {
            self.entered.send(()).unwrap();
            Ok(self.release.lock().unwrap().recv().unwrap())
        }
    }

    #[test]
    fn stop_during_the_text_model_does_not_install_the_transcript() {
        let journal = tempfile::tempdir().unwrap();
        let source = journal.path().join("plain.txt");
        std::fs::write(&source, "a short imported note").unwrap();
        let original = std::fs::read(&source).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let stop = WorkStop::new();
        let seams = ImportSeams {
            text: Box::new(BlockingText {
                entered: entered_tx,
                release: std::sync::Mutex::new(release_rx),
            }),
            image: Box::new(solstone_core_import_sources::NullWireClient),
            model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
            pdf: Box::new(solstone_core_import_sources::NullPdfWorker),
            reindex: Box::new(crate::registry::IndexScan {
                journal: journal.path().to_path_buf(),
            }),
        };
        let handle = start(
            vec![
                "--timestamp".to_owned(),
                "20260818_062652".to_owned(),
                source.display().to_string(),
            ],
            journal.path().to_path_buf(),
            seams,
            stop,
        )
        .unwrap();
        entered_rx.recv().unwrap();
        handle.cancel();
        release_tx
            .send(solstone_core_generate::GenerateResponse::Cancelled)
            .unwrap();
        let (run, _) = handle.join();
        assert_eq!(run.exit_code, 1, "{}", run.stderr);
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let mut transcripts = Vec::new();
        let chronicle = journal.path().join("chronicle");
        if chronicle.exists() {
            let mut pending = vec![chronicle];
            while let Some(directory) = pending.pop() {
                for entry in std::fs::read_dir(directory).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        pending.push(path);
                    } else if path.file_name().and_then(|name| name.to_str())
                        == Some("conversation_transcript.jsonl")
                    {
                        transcripts.push(path);
                    }
                }
            }
        }
        assert!(
            transcripts.is_empty(),
            "stopped text import installed {transcripts:?}"
        );
    }

    #[test]
    fn preview_of_sync_save_and_audio_archive_does_not_publish() {
        let journal = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("clip.m4a"), b"not audio").unwrap();
        let seams = idle_seams(journal.path());
        let sync = preview(
            &[
                "--sync".to_owned(),
                "audio".to_owned(),
                "--save".to_owned(),
                "--path".to_owned(),
                folder.path().display().to_string(),
            ],
            journal.path(),
            &seams,
        );
        assert_eq!(sync.exit_code, 0, "{}", sync.stderr);
        assert!(sync.stdout.contains("preview"), "{}", sync.stdout);
        assert!(!sync.stdout.contains("saved="), "{}", sync.stdout);
        assert!(!journal.path().join("chronicle").exists());

        let archive_dir = tempfile::tempdir().unwrap();
        let archive = preview(
            &[
                "--audio-archive".to_owned(),
                archive_dir.path().display().to_string(),
                "--source-zone".to_owned(),
                "UTC".to_owned(),
            ],
            journal.path(),
            &seams,
        );
        assert_eq!(archive.exit_code, 0, "{}", archive.stderr);
        assert!(
            archive.stdout.contains("audio archive preview"),
            "{}",
            archive.stdout
        );
        assert!(!journal.path().join("chronicle").exists());
        assert!(
            !journal
                .path()
                .join("imports")
                .join("20260809_090000")
                .join("imported.json")
                .exists()
        );

        let connect = preview(
            &["--connect".to_owned(), "oura".to_owned()],
            journal.path(),
            &seams,
        );
        assert_eq!(connect.exit_code, 1, "{}", connect.stderr);
        assert!(connect.stderr.contains("desktop importer"));
        assert!(!journal.path().join("config").exists());
    }

    #[test]
    fn admit_arguments_parses_without_reading_a_source() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing").join("notes.pdf");
        assert!(admit_arguments(&["--list-importers".to_owned()]).is_ok());
        assert!(admit_arguments(&["--nope".to_owned()]).is_err());
        assert!(admit_arguments(&[]).is_err());
        assert!(
            admit_arguments(&[
                "--source".to_owned(),
                "document".to_owned(),
                missing.display().to_string(),
            ])
            .is_ok()
        );
        assert!(!missing.exists());
    }

    #[test]
    fn classify_arguments_names_sync_path_and_body_without_reading_a_source() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("export.zip");
        let missing_arg = missing.display().to_string();

        let obsidian = classify_arguments(&["--sync".to_owned(), "obsidian".to_owned()]).unwrap();
        assert_eq!(obsidian.sync_source.as_deref(), Some("obsidian"));
        assert_eq!(obsidian.explicit_path, None);
        assert_eq!(obsidian.body, BodyAdmission::NotBody);

        let audio = classify_arguments(&[
            "--sync".to_owned(),
            "audio".to_owned(),
            "--path".to_owned(),
            "/clips".to_owned(),
        ])
        .unwrap();
        assert_eq!(audio.sync_source.as_deref(), Some("audio"));
        assert_eq!(audio.explicit_path.as_deref(), Some(Path::new("/clips")));
        assert_eq!(audio.body, BodyAdmission::NotBody);

        let ordinary = classify_arguments(&["--sync".to_owned(), "oura".to_owned()]).unwrap();
        assert_eq!(ordinary.sync_source.as_deref(), Some("oura"));
        assert_eq!(ordinary.body, BodyAdmission::Held);

        let saved =
            classify_arguments(&["--sync".to_owned(), "oura".to_owned(), "--save".to_owned()])
                .unwrap();
        assert_eq!(saved.body, BodyAdmission::Held);

        let confirmed = classify_arguments(&[
            "--sync".to_owned(),
            "oura".to_owned(),
            "--confirm-body-save".to_owned(),
        ])
        .unwrap();
        assert_eq!(confirmed.body, BodyAdmission::StoredOura);

        let scheduled = classify_arguments(&[
            "--sync".to_owned(),
            "oura".to_owned(),
            "--scheduled".to_owned(),
        ])
        .unwrap();
        assert_eq!(scheduled.body, BodyAdmission::StoredOura);

        let connect = classify_arguments(&["--connect".to_owned(), "oura".to_owned()]).unwrap();
        assert_eq!(connect.sync_source, None);
        assert_eq!(connect.body, BodyAdmission::Held);

        let apple = classify_arguments(&[
            "--source".to_owned(),
            "apple_health".to_owned(),
            missing_arg,
        ])
        .unwrap();
        assert_eq!(apple.sync_source, None);
        assert_eq!(apple.explicit_path, None);
        assert_eq!(apple.body, BodyAdmission::Held);
        assert!(!missing.exists());

        let document = classify_arguments(&[
            "--source".to_owned(),
            "document".to_owned(),
            "notes.pdf".to_owned(),
        ])
        .unwrap();
        assert_eq!(document.body, BodyAdmission::NotBody);
        assert!(classify_arguments(&["--nope".to_owned()]).is_err());
    }

    #[test]
    fn stored_oura_sync_keeps_the_handwritten_gate() {
        let journal = tempfile::tempdir().unwrap();
        let seams = idle_seams(journal.path());
        for args in [
            vec![
                "--sync".to_owned(),
                "oura".to_owned(),
                "--save".to_owned(),
                "--confirm-body-save".to_owned(),
            ],
            vec![
                "--sync".to_owned(),
                "oura".to_owned(),
                "--save".to_owned(),
                "--scheduled".to_owned(),
            ],
        ] {
            let (run, id) = run_library(&args, journal.path(), &seams, true);
            assert_eq!(run.exit_code, 1, "{args:?} {}", run.stderr);
            assert!(id.is_none(), "{args:?}");
            assert!(
                !run.stderr.contains("desktop importer"),
                "{args:?} {}",
                run.stderr
            );
            assert!(
                run.stderr.contains("missing_approval_artifact"),
                "{args:?} {}",
                run.stderr
            );
            assert!(!journal.path().join("imports").join("oura.json").exists());
            assert!(!journal.path().join("chronicle").exists());
        }
    }

    #[test]
    fn parent_cancel_during_pdf_retains_the_call_and_skips_install() {
        let journal = tempfile::tempdir().unwrap();
        let pdf = journal.path().join("notes.pdf");
        std::fs::write(&pdf, b"%PDF-1.1\n").unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let parent = Arc::new(AtomicBool::new(false));
        let stop = WorkStop::linked(Arc::clone(&parent), None);
        let seams = ImportSeams {
            text: Box::new(IdleText),
            image: Box::new(solstone_core_import_sources::NullWireClient),
            model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
            pdf: Box::new(BlockingPdf {
                entered: entered_tx,
                release: release_rx,
                payload: Some(PdfPayload::default()),
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
            stop,
        )
        .unwrap();
        entered_rx.recv().unwrap();
        parent.store(true, std::sync::atomic::Ordering::Release);
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            done_tx.send(handle.join()).unwrap();
        });
        assert_eq!(done_rx.try_recv().unwrap_err(), TryRecvError::Empty);
        release_tx.send(()).unwrap();
        let (run, _) = done_rx.recv().unwrap();
        assert_ne!(run.exit_code, 0, "{}", run.stdout);
        assert!(!journal.path().join("chronicle").exists());
        assert!(
            !journal
                .path()
                .join("imports")
                .join("20260809_090000")
                .join("imported.json")
                .exists()
        );
    }

    #[test]
    fn host_checkpoint_during_pdf_retains_the_call_and_skips_install() {
        let journal = tempfile::tempdir().unwrap();
        let pdf = journal.path().join("notes.pdf");
        std::fs::write(&pdf, b"%PDF-1.1\n").unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let parent = Arc::new(AtomicBool::new(false));
        let host = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&host);
        let stop = WorkStop::linked_with_checkpoint(Arc::clone(&parent), None, move || {
            seen.load(std::sync::atomic::Ordering::Acquire)
        });
        let seams = ImportSeams {
            text: Box::new(IdleText),
            image: Box::new(solstone_core_import_sources::NullWireClient),
            model: Box::new(solstone_core_import_sources::NullDocumentModelClient),
            pdf: Box::new(BlockingPdf {
                entered: entered_tx,
                release: release_rx,
                payload: Some(PdfPayload::default()),
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
            stop,
        )
        .unwrap();
        entered_rx.recv().unwrap();
        host.store(true, std::sync::atomic::Ordering::Release);
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            done_tx.send(handle.join()).unwrap();
        });
        assert_eq!(done_rx.try_recv().unwrap_err(), TryRecvError::Empty);
        release_tx.send(()).unwrap();
        let (run, _) = done_rx.recv().unwrap();
        assert_ne!(run.exit_code, 0, "{}", run.stdout);
        assert!(!parent.load(std::sync::atomic::Ordering::Acquire));
        assert!(!journal.path().join("chronicle").exists());
        assert!(
            !journal
                .path()
                .join("imports")
                .join("20260809_090000")
                .join("imported.json")
                .exists()
        );
    }
}
