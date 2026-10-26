// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process PDF worker for an embedded caller that already holds the
//! admitted PDFium library path.
//!
//! This module is behind the `in-process-pdf` feature so the default
//! `solstone-core` graph does not link `libloading`. It does not write
//! stdout and does not call `setrlimit`. It does not enforce the helper's
//! 2,048 MiB address-space budget, 60 CPU seconds, or 90-second caller
//! timeout. The extractor still has no page-count, metadata-allocation, or
//! bitmap-byte budget. A native call that is already running stays owned
//! until it returns. This adapter cannot kill a wedged native frame.

use std::collections::BTreeSet;
use std::path::PathBuf;

use solstone_core_import_sources::document::{
    PdfCommand, PdfMetadata, PdfPage, PdfPayload, PdfRenderPayload, PdfWorkerRequest, WorkerFailure,
};
use solstone_core_pdf::{
    InProcessCommand, InProcessDocument, InProcessError, InProcessRequest, execute_in_process,
};

/// [`solstone_core_import_sources::document::PdfWorker`] over
/// [`execute_in_process`].
///
/// `library` is the caller's admitted path. Android passes
/// `nativeLibraryDir/libpdfium.so`. iOS passes
/// `Frameworks/PDFium.framework/PDFium` from the host app or JournalWorker.
/// This type does not call `pdfium_library_path`.
pub struct InProcessPdfWorker {
    library: PathBuf,
}

impl InProcessPdfWorker {
    #[must_use]
    pub fn new(library: impl Into<PathBuf>) -> Self {
        Self {
            library: library.into(),
        }
    }
}

impl solstone_core_import_sources::document::PdfWorker for InProcessPdfWorker {
    fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        if !solstone_core_import::work_continues() {
            return Err(WorkerFailure::Cancelled);
        }
        let mapped = mapped_request(request);
        let result = execute_in_process(&self.library, &mapped, &|| {
            solstone_core_import::work_continues()
        });
        if !solstone_core_import::work_continues() {
            return Err(WorkerFailure::Cancelled);
        }
        result.map(payload_from_document).map_err(worker_failure)
    }
}

pub(crate) fn mapped_request(request: &PdfWorkerRequest) -> InProcessRequest {
    let (render_pages, render_dir, dpi) = match &request.render {
        Some(render) => (
            render.pages.clone(),
            Some(render.render_dir.clone()),
            render.dpi,
        ),
        None => (BTreeSet::new(), None, 72),
    };
    InProcessRequest {
        command: match request.command {
            PdfCommand::Inspect => InProcessCommand::Inspect,
            PdfCommand::Extract => InProcessCommand::Extract,
        },
        source: request.source.clone(),
        password: request.password.clone(),
        render_pages,
        render_dir,
        dpi,
    }
}

fn payload_from_document(document: InProcessDocument) -> PdfPayload {
    PdfPayload {
        schema: document.schema,
        engine: document.engine,
        sha256: document.sha256,
        page_count: document.page_count,
        encrypted: document.encrypted,
        metadata: PdfMetadata {
            title: document.title,
            author: document.author,
            creation_date: document.creation_date,
            mod_date: document.mod_date,
            producer: document.producer,
        },
        pages: document
            .pages
            .into_iter()
            .map(|page| PdfPage {
                index: page.index,
                chars: page.chars,
                width_pt: page.width_pt,
                height_pt: page.height_pt,
                image_area_fraction: page.image_area_fraction,
                rendered: page.rendered,
                error: page.error,
                text: page.text,
            })
            .collect(),
        render: match (document.render_dpi, document.render_dir) {
            (Some(dpi), Some(dir)) => Some(PdfRenderPayload { dpi, dir }),
            _ => None,
        },
        warnings: document.warnings,
    }
}

