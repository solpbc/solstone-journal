// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Top-level dispatch for source bodies that depend on the import contract.

use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::ffi::OsString;

use solstone_core_generate::OneShotClient;
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::publish::NativePublicationOperations;
use solstone_core_import::{ImportResult, RegistrySource, cli_render};
use solstone_core_import_host::cli_argv::RegistryDispatch;
use solstone_core_import_sources::archive::{
    ArchiveMergeOptions, ArchiveMergeResult, FullReindexRequester, ReindexStatus, RetryDisposition,
    merge_journal_archive, plan_journal_archive,
};
use solstone_core_import_sources::{
    ImportSourcesError, chatgpt, claude, document, gemini, ics, image, kindle, obsidian,
};
#[cfg(windows)]
use solstone_core_local::install::pdfium_readiness::verified_windows_pdfium_package;
#[cfg(windows)]
use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperError, BoundedHelperRequest, BoundedHelperResourceLimits,
    run_bounded_helper,
};
use solstone_core_transfer::{RescanOutcome, send_indexer_rescan};

struct SupervisorRescan {
    journal: PathBuf,
}

impl FullReindexRequester for SupervisorRescan {
    fn request_full_reindex(&self) -> Result<bool, String> {
        match send_indexer_rescan(&self.journal) {
            RescanOutcome::Queued => Ok(true),
            RescanOutcome::Unavailable | RescanOutcome::NotNeeded => Ok(false),
        }
    }
}

const PDF_WORKER_TIMEOUT: Duration = Duration::from_secs(90);

pub fn run(dispatch: RegistryDispatch, journal: &Path) -> CliRun {
    match dispatch.source {
        RegistrySource::Ics => preview_only(dispatch, ics::preview),
        RegistrySource::Obsidian => preview_only(dispatch, obsidian::preview),
        RegistrySource::Claude => preview_only(dispatch, claude::preview),
        RegistrySource::Chatgpt => preview_only(dispatch, chatgpt::preview),
        RegistrySource::Kindle => preview_only(dispatch, kindle::preview),
        RegistrySource::Gemini => preview_only(dispatch, gemini::preview),
        RegistrySource::Document => run_document(dispatch, journal),
        RegistrySource::Image => run_image(dispatch, journal),
        RegistrySource::JournalArchive => run_archive(dispatch, journal),
        RegistrySource::AppleHealth | RegistrySource::Oura => {
            unreachable!("resolver preempts body")
        }
    }
}

fn preview_only<E>(
    dispatch: RegistryDispatch,
    preview: impl FnOnce(&Path) -> Result<solstone_core_import::ImportPreview, E>,
) -> CliRun
where
    E: std::fmt::Display,
{
    if !dispatch.dry_run {
        return failure(cli_render::source_preview_only_refusal(dispatch.source));
    }
    match preview(&dispatch.media) {
        Ok(preview) => success(cli_render::source_preview(dispatch.source, &preview)),
        Err(error) => failure(format!(
            "{} preview failed: {error}\n",
            dispatch.source.name()
        )),
    }
}

