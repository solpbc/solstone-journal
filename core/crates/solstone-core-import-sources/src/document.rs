// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native PDF document import source.

use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(not(windows))]
use std::thread;
#[cfg(not(windows))]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use chrono::{DateTime, Local};
use image::ImageFormat;
use serde::{Deserialize, Serialize};
use serde_json::json;
use solstone_core_depict::resize_for_vlm;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient, RefusalReason,
};
use solstone_core_import::{
    CreatedSegment, ImportPreview, ImportResult, PublicationInput, PublicationOperations,
    PublicationStatus, hash_source, publish_with_operations,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, create_directory_with_mode, install_file, write_jsonl, write_text,
};
use solstone_core_segment::StreamHints;

const PAGE_TEXT_MIN_CHARS: usize = 50;
const PAGE_IMAGE_DESCRIBE_MIN: f64 = 0.10;
const MODEL_CALLS_MAX_PER_DOCUMENT: u64 = 50;
const RENDER_DPI: i32 = 150;
// The sol-pdf/1 worker protocol emits exactly one JSON response object on stdout.
pub const PDF_WORKER_STDOUT_MAX_BYTES: usize = 8 * 1024 * 1024;
// Worker stderr is diagnostic-only, never protocol-bearing.
pub const PDF_WORKER_STDERR_MAX_BYTES: usize = 64 * 1024;
#[cfg(windows)]
pub const PDF_WORKER_CPU_RATE_PER_10_000: u32 = 2_500;
#[cfg(windows)]
pub const PDF_WORKER_COMMITTED_MEMORY_BYTES: usize = 2 * 1024 * 1024 * 1024;
const STREAM: &str = "import.document";
const TRANSCRIPT: &str = "document_transcript.md";
const ORIGINAL: &str = "original.pdf";
const FILE_MODE: u32 = 0o600;
const DIRECTORY_MODE: u32 = 0o700;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const MARKER_MODEL_EXTRACTED: &str =
    "> [model-extracted from page image — may contain errors; original: pages/page-{NNNN}.png]";
const MARKER_IMAGE_DESCRIPTION: &str =
    "> [image description — model-generated; original: pages/page-{NNNN}.png]";
const MARKER_PAGE_TEXT_UNAVAILABLE_WITH_RASTER: &str =
    "> [page text unavailable — {reason}; page image preserved at pages/page-{NNNN}.png]";
const MARKER_IMAGE_DESCRIPTION_UNAVAILABLE_WITH_RASTER: &str =
    "> [image description unavailable — {reason}; page image preserved at pages/page-{NNNN}.png]";
const MARKER_PAGE_TEXT_UNAVAILABLE_NO_RASTER: &str =
    "> [page text unavailable — {reason}; no page image could be produced]";
const MARKER_IMAGE_DESCRIPTION_UNAVAILABLE_NO_RASTER: &str =
    "> [image description unavailable — {reason}; no page image could be produced]";
const DESCRIBE_PROMPT: &str = "Describe this image in detail. Include any visible text, people, objects, setting, and notable context. Return a concise natural-language description.";

