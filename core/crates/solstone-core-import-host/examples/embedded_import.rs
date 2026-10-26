// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! How an embedded caller previews and runs one import inside its process.
//!
//! `start` moves the attempt onto a fresh thread named `solstone-import`.
//! That thread has no Tokio runtime. Generic audio builds its current-thread
//! runtime there, and only there. The caller of `start` may already be on a
//! runtime or UI thread. `join` blocks until native PDF, FFmpeg, and model
//! calls have returned. Do not call `join`, or drop the handle, on a UI
//! thread: drop joins too. `cancel` records stopping intent and returns
//! without waiting.
//!
//! This example does not probe the supervisor and does not spawn a sibling
//! `solstone-core` or `solstone-core-pdf` process.
//!
//! In-process PDF needs the `in-process-pdf` feature and an explicit library
//! path. Android passes `nativeLibraryDir/libpdfium.so` (the packaged file
//! is also at `lib/arm64-v8a/libpdfium.so`). iOS passes
//! `Frameworks/PDFium.framework/PDFium` from the host app and from
//! JournalWorker. The in-process call does not enforce the helper's 2,048
//! MiB address-space budget, 60 CPU seconds, or 90-second timeout, and the
//! extractor has no page-count, metadata-allocation, or bitmap-byte budget.
//! A native frame that is still running after cancel stays owned until it
//! returns. This call cannot kill that frame.
//!
//! Apple Health import, Oura sync, and Oura connect stay on the desktop
//! importer. `connect_oura` is not available here.
//!
//! This example calls `cancel` immediately after `start` so the sequence is
//! visible. Delete that call to let the attempt finish.
//!
//! Usage: `embedded_import <journal> <pdfium-library> [importer arguments...]`

use std::path::PathBuf;

use solstone_core_generate::Cancellation;
use solstone_core_generate_wire::{EndpointRuntime, respond};
use solstone_core_import::WorkStop;
use solstone_core_import_host::call::{ImportSeams, preview, start};
use solstone_core_import_host::pdf_in_process::InProcessPdfWorker;
use solstone_core_import_host::registry::IndexScan;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(journal) = args.next() else {
        eprintln!("usage: embedded_import <journal> <pdfium-library> [importer arguments...]");
        std::process::exit(2);
    };
    let Some(library) = args.next() else {
        eprintln!("usage: embedded_import <journal> <pdfium-library> [importer arguments...]");
        std::process::exit(2);
    };
    let mut import_args: Vec<String> = args.collect();
    if import_args.is_empty() {
        import_args.push("--help".to_owned());
    }
    let journal = PathBuf::from(journal);
    let stop = WorkStop::new();
    let seams = embedded_seams(&journal, PathBuf::from(library), stop.clone());
    let previewed = preview(&import_args, &journal, &seams);
    print!("{}", previewed.stdout);
    eprint!("{}", previewed.stderr);
    if previewed.exit_code != 0 {
        std::process::exit(previewed.exit_code);
    }
    let handle = start(import_args, journal, seams, stop).unwrap_or_else(|error| {
        eprintln!("could not start import: {error}");
        std::process::exit(1);
    });
    handle.cancel();
    let (run, import_id) = handle.join();
    print!("{}", run.stdout);
    eprint!("{}", run.stderr);
    if let Some(import_id) = import_id {
        eprintln!("import id: {import_id}");
    }
    std::process::exit(run.exit_code);
}

fn embedded_seams(journal: &std::path::Path, library: PathBuf, stop: WorkStop) -> ImportSeams {
    let generate = EmbeddedGenerate {
        journal: journal.to_path_buf(),
        runtime: EndpointRuntime::new(1),
        stop,
    };
    ImportSeams {
        text: Box::new(generate.clone_text()),
        image: Box::new(generate.clone_image()),
        model: Box::new(generate),
        pdf: Box::new(InProcessPdfWorker::new(library)),
        reindex: Box::new(IndexScan {
            journal: journal.to_path_buf(),
        }),
    }
}

/// In-process Generate client. Model calls carry the attempt's cancellation.
struct EmbeddedGenerate {
    journal: PathBuf,
    runtime: EndpointRuntime,
    stop: WorkStop,
}

impl EmbeddedGenerate {
    fn clone_text(&self) -> Self {
        Self {
            journal: self.journal.clone(),
            runtime: EndpointRuntime::new(1),
            stop: self.stop.clone(),
        }
    }

    fn clone_image(&self) -> Self {
        self.clone_text()
    }

    fn generate(
        &self,
        request: &solstone_core_generate::GenerateRequest,
    ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError> {
        let stop = &self.stop;
        let cancelled = || stop.is_requested();
        let cancellation = Cancellation::from_fn(&cancelled);
        respond(
            request,
            &self.runtime,
            &self.journal,
            |_| None,
            &cancellation,
        )
        .map_err(solstone_core_generate::ClientError::Decode)
    }
}

impl solstone_core_import::text::WireClient for EmbeddedGenerate {
    fn execute(
        &self,
        request: &solstone_core_generate::GenerateRequest,
    ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError> {
        self.generate(request)
    }
}

impl solstone_core_import_sources::image::WireClient for EmbeddedGenerate {
    fn execute(
        &self,
        request: &solstone_core_generate::GenerateRequest,
    ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError> {
        self.generate(request)
    }
}

impl solstone_core_import_sources::document::DocumentModelClient for EmbeddedGenerate {
    fn execute(
        &self,
        request: &solstone_core_generate::GenerateRequest,
    ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError> {
        self.generate(request)
    }
}