fn run_document(dispatch: RegistryDispatch, journal: &Path) -> CliRun {
    #[cfg(windows)]
    let worker = match WindowsPdfWorker::from_verified_package(PDF_WORKER_TIMEOUT) {
        Ok(worker) => worker,
        Err(error) => return failure(format!("{error}\n")),
    };
    #[cfg(not(windows))]
    let worker_path = match pdf_worker_sibling() {
        Ok(path) => path,
        Err(error) => return failure(format!("{error}\n")),
    };
    #[cfg(not(windows))]
    let worker = document::SystemPdfWorker::new(worker_path, PDF_WORKER_TIMEOUT);
    if dispatch.dry_run {
        let preview = document::preview(
            document::DocumentPreviewRequest {
                source: &dispatch.media,
                password: None,
                now: SystemTime::now(),
            },
            &worker,
        );
        return success(cli_render::source_preview(dispatch.source, &preview));
    }
    let model = match OneShotClient::sibling() {
        Ok(client) => document::SystemDocumentModelClient::new(client),
        Err(error) => return failure(format!("{}\n", error_text(error))),
    };
    if let Some(refused) = refuse_if_live_running(journal, &dispatch.timestamp, dispatch.source) {
        return refused;
    }
    let started_at_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let facts = match solstone_core_import::admit_running_attempt(
        journal,
        &dispatch.timestamp,
        started_at_ms,
        Some("document"),
    ) {
        Ok(f) => f,
        Err(err) => {
            return failure(format!("{} import failed: {err}\n", dispatch.source.name()));
        }
    };
    let publication = NativePublicationOperations;
    let outcome = match solstone_core_import_sources::run_native_producer(
        solstone_core_import_sources::NativeProducerRequest {
            journal_root: journal,
            source_path: &dispatch.media,
            import_id: &dispatch.timestamp,
            source: dispatch.source,
            revision: None,
            password: None,
            force: dispatch.force,
            expected_generation: Some(facts.generation),
        },
        &solstone_core_import_sources::NullWireClient,
        &worker,
        &model,
        &publication,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            return failure(format!(
                "{} import failed: {error}\n",
                dispatch.source.name()
            ));
        }
    };
    let proj = solstone_core_import::project_import_result(journal, &dispatch.timestamp);
    let mut errors = outcome.errors;
    if let Some(err) = proj.error
        && !errors.contains(&err)
    {
        errors.push(err);
    }
    let result = ImportResult {
        entries_written: outcome.entries_written,
        entities_seeded: 0,
        files_created: outcome
            .files_created
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        errors,
        summary: format!("imported {} PDF documents", outcome.entries_written),
        hard_failures: Vec::new(),
        segments: None,
        date_range: proj.date_range.clone(),
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: None,
    };
    render_result(dispatch.source, result)
}

/// Windows PDF owner: executes only the payload members declared in the
/// verified package, under a one-shot bounded Job.
#[cfg(windows)]
struct WindowsPdfWorker {
    package_root: PathBuf,
    executable: PathBuf,
    pdfium_library: PathBuf,
    current_directory: PathBuf,
    timeout: Duration,
}

#[cfg(windows)]
impl WindowsPdfWorker {
    fn from_verified_package(timeout: Duration) -> Result<Self, String> {
        let package = verified_windows_pdfium_package()?;
        let current_directory =
            package
                .worker
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| {
                    format!(
                        "signed PDF worker has no containing directory: {}",
                        package.worker.display()
                    )
                })?;
        Ok(Self {
            package_root: package.package_root,
            executable: package.worker,
            pdfium_library: package.library,
            current_directory,
            timeout,
        })
    }

    fn arguments(
        request: &document::PdfWorkerRequest,
    ) -> Result<Vec<String>, document::WorkerFailure> {
        let command = match request.command {
            document::PdfCommand::Inspect => "inspect",
            document::PdfCommand::Extract => "extract",
        };
        let mut arguments = vec![command.to_owned()];
        arguments.push(absolute_windows_argument(&request.source, "source PDF")?);
        if let Some(password) = &request.password {
            arguments.push("--password".to_owned());
            arguments.push(password.clone());
        }
        if let Some(render) = &request.render {
            arguments.push("--render-pages".to_owned());
            arguments.push(
                render
                    .pages
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            );
            arguments.push("--render-dir".to_owned());
            arguments.push(absolute_windows_argument(
                &render.render_dir,
                "render directory",
            )?);
            arguments.push("--dpi".to_owned());
            arguments.push(render.dpi.to_string());
        }
        Ok(arguments)
    }
}