/// A parsed successful `sol-pdf/1` response.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PdfPayload {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub page_count: usize,
    #[serde(default)]
    pub encrypted: bool,
    #[serde(default)]
    pub metadata: PdfMetadata,
    #[serde(default)]
    pub pages: Vec<PdfPage>,
    #[serde(default)]
    pub render: Option<PdfRenderPayload>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PdfMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub creation_date: Option<String>,
    pub mod_date: Option<String>,
    pub producer: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PdfPage {
    pub index: usize,
    #[serde(default)]
    pub chars: usize,
    #[serde(default)]
    pub width_pt: f64,
    #[serde(default)]
    pub height_pt: f64,
    #[serde(default)]
    pub image_area_fraction: f64,
    pub rendered: Option<String>,
    pub error: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PdfRenderPayload {
    pub dpi: i32,
    pub dir: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PdfCommand {
    Inspect,
    Extract,
}

impl PdfCommand {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Extract => "extract",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PdfRenderOptions {
    pub pages: BTreeSet<usize>,
    pub render_dir: PathBuf,
    pub dpi: i32,
}

#[derive(Clone, Debug)]
pub struct PdfWorkerRequest {
    pub command: PdfCommand,
    pub source: PathBuf,
    pub password: Option<String>,
    pub render: Option<PdfRenderOptions>,
}

#[derive(Clone, Debug)]
pub enum WorkerFailure {
    Process {
        exit_code: Option<i32>,
        error: String,
        detail: Option<String>,
    },
    TimedOut {
        timeout: Duration,
    },
    ProtocolViolation {
        detail: String,
    },
}

/// PDF worker boundary. Fakes receive typed input rather than needing to parse argv.
pub trait PdfWorker {
    fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure>;
}

/// Runtime `solstone-core-pdf` worker. The owning executable resolves its path.
pub struct SystemPdfWorker {
    executable: PathBuf,
    timeout: Duration,
}

impl SystemPdfWorker {
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            executable: executable.into(),
            timeout,
        }
    }
}

impl PdfWorker for SystemPdfWorker {
    fn execute(&self, request: &PdfWorkerRequest) -> Result<PdfPayload, WorkerFailure> {
        #[cfg(windows)]
        {
            let _ = request;
            return Err(WorkerFailure::Process {
                exit_code: Some(1),
                error: "Windows PDF worker requires the bounded package owner".to_owned(),
                detail: None,
            });
        }
        #[cfg(not(windows))]
        {
            let mut command = Command::new(&self.executable);
            command
                .arg(request.command.as_str())
                .arg(&request.source)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(password) = &request.password {
                command.arg("--password").arg(password);
            }
            if let Some(render) = &request.render {
                command
                    .arg("--render-pages")
                    .arg(
                        render
                            .pages
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                    .arg("--render-dir")
                    .arg(&render.render_dir)
                    .arg("--dpi")
                    .arg(render.dpi.to_string());
            }
            let mut child = command.spawn().map_err(|error| WorkerFailure::Process {
                exit_code: Some(1),
                error: "worker spawn failed".to_owned(),
                detail: Some(error.to_string()),
            })?;
            let stdout = child.stdout.take().expect("stdout is piped");
            let stderr = child.stderr.take().expect("stderr is piped");
            let mut stdout_reader = Some(thread::spawn(move || {
                read_worker_stream(stdout, PDF_WORKER_STDOUT_MAX_BYTES)
            }));
            let mut stderr_reader = Some(thread::spawn(move || {
                read_worker_stream(stderr, PDF_WORKER_STDERR_MAX_BYTES)
            }));
            let mut stdout_bytes = None;
            let mut stderr_bytes = None;
            let started = Instant::now();
            enum WaitOutcome {
                Exited(std::process::ExitStatus),
                TimedOut,
                Failed(String),
            }
            let outcome = loop {
                if stdout_bytes.is_none()
                    && stdout_reader
                        .as_ref()
                        .is_some_and(thread::JoinHandle::is_finished)
                {
                    let reader = stdout_reader.take().expect("stdout reader is present");
                    match finish_worker_reader(reader, "stdout", PDF_WORKER_STDOUT_MAX_BYTES) {
                        Ok(bytes) => stdout_bytes = Some(bytes),
                        Err(failure) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            if let Some(reader) = stderr_reader.take() {
                                let _ = finish_worker_reader(
                                    reader,
                                    "stderr",
                                    PDF_WORKER_STDERR_MAX_BYTES,
                                );
                            }
                            return Err(failure);
                        }
                    }
                }
                if stderr_bytes.is_none()
                    && stderr_reader
                        .as_ref()
                        .is_some_and(thread::JoinHandle::is_finished)
                {
                    let reader = stderr_reader.take().expect("stderr reader is present");
                    match finish_worker_reader(reader, "stderr", PDF_WORKER_STDERR_MAX_BYTES) {
                        Ok(bytes) => stderr_bytes = Some(bytes),
                        Err(failure) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            if let Some(reader) = stdout_reader.take() {
                                let _ = finish_worker_reader(
                                    reader,
                                    "stdout",
                                    PDF_WORKER_STDOUT_MAX_BYTES,
                                );
                            }
                            return Err(failure);
                        }
                    }
                }
                match child.try_wait() {
                    Ok(Some(status)) => break WaitOutcome::Exited(status),
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break WaitOutcome::Failed(error.to_string());
                    }
                    Ok(None) if started.elapsed() >= self.timeout => {
                        let _ = child.kill();
                        break match child.wait() {
                            Ok(_) => WaitOutcome::TimedOut,
                            Err(error) => WaitOutcome::Failed(error.to_string()),
                        };
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                }
            };
            let stdout = match stdout_bytes {
                Some(bytes) => Ok(bytes),
                None => finish_worker_reader(
                    stdout_reader.take().expect("stdout reader is unresolved"),
                    "stdout",
                    PDF_WORKER_STDOUT_MAX_BYTES,
                ),
            };
            let stderr = match stderr_bytes {
                Some(bytes) => Ok(bytes),
                None => finish_worker_reader(
                    stderr_reader.take().expect("stderr reader is unresolved"),
                    "stderr",
                    PDF_WORKER_STDERR_MAX_BYTES,
                ),
            };
            let stdout = stdout?;
            let stderr = stderr?;
            let status = match outcome {
                WaitOutcome::Exited(status) => status,
                WaitOutcome::TimedOut => {
                    return Err(WorkerFailure::TimedOut {
                        timeout: self.timeout,
                    });
                }
                WaitOutcome::Failed(detail) => {
                    return Err(WorkerFailure::Process {
                        exit_code: Some(1),
                        error: "worker wait failed".to_owned(),
                        detail: Some(detail),
                    });
                }
            };
            if status.code().is_none() {
                return Err(WorkerFailure::Process {
                    exit_code: None,
                    error: "PDF worker terminated by signal".to_owned(),
                    detail: Some(worker_termination_detail(&status, &stderr)),
                });
            }
            parse_worker_response(
                request.command,
                status.code().expect("non-signal status has an exit code"),
                &stdout,
                &stderr,
            )
        }
    }
}

#[cfg(not(windows))]
enum WorkerStreamRead {
    Complete(Vec<u8>),
    LimitExceeded,
}

