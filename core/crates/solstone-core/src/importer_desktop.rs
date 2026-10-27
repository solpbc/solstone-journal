// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Desktop adapter for `solstone journal importer`.
//!
//! Probes the supervisor once, builds the sibling PDF worker and Generate
//! client, then calls the shared library entry. Help does not probe.

use std::io::Write;
use std::path::Path;
#[cfg(not(windows))]
use std::path::PathBuf;
use std::time::Duration;

use solstone_core_generate::OneShotClient;
use solstone_core_import::text::SystemWireClient as TextWire;
use solstone_core_import_host::call::{ImportSeams, execute_desktop};
use solstone_core_import_host::cli_argv::{
    force_day_notice_with, grammar_outcome_before_preflight, preview_arguments_with,
};
use solstone_core_import_host::registry::IndexScan;
use solstone_core_import_sources::document::{
    self, DocumentModelClient, PdfWorker, SystemDocumentModelClient, SystemPdfWorker,
};
use solstone_core_import_sources::image::SystemWireClient as ImageWire;
use solstone_core_segment::{SUPERVISOR_MESSAGE, SupervisorRefusal, require_solstone};

const PDF_WORKER_TIMEOUT: Duration = Duration::from_secs(90);

pub fn run(arguments: &[String], journal: &Path) -> (String, String, i32) {
    run_with(arguments, journal, || require_solstone(journal))
}

fn run_with(
    arguments: &[String],
    journal: &Path,
    preflight: impl FnOnce() -> Result<(), SupervisorRefusal>,
) -> (String, String, i32) {
    if let Some(outcome) = grammar_outcome_before_preflight(arguments) {
        return (outcome.stdout, outcome.stderr, outcome.exit_code);
    }
    if let Err(refusal) = preflight() {
        return refusal_output(refusal);
    }
    let notice = match force_day_notice_with(arguments, journal, || Ok(())) {
        Ok(notice) => notice,
        Err(run) => return (run.stdout, run.stderr, run.exit_code),
    };
    if let Some(notice) = notice {
        let json_mode = arguments.iter().any(|argument| argument == "--json");
        if let Err(error) = solstone_core_import::flush_force_notice(
            json_mode,
            &notice,
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        ) {
            return (
                String::new(),
                format!("could not show import day notice: {error}\n"),
                1,
            );
        }
    }
    let mut execution_arguments = arguments.to_vec();
    let preview_args = match preview_arguments_with(arguments, journal, || Ok(())) {
        Ok(args) => args,
        Err(run) => return (run.stdout, run.stderr, run.exit_code),
    };
    let seams = desktop_seams(journal);
    if let Some(preview_args) = preview_args {
        execution_arguments = preview_args[1..].to_vec();
        let (preview, _) = execute_desktop(&preview_args, journal, &seams);
        if preview.exit_code != 0 {
            return (preview.stdout, preview.stderr, preview.exit_code);
        }
        let announces =
            solstone_core_import::size::ImportSize::rendered_announces_work(&preview.stdout);
        let mut notice: Box<dyn Write> = if arguments.iter().any(|argument| argument == "--json") {
            Box::new(std::io::stderr())
        } else {
            Box::new(std::io::stdout())
        };
        if announces
            && let Err(error) = notice
                .write_all(preview.stdout.as_bytes())
                .and_then(|()| notice.flush())
        {
            return (
                String::new(),
                format!("could not show import size: {error}\n"),
                1,
            );
        }
    }
    let (run, _) = execute_desktop(&execution_arguments, journal, &seams);
    (run.stdout, run.stderr, run.exit_code)
}

fn refusal_output(refusal: SupervisorRefusal) -> (String, String, i32) {
    match refusal {
        SupervisorRefusal::SpawnedUnavailable => (String::new(), String::new(), 75),
        SupervisorRefusal::Unavailable => (String::new(), format!("{SUPERVISOR_MESSAGE}\n"), 1),
    }
}

fn desktop_seams(journal: &Path) -> ImportSeams {
    ImportSeams {
        text: Box::new(TextWire::sibling()),
        image: Box::new(ImageWire),
        model: Box::new(desktop_model()),
        pdf: Box::new(desktop_pdf()),
        reindex: Box::new(IndexScan {
            journal: journal.to_path_buf(),
        }),
    }
}