#[cfg(windows)]
impl document::PdfWorker for WindowsPdfWorker {
    fn execute(
        &self,
        request: &document::PdfWorkerRequest,
    ) -> Result<document::PdfPayload, document::WorkerFailure> {
        let system_root = env::var_os("SystemRoot")
            .filter(|value| !value.is_empty())
            .ok_or(document::WorkerFailure::Process {
                exit_code: Some(1),
                error: "Windows worker environment is unavailable".to_owned(),
                detail: Some("SystemRoot is missing".to_owned()),
            })?;
        let mut resources = solstone_core_system::process::BoundedHelperResources::new();
        resources.retain(std::sync::Arc::new(request.resources.clone()));
        let output = run_bounded_helper(BoundedHelperRequest {
            resources,
            package_root: self.package_root.clone(),
            executable: self.executable.clone(),
            current_directory: self.current_directory.clone(),
            arguments: Self::arguments(request)?,
            environment: BTreeMap::from([
                (OsString::from("SystemRoot"), system_root),
                (
                    OsString::from("SOLSTONE_CORE_PDF_LIBRARY"),
                    self.pdfium_library.clone().into_os_string(),
                ),
            ]),
            stdin: Vec::new(),
            budget: BoundedHelperBudget {
                timeout: self.timeout,
                stdin_limit_bytes: 1,
                stdout_limit_bytes: document::PDF_WORKER_STDOUT_MAX_BYTES,
                stderr_limit_bytes: document::PDF_WORKER_STDERR_MAX_BYTES,
            },
            resource_limits: Some(BoundedHelperResourceLimits {
                cpu_rate_per_10_000: document::PDF_WORKER_CPU_RATE_PER_10_000,
                committed_memory_bytes: document::PDF_WORKER_COMMITTED_MEMORY_BYTES,
            }),
        })
        .map_err(|error| match error.cause() {
            BoundedHelperError::DeadlineExceeded { .. } if error.cleanup().is_none() => {
                document::WorkerFailure::TimedOut {
                    timeout: self.timeout,
                }
            }
            BoundedHelperError::OutputLimitExceeded { stream, .. } if error.cleanup().is_none() => {
                document::WorkerFailure::ProtocolViolation {
                    detail: format!("PDF worker {stream} exceeds its byte limit"),
                }
            }
            _ => document::WorkerFailure::Process {
                exit_code: Some(1),
                error: "bounded PDF worker failed".to_owned(),
                detail: Some(error.to_string()),
            },
        })?;
        document::parse_worker_response(
            request.command,
            output.exit_code,
            &output.stdout,
            &output.stderr,
        )
    }
}

#[cfg(windows)]
fn absolute_windows_argument(path: &Path, label: &str) -> Result<String, document::WorkerFailure> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| document::WorkerFailure::Process {
                exit_code: Some(1),
                error: "PDF worker path preparation failed".to_owned(),
                detail: Some(error.to_string()),
            })?
            .join(path)
    };
    path.into_os_string()
        .into_string()
        .map_err(|_| document::WorkerFailure::Process {
            exit_code: Some(1),
            error: "PDF worker path preparation failed".to_owned(),
            detail: Some(format!("{label} cannot cross the helper argument boundary")),
        })
}

fn run_image(dispatch: RegistryDispatch, journal: &Path) -> CliRun {
    if dispatch.dry_run {
        return success(cli_render::source_preview(
            dispatch.source,
            &image::preview(&dispatch.media),
        ));
    }
    if let Some(refused) = refuse_if_live_running(journal, &dispatch.timestamp, dispatch.source) {
        return refused;
    }
    let started_at_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let facts = match solstone_core_import::admit_running_attempt(
        journal,
        &dispatch.timestamp,
        started_at_ms,
        Some("image"),
    ) {
        Ok(f) => f,
        Err(err) => {
            return failure(format!("{} import failed: {err}\n", dispatch.source.name()));
        }
    };
    let wire = image::SystemWireClient;
    let outcome = match solstone_core_import_sources::run_native_producer(
        solstone_core_import_sources::NativeProducerRequest {
            journal_root: journal,
            source_path: &dispatch.media,
            import_id: &dispatch.timestamp,
            source: dispatch.source,
            revision: None,
            password: None,
            force: dispatch.force,
            expected_generation: Some(facts.generation),
        },
        &wire,
        &solstone_core_import_sources::NullPdfWorker,
        &solstone_core_import_sources::NullDocumentModelClient,
        &NativePublicationOperations,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            return failure(format!(
                "{} import failed: {error}\n",
                dispatch.source.name()
            ));
        }
    };
    let proj = solstone_core_import::project_import_result(journal, &dispatch.timestamp);
    let mut errors = outcome.errors;
    if let Some(err) = proj.error
        && !errors.contains(&err)
    {
        errors.push(err);
    }
    let result = ImportResult {
        entries_written: outcome.entries_written,
        entities_seeded: 0,
        files_created: outcome
            .files_created
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        errors,
        summary: "imported 1 image".to_owned(),
        hard_failures: Vec::new(),
        segments: None,
        date_range: proj.date_range.clone(),
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: None,
    };
    render_result(dispatch.source, result)
}