#[cfg(not(windows))]
fn read_worker_stream(
    mut stream: impl Read,
    maximum_bytes: usize,
) -> std::io::Result<WorkerStreamRead> {
    let mut bytes = Vec::new();
    stream
        .by_ref()
        .take((maximum_bytes as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum_bytes {
        Ok(WorkerStreamRead::LimitExceeded)
    } else {
        Ok(WorkerStreamRead::Complete(bytes))
    }
}

#[cfg(not(windows))]
fn finish_worker_reader(
    reader: thread::JoinHandle<std::io::Result<WorkerStreamRead>>,
    stream: &'static str,
    maximum_bytes: usize,
) -> Result<Vec<u8>, WorkerFailure> {
    match reader.join() {
        Err(_) => Err(WorkerFailure::Process {
            exit_code: Some(1),
            error: "worker output failed".to_owned(),
            detail: Some(format!("{stream} reader panicked")),
        }),
        Ok(Err(io_error)) => Err(WorkerFailure::Process {
            exit_code: Some(1),
            error: "worker output failed".to_owned(),
            detail: Some(io_error.to_string()),
        }),
        Ok(Ok(WorkerStreamRead::LimitExceeded)) => Err(WorkerFailure::ProtocolViolation {
            detail: format!("PDF worker {stream} exceeds {maximum_bytes}-byte limit"),
        }),
        Ok(Ok(WorkerStreamRead::Complete(bytes))) => Ok(bytes),
    }
}

#[cfg(not(windows))]
fn worker_termination_detail(status: &std::process::ExitStatus, stderr: &[u8]) -> String {
    let mut detail = "PDF worker terminated by signal".to_owned();
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(status) {
        detail.push(' ');
        detail.push_str(&signal.to_string());
    }
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = collapse_line(&stderr);
    if !stderr.is_empty() {
        detail.push_str(": ");
        detail.push_str(&stderr);
    }
    detail
}

/// Decode and validate a bounded `sol-pdf/1` response.
///
/// Process owners use this after they have established their own launch,
/// containment, and output-capture guarantees.
pub fn parse_worker_response(
    command: PdfCommand,
    exit_code: i32,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<PdfPayload, WorkerFailure> {
    if exit_code == 0 {
        let payload = serde_json::from_slice(stdout).map_err(|error| WorkerFailure::Process {
            exit_code: Some(exit_code),
            error: "worker response decode failed".to_owned(),
            detail: Some(error.to_string()),
        })?;
        validate_worker_payload(&payload, command)
            .map_err(|detail| WorkerFailure::ProtocolViolation { detail })?;
        return Ok(payload);
    }
    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<String>,
        detail: Option<String>,
    }
    let body = serde_json::from_slice::<ErrorBody>(stdout).unwrap_or(ErrorBody {
        error: None,
        detail: Some(String::from_utf8_lossy(stderr).into_owned()),
    });
    Err(WorkerFailure::Process {
        exit_code: Some(exit_code),
        error: body.error.unwrap_or_else(|| "PDF worker failed".to_owned()),
        detail: body.detail,
    })
}

fn validate_worker_payload(payload: &PdfPayload, command: PdfCommand) -> Result<(), String> {
    if payload.schema != "sol-pdf/1" {
        return Err("expected sol-pdf/1 response".to_owned());
    }
    if payload.engine.is_empty() {
        return Err("response engine is missing".to_owned());
    }
    if payload.pages.len() != payload.page_count {
        return Err(format!(
            "response page count {} does not match {} page records",
            payload.page_count,
            payload.pages.len()
        ));
    }
    if command == PdfCommand::Extract
        && payload
            .pages
            .iter()
            .any(|page| page.error.is_none() && page.text.is_none())
    {
        return Err("extract response is missing page text".to_owned());
    }
    Ok(())
}

/// Model boundary for document page calls.
pub trait DocumentModelClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError>;
}

/// Runtime model client, constructed by the caller with its explicit executable path.
pub struct SystemDocumentModelClient {
    client: OneShotClient,
}

impl SystemDocumentModelClient {
    #[must_use]
    pub fn new(client: OneShotClient) -> Self {
        Self { client }
    }
}

impl DocumentModelClient for SystemDocumentModelClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.client.execute(request)
    }
}

pub struct DocumentPreviewRequest<'a> {
    pub source: &'a Path,
    pub password: Option<&'a str>,
    pub now: SystemTime,
}

pub struct DocumentImportRequest<'a> {
    pub source: &'a Path,
    pub journal_root: &'a Path,
    pub import_dir: &'a Path,
    pub import_id: &'a str,
    pub revision: Option<&'a str>,
    pub password: Option<&'a str>,
    pub force: bool,
    pub now: SystemTime,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DocumentFailure {
    Internal { detail: String },
    Protocol { detail: String },
    CallerBug { detail: String },
    PasswordRequired,
    Corrupt { detail: String },
    RenderIo { detail: String },
    TimedOut { timeout: Duration },
}

impl fmt::Display for DocumentFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal { detail } => {
                write!(formatter, "PDF worker internal failure ({detail})")
            }
            Self::Protocol { detail } => {
                write!(formatter, "PDF worker protocol failure ({detail})")
            }
            Self::CallerBug { detail } => {
                write!(formatter, "PDF import configuration error ({detail})")
            }
            Self::PasswordRequired => formatter.write_str("encrypted PDF requires a password"),
            Self::Corrupt { detail } => write!(formatter, "corrupt or unreadable PDF ({detail})"),
            Self::RenderIo { detail } => write!(formatter, "PDF render output failed ({detail})"),
            Self::TimedOut { timeout } => write!(
                formatter,
                "PDF worker timed out after {}s",
                timeout.as_secs_f64()
            ),
        }
    }
}

/// Whether a path is a PDF file or a directory containing one.
#[must_use]
pub fn detect(path: &Path) -> bool {
    !find_pdfs(path).is_empty()
}