pub(crate) fn worker_failure(error: InProcessError) -> WorkerFailure {
    match error {
        InProcessError::Cancelled => WorkerFailure::Cancelled,
        InProcessError::ResponseTooLarge => WorkerFailure::ProtocolViolation {
            detail: "response exceeds 8 MiB".to_owned(),
        },
        InProcessError::Library(error) => WorkerFailure::Process {
            exit_code: Some(1),
            error,
            detail: None,
        },
        InProcessError::Rejected { error, detail } => WorkerFailure::Process {
            exit_code: Some(exit_code_for(&error)),
            error,
            detail,
        },
    }
}

fn exit_code_for(error: &str) -> i32 {
    match error {
        "usage" => 2,
        "encrypted" => 3,
        "corrupt" => 4,
        "render-io" => 5,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use solstone_core_import_sources::document::{PdfRenderOptions, PdfWorker};

    use super::*;

    fn sample_request(command: PdfCommand, render: Option<PdfRenderOptions>) -> PdfWorkerRequest {
        PdfWorkerRequest {
            command,
            source: PathBuf::from("notes.pdf"),
            password: Some("secret".to_owned()),
            render,
            #[cfg(windows)]
            resources: solstone_core_import_sources::document::PdfWorkerResources::default(),
        }
    }

    #[test]
    fn request_mapping_copies_command_source_password_and_render() {
        let mapped = mapped_request(&sample_request(
            PdfCommand::Inspect,
            Some(PdfRenderOptions {
                pages: BTreeSet::from([1, 3]),
                render_dir: PathBuf::from("pages"),
                dpi: 144,
            }),
        ));
        assert_eq!(mapped.command, InProcessCommand::Inspect);
        assert_eq!(mapped.source, Path::new("notes.pdf"));
        assert_eq!(mapped.password.as_deref(), Some("secret"));
        assert_eq!(mapped.render_pages, BTreeSet::from([1, 3]));
        assert_eq!(mapped.render_dir.as_deref(), Some(Path::new("pages")));
        assert_eq!(mapped.dpi, 144);

        let plain = mapped_request(&sample_request(PdfCommand::Extract, None));
        assert_eq!(plain.command, InProcessCommand::Extract);
        assert!(plain.render_pages.is_empty());
        assert!(plain.render_dir.is_none());
        assert_eq!(plain.dpi, 72);
    }

    #[test]
    fn error_mapping_keeps_cancellation_distinct_from_process_failures() {
        assert!(matches!(
            worker_failure(InProcessError::Cancelled),
            WorkerFailure::Cancelled
        ));
        assert!(matches!(
            worker_failure(InProcessError::ResponseTooLarge),
            WorkerFailure::ProtocolViolation { detail } if detail == "response exceeds 8 MiB"
        ));
        for (error, exit_code) in [
            ("usage", 2),
            ("encrypted", 3),
            ("corrupt", 4),
            ("render-io", 5),
            ("internal", 1),
        ] {
            match worker_failure(InProcessError::Rejected {
                error: error.to_owned(),
                detail: Some("detail".to_owned()),
            }) {
                WorkerFailure::Process {
                    exit_code: actual,
                    error: owned,
                    detail,
                } => {
                    assert_eq!(actual, Some(exit_code), "{error}");
                    assert_eq!(owned, error);
                    assert_eq!(detail.as_deref(), Some("detail"));
                }
                other => panic!("expected a process failure, got {other:?}"),
            }
        }
        match worker_failure(InProcessError::Library("missing".to_owned())) {
            WorkerFailure::Process {
                exit_code,
                error,
                detail,
            } => {
                assert_eq!(exit_code, Some(1));
                assert_eq!(error, "missing");
                assert!(detail.is_none());
            }
            other => panic!("expected a library process failure, got {other:?}"),
        }
    }

    #[test]
    fn cancellation_before_the_native_call_is_not_a_missing_library() {
        let stop = solstone_core_import::WorkStop::new();
        stop.request();
        let _guard = solstone_core_import::WorkStopGuard::install(&stop);
        let worker = InProcessPdfWorker::new(Path::new("no-such-libpdfium"));
        let error = worker
            .execute(&sample_request(PdfCommand::Extract, None))
            .unwrap_err();
        assert!(matches!(error, WorkerFailure::Cancelled));
    }
}
