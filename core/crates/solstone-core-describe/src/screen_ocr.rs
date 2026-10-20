// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private OCR occurrence state and deterministic folding for screen sources.
//!
//! This module deliberately has no public record or indexer integration. Raw OCR
//! text, confidence, geometry, and recurrence live only in the private sidecar.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_describe_categories::CATEGORIES_META;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims};
use unicode_normalization::UnicodeNormalization;

use crate::decode::QualifiedFrame;
use crate::selection::{CategoryOverride, Importance, resolved_importance};

pub(crate) const SIDECAR_SCHEMA: u32 = 2;
pub(crate) const MAX_HEADER_BYTES: usize = 4 * 1024;
pub(crate) const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_IMAGE_PIXELS: u64 = 40_000_000;
pub(crate) const MAX_REGIONS: usize = 3_000;
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_READY_BYTES: usize = 16 * 1024;
const MAX_ERROR_BYTES: usize = 512;
const MAX_TEXT_BYTES: usize = 4 * 1024;
const MAX_GEOMETRY_POINTS: usize = 16;
const MAX_STDERR_BYTES: usize = 8 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(120);
const PROTOCOL_VERSION: u16 = 1;
const PINNED_HELPER_RECIPE: &[u8] = include_bytes!("../../../screen-ocr-helper/recipe.toml");

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageState {
    NotRun,
    Empty,
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DecodeCoverage {
    Complete,
    Partial,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WordOverlapState {
    Absent,
    Empty,
    NotComparable,
    Comparable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OcrWordState {
    NotRun,
    Error,
    Empty,
    Present,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExtractionTextState {
    NotReached,
    Failed,
    Empty,
    Present,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExtractionOutcome {
    NotReached,
    Failed,
    Completed,
}

/// Counts only; this diagnostic never contains OCR text or source paths and has
/// no effect on requests, output rows, or processing verdicts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OverlapDiagnostic {
    pub state: WordOverlapState,
    pub ocr: OcrWordState,
    pub extraction: ExtractionTextState,
    pub denominator: u64,
    pub overlapping_words: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RawOccurrence {
    pub line_order: u32,
    pub text: String,
    pub confidence: Option<f32>,
    pub geometry: Vec<[f32; 2]>,
    pub recurrence: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct FrameCoverage {
    pub frame_id: u64,
    /// Milliseconds from the source's decoded PTS × time_base value. The
    /// segment clock is not included here.
    pub segment_offset_ms: i64,
    pub png_sha256: String,
    pub state: CoverageState,
    pub error_code: Option<String>,
    pub occurrences: Vec<RawOccurrence>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PrivateSidecar {
    pub schema: u32,
    /// Opaque hash over the admitted segment identity and exact source path.
    pub source_id: String,
    pub segment_identity: String,
    pub source_sha256: String,
    pub recipe_id: String,
    pub admission_policy_id: String,
    pub decode: DecodeCoverage,
    pub state: CoverageState,
    pub frames: Vec<FrameCoverage>,
    pub word_overlap: OverlapDiagnostic,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FoldedLine {
    pub segment_offset_ms: i64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrivateFoldedSnapshot {
    pub schema: u32,
    pub segment_identity: String,
    pub recipe_id: String,
    pub lines: Vec<FoldedLine>,
    pub word_overlap: OverlapDiagnostic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceBinding {
    pub source_id: String,
    pub segment_identity: String,
    pub source_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OcrFailure {
    Unavailable,
    Timeout,
    Protocol,
    Runtime,
}

impl OcrFailure {
    fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "helper_unavailable",
            Self::Timeout => "helper_timeout",
            Self::Protocol => "helper_protocol",
            Self::Runtime => "helper_runtime",
        }
    }
}

pub(crate) trait OcrWorker {
    fn recognize(&mut self, png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure>;
}

pub(crate) trait OcrWorkerFactory {
    fn recipe_id(&mut self) -> String;
    fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure>;
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HelperReadyIdentity {
    pub protocol_version: u16,
    pub recipe_sha256: String,
    pub helper_sha256: String,
    pub detector_sha256: String,
    pub recognizer_sha256: String,
    pub dictionary_sha256: String,
    pub runtime_sha256: String,
    pub os: String,
    pub arch: String,
    pub settings: Value,
    pub limits: Value,
}

impl HelperReadyIdentity {
    fn cache_id(&self) -> String {
        serde_json::to_vec(self)
            .map(|bytes| hex(&Sha256::digest(bytes)))
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedHelper {
    pub executable: PathBuf,
    pub asset_dir: PathBuf,
    pub runtime_root: PathBuf,
    pub runtime_library: PathBuf,
    pub expected_ready: HelperReadyIdentity,
}

/// The distribution crate supplies this boundary when it can resolve an
/// installed member. Until then, describe records an unavailable OCR gap.
pub(crate) trait HelperResolver {
    fn resolve(&self) -> Option<ResolvedHelper>;
}

#[cfg(test)]
pub(crate) struct UnavailableHelperResolver;

#[cfg(test)]
impl HelperResolver for UnavailableHelperResolver {
    fn resolve(&self) -> Option<ResolvedHelper> {
        None
    }
}

pub(crate) struct InstalledHelperResolver;

#[derive(Deserialize)]
struct ReadyRecipe {
    schema_version: u32,
    protocol_version: u16,
    settings: Value,
    limits: Value,
}

impl HelperResolver for InstalledHelperResolver {
    fn resolve(&self) -> Option<ResolvedHelper> {
        let bundle = solstone_core_installed_payload::resolve_current_screen_ocr_bundle().ok()?;
        let recipe_bytes = fs::read(bundle.recipe.path()).ok()?;
        if hex(&Sha256::digest(&recipe_bytes)) != bundle.recipe.sha256() {
            return None;
        }
        let text = std::str::from_utf8(&recipe_bytes).ok()?;
        let recipe: ReadyRecipe = toml_edit::de::from_str(text).ok()?;
        if recipe.schema_version != 1 || recipe.protocol_version != PROTOCOL_VERSION {
            return None;
        }
        let asset_dir = bundle.detector.path().parent()?.to_path_buf();
        let runtime_root = bundle.runtime_library.path().parent()?.to_path_buf();
        Some(ResolvedHelper {
            executable: bundle.helper.path().to_path_buf(),
            asset_dir,
            runtime_root,
            runtime_library: bundle.runtime_library.path().to_path_buf(),
            expected_ready: HelperReadyIdentity {
                protocol_version: recipe.protocol_version,
                recipe_sha256: bundle.recipe.sha256().to_owned(),
                helper_sha256: bundle.helper.sha256().to_owned(),
                detector_sha256: bundle.detector.sha256().to_owned(),
                recognizer_sha256: bundle.recognizer.sha256().to_owned(),
                dictionary_sha256: bundle.dictionary.sha256().to_owned(),
                runtime_sha256: bundle.runtime_library.sha256().to_owned(),
                os: std::env::consts::OS.to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
                settings: recipe.settings,
                limits: recipe.limits,
            },
        })
    }
}

pub(crate) struct ResolvedWorkerFactory<'a> {
    resolver: &'a dyn HelperResolver,
    resolved: Option<Option<ResolvedHelper>>,
}

impl<'a> ResolvedWorkerFactory<'a> {
    pub(crate) fn new(resolver: &'a dyn HelperResolver) -> Self {
        Self {
            resolver,
            resolved: None,
        }
    }

    fn helper(&mut self) -> Option<ResolvedHelper> {
        self.resolved
            .get_or_insert_with(|| self.resolver.resolve())
            .clone()
    }
}

impl OcrWorkerFactory for ResolvedWorkerFactory<'_> {
    fn recipe_id(&mut self) -> String {
        self.helper()
            .map(|helper| helper.expected_ready.cache_id())
            .unwrap_or_else(|| hex(&Sha256::digest(PINNED_HELPER_RECIPE)))
    }

    fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure> {
        let Some(helper) = self.helper() else {
            return Ok(None);
        };
        ProcessWorker::launch(helper).map(|worker| Some(Box::new(worker) as Box<dyn OcrWorker>))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WorkerHeader {
    Ready {
        version: u16,
    },
    Request {
        version: u16,
        request_id: String,
        png_sha256: String,
    },
    Response {
        version: u16,
        request_id: String,
        png_sha256: String,
    },
    Error {
        version: u16,
        request_id: String,
        png_sha256: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerResponse {
    regions: Vec<WorkerRegion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRegion {
    text: String,
    confidence: Option<f32>,
    points: Vec<WorkerPoint>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerPoint {
    x: f32,
    y: f32,
}

type WorkerFrame = Result<(WorkerHeader, Vec<u8>), OcrFailure>;

struct ProcessWorker {
    child: Option<Child>,
    requests: Option<SyncSender<(WorkerHeader, Vec<u8>)>>,
    messages: Option<Receiver<WorkerFrame>>,
    stderr_overflow: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    request_number: u64,
    failed: bool,
}

impl ProcessWorker {
    fn launch(helper: ResolvedHelper) -> Result<Self, OcrFailure> {
        if !helper.executable.is_absolute()
            || !helper.asset_dir.is_absolute()
            || !helper.runtime_root.is_absolute()
            || !helper.runtime_library.is_absolute()
            || !valid_ready_identity(&helper.expected_ready)
        {
            return Err(OcrFailure::Unavailable);
        }
        let mut child = Command::new(&helper.executable)
            .arg("--asset-dir")
            .arg(&helper.asset_dir)
            .arg("--runtime-root")
            .arg(&helper.runtime_root)
            .arg("--runtime-library")
            .arg(&helper.runtime_library)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .spawn()
            .map_err(|_| OcrFailure::Unavailable)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            terminate_child(&mut child);
            OcrFailure::Unavailable
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            terminate_child(&mut child);
            OcrFailure::Unavailable
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            terminate_child(&mut child);
            OcrFailure::Unavailable
        })?;

        let (request_tx, request_rx) = mpsc::sync_channel::<(WorkerHeader, Vec<u8>)>(1);
        let (message_tx, message_rx) = mpsc::sync_channel::<WorkerFrame>(1);
        let stderr_overflow = Arc::new(AtomicBool::new(false));
        let stdout_thread = thread::spawn(move || {
            let mut stdout = stdout;
            loop {
                let message = match read_worker_message(&mut stdout) {
                    Ok(Some(message)) => Ok(message),
                    Ok(None) => Err(OcrFailure::Protocol),
                    Err(error) => Err(error),
                };
                let finished = message.is_err();
                if message_tx.send(message).is_err() || finished {
                    break;
                }
            }
        });
        let writer_thread = thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok((header, body)) = request_rx.recv() {
                if write_worker_message(&mut stdin, &header, &body).is_err() {
                    break;
                }
            }
        });
        let overflow = Arc::clone(&stderr_overflow);
        let stderr_thread = thread::spawn(move || drain_stderr(stderr, overflow));
        let mut worker = Self {
            child: Some(child),
            requests: Some(request_tx),
            messages: Some(message_rx),
            stderr_overflow,
            threads: vec![stdout_thread, writer_thread, stderr_thread],
            request_number: 0,
            failed: false,
        };
        let ready = worker.receive(STARTUP_TIMEOUT)?;
        if !matches!(
            ready.0,
            WorkerHeader::Ready {
                version: PROTOCOL_VERSION
            }
        ) || ready.1.len() > MAX_READY_BYTES
        {
            return worker.fail(OcrFailure::Protocol);
        }
        let identity = serde_json::from_slice::<HelperReadyIdentity>(&ready.1)
            .map_err(|_| OcrFailure::Protocol)?;
        if !valid_ready_identity(&identity) || identity != helper.expected_ready {
            return worker.fail(OcrFailure::Protocol);
        }
        Ok(worker)
    }

    fn receive(&self, timeout: Duration) -> Result<(WorkerHeader, Vec<u8>), OcrFailure> {
        if self.stderr_overflow.load(Ordering::Relaxed) {
            return Err(OcrFailure::Protocol);
        }
        let receiver = self.messages.as_ref().ok_or(OcrFailure::Protocol)?;
        let message = receiver
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => OcrFailure::Timeout,
                mpsc::RecvTimeoutError::Disconnected => OcrFailure::Protocol,
            })??;
        if self.stderr_overflow.load(Ordering::Relaxed) {
            return Err(OcrFailure::Protocol);
        }
        Ok(message)
    }

    fn fail<T>(&mut self, error: OcrFailure) -> Result<T, OcrFailure> {
        self.failed = true;
        self.terminate();
        Err(error)
    }

    fn terminate(&mut self) {
        self.requests.take();
        self.messages.take();
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child);
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl OcrWorker for ProcessWorker {
    fn recognize(&mut self, png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
        if self.failed {
            return Err(OcrFailure::Protocol);
        }
        let result = self.recognize_inner(png);
        if let Err(error) = result {
            return self.fail(error);
        }
        result
    }
}

impl ProcessWorker {
    fn recognize_inner(&mut self, png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
        let (width, height) = validate_png(png).map_err(|_| OcrFailure::Protocol)?;
        let request_id = format!("frame_{}", self.request_number);
        self.request_number = self.request_number.saturating_add(1);
        let png_sha256 = png_sha256(png);
        let header = WorkerHeader::Request {
            version: PROTOCOL_VERSION,
            request_id: request_id.clone(),
            png_sha256: png_sha256.clone(),
        };
        self.requests
            .as_ref()
            .ok_or(OcrFailure::Protocol)?
            .send((header, png.to_vec()))
            .map_err(|_| OcrFailure::Protocol)?;
        let (header, body) = self.receive(INFERENCE_TIMEOUT)?;
        match &header {
            WorkerHeader::Response { .. }
                if response_binding_matches(&header, &request_id, &png_sha256) =>
            {
                decode_worker_response(&body, width, height)
            }
            WorkerHeader::Error { .. }
                if response_binding_matches(&header, &request_id, &png_sha256) =>
            {
                Err(OcrFailure::Runtime)
            }
            _ => Err(OcrFailure::Protocol),
        }
    }
}

fn response_binding_matches(header: &WorkerHeader, request_id: &str, png_sha256: &str) -> bool {
    match header {
        WorkerHeader::Response {
            version,
            request_id: response_id,
            png_sha256: response_hash,
        }
        | WorkerHeader::Error {
            version,
            request_id: response_id,
            png_sha256: response_hash,
        } => {
            *version == PROTOCOL_VERSION && response_id == request_id && response_hash == png_sha256
        }
        _ => false,
    }
}

impl Drop for ProcessWorker {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn drain_stderr(mut stderr: impl Read, overflow: Arc<AtomicBool>) {
    let mut observed = 0_usize;
    let mut buffer = [0_u8; 1024];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                observed = observed.saturating_add(count);
                if observed > MAX_STDERR_BYTES {
                    overflow.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

fn valid_ready_identity(identity: &HelperReadyIdentity) -> bool {
    identity.protocol_version == PROTOCOL_VERSION
        && [
            identity.recipe_sha256.as_str(),
            identity.helper_sha256.as_str(),
            identity.detector_sha256.as_str(),
            identity.recognizer_sha256.as_str(),
            identity.dictionary_sha256.as_str(),
            identity.runtime_sha256.as_str(),
        ]
        .into_iter()
        .all(valid_digest)
        && !identity.os.is_empty()
        && !identity.arch.is_empty()
        && identity.settings.is_object()
        && identity.limits.is_object()
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn read_worker_message(
    reader: &mut impl Read,
) -> Result<Option<(WorkerHeader, Vec<u8>)>, OcrFailure> {
    let mut header_len = [0_u8; 4];
    match reader.read(&mut header_len[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(_) => return Err(OcrFailure::Protocol),
    }
    reader
        .read_exact(&mut header_len[1..])
        .map_err(|_| OcrFailure::Protocol)?;
    let header_len = u32::from_be_bytes(header_len) as usize;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err(OcrFailure::Protocol);
    }
    let mut header_bytes = vec![0_u8; header_len];
    reader
        .read_exact(&mut header_bytes)
        .map_err(|_| OcrFailure::Protocol)?;
    let header =
        serde_json::from_slice::<WorkerHeader>(&header_bytes).map_err(|_| OcrFailure::Protocol)?;
    let mut body_len = [0_u8; 4];
    reader
        .read_exact(&mut body_len)
        .map_err(|_| OcrFailure::Protocol)?;
    let body_len = u32::from_be_bytes(body_len) as usize;
    let limit = match &header {
        WorkerHeader::Ready { .. } => MAX_READY_BYTES,
        WorkerHeader::Response { .. } => MAX_RESPONSE_BYTES,
        WorkerHeader::Error { .. } => MAX_ERROR_BYTES,
        WorkerHeader::Request { .. } => return Err(OcrFailure::Protocol),
    };
    if body_len > limit {
        return Err(OcrFailure::Protocol);
    }
    let mut body = vec![0_u8; body_len];
    reader
        .read_exact(&mut body)
        .map_err(|_| OcrFailure::Protocol)?;
    Ok(Some((header, body)))
}

fn write_worker_message(
    writer: &mut impl Write,
    header: &WorkerHeader,
    body: &[u8],
) -> io::Result<()> {
    let header_bytes = serde_json::to_vec(header).map_err(io::Error::other)?;
    if header_bytes.is_empty()
        || header_bytes.len() > MAX_HEADER_BYTES
        || body.len() > MAX_PNG_BYTES
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame limit"));
    }
    writer.write_all(&(header_bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&header_bytes)?;
    writer.write_all(&(body.len() as u32).to_be_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

fn decode_worker_response(
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<RawOccurrence>, OcrFailure> {
    let response =
        serde_json::from_slice::<WorkerResponse>(bytes).map_err(|_| OcrFailure::Protocol)?;
    if response.regions.len() > MAX_REGIONS || width == 0 || height == 0 {
        return Err(OcrFailure::Protocol);
    }
    response
        .regions
        .into_iter()
        .enumerate()
        .map(|(line_order, region)| {
            if region.text.len() > MAX_TEXT_BYTES
                || region.points.is_empty()
                || region.points.len() > MAX_GEOMETRY_POINTS
                || region
                    .confidence
                    .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
                || region.points.iter().any(|point| {
                    !point.x.is_finite()
                        || !point.y.is_finite()
                        || point.x < 0.0
                        || point.y < 0.0
                        || point.x > width as f32
                        || point.y > height as f32
                })
            {
                return Err(OcrFailure::Protocol);
            }
            Ok(RawOccurrence {
                line_order: u32::try_from(line_order).map_err(|_| OcrFailure::Protocol)?,
                text: region.text,
                confidence: region.confidence,
                geometry: region
                    .points
                    .into_iter()
                    .map(|point| [point.x, point.y])
                    .collect(),
                recurrence: 0,
            })
        })
        .collect()
}

/// The source slot includes the exact encoded path and admitted physical
/// segment identity. Its sidecar separately binds the source content digest.
pub(crate) fn source_identity(segment_identity: &str, path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"solstone-screen-ocr-source-v1\0");
    digest.update(segment_identity.as_bytes());
    digest.update([0]);
    digest.update(path.as_os_str().as_encoded_bytes());
    hex(&digest.finalize())
}

pub(crate) fn source_binding(
    claims: &OutputClaims,
    input: &InputObservation,
    source_path: &Path,
    segment: &Path,
) -> std::io::Result<SourceBinding> {
    let segment_identity = claims.segment_identity(segment)?.observation_id();
    Ok(SourceBinding {
        source_id: source_identity(&segment_identity, source_path),
        segment_identity,
        source_sha256: hex(&input.input_sha256()),
    })
}

pub(crate) fn png_sha256(png: &[u8]) -> String {
    hex(&Sha256::digest(png))
}

pub(crate) fn sidecar_path(segment: &Path, source_id: &str) -> PathBuf {
    segment.join(format!(".screen-ocr-v{SIDECAR_SCHEMA}-{source_id}.json"))
}

pub(crate) fn load_sidecar(path: &Path) -> Option<PrivateSidecar> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let sidecar = serde_json::from_slice::<PrivateSidecar>(&bytes).ok()?;
    (sidecar.schema == SIDECAR_SCHEMA).then_some(sidecar)
}

pub(crate) fn load_segment_sidecars(segment: &Path) -> Vec<PrivateSidecar> {
    let mut sidecars = fs::read_dir(segment)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.starts_with(&format!(".screen-ocr-v{SIDECAR_SCHEMA}-"))
                .then(|| load_sidecar(&entry.path()))
                .flatten()
        })
        .collect::<Vec<_>>();
    sidecars.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    sidecars
}

pub(crate) fn decode_coverage(decode_failed: bool, frame_count: usize) -> DecodeCoverage {
    match (decode_failed, frame_count) {
        (false, _) => DecodeCoverage::Complete,
        (true, 0) => DecodeCoverage::Failed,
        (true, _) => DecodeCoverage::Partial,
    }
}

pub(crate) fn cache_matches(
    cached: &PrivateSidecar,
    expected: &PrivateSidecar,
    ignored_ids: &BTreeSet<u64>,
    redo: bool,
) -> bool {
    if redo {
        return false;
    }
    let all_frames_ignored = !expected.frames.is_empty()
        && expected
            .frames
            .iter()
            .all(|frame| ignored_ids.contains(&frame.frame_id));
    if cached.schema != SIDECAR_SCHEMA
        || cached.source_id != expected.source_id
        || cached.segment_identity != expected.segment_identity
        || cached.source_sha256 != expected.source_sha256
        || cached.recipe_id != expected.recipe_id
        || cached.admission_policy_id != expected.admission_policy_id
        || cached.decode != expected.decode
        || cached.frames.len() != expected.frames.len()
        || !(matches!(cached.state, CoverageState::Empty | CoverageState::Success)
            || (all_frames_ignored && cached.state == CoverageState::NotRun))
    {
        return false;
    }
    cached
        .frames
        .iter()
        .zip(&expected.frames)
        .all(|(cached, expected)| {
            if cached.frame_id != expected.frame_id
                || cached.segment_offset_ms != expected.segment_offset_ms
                || cached.png_sha256 != expected.png_sha256
            {
                return false;
            }
            if ignored_ids.contains(&cached.frame_id) {
                cached.state == CoverageState::NotRun && cached.occurrences.is_empty()
            } else {
                matches!(cached.state, CoverageState::Empty | CoverageState::Success)
            }
        })
}

pub(crate) fn expected_sidecar(
    source_id: String,
    segment_identity: String,
    source_sha256: String,
    recipe_id: String,
    admission_policy_id: String,
    decode: DecodeCoverage,
    frames: &[QualifiedFrame],
) -> PrivateSidecar {
    PrivateSidecar {
        schema: SIDECAR_SCHEMA,
        source_id,
        segment_identity,
        source_sha256,
        recipe_id,
        admission_policy_id,
        decode,
        state: CoverageState::NotRun,
        frames: frames
            .iter()
            .map(|frame| FrameCoverage {
                frame_id: frame.frame_id,
                segment_offset_ms: segment_offset_millis(frame.timestamp),
                png_sha256: png_sha256(&frame.png),
                state: CoverageState::NotRun,
                error_code: None,
                occurrences: Vec::new(),
            })
            .collect(),
        word_overlap: OverlapDiagnostic {
            state: WordOverlapState::NotComparable,
            ocr: OcrWordState::NotRun,
            extraction: ExtractionTextState::NotReached,
            denominator: 0,
            overlapping_words: 0,
        },
    }
}

/// Process retained frames sequentially through one worker for this burst.
/// Worker failures only update private coverage and never escape into C/S/E.
pub(crate) fn run_worker(
    sidecar: &mut PrivateSidecar,
    frames: &[QualifiedFrame],
    ignored_ids: &BTreeSet<u64>,
    worker: Option<&mut dyn OcrWorker>,
) {
    let Some(worker) = worker else {
        summarize_coverage(sidecar);
        return;
    };
    let mut fatal = false;
    for coverage in &mut sidecar.frames {
        if ignored_ids.contains(&coverage.frame_id) {
            continue;
        }
        let Some(frame) = frames
            .iter()
            .find(|frame| frame.frame_id == coverage.frame_id)
        else {
            coverage.state = CoverageState::Error;
            coverage.error_code = Some("frame_missing".to_owned());
            fatal = true;
            continue;
        };
        if fatal {
            continue;
        }
        if let Err(code) = validate_png(&frame.png) {
            coverage.state = CoverageState::Error;
            coverage.error_code = Some(code.to_owned());
            fatal = true;
            continue;
        }
        match worker.recognize(&frame.png) {
            Ok(occurrences) if occurrences.len() <= MAX_REGIONS => {
                coverage.state = if occurrences.is_empty() {
                    CoverageState::Empty
                } else {
                    CoverageState::Success
                };
                coverage.occurrences = occurrences;
            }
            Ok(_) => {
                coverage.state = CoverageState::Error;
                coverage.error_code = Some("region_limit".to_owned());
                fatal = true;
            }
            Err(error) => {
                coverage.state = CoverageState::Error;
                coverage.error_code = Some(error.code().to_owned());
                fatal = true;
            }
        }
    }
    annotate_recurrence(sidecar);
    summarize_coverage(sidecar);
}

pub(crate) fn run_worker_with_factory(
    sidecar: &mut PrivateSidecar,
    frames: &[QualifiedFrame],
    ignored_ids: &BTreeSet<u64>,
    factory: &mut dyn OcrWorkerFactory,
) {
    if sidecar
        .frames
        .iter()
        .all(|frame| ignored_ids.contains(&frame.frame_id))
    {
        summarize_coverage(sidecar);
        return;
    }
    match factory.create() {
        Ok(Some(mut worker)) => {
            run_worker(sidecar, frames, ignored_ids, Some(worker.as_mut()));
        }
        Ok(None) => run_worker(sidecar, frames, ignored_ids, None),
        Err(error) => {
            let Some(frame) = sidecar
                .frames
                .iter_mut()
                .find(|frame| !ignored_ids.contains(&frame.frame_id))
            else {
                summarize_coverage(sidecar);
                return;
            };
            frame.state = CoverageState::Error;
            frame.error_code = Some(error.code().to_owned());
            summarize_coverage(sidecar);
        }
    }
}

fn annotate_recurrence(sidecar: &mut PrivateSidecar) {
    let mut counts = BTreeMap::<String, u32>::new();
    for frame in &mut sidecar.frames {
        for occurrence in &mut frame.occurrences {
            let identity = occurrence.text.nfc().collect::<String>();
            let count = counts.entry(identity).or_default();
            *count = count.saturating_add(1);
            occurrence.recurrence = *count;
        }
    }
}

fn validate_png(png: &[u8]) -> Result<(u32, u32), &'static str> {
    if png.len() > MAX_PNG_BYTES {
        return Err("png_size_limit");
    }
    let reader = image::ImageReader::new(Cursor::new(png))
        .with_guessed_format()
        .map_err(|_| "invalid_png")?;
    if reader.format() != Some(image::ImageFormat::Png) {
        return Err("invalid_png");
    }
    let (width, height) = reader.into_dimensions().map_err(|_| "invalid_png")?;
    if width == 0
        || height == 0
        || u64::from(width).saturating_mul(u64::from(height)) > MAX_IMAGE_PIXELS
    {
        return Err("pixel_limit");
    }
    Ok((width, height))
}

fn summarize_coverage(sidecar: &mut PrivateSidecar) {
    sidecar.state = if sidecar
        .frames
        .iter()
        .any(|frame| frame.state == CoverageState::Error)
    {
        CoverageState::Error
    } else if sidecar
        .frames
        .iter()
        .any(|frame| frame.state == CoverageState::Success)
    {
        CoverageState::Success
    } else if sidecar
        .frames
        .iter()
        .any(|frame| frame.state == CoverageState::Empty)
    {
        CoverageState::Empty
    } else {
        CoverageState::NotRun
    };
}

pub(crate) fn folded_path(segment: &Path) -> PathBuf {
    segment.join(format!(".screen-ocr-v{SIDECAR_SCHEMA}-folded.json"))
}

pub(crate) fn publish_sidecar(
    segment: &Path,
    path: &Path,
    sidecar: &PrivateSidecar,
    input: &InputObservation,
    claims: &OutputClaims,
) -> std::io::Result<()> {
    let _commit = input.commit(claims)?;
    let mut sources = load_segment_sidecars(segment)
        .into_iter()
        .filter(|cached| {
            cached.segment_identity == sidecar.segment_identity
                && cached.recipe_id == sidecar.recipe_id
                && cached.source_id != sidecar.source_id
        })
        .collect::<Vec<_>>();
    let mut sidecar = sidecar.clone();
    sidecar.word_overlap = word_overlap_diagnostic(&sidecar, "", ExtractionOutcome::NotReached);
    sources.push(sidecar.clone());
    sources.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    install_sidecar_and_folded(segment, path, &sidecar, &sources)
}

/// Update the private OCR/Qwen word diagnostic after extraction has completed.
/// This writes only typed state and counts; the extraction text is never stored.
#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_extraction_diagnostic(
    segment: &Path,
    path: &Path,
    source_id: &str,
    segment_identity: &str,
    recipe_id: &str,
    source_sha256: &str,
    extraction_text: &str,
    outcome: ExtractionOutcome,
    input: &InputObservation,
    claims: &OutputClaims,
) -> std::io::Result<()> {
    let _commit = input.commit(claims)?;
    let mut sidecar = load_sidecar(path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "private OCR state unavailable",
        )
    })?;
    if sidecar.source_id != source_id
        || sidecar.segment_identity != segment_identity
        || sidecar.recipe_id != recipe_id
        || sidecar.source_sha256 != source_sha256
        || sidecar.source_sha256 != hex(&input.input_sha256())
    {
        return Err(std::io::Error::other("private OCR binding changed"));
    }
    sidecar.word_overlap = word_overlap_diagnostic(&sidecar, extraction_text, outcome);
    let mut sources = load_segment_sidecars(segment)
        .into_iter()
        .filter(|cached| {
            cached.segment_identity == segment_identity
                && cached.recipe_id == recipe_id
                && cached.source_id != source_id
        })
        .collect::<Vec<_>>();
    sources.push(sidecar.clone());
    sources.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    install_sidecar_and_folded(segment, path, &sidecar, &sources)
}

fn install_sidecar_and_folded(
    segment: &Path,
    path: &Path,
    sidecar: &PrivateSidecar,
    sources: &[PrivateSidecar],
) -> std::io::Result<()> {
    let sidecar_bytes =
        serde_json::to_vec(sidecar).map_err(|error| std::io::Error::other(error.to_string()))?;
    let folded = PrivateFoldedSnapshot {
        schema: SIDECAR_SCHEMA,
        segment_identity: sidecar.segment_identity.clone(),
        recipe_id: sidecar.recipe_id.clone(),
        lines: fold(sources),
        word_overlap: combined_word_overlap(sources),
    };
    let folded_bytes =
        serde_json::to_vec(&folded).map_err(|error| std::io::Error::other(error.to_string()))?;
    let aggregate_path = folded_path(segment);
    let install = |path: &Path, bytes: &[u8]| {
        atomic_replace(path, bytes, AtomicWriteOptions { mode: Some(0o600) })
            .map_err(|error| std::io::Error::other(error.to_string()))
    };
    install(path, &sidecar_bytes)?;
    if let Err(error) = install(&aggregate_path, &folded_bytes) {
        let _ = fs::remove_file(aggregate_path);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
fn publish_pair_with(
    sidecar_path: &Path,
    sidecar_bytes: &[u8],
    folded_path: &Path,
    folded_bytes: &[u8],
    mut install: impl FnMut(&Path, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    install(sidecar_path, sidecar_bytes)?;
    install(folded_path, folded_bytes)
}

pub(crate) fn invalidate_source_state(
    segment: &Path,
    path: &Path,
    source_id: &str,
    segment_identity: &str,
    recipe_id: &str,
    claims: &OutputClaims,
) -> std::io::Result<()> {
    let _commit = claims.commit(segment)?;
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let sources = load_segment_sidecars(segment)
        .into_iter()
        .filter(|sidecar| {
            sidecar.segment_identity == segment_identity
                && sidecar.recipe_id == recipe_id
                && sidecar.source_id != source_id
        })
        .collect::<Vec<_>>();
    let aggregate_path = folded_path(segment);
    if sources.is_empty() {
        match fs::remove_file(aggregate_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        return Ok(());
    }
    let snapshot = PrivateFoldedSnapshot {
        schema: SIDECAR_SCHEMA,
        segment_identity: segment_identity.to_owned(),
        recipe_id: recipe_id.to_owned(),
        lines: fold(&sources),
        word_overlap: combined_word_overlap(&sources),
    };
    let bytes =
        serde_json::to_vec(&snapshot).map_err(|error| std::io::Error::other(error.to_string()))?;
    atomic_replace(
        &aggregate_path,
        &bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

pub(crate) fn segment_offset_millis(source_seconds: f64) -> i64 {
    if source_seconds.is_finite() {
        (source_seconds * 1_000.0).round() as i64
    } else {
        0
    }
}

/// A frame opts out only when available metadata names a known visible
/// primary or secondary category whose effective importance is Ignore.
/// The overlap bit intentionally does not suppress the secondary category.
pub(crate) fn ignored_frame_ids(
    analyses: &BTreeMap<u64, serde_json::Value>,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> BTreeSet<u64> {
    analyses
        .iter()
        .filter_map(|(frame_id, analysis)| {
            ["primary", "secondary"]
                .into_iter()
                .any(|field| {
                    analysis
                        .get(field)
                        .and_then(serde_json::Value::as_str)
                        .filter(|name| {
                            CATEGORIES_META
                                .iter()
                                .any(|category| category.name == *name)
                        })
                        .is_some_and(|name| {
                            resolved_importance(Some(name), overrides) == Importance::Ignore
                        })
                })
                .then_some(*frame_id)
        })
        .collect()
}

/// Hash only the effective ignore policy and per-frame admission decisions.
pub(crate) fn admission_policy_id(
    analyses: &BTreeMap<u64, serde_json::Value>,
    overrides: &BTreeMap<String, CategoryOverride>,
) -> String {
    let ignored_categories = CATEGORIES_META
        .iter()
        .filter(|category| {
            resolved_importance(Some(category.name), overrides) == Importance::Ignore
        })
        .map(|category| category.name)
        .collect::<Vec<_>>();
    let ignored_frames = ignored_frame_ids(analyses, overrides);
    let mut digest = Sha256::new();
    digest.update(b"solstone-screen-ocr-admission-v1\0");
    for category in ignored_categories {
        digest.update(category.as_bytes());
        digest.update([0]);
    }
    digest.update([0xff]);
    for frame_id in ignored_frames {
        digest.update(frame_id.to_le_bytes());
    }
    hex(&digest.finalize())
}

/// Fold private source sidecars in deterministic segment-time order. Source
/// identity remains a tie-breaker and is never replaced by frame ID/time.
pub(crate) fn fold(sidecars: &[PrivateSidecar]) -> Vec<FoldedLine> {
    let mut candidates = Vec::new();
    for sidecar in sidecars {
        for frame in &sidecar.frames {
            if !matches!(frame.state, CoverageState::Empty | CoverageState::Success) {
                continue;
            }
            let mut ordered_occurrences = frame.occurrences.iter().enumerate().collect::<Vec<_>>();
            ordered_occurrences.sort_by_key(|(index, occurrence)| (occurrence.line_order, *index));
            let mut redact_following = false;
            for (occurrence_index, occurrence) in ordered_occurrences {
                for (subline_order, text) in
                    safe_lines_with_state(&occurrence.text, &mut redact_following)
                        .into_iter()
                        .enumerate()
                {
                    candidates.push((
                        frame.segment_offset_ms,
                        sidecar.source_id.as_str(),
                        frame.frame_id,
                        occurrence.line_order,
                        occurrence_index,
                        subline_order,
                        text,
                    ));
                }
            }
        }
    }
    candidates.sort_by(|left, right| {
        (left.0, left.1, left.2, left.3, left.4, left.5)
            .cmp(&(right.0, right.1, right.2, right.3, right.4, right.5))
    });
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|(segment_offset_ms, _, _, _, _, _, text)| {
            let identity = text.nfc().collect::<String>();
            seen.insert(identity).then_some(FoldedLine {
                segment_offset_ms,
                text,
            })
        })
        .collect()
}

pub(crate) fn word_overlap_diagnostic(
    sidecar: &PrivateSidecar,
    extraction_text: &str,
    outcome: ExtractionOutcome,
) -> OverlapDiagnostic {
    let ocr_lines = fold(std::slice::from_ref(sidecar));
    let ocr_tokens = word_tokens(
        &ocr_lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let extraction_tokens = word_tokens(extraction_text);
    let ocr = if sidecar.state == CoverageState::NotRun {
        OcrWordState::NotRun
    } else if sidecar.state == CoverageState::Error {
        OcrWordState::Error
    } else if ocr_tokens.is_empty() {
        OcrWordState::Empty
    } else {
        OcrWordState::Present
    };
    let extraction = match outcome {
        ExtractionOutcome::NotReached => ExtractionTextState::NotReached,
        ExtractionOutcome::Failed => ExtractionTextState::Failed,
        ExtractionOutcome::Completed if extraction_tokens.is_empty() => ExtractionTextState::Empty,
        ExtractionOutcome::Completed => ExtractionTextState::Present,
    };
    let state = match (ocr, extraction) {
        (_, ExtractionTextState::NotReached | ExtractionTextState::Failed) => {
            WordOverlapState::NotComparable
        }
        (OcrWordState::NotRun, _) => WordOverlapState::Absent,
        (OcrWordState::Error, _) => WordOverlapState::NotComparable,
        (OcrWordState::Empty, _) | (_, ExtractionTextState::Empty) => WordOverlapState::Empty,
        (OcrWordState::Present, ExtractionTextState::Present) => WordOverlapState::Comparable,
    };
    let overlapping_words = if state == WordOverlapState::Comparable {
        multiset_overlap(&ocr_tokens, &extraction_tokens)
    } else {
        0
    };
    OverlapDiagnostic {
        state,
        ocr,
        extraction,
        denominator: ocr_tokens.len() as u64,
        overlapping_words,
    }
}

fn combined_word_overlap(sidecars: &[PrivateSidecar]) -> OverlapDiagnostic {
    let mut denominator = 0_u64;
    let mut overlapping_words = 0_u64;
    let mut states = sidecars.iter().map(|sidecar| &sidecar.word_overlap);
    let first = states.next();
    let mut state = first.map_or(WordOverlapState::Absent, |diagnostic| diagnostic.state);
    let mut all_same = first.is_some();
    for diagnostic in states {
        denominator = denominator.saturating_add(diagnostic.denominator);
        overlapping_words = overlapping_words.saturating_add(diagnostic.overlapping_words);
        all_same &= diagnostic.state == state;
        if !all_same {
            state = WordOverlapState::NotComparable;
        }
    }
    if let Some(diagnostic) = first {
        denominator = denominator.saturating_add(diagnostic.denominator);
        overlapping_words = overlapping_words.saturating_add(diagnostic.overlapping_words);
    }
    let mut ocr = OcrWordState::NotRun;
    let mut extraction = ExtractionTextState::NotReached;
    for diagnostic in sidecars.iter().map(|sidecar| &sidecar.word_overlap) {
        if diagnostic.ocr == OcrWordState::Present {
            ocr = OcrWordState::Present;
        } else if ocr != OcrWordState::Present && diagnostic.ocr == OcrWordState::Error {
            ocr = OcrWordState::Error;
        } else if ocr == OcrWordState::NotRun && diagnostic.ocr == OcrWordState::Empty {
            ocr = OcrWordState::Empty;
        }
        if diagnostic.extraction == ExtractionTextState::Failed {
            extraction = ExtractionTextState::Failed;
        } else if extraction != ExtractionTextState::Failed
            && diagnostic.extraction == ExtractionTextState::Present
        {
            extraction = ExtractionTextState::Present;
        } else if matches!(extraction, ExtractionTextState::NotReached)
            && diagnostic.extraction == ExtractionTextState::Empty
        {
            extraction = ExtractionTextState::Empty;
        }
    }
    OverlapDiagnostic {
        state,
        ocr,
        extraction,
        denominator,
        overlapping_words,
    }
}

fn multiset_overlap(left: &[String], right: &[String]) -> u64 {
    let mut counts = BTreeMap::<&str, u64>::new();
    for token in right {
        *counts.entry(token).or_default() += 1;
    }
    let mut overlap = 0_u64;
    for token in left {
        if let Some(count) = counts.get_mut(token.as_str())
            && *count > 0
        {
            *count -= 1;
            overlap += 1;
        }
    }
    overlap
}

fn word_tokens(text: &str) -> Vec<String> {
    let normalized = text.nfc().collect::<String>();
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in normalized.chars() {
        if character.is_alphanumeric()
            || (unicode_normalization::char::canonical_combining_class(character) != 0
                && !current.is_empty())
        {
            current.extend(character.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(current.nfc().collect());
            current.clear();
        }
    }
    if !current.is_empty() {
        tokens.push(current.nfc().collect());
    }
    tokens
}

/// Best-effort privacy filtering for a future public fold. The private sidecar
/// retains the unmodified occurrence for audit/reprocessing.
#[cfg(test)]
pub(crate) fn safe_lines(text: &str) -> Vec<String> {
    safe_lines_with_state(text, &mut false)
}

fn safe_lines_with_state(text: &str, redact_following: &mut bool) -> Vec<String> {
    let normalized = text.nfc().collect::<String>();
    let mut result = Vec::new();
    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if *redact_following {
            result.push("[REDACTED]".to_owned());
            *redact_following = mask_password_line(trimmed).1;
            continue;
        }
        let (line, next_secret) = mask_password_line(trimmed);
        *redact_following = next_secret;
        let line = mask_secret_url_parameters(&line);
        let line = mask_high_entropy_tokens(&line);
        let line = line.trim().to_owned();
        if line.chars().any(char::is_alphanumeric) {
            result.push(line);
        }
    }
    result
}

fn mask_password_line(line: &str) -> (String, bool) {
    let lower = line.to_ascii_lowercase();
    let Some(position) = ["password", "passphrase", "passwd"]
        .iter()
        .filter_map(|needle| lower.find(needle))
        .min()
    else {
        return (line.to_owned(), false);
    };
    let suffix = &line[position..];
    if let Some(separator) = suffix.find([':', '=']) {
        let value_start = position + separator + 1;
        if line[value_start..].trim().is_empty() {
            return ("[REDACTED]".to_owned(), true);
        }
        let mut masked = line[..value_start].to_owned();
        masked.push_str("[REDACTED]");
        return (masked, false);
    }
    (line.to_owned(), false)
}

fn mask_secret_url_parameters(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0;
    while cursor < line.len() {
        let rest = &line[cursor..];
        let start = ["https://", "http://"]
            .iter()
            .filter_map(|scheme| rest.find(scheme).map(|offset| cursor + offset))
            .min();
        let Some(start) = start else {
            output.push_str(rest);
            break;
        };
        output.push_str(&line[cursor..start]);
        let end = line[start..]
            .find(char::is_whitespace)
            .map_or(line.len(), |offset| start + offset);
        let (url, suffix) = trim_url_suffix(&line[start..end]);
        output.push_str(&mask_url_query(url));
        output.push_str(suffix);
        cursor = end;
    }
    output
}

fn trim_url_suffix(url: &str) -> (&str, &str) {
    let end = url
        .char_indices()
        .rev()
        .find(|(_, ch)| !matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}'))
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    (&url[..end], &url[end..])
}

fn mask_url_query(url: &str) -> String {
    let Some(query_start) = url.find('?') else {
        return url.to_owned();
    };
    let query_end = url[query_start + 1..]
        .find('#')
        .map_or(url.len(), |offset| query_start + 1 + offset);
    let mut output = url[..query_start + 1].to_owned();
    for (index, pair) in url[query_start + 1..query_end].split('&').enumerate() {
        if index > 0 {
            output.push('&');
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        output.push_str(key);
        if pair.contains('=') {
            output.push('=');
            if is_secret_parameter(key) && !value.is_empty() {
                output.push_str("[REDACTED]");
            } else {
                output.push_str(value);
            }
        }
    }
    output.push_str(&url[query_end..]);
    output
}

fn is_secret_parameter(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '.'], "_");
    [
        "token",
        "access_token",
        "refresh_token",
        "auth",
        "authorization",
        "secret",
        "password",
        "passwd",
        "api_key",
        "apikey",
        "credential",
        "session",
        "signature",
        "sig",
    ]
    .iter()
    .any(|secret| key == *secret || key.ends_with(&format!("_{secret}")))
}

fn mask_high_entropy_tokens(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    for token in line.split_inclusive(char::is_whitespace) {
        let trimmed = token.trim_end_matches(char::is_whitespace);
        let whitespace = &token[trimmed.len()..];
        let (leading, candidate, trailing) = secret_token_parts(trimmed);
        if looks_like_secret(candidate) {
            output.push_str(leading);
            output.push_str("[REDACTED]");
            output.push_str(trailing);
        } else {
            output.push_str(trimmed);
        }
        output.push_str(whitespace);
    }
    output
}

fn secret_token_parts(token: &str) -> (&str, &str, &str) {
    let Some(start) = token.find(|ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    else {
        return (token, "", "");
    };
    let end = token
        .rfind(|ch: char| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        .map_or(start, |index| index + 1);
    (&token[..start], &token[start..end], &token[end..])
}

fn looks_like_secret(token: &str) -> bool {
    if token.len() < 32 || token.starts_with("http") {
        return false;
    }
    let mut counts = [0_u32; 256];
    for byte in token.bytes() {
        if !byte.is_ascii_alphanumeric() && byte != b'_' && byte != b'-' {
            return false;
        }
        counts[usize::from(byte)] += 1;
    }
    let length = token.len() as f64;
    let entropy = counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let probability = f64::from(*count) / length;
            -probability * probability.log2()
        })
        .sum::<f64>();
    entropy >= 4.0
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sidecar(source_id: &str, frame_id: u64, offset: i64, text: &str) -> PrivateSidecar {
        PrivateSidecar {
            schema: SIDECAR_SCHEMA,
            source_id: source_id.to_owned(),
            segment_identity: "segment".to_owned(),
            source_sha256: "source".to_owned(),
            recipe_id: "recipe".to_owned(),
            admission_policy_id: "policy".to_owned(),
            decode: DecodeCoverage::Complete,
            state: CoverageState::Success,
            frames: vec![FrameCoverage {
                frame_id,
                segment_offset_ms: offset,
                png_sha256: "png".to_owned(),
                state: CoverageState::Success,
                error_code: None,
                occurrences: vec![RawOccurrence {
                    line_order: 0,
                    text: text.to_owned(),
                    confidence: Some(0.9),
                    geometry: vec![[1.0, 2.0]],
                    recurrence: 1,
                }],
            }],
            word_overlap: OverlapDiagnostic {
                state: WordOverlapState::NotComparable,
                ocr: OcrWordState::Present,
                extraction: ExtractionTextState::NotReached,
                denominator: 0,
                overlapping_words: 0,
            },
        }
    }

    #[test]
    fn only_known_ignored_primary_or_secondary_categories_opt_out() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "code".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Ignore),
                extraction: None,
            },
        );
        let analyses = BTreeMap::from([
            (
                1,
                json!({"primary":"messaging", "secondary":"code", "overlap":true}),
            ),
            (2, json!({"primary":"unknown", "secondary":"missing"})),
            (3, json!({"primary":"messaging", "secondary":"none"})),
        ]);
        assert_eq!(
            ignored_frame_ids(&analyses, &overrides),
            BTreeSet::from([1])
        );
        assert_eq!(
            ignored_frame_ids(&BTreeMap::new(), &overrides),
            BTreeSet::new()
        );
    }

    #[test]
    fn source_identity_distinguishes_equal_frame_ids_and_timestamps() {
        let left = source_identity(
            "segment-identity",
            Path::new("day/default/120000_60/a.webm"),
        );
        let right = source_identity(
            "segment-identity",
            Path::new("day/default/120000_60/b.webm"),
        );
        assert_ne!(left, right);
        assert_ne!(
            source_identity("other-segment", Path::new("day/default/120000_60/a.webm")),
            left
        );
        assert_eq!(segment_offset_millis(3.5), 3_500);
    }

    #[test]
    fn absent_resolver_uses_the_pinned_recipe_identity_without_starting_a_worker() {
        let resolver = UnavailableHelperResolver;
        let mut factory = ResolvedWorkerFactory::new(&resolver);
        assert_eq!(
            factory.recipe_id(),
            hex(&Sha256::digest(PINNED_HELPER_RECIPE))
        );
        assert!(factory.create().unwrap().is_none());
    }

    #[test]
    fn helper_response_protocol_validates_framing_identity_and_bounded_payloads() {
        let request_id = "frame_4";
        let png_hash = "ab".repeat(32);
        let response_header = WorkerHeader::Response {
            version: PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            png_sha256: png_hash.clone(),
        };
        assert!(response_binding_matches(
            &response_header,
            request_id,
            &png_hash
        ));
        assert!(!response_binding_matches(
            &response_header,
            "frame_5",
            &png_hash
        ));
        assert!(!response_binding_matches(
            &response_header,
            request_id,
            &"cd".repeat(32)
        ));
        assert!(!response_binding_matches(
            &WorkerHeader::Response {
                version: PROTOCOL_VERSION + 1,
                request_id: request_id.to_owned(),
                png_sha256: png_hash.clone(),
            },
            request_id,
            &png_hash
        ));

        let body = serde_json::to_vec(&serde_json::json!({
            "regions": [{
                "text": "hello 世界",
                "confidence": 0.9,
                "points": [{"x": 0.0, "y": 0.0}, {"x": 1.0, "y": 1.0}]
            }]
        }))
        .unwrap();
        let mut framed = Vec::new();
        write_worker_message(&mut framed, &response_header, &body).unwrap();
        let (read_header, read_body) = read_worker_message(&mut Cursor::new(framed))
            .unwrap()
            .unwrap();
        assert!(response_binding_matches(
            &read_header,
            request_id,
            &png_hash
        ));
        assert_eq!(
            decode_worker_response(&read_body, 1, 1).unwrap()[0].text,
            "hello 世界"
        );

        let too_many_regions = serde_json::to_vec(&serde_json::json!({
            "regions": (0..=MAX_REGIONS).map(|_| serde_json::json!({
                "text": "x",
                "confidence": null,
                "points": [{"x": 0.0, "y": 0.0}]
            })).collect::<Vec<_>>()
        }))
        .unwrap();
        assert_eq!(
            decode_worker_response(&too_many_regions, 1, 1),
            Err(OcrFailure::Protocol)
        );
        let long_text = serde_json::to_vec(&serde_json::json!({
            "regions": [{
                "text": "x".repeat(MAX_TEXT_BYTES + 1),
                "confidence": null,
                "points": [{"x": 0.0, "y": 0.0}]
            }]
        }))
        .unwrap();
        assert_eq!(
            decode_worker_response(&long_text, 1, 1),
            Err(OcrFailure::Protocol)
        );
        let invalid_geometry = serde_json::to_vec(&serde_json::json!({
            "regions": [{
                "text": "x",
                "confidence": null,
                "points": [{"x": 2.0, "y": 0.0}]
            }]
        }))
        .unwrap();
        assert_eq!(
            decode_worker_response(&invalid_geometry, 1, 1),
            Err(OcrFailure::Protocol)
        );

        let mut oversized_header = Vec::new();
        oversized_header.extend_from_slice(&((MAX_HEADER_BYTES as u32) + 1).to_be_bytes());
        assert_eq!(
            read_worker_message(&mut Cursor::new(oversized_header)),
            Err(OcrFailure::Protocol)
        );
        let mut oversized_response = Vec::new();
        let header_bytes = serde_json::to_vec(&response_header).unwrap();
        oversized_response.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
        oversized_response.extend_from_slice(&header_bytes);
        oversized_response.extend_from_slice(&((MAX_RESPONSE_BYTES as u32) + 1).to_be_bytes());
        assert_eq!(
            read_worker_message(&mut Cursor::new(oversized_response)),
            Err(OcrFailure::Protocol)
        );
    }

    #[test]
    fn fake_atomic_publication_failure_returns_only_a_content_free_error() {
        let sidecar_bytes = br#"{"state":"success","text":"private"}"#;
        let folded_bytes = br#"{"lines":[]}"#;
        let mut calls = 0;
        let result = publish_pair_with(
            Path::new(".private-sidecar"),
            sidecar_bytes,
            Path::new(".private-fold"),
            folded_bytes,
            |_, _| {
                calls += 1;
                if calls == 2 {
                    Err(std::io::Error::other("injected failure"))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(calls, 2);
        assert_eq!(
            result.map_err(|_| "private_publication_failed"),
            Err("private_publication_failed")
        );
    }

    #[test]
    fn fold_sorts_before_first_seen_dedup_and_keeps_source_identity_as_tiebreaker() {
        let sidecars = vec![
            sidecar("source-b", 1, 1_000, "same"),
            sidecar("source-a", 1, 1_000, "same"),
            sidecar("source-c", 2, 500, "earlier"),
        ];
        assert_eq!(
            fold(&sidecars),
            vec![
                FoldedLine {
                    segment_offset_ms: 500,
                    text: "earlier".to_owned(),
                },
                FoldedLine {
                    segment_offset_ms: 1_000,
                    text: "same".to_owned(),
                },
            ]
        );
        assert_eq!(sidecars[0].source_id, "source-b");
        assert_eq!(sidecars[1].source_id, "source-a");
    }

    #[test]
    fn privacy_fold_masks_secrets_but_keeps_useful_urls_commands_references_and_unicode() {
        let lines = safe_lines(
            "open https://example.test/path?view=compact&access_token=abc123 and run ls -la\npassword:\nhunter2\nref-7 العربية café\nAb3dEf5gHi7jKl9mNo1pQr2sTu4vWx6y",
        );
        assert_eq!(
            lines[0],
            "open https://example.test/path?view=compact&access_token=[REDACTED] and run ls -la"
        );
        assert_eq!(lines[1], "[REDACTED]");
        assert_eq!(lines[2], "[REDACTED]");
        assert_eq!(lines[3], "ref-7 العربية café");
        assert_eq!(lines[4], "[REDACTED]");
        assert_eq!(safe_lines("é"), safe_lines("e\u{301}"));
    }

    #[test]
    fn frame_masking_carries_password_prompt_state_across_occurrences_before_dedup() {
        let mut source = sidecar("source", 1, 0, "unused");
        source.frames[0].occurrences = vec![
            RawOccurrence {
                line_order: 4,
                text: "swordfish".to_owned(),
                confidence: None,
                geometry: Vec::new(),
                recurrence: 1,
            },
            RawOccurrence {
                line_order: 3,
                text: "Password:".to_owned(),
                confidence: None,
                geometry: Vec::new(),
                recurrence: 1,
            },
        ];
        assert_eq!(
            fold(&[source]),
            vec![FoldedLine {
                segment_offset_ms: 0,
                text: "[REDACTED]".to_owned(),
            }]
        );
    }

    #[test]
    fn word_overlap_diagnostic_covers_states_counts_and_unicode_tokens() {
        let mut source = sidecar("source", 1, 0, "Cafe\u{301} hello café");
        let comparable =
            word_overlap_diagnostic(&source, "café HELLO world", ExtractionOutcome::Completed);
        assert_eq!(comparable.state, WordOverlapState::Comparable);
        assert_eq!(comparable.ocr, OcrWordState::Present);
        assert_eq!(comparable.extraction, ExtractionTextState::Present);
        assert_eq!(comparable.denominator, 3);
        assert_eq!(comparable.overlapping_words, 2);

        let encoded = serde_json::to_string(&comparable).unwrap();
        assert!(!encoded.contains("café"));
        assert!(!encoded.contains("hello"));

        source.state = CoverageState::NotRun;
        source.frames[0].state = CoverageState::NotRun;
        let absent = word_overlap_diagnostic(&source, "Qwen words", ExtractionOutcome::Completed);
        assert_eq!(absent.state, WordOverlapState::Absent);
        assert_eq!(absent.ocr, OcrWordState::NotRun);

        source.state = CoverageState::Empty;
        source.frames[0].state = CoverageState::Empty;
        source.frames[0].occurrences.clear();
        let ocr_empty =
            word_overlap_diagnostic(&source, "Qwen words", ExtractionOutcome::Completed);
        assert_eq!(ocr_empty.state, WordOverlapState::Empty);
        assert_eq!(ocr_empty.ocr, OcrWordState::Empty);

        source.state = CoverageState::Success;
        source.frames[0].state = CoverageState::Success;
        source.frames[0].occurrences = vec![RawOccurrence {
            line_order: 0,
            text: "useful text".to_owned(),
            confidence: None,
            geometry: Vec::new(),
            recurrence: 1,
        }];
        let extraction_empty = word_overlap_diagnostic(&source, "", ExtractionOutcome::Completed);
        assert_eq!(extraction_empty.state, WordOverlapState::Empty);
        assert_eq!(extraction_empty.extraction, ExtractionTextState::Empty);

        source.state = CoverageState::Error;
        source.frames[0].state = CoverageState::Error;
        let ocr_error = word_overlap_diagnostic(&source, "useful", ExtractionOutcome::Completed);
        assert_eq!(ocr_error.state, WordOverlapState::NotComparable);
        assert_eq!(ocr_error.ocr, OcrWordState::Error);

        source.state = CoverageState::Success;
        source.frames[0].state = CoverageState::Success;
        let failed = word_overlap_diagnostic(&source, "useful", ExtractionOutcome::Failed);
        assert_eq!(failed.state, WordOverlapState::NotComparable);
        assert_eq!(failed.extraction, ExtractionTextState::Failed);
        let not_reached = word_overlap_diagnostic(&source, "", ExtractionOutcome::NotReached);
        assert_eq!(not_reached.state, WordOverlapState::NotComparable);
        assert_eq!(not_reached.extraction, ExtractionTextState::NotReached);
    }

    #[test]
    fn cache_reuse_requires_exact_bindings_and_redo_always_invalidates() {
        let expected = sidecar("source", 1, 1_250, "");
        let mut cached = expected.clone();
        cached.frames[0].state = CoverageState::Empty;
        cached.state = CoverageState::Empty;
        assert!(cache_matches(&cached, &expected, &BTreeSet::new(), false));
        assert!(!cache_matches(&cached, &expected, &BTreeSet::new(), true));
        let mut changed = expected.clone();
        changed.frames[0].png_sha256.push_str("-changed");
        assert!(!cache_matches(&cached, &changed, &BTreeSet::new(), false));
        let ignored = BTreeSet::from([1]);
        cached.frames[0].state = CoverageState::NotRun;
        cached.frames[0].occurrences.clear();
        cached.state = CoverageState::NotRun;
        assert!(cache_matches(&cached, &expected, &ignored, false));
    }

    fn tiny_png() -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    struct FakeWorker {
        result: Result<Vec<RawOccurrence>, OcrFailure>,
        calls: usize,
    }

    impl OcrWorker for FakeWorker {
        fn recognize(&mut self, _png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure> {
            self.calls += 1;
            self.result.clone()
        }
    }

    fn expected_with_frames() -> (PrivateSidecar, Vec<QualifiedFrame>) {
        let frames = vec![
            QualifiedFrame {
                frame_id: 1,
                timestamp: 2.0,
                png: tiny_png(),
            },
            QualifiedFrame {
                frame_id: 2,
                timestamp: 7.0,
                png: tiny_png(),
            },
        ];
        let sidecar = expected_sidecar(
            "source".to_owned(),
            "segment".to_owned(),
            "digest".to_owned(),
            "recipe".to_owned(),
            "policy".to_owned(),
            DecodeCoverage::Complete,
            &frames,
        );
        (sidecar, frames)
    }

    #[test]
    fn helper_absence_and_empty_success_have_distinct_coverage_without_processes() {
        let (mut sidecar, frames) = expected_with_frames();
        run_worker(&mut sidecar, &frames, &BTreeSet::new(), None);
        assert_eq!(sidecar.state, CoverageState::NotRun);
        assert!(
            sidecar
                .frames
                .iter()
                .all(|frame| frame.state == CoverageState::NotRun)
        );

        let (mut sidecar, frames) = expected_with_frames();
        let mut worker = FakeWorker {
            result: Ok(Vec::new()),
            calls: 0,
        };
        run_worker(&mut sidecar, &frames, &BTreeSet::new(), Some(&mut worker));
        assert_eq!(worker.calls, 2);
        assert_eq!(sidecar.state, CoverageState::Empty);
        assert!(
            sidecar
                .frames
                .iter()
                .all(|frame| frame.state == CoverageState::Empty)
        );
    }

    #[test]
    fn protocol_and_timeout_failures_latch_and_leave_later_frames_not_run() {
        for failure in [OcrFailure::Protocol, OcrFailure::Timeout] {
            let (mut sidecar, frames) = expected_with_frames();
            let mut worker = FakeWorker {
                result: Err(failure),
                calls: 0,
            };
            run_worker(&mut sidecar, &frames, &BTreeSet::new(), Some(&mut worker));
            assert_eq!(worker.calls, 1);
            assert_eq!(sidecar.state, CoverageState::Error);
            assert_eq!(sidecar.frames[0].state, CoverageState::Error);
            assert_eq!(sidecar.frames[1].state, CoverageState::NotRun);
        }
    }

    #[test]
    fn ignored_secondary_category_skips_ocr_even_when_analysis_marks_overlap() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "code".to_owned(),
            CategoryOverride {
                importance: Some(Importance::Ignore),
                extraction: None,
            },
        );
        let analyses = BTreeMap::from([(
            1,
            json!({"primary":"messaging", "secondary":"code", "overlap":true}),
        )]);
        let ignored = ignored_frame_ids(&analyses, &overrides);
        let (mut sidecar, frames) = expected_with_frames();
        let mut worker = FakeWorker {
            result: Ok(Vec::new()),
            calls: 0,
        };
        run_worker(&mut sidecar, &frames, &ignored, Some(&mut worker));
        assert_eq!(worker.calls, 1);
        assert_eq!(sidecar.frames[0].state, CoverageState::NotRun);
        assert_eq!(sidecar.frames[1].state, CoverageState::Empty);
    }
}