pub fn preview(request: DocumentPreviewRequest<'_>, worker: &dyn PdfWorker) -> ImportPreview {
    let pdfs = find_pdfs(request.source);
    if pdfs.is_empty() {
        return ImportPreview {
            date_range: (String::new(), String::new()),
            item_count: 0,
            entity_count: 0,
            summary: "No PDF documents found".to_owned(),
        };
    }
    let mut dates = Vec::new();
    let mut page_count = 0usize;
    let mut failures = Vec::new();
    for source in &pdfs {
        match worker.execute(&worker_request(
            PdfCommand::Inspect,
            source,
            request.password,
            None,
        )) {
            Ok(payload) => {
                dates.push(claim_timestamp(&payload, source, request.now).timestamp);
                page_count += payload.page_count;
            }
            Err(failure) => failures.push(owner_message(source, &failure)),
        }
    }
    let mut days = dates.into_iter().map(day_for).collect::<Vec<_>>();
    days.sort();
    let summary = if failures.is_empty() {
        format!("{} PDF documents, {page_count} total pages", pdfs.len())
    } else {
        format!(
            "{} PDF documents, {page_count} total pages; {} unreadable ({})",
            pdfs.len(),
            failures.len(),
            failures.join("; ")
        )
    };
    ImportPreview {
        date_range: match (days.first(), days.last()) {
            (Some(first), Some(last)) => (first.clone(), last.clone()),
            _ => (String::new(), String::new()),
        },
        item_count: u64::try_from(pdfs.len()).expect("PDF count fits u64"),
        entity_count: 0,
        summary,
    }
}

pub fn import(
    request: DocumentImportRequest<'_>,
    worker: &dyn PdfWorker,
    model: &dyn DocumentModelClient,
    publication: &dyn PublicationOperations,
) -> ImportResult {
    let pdfs = find_pdfs(request.source);
    if pdfs.is_empty() {
        return empty_result("No PDF documents found to import");
    }
    let mut files_created = Vec::new();
    let mut errors = Vec::new();
    let mut hard_failures = Vec::new();
    let mut created_segments = Vec::new();
    let mut manifest_entries = Vec::new();
    let mut timestamps = Vec::new();
    let mut occupied = HashSet::new();

    for (input_index, source) in pdfs.iter().enumerate() {
        let first = match worker.execute(&worker_request(
            PdfCommand::Extract,
            source,
            request.password,
            None,
        )) {
            Ok(payload) => payload,
            Err(failure) => {
                let message = owner_message(source, &failure);
                errors.push(message.clone());
                hard_failures.push(message);
                continue;
            }
        };
        let source_hash = match hash_source(source) {
            Ok(hash) => hash.into_inner(),
            Err(error) => {
                errors.push(format!(
                    "{}: document import failed ({error})",
                    display_name(source)
                ));
                continue;
            }
        };
        let timestamp = claim_timestamp(&first, source, request.now);
        let claim = claim_segment(
            request.journal_root,
            timestamp.timestamp,
            if first.sha256.is_empty() {
                &source_hash
            } else {
                &first.sha256
            },
            &mut occupied,
            request.force,
        );
        if claim.already_imported {
            errors.push(format!(
                "{}: skipped (already imported; use --force to regenerate)",
                display_name(source)
            ));
            continue;
        }
        let segment_dir = request
            .journal_root
            .join("chronicle")
            .join(&claim.day)
            .join(STREAM)
            .join(&claim.segment);
        let render_pages = render_set(&first);
        let render = if render_pages.is_empty() {
            None
        } else {
            let render_dir = match create_temporary_render_directory() {
                Ok(render_dir) => render_dir,
                Err(error) => {
                    errors.push(format!(
                        "{}: document import failed ({error})",
                        display_name(source)
                    ));
                    continue;
                }
            };
            match worker.execute(&worker_request(
                PdfCommand::Extract,
                source,
                request.password,
                Some(PdfRenderOptions {
                    pages: render_pages,
                    render_dir: render_dir.path().to_path_buf(),
                    dpi: RENDER_DPI,
                }),
            )) {
                Ok(payload) => Some((render_dir, payload)),
                Err(failure) => {
                    let message = owner_message(source, &failure);
                    errors.push(message.clone());
                    hard_failures.push(message);
                    continue;
                }
            }
        };
        let (rasters, render_errors) = if let Some((render_dir, payload)) = &render {
            rendered_pages(payload, render_dir.path())
        } else {
            Default::default()
        };
        let second = render.as_ref().map(|(_, payload)| payload);
        let warnings = merge_warnings(
            &first.warnings,
            second.map_or(&[], |payload| &payload.warnings),
        );
        let prepared = render_document(
            RenderDocumentInput {
                source,
                payload: &first,
                rasters: &rasters,
                render_errors: &render_errors,
                timestamp: &timestamp,
                claim_timestamp: claim.timestamp,
                warnings: &warnings,
            },
            model,
        );
        errors.extend(
            prepared
                .warnings
                .iter()
                .map(|warning| format!("{}: {warning}", display_name(source))),
        );
        match install_artifacts(
            source,
            &segment_dir,
            &prepared,
            request.journal_root,
            &claim.day,
            publication,
        ) {
            Ok(transcript) => files_created.push(transcript),
            Err(error) => {
                let failure = format!("{}: document import failed ({error})", display_name(source));
                if error.is_marker_failure() {
                    hard_failures.push(failure.clone());
                }
                errors.push(failure);
                continue;
            }
        }
        timestamps.push(claim.timestamp);
        created_segments.push(CreatedSegment {
            day: claim.day.clone(),
            segment: claim.segment.clone(),
            stream: STREAM.to_owned(),
            hints: StreamHints::default(),
        });
        manifest_entries.push(json!({
            "id": format!("document-{input_index}"),
            "title": source.file_stem().unwrap_or_default().to_string_lossy(),
            "date": claim.day,
            "type": "document",
            "preview": char_prefix(&prepared.transcript, 200),
            "meta": {
                "page_count": first.page_count,
                "engine": first.engine,
                "timestamp_source": timestamp.source,
                "text_layer_pages": prepared.stats.text_layer_pages,
                "model_extracted_pages": prepared.stats.model_extracted_pages,
                "unavailable_pages": prepared.stats.unavailable_pages,
                "image_described_pages": prepared.stats.image_described_pages,
                "model_calls": prepared.stats.model_calls,
                "warnings": prepared.warnings,
            },
            "segments": [{"day": created_segments.last().expect("created segment").day, "key": created_segments.last().expect("created segment").segment}],
        }));
    }

    if !manifest_entries.is_empty()
        && let Err(error) = write_document_content_manifest(request.import_dir, &manifest_entries)
    {
        errors.push(format!("document content manifest: {error}"));
    }
    if !created_segments.is_empty() {
        let paths = files_created.iter().map(PathBuf::from).collect::<Vec<_>>();
        match publish_with_operations(
            PublicationInput {
                journal: request.journal_root,
                import_dir: Some(request.import_dir),
                import_id: request.import_id,
                importer: "document",
                revision: request.revision,
                segments: &created_segments,
                files_created: &paths,
            },
            publication,
        ) {
            Ok(record) if record.status == PublicationStatus::Failure => {
                let failure =
                    "document publication failed (one or more publication operations failed)"
                        .to_owned();
                errors.push(failure.clone());
                hard_failures.push(failure);
            }
            Ok(_) => {}
            Err(error) => {
                let failure = format!("document publication failed ({error})");
                errors.push(failure.clone());
                hard_failures.push(failure);
            }
        }
    }
    let days = created_segments
        .iter()
        .map(|segment| segment.day.clone())
        .collect::<BTreeSet<_>>();
    ImportResult {
        entries_written: u64::try_from(created_segments.len()).expect("segment count fits u64"),
        entities_seeded: 0,
        files_created,
        errors,
        summary: format!(
            "Imported {} PDF documents across {} days into {} segments",
            created_segments.len(),
            days.len(),
            created_segments.len()
        ),
        hard_failures,
        segments: (!created_segments.is_empty()).then(|| {
            created_segments
                .iter()
                .map(|segment| (segment.day.clone(), segment.segment.clone()))
                .collect()
        }),
        date_range: date_range(&timestamps),
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: None,
    }
}