fn run_archive(dispatch: RegistryDispatch, journal: &Path) -> CliRun {
    if dispatch.dry_run {
        return match plan_journal_archive(&dispatch.media) {
            Ok(plan) => success(cli_render::source_preview(dispatch.source, &plan.into())),
            Err(error) => failure(format!(
                "{} preview failed: {error}\n",
                dispatch.source.name()
            )),
        };
    }
    let options = ArchiveMergeOptions {
        working_root: journal.join("imports").join("archive-merge-work"),
        ..ArchiveMergeOptions::default()
    };
    match merge_journal_archive(
        &dispatch.media,
        journal,
        &options,
        Some(&SupervisorRescan {
            journal: journal.to_path_buf(),
        }),
    ) {
        Ok(outcome) => match outcome.retry_disposition {
            RetryDisposition::Applied => success(cli_render::source_archive_merge_complete(
                dispatch.source,
                outcome.merge_summary.segments_copied,
                outcome.merge_summary.imports_copied,
                outcome.merge_summary.entities_created,
                outcome.merge_summary.entities_merged,
                outcome.merge_summary.facets_created,
                outcome.merge_summary.facets_merged,
            )),
            RetryDisposition::IdempotentNoop => {
                success(cli_render::source_archive_already_present(dispatch.source))
            }
            RetryDisposition::Incomplete => failure(cli_render::source_archive_incomplete(
                dispatch.source,
                &archive_incomplete_detail(&outcome),
            )),
        },
        Err(error) => archive_failure(dispatch.source, error),
    }
}

fn archive_incomplete_detail(outcome: &ArchiveMergeResult) -> String {
    if !outcome.errors.is_empty() {
        return outcome.errors.join("; ");
    }
    if let ReindexStatus::NotAccepted { detail } = &outcome.reindex_status {
        return detail.clone();
    }
    format!(
        "segments_skipped={} segments_errored={} entities_staged={}",
        outcome.merge_summary.segments_skipped,
        outcome.merge_summary.segments_errored,
        outcome.merge_summary.entities_staged,
    )
}

/// Thin wrapper over the shared guard in `solstone-core-import`, which the generic audio and
/// text producers also use. One guard, so the two can never disagree about liveness.
fn refuse_if_live_running(
    journal: &Path,
    import_id: &str,
    source: RegistrySource,
) -> Option<CliRun> {
    solstone_core_import::refuse_if_live_running(journal, import_id, source.name())
        .map(|message| failure(format!("{message}\n")))
}

fn render_result(source: RegistrySource, result: ImportResult) -> CliRun {
    if result.entries_written > 0 && result.hard_failures.is_empty() {
        success(cli_render::source_import_complete(source, &result))
    } else {
        failure(cli_render::source_import_failure(source, &result))
    }
}

fn archive_failure(source: RegistrySource, error: ImportSourcesError) -> CliRun {
    failure(format!("{} import failed: {error}\n", source.name()))
}

fn pdf_worker_sibling() -> Result<PathBuf, String> {
    let current = env::current_exe().map_err(|error| error.to_string())?;
    let parent = current
        .parent()
        .ok_or_else(|| "current executable has no parent".to_owned())?;
    let path = parent.join("solstone-core-pdf");
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("missing sibling executable {}", path.display()))
    }
}