enum DesktopPdf {
    #[cfg(not(windows))]
    Ready(SystemPdfWorker),
    #[cfg(windows)]
    Ready(document::WindowsPdfWorker),
    Missing(String),
}

impl PdfWorker for DesktopPdf {
    fn prepare(&self) -> Result<(), String> {
        match self {
            Self::Ready(worker) => worker.prepare(),
            Self::Missing(error) => Err(error.clone()),
        }
    }

    fn execute(
        &self,
        request: &document::PdfWorkerRequest,
    ) -> Result<document::PdfPayload, document::WorkerFailure> {
        match self {
            Self::Ready(worker) => worker.execute(request),
            Self::Missing(error) => Err(document::WorkerFailure::Process {
                exit_code: Some(1),
                error: error.clone(),
                detail: None,
            }),
        }
    }
}

fn desktop_pdf() -> DesktopPdf {
    #[cfg(windows)]
    {
        match document::WindowsPdfWorker::from_verified_package(PDF_WORKER_TIMEOUT) {
            Ok(worker) => DesktopPdf::Ready(worker),
            Err(error) => DesktopPdf::Missing(error),
        }
    }
    #[cfg(not(windows))]
    {
        match pdf_worker_sibling() {
            Ok(path) => DesktopPdf::Ready(SystemPdfWorker::new(path, PDF_WORKER_TIMEOUT)),
            Err(error) => DesktopPdf::Missing(error),
        }
    }
}

#[cfg(not(windows))]
fn pdf_worker_sibling() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let parent = current
        .parent()
        .ok_or_else(|| "current executable has no parent".to_owned())?;
    let path = parent.join(format!("solstone-core-pdf{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("missing sibling executable {}", path.display()))
    }
}

enum DesktopModel {
    Ready(Box<SystemDocumentModelClient>),
    Missing(String),
}

impl DocumentModelClient for DesktopModel {
    fn prepare(&self) -> Result<(), String> {
        match self {
            Self::Ready(model) => model.prepare(),
            Self::Missing(error) => Err(error.clone()),
        }
    }

    fn execute(
        &self,
        request: &solstone_core_generate::GenerateRequest,
    ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError> {
        match self {
            Self::Ready(model) => model.execute(request),
            Self::Missing(error) => {
                Err(solstone_core_generate::ClientError::Resolve(error.clone()))
            }
        }
    }

    fn finish(&self) {
        if let Self::Ready(model) = self {
            model.finish();
        }
    }
}

fn desktop_model() -> DesktopModel {
    match OneShotClient::sibling() {
        Ok(client) => DesktopModel::Ready(Box::new(SystemDocumentModelClient::new(client))),
        Err(error) => DesktopModel::Missing(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_runs_before_the_supervisor_preflight() {
        let journal = Path::new(".");
        let mut probed = false;
        let unknown = run_with(&["--nonsense".to_owned()], journal, || {
            probed = true;
            Ok(())
        });
        assert!(!probed);
        assert_eq!(unknown.2, 2);
        assert!(unknown.1.contains("unrecognized arguments: --nonsense"));
        assert!(unknown.1.contains("usage: solstone journal importer"));

        let help = run(&["-h".to_owned()], journal);
        assert_eq!(help.2, 0);
        assert_eq!(help.0, solstone_core_import::cli_render::HELP);
        assert!(help.1.is_empty());

        let direct = run(&["--nonsense".to_owned()], journal);
        assert_eq!(direct.2, 2);
        assert!(direct.1.contains("unrecognized arguments: --nonsense"));
        assert!(direct.0.is_empty());

        let spawned = run_with(&["file".to_owned()], journal, || {
            Err(SupervisorRefusal::SpawnedUnavailable)
        });
        assert_eq!(spawned, (String::new(), String::new(), 75));

        let unavailable = run_with(&["file".to_owned()], journal, || {
            Err(SupervisorRefusal::Unavailable)
        });
        assert_eq!(unavailable.0, "");
        assert_eq!(unavailable.1, format!("{SUPERVISOR_MESSAGE}\n"));
        assert_eq!(unavailable.2, 1);
    }
}