fn empty_result(summary: &str) -> ImportResult {
    ImportResult {
        entries_written: 0,
        entities_seeded: 0,
        files_created: Vec::new(),
        errors: Vec::new(),
        summary: summary.to_owned(),
        hard_failures: Vec::new(),
        segments: None,
        date_range: None,
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: None,
    }
}

fn worker_request(
    command: PdfCommand,
    source: &Path,
    password: Option<&str>,
    render: Option<PdfRenderOptions>,
) -> PdfWorkerRequest {
    PdfWorkerRequest {
        command,
        source: source.to_path_buf(),
        password: password.map(str::to_owned),
        render,
    }
}

fn find_pdfs(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return if is_pdf(path) {
            vec![path.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut pdfs = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry| entry.is_file() && is_pdf(entry))
        .collect::<Vec<_>>();
    pdfs.sort();
    pdfs
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

#[derive(Clone)]
struct TimestampChoice {
    timestamp: SystemTime,
    source: &'static str,
}

fn claim_timestamp(payload: &PdfPayload, source: &Path, now: SystemTime) -> TimestampChoice {
    for date in [&payload.metadata.mod_date, &payload.metadata.creation_date] {
        if let Some(timestamp) = date
            .as_deref()
            .and_then(parse_metadata_date)
            .filter(|timestamp| timestamp_in_window(*timestamp, now))
        {
            return TimestampChoice {
                timestamp,
                source: "pdf-metadata",
            };
        }
    }
    if let Ok(metadata) = fs::metadata(source)
        && let Ok(timestamp) = metadata.modified()
        && timestamp_in_window(timestamp, now)
    {
        return TimestampChoice {
            timestamp,
            source: "file-mtime",
        };
    }
    TimestampChoice {
        timestamp: now,
        source: "import-time",
    }
}

fn parse_metadata_date(value: &str) -> Option<SystemTime> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(SystemTime::from)
}

fn timestamp_in_window(timestamp: SystemTime, now: SystemTime) -> bool {
    timestamp.duration_since(UNIX_EPOCH).is_ok()
        && timestamp <= now.checked_add(Duration::from_secs(86_400)).unwrap_or(now)
}

#[derive(Clone)]
struct SegmentClaim {
    day: String,
    segment: String,
    timestamp: SystemTime,
    already_imported: bool,
}

fn claim_segment(
    journal: &Path,
    start: SystemTime,
    sha256: &str,
    occupied: &mut HashSet<(String, String)>,
    force: bool,
) -> SegmentClaim {
    let mut timestamp = start;
    loop {
        let local: DateTime<Local> = timestamp.into();
        let day = local.format("%Y%m%d").to_string();
        let segment = format!("{}_0", local.format("%H%M%S"));
        if !occupied.insert((day.clone(), segment.clone())) {
            timestamp = timestamp
                .checked_add(Duration::from_secs(1))
                .expect("timestamp increment fits");
            continue;
        }
        let candidate = journal
            .join("chronicle")
            .join(&day)
            .join(STREAM)
            .join(&segment);
        if !candidate.exists() {
            return SegmentClaim {
                day,
                segment,
                timestamp,
                already_imported: false,
            };
        }
        let matching = hash_source(&candidate.join(ORIGINAL))
            .map(|hash| hash.as_str() == sha256)
            .unwrap_or(false);
        if matching {
            return SegmentClaim {
                day,
                segment,
                timestamp,
                already_imported: !force,
            };
        }
        timestamp = timestamp
            .checked_add(Duration::from_secs(1))
            .expect("timestamp increment fits");
    }
}

fn render_set(payload: &PdfPayload) -> BTreeSet<usize> {
    payload
        .pages
        .iter()
        .filter(|page| {
            page.error.is_none()
                && (page.chars < PAGE_TEXT_MIN_CHARS
                    || page.image_area_fraction >= PAGE_IMAGE_DESCRIBE_MIN)
        })
        .map(|page| page.index)
        .collect()
}

fn rendered_pages(
    payload: &PdfPayload,
    render_dir: &Path,
) -> (
    std::collections::BTreeMap<usize, PathBuf>,
    std::collections::BTreeMap<usize, String>,
) {
    let mut rasters = std::collections::BTreeMap::new();
    let mut errors = std::collections::BTreeMap::new();
    for page in &payload.pages {
        if let Some(error) = &page.error {
            errors.insert(page.index, collapse_line(error));
            continue;
        }
        let Some(rendered) = &page.rendered else {
            continue;
        };
        let path = render_dir.join(rendered);
        if path.is_file() {
            rasters.insert(page.index, path);
        } else {
            errors.insert(
                page.index,
                format!("page {}: rendered page image missing", page.index),
            );
        }
    }
    (rasters, errors)
}

fn merge_warnings(first: &[String], second: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    first
        .iter()
        .chain(second)
        .map(|warning| collapse_line(warning))
        .filter(|warning| !warning.is_empty() && seen.insert(warning.clone()))
        .collect()
}

#[derive(Default)]
struct RenderStats {
    text_layer_pages: u64,
    model_extracted_pages: u64,
    unavailable_pages: u64,
    image_described_pages: u64,
    model_calls: u64,
}

struct PreparedDocument {
    transcript: String,
    rasters: std::collections::BTreeMap<usize, PathBuf>,
    warnings: Vec<String>,
    stats: RenderStats,
}

struct RenderDocumentInput<'a> {
    source: &'a Path,
    payload: &'a PdfPayload,
    rasters: &'a std::collections::BTreeMap<usize, PathBuf>,
    render_errors: &'a std::collections::BTreeMap<usize, String>,
    timestamp: &'a TimestampChoice,
    claim_timestamp: SystemTime,
    warnings: &'a [String],
}