fn error_text(error: solstone_core_generate::ClientError) -> String {
    match error {
        solstone_core_generate::ClientError::Resolve(detail)
        | solstone_core_generate::ClientError::Decode(detail) => detail,
        solstone_core_generate::ClientError::Io {
            primary,
            cleanup: None,
        } => primary,
        solstone_core_generate::ClientError::Io {
            primary,
            cleanup: Some(cleanup),
        } => format!("{primary} (cleanup: {cleanup})"),
        protocol @ solstone_core_generate::ClientError::Protocol(_) => protocol.to_string(),
        process @ (solstone_core_generate::ClientError::ProcessIo(_)
        | solstone_core_generate::ClientError::InvalidResponse(_)
        | solstone_core_generate::ClientError::UnexpectedChild(_)) => process.to_string(),
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(stderr: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr,
        exit_code: 1,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};

    use super::*;

    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn image_dispatch(image_path: &Path, timestamp: &str) -> RegistryDispatch {
        RegistryDispatch {
            source: RegistrySource::Image,
            media: image_path.to_path_buf(),
            timestamp: timestamp.to_owned(),
            dry_run: false,
            force: false,
        }
    }

    fn image_time(img: &Path) -> (String, String) {
        let modified = img.metadata().unwrap().modified().unwrap();
        let dt: chrono::DateTime<chrono::Local> = modified.into();
        (
            dt.format("%Y%m%d").to_string(),
            format!("{}_0", dt.format("%H%M%S")),
        )
    }

    #[test]
    fn image_publication_advances_stream_and_dirties_the_day_before_success() {
        let journal = tempfile::tempdir().unwrap();
        let img = journal.path().join("sample.png");
        fs::write(&img, TINY_PNG).unwrap();
        let (day, seg) = image_time(&img);
        let dispatch = image_dispatch(&img, "20260809_090000");

        let run = run(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stdout.contains("imported 1 image"));
        assert!(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("import.image")
                .join(&seg)
                .join("stream.json")
                .is_file()
        );
        assert!(matches!(
            read_health_marker(journal.path(), &day, HealthMarkerKind::Stream).unwrap(),
            // Touched when the original is installed and again by publication.
            HealthMarkerState::Versioned { marker, .. } if marker.generation == 2
        ));
    }

    #[test]
    fn image_stream_publication_failure_is_terminal_and_recorded() {
        let journal = tempfile::tempdir().unwrap();
        let img = journal.path().join("sample.png");
        fs::write(&img, TINY_PNG).unwrap();
        let (day, seg) = image_time(&img);
        fs::create_dir_all(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("import.image")
                .join(&seg)
                .join("stream.json"),
        )
        .unwrap();

        let dispatch = image_dispatch(&img, "20260809_090000");
        let run = run(dispatch, journal.path());

        assert_ne!(run.exit_code, 0);
        assert!(run.stderr.contains("image import failed:"));
        assert!(run.stderr.contains("publication failed"), "{}", run.stderr);
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(journal.path().join("imports/20260809_090000/imported.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["status"], "failure");
        assert_eq!(
            record["segments"][0]["outcome"]["status"],
            "failed_at_marker_write"
        );
    }

    #[test]
    fn image_day_marker_blocked_at_install_is_terminal_and_the_original_stays() {
        let journal = tempfile::tempdir().unwrap();
        let img = journal.path().join("sample.png");
        fs::write(&img, TINY_PNG).unwrap();
        let (day, seg) = image_time(&img);
        fs::create_dir_all(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("health/stream.updated"),
        )
        .unwrap();

        let dispatch = image_dispatch(&img, "20260809_090000");
        let run = run(dispatch, journal.path());

        // The marker is touched as soon as the original is installed, so a blocked marker
        // stops the import there: typed, terminal, and the owner's original is retained.
        // (A marker that fails later, during publication, is covered by the producer test
        // `a_day_marker_that_fails_at_publication_is_terminal_after_the_content_is_installed`.)
        assert_ne!(run.exit_code, 0);
        assert!(
            run.stderr.contains("image import failed:"),
            "{}",
            run.stderr
        );
        assert!(run.stderr.contains("remains installed"), "{}", run.stderr);
        assert!(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("import.image")
                .join(&seg)
                .join("original.png")
                .is_file()
        );
    }

    #[test]
    fn image_publication_lifecycle_success() {
        let journal = tempfile::tempdir().unwrap();
        let image_path = journal.path().join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();

        let dispatch = image_dispatch(&image_path, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        let proj = solstone_core_import::project_import_result(journal.path(), "20260809_090000");
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Success);
    }

    #[test]
    fn image_cli_refuses_a_live_running_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let image_path = journal.path().join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let facts = solstone_core_import::admit_running_attempt(
            journal.path(),
            "20260809_090000",
            now_ms,
            Some("image"),
        )
        .unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Running);

        let dispatch = image_dispatch(&image_path, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stderr.contains("image import failed:"));
        assert!(run.stderr.contains("already running"), "{}", run.stderr);

        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let after = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(after.generation, 1);
        assert_eq!(after.state, solstone_core_import::AttemptState::Running);
    }
}