fn render_document(
    input: RenderDocumentInput<'_>,
    model: &dyn DocumentModelClient,
) -> PreparedDocument {
    let mut stats = RenderStats::default();
    let mut pages = input.payload.pages.clone();
    pages.sort_by_key(|page| page.index);
    let sections = pages
        .iter()
        .map(|page| render_page(page, input.rasters, input.render_errors, &mut stats, model))
        .collect::<Vec<_>>();
    let title = input
        .source
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let header = render_header(
        &title,
        input.payload,
        &day_for(input.claim_timestamp),
        input.timestamp.source,
        &stats,
        input.warnings,
    );
    PreparedDocument {
        transcript: format!("{header}\n\n{}\n", sections.join("\n\n")),
        rasters: input.rasters.clone(),
        warnings: input.warnings.to_vec(),
        stats,
    }
}

fn render_page(
    page: &PdfPage,
    rasters: &std::collections::BTreeMap<usize, PathBuf>,
    render_errors: &std::collections::BTreeMap<usize, String>,
    stats: &mut RenderStats,
    model: &dyn DocumentModelClient,
) -> String {
    let mut section = format!("## Page {}\n\n", page.index);
    let raster = rasters.get(&page.index);
    if page.error.is_none()
        && page.chars >= PAGE_TEXT_MIN_CHARS
        && let Some(text) = page.text.as_deref()
    {
        stats.text_layer_pages += 1;
        section.push_str(text);
        if !text.ends_with('\n') {
            section.push('\n');
        }
        if page.image_area_fraction >= PAGE_IMAGE_DESCRIBE_MIN {
            section.push('\n');
            if let Some(raster) = raster {
                match generate_for_page(
                    DESCRIBE_PROMPT,
                    raster,
                    "import.document.describe",
                    stats,
                    model,
                ) {
                    Ok(text) => {
                        stats.image_described_pages += 1;
                        section.push_str(&model_block(
                            &marker(MARKER_IMAGE_DESCRIPTION, page.index, None),
                            &text,
                        ));
                    }
                    Err(reason) => section.push_str(&marker(
                        MARKER_IMAGE_DESCRIPTION_UNAVAILABLE_WITH_RASTER,
                        page.index,
                        Some(&reason),
                    )),
                }
            } else {
                section.push_str(&marker(
                    MARKER_IMAGE_DESCRIPTION_UNAVAILABLE_NO_RASTER,
                    page.index,
                    Some(&page_failure_reason(page, render_errors)),
                ));
            }
            section.push('\n');
        }
        return section.trim_end_matches('\n').to_owned();
    }
    if page.error.is_none() && page.chars >= PAGE_TEXT_MIN_CHARS {
        stats.unavailable_pages += 1;
        section.push_str(&marker(
            if raster.is_some() {
                MARKER_PAGE_TEXT_UNAVAILABLE_WITH_RASTER
            } else {
                MARKER_PAGE_TEXT_UNAVAILABLE_NO_RASTER
            },
            page.index,
            Some(&format!("page {}: text layer missing", page.index)),
        ));
        return section.trim_end_matches('\n').to_owned();
    }
    if page.error.is_none()
        && page.chars < PAGE_TEXT_MIN_CHARS
        && let Some(raster) = raster
    {
        match generate_for_page(
            reading_prompt(),
            raster,
            "import.document.vision",
            stats,
            model,
        ) {
            Ok(text) => {
                stats.model_extracted_pages += 1;
                section.push_str(&model_block(
                    &marker(MARKER_MODEL_EXTRACTED, page.index, None),
                    &text,
                ));
            }
            Err(reason) => {
                stats.unavailable_pages += 1;
                section.push_str(&marker(
                    MARKER_PAGE_TEXT_UNAVAILABLE_WITH_RASTER,
                    page.index,
                    Some(&reason),
                ));
            }
        }
        section.push('\n');
        return section.trim_end_matches('\n').to_owned();
    }
    stats.unavailable_pages += 1;
    section.push_str(&marker(
        MARKER_PAGE_TEXT_UNAVAILABLE_NO_RASTER,
        page.index,
        Some(&page_failure_reason(page, render_errors)),
    ));
    section.trim_end_matches('\n').to_owned()
}

fn generate_for_page(
    prompt: &str,
    raster: &Path,
    context: &str,
    stats: &mut RenderStats,
    model: &dyn DocumentModelClient,
) -> Result<String, String> {
    if stats.model_calls >= MODEL_CALLS_MAX_PER_DOCUMENT {
        return Err("model-call limit reached".to_owned());
    }
    stats.model_calls += 1;
    let png = fs::read(raster).map_err(|error| collapse_line(&error.to_string()))?;
    let image = image::load_from_memory(&png).map_err(|error| collapse_line(&error.to_string()))?;
    let mut resized = Cursor::new(Vec::new());
    resize_for_vlm(image)
        .write_to(&mut resized, ImageFormat::Png)
        .map_err(|error| collapse_line(&error.to_string()))?;
    let request = GenerateRequest {
        id: None,
        context: context.to_owned(),
        contents: vec![
            ContentPart::Text {
                text: prompt.to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(resized.into_inner()),
            },
        ],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 4096,
        thinking_budget: None,
        timeout_s: None,
        json_output: false,
        json_schema: None,
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    };
    match model.execute(&request) {
        Ok(GenerateResponse::Generated(response)) if !response.text.trim().is_empty() => {
            Ok(response.text.trim().to_owned())
        }
        Ok(GenerateResponse::Generated(_)) => Err("empty model response".to_owned()),
        Ok(GenerateResponse::Refused(response))
            if response.reason == RefusalReason::NoEngineConfigured =>
        {
            Err("no brain configured".to_owned())
        }
        Ok(GenerateResponse::Refused(response)) => {
            Err(format!("model refused: {}", response.reason.as_str()))
        }
        Err(error) => Err(collapse_line(&format!("{error}"))),
    }
}

fn reading_prompt() -> &'static str {
    let source =
        include_str!("../../solstone-core-describe-categories/assets/categories/reading.md");
    source
        .split_once("\n}\n")
        .map_or(source, |(_, prompt)| prompt)
}

fn marker(template: &str, index: usize, reason: Option<&str>) -> String {
    let indexed = template.replace("{NNNN}", &format!("{index:04}"));
    reason.map_or(indexed.clone(), |reason| {
        indexed.replace("{reason}", &collapse_line(reason))
    })
}

fn model_block(marker: &str, text: &str) -> String {
    std::iter::once(marker.to_owned())
        .chain(text.split('\n').map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        }))
        .collect::<Vec<_>>()
        .join("\n")
}

fn page_failure_reason(
    page: &PdfPage,
    render_errors: &std::collections::BTreeMap<usize, String>,
) -> String {
    collapse_line(
        page.error
            .as_deref()
            .or_else(|| render_errors.get(&page.index).map(String::as_str))
            .unwrap_or(&format!("page {}: page image missing", page.index)),
    )
}

fn render_header(
    title: &str,
    payload: &PdfPayload,
    date: &str,
    timestamp_source: &str,
    stats: &RenderStats,
    warnings: &[String],
) -> String {
    let mut lines = vec![
        format!("# {title}"),
        String::new(),
        "**Type:** Document".to_owned(),
        format!("**Pages:** {}", payload.page_count),
        format!("**Date:** {date} ({timestamp_source})"),
        format!(
            "**Extraction:** {} — {} text-layer, {} model-extracted, {} unavailable of {} pages; {} image-described; {} model calls",
            if payload.engine.is_empty() {
                "unknown"
            } else {
                &payload.engine
            },
            stats.text_layer_pages,
            stats.model_extracted_pages,
            stats.unavailable_pages,
            payload.page_count,
            stats.image_described_pages,
            stats.model_calls
        ),
    ];
    if !warnings.is_empty() {
        lines.push(String::new());
        lines.push("**Worker warnings:**".to_owned());
        lines.extend(warnings.iter().map(|warning| format!("- {warning}")));
    }
    lines.push(String::new());
    lines.push("---".to_owned());
    lines.join("\n")
}

#[derive(Debug)]
enum ArtifactInstallError {
    Write(String),
    StreamMarker { day: String, detail: String },
}

impl ArtifactInstallError {
    fn is_marker_failure(&self) -> bool {
        matches!(self, Self::StreamMarker { .. })
    }
}

impl fmt::Display for ArtifactInstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write(detail) => formatter.write_str(detail),
            Self::StreamMarker { day, detail } => write!(
                formatter,
                "original PDF for {day} remains installed, but could not advance its stream marker: {detail}"
            ),
        }
    }
}

fn install_artifacts(
    source: &Path,
    segment_dir: &Path,
    prepared: &PreparedDocument,
    journal_root: &Path,
    day: &str,
    publication: &dyn PublicationOperations,
) -> Result<String, ArtifactInstallError> {
    let pages_dir = segment_dir.join("pages");
    create_document_artifact_directories(segment_dir, &pages_dir)
        .map_err(ArtifactInstallError::Write)?;
    install_original_pdf(
        source,
        &segment_dir.join(ORIGINAL),
        journal_root,
        day,
        publication,
    )?;
    for (index, raster) in &prepared.rasters {
        install_page_raster(raster, &pages_dir.join(page_name(*index)))
            .map_err(ArtifactInstallError::Write)?;
    }
    let transcript = segment_dir.join(TRANSCRIPT);
    write_document_transcript(&transcript, &prepared.transcript)
        .map_err(ArtifactInstallError::Write)?;
    Ok(transcript.display().to_string())
}

fn create_document_artifact_directories(
    segment_dir: &Path,
    pages_dir: &Path,
) -> Result<(), String> {
    create_directory_with_mode(segment_dir, DIRECTORY_MODE).map_err(|error| error.to_string())?;
    create_directory_with_mode(pages_dir, DIRECTORY_MODE).map_err(|error| error.to_string())
}

fn install_original_pdf(
    source: &Path,
    destination: &Path,
    journal_root: &Path,
    day: &str,
    publication: &dyn PublicationOperations,
) -> Result<(), ArtifactInstallError> {
    let modified = fs::metadata(source)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| ArtifactInstallError::Write(error.to_string()))?;
    install_source_file_before_metadata(source, destination)
        .map_err(ArtifactInstallError::Write)?;
    publication
        .touch_stream_health_marker(journal_root, day)
        .map_err(|detail| ArtifactInstallError::StreamMarker {
            day: day.to_owned(),
            detail,
        })?;
    set_installed_modified_time(destination, modified).map_err(ArtifactInstallError::Write)
}

fn install_page_raster(source: &Path, destination: &Path) -> Result<(), String> {
    let modified = fs::metadata(source)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| error.to_string())?;
    install_source_file_before_metadata(source, destination)?;
    set_installed_modified_time(destination, modified)
}

fn install_source_file_before_metadata(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "destination has no parent".to_owned())?;
    create_directory_with_mode(parent, DIRECTORY_MODE).map_err(|error| error.to_string())?;
    let temporary = create_temporary_file(parent, destination.file_name().unwrap_or_default());
    fs::copy(source, &temporary).map_err(|error| error.to_string())?;
    install_file(
        &temporary,
        destination,
        AtomicWriteOptions {
            mode: Some(FILE_MODE),
        },
    )
    .map_err(|error| error.to_string())
}

fn set_installed_modified_time(destination: &Path, modified: SystemTime) -> Result<(), String> {
    File::open(destination)
        .and_then(|file| file.set_times(fs::FileTimes::new().set_modified(modified)))
        .map_err(|error| error.to_string())
}

fn create_temporary_file(parent: &Path, name: &std::ffi::OsStr) -> PathBuf {
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{}.{}.tmp", name.to_string_lossy(), sequence));
        if OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .is_ok()
        {
            return candidate;
        }
    }
}

struct TemporaryRenderDirectory {
    path: PathBuf,
}

impl TemporaryRenderDirectory {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryRenderDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_temporary_render_directory() -> Result<TemporaryRenderDirectory, String> {
    let parent = std::env::temp_dir();
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            "solstone-document-render-{}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => {
                #[cfg(unix)]
                fs::set_permissions(&path, fs::Permissions::from_mode(DIRECTORY_MODE))
                    .map_err(|error| error.to_string())?;
                return Ok(TemporaryRenderDirectory { path });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn write_document_transcript(path: &Path, text: &str) -> Result<(), String> {
    write_text(
        path,
        text,
        AtomicWriteOptions {
            mode: Some(FILE_MODE),
        },
    )
    .map_err(|error| error.to_string())
}

fn write_document_content_manifest(
    import_dir: &Path,
    entries: &[serde_json::Value],
) -> Result<(), String> {
    create_directory_with_mode(import_dir, DIRECTORY_MODE).map_err(|error| error.to_string())?;
    write_jsonl(
        import_dir.join("content_manifest.jsonl"),
        entries.iter().cloned(),
        AtomicWriteOptions {
            mode: Some(FILE_MODE),
        },
    )
    .map_err(|error| error.to_string())
}

fn owner_message(source: &Path, failure: &WorkerFailure) -> String {
    format!("{}: {}", display_name(source), owner_failure(failure))
}

fn owner_failure(failure: &WorkerFailure) -> DocumentFailure {
    match failure {
        WorkerFailure::TimedOut { timeout } => DocumentFailure::TimedOut { timeout: *timeout },
        WorkerFailure::ProtocolViolation { detail } => DocumentFailure::Protocol {
            detail: collapse_line(detail),
        },
        WorkerFailure::Process {
            exit_code,
            error,
            detail,
        } => {
            let detail = collapse_line(detail.as_deref().unwrap_or(error));
            match exit_code {
                Some(1) => DocumentFailure::Internal { detail },
                Some(2) => DocumentFailure::CallerBug { detail },
                Some(3) => DocumentFailure::PasswordRequired,
                Some(4) => DocumentFailure::Corrupt { detail },
                Some(5) => DocumentFailure::RenderIo { detail },
                Some(_) | None => DocumentFailure::Internal { detail },
            }
        }
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
fn collapse_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn page_name(index: usize) -> String {
    format!("page-{index:04}.png")
}
fn char_prefix(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}
fn day_for(timestamp: SystemTime) -> String {
    let local: DateTime<Local> = timestamp.into();
    local.format("%Y%m%d").to_string()
}
fn date_range(timestamps: &[SystemTime]) -> Option<(String, String)> {
    let mut days = timestamps.iter().copied().map(day_for).collect::<Vec<_>>();
    days.sort();
    days.first()
        .zip(days.last())
        .map(|(first, last)| (first.clone(), last.clone()))
}
