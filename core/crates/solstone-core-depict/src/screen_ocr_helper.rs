// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pure-Rust subprocess client for the pinned `solstone-screen-ocr-helper` binary.
//!
//! Lives outside `solstone-core-describe` so static `musl-static` binaries such as
//! `solstone-core` can run local OCR over stdin/stdout pipes without pulling in
//! FFmpeg, ONNX Runtime, or `libloading`.

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

pub const MAX_HEADER_BYTES: usize = 4 * 1024;
pub const MAX_PNG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;
pub const MAX_REGIONS: usize = 3_000;
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_READY_BYTES: usize = 16 * 1024;
pub const MAX_ERROR_BYTES: usize = 512;
pub const MAX_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_GEOMETRY_POINTS: usize = 16;
const MAX_STDERR_BYTES: usize = 8 * 1024;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const INFERENCE_TIMEOUT: Duration = Duration::from_secs(120);
pub const PROTOCOL_VERSION: u16 = 1;
pub const PINNED_HELPER_RECIPE: &[u8] = include_bytes!("../../../screen-ocr-helper/recipe.toml");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawOccurrence {
    pub line_order: u32,
    pub text: String,
    pub confidence: Option<f32>,
    pub geometry: Vec<[f32; 2]>,
    pub recurrence: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrFailure {
    Unavailable,
    Timeout,
    Protocol,
    Runtime,
}

impl OcrFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "helper_unavailable",
            Self::Timeout => "helper_timeout",
            Self::Protocol => "helper_protocol",
            Self::Runtime => "helper_runtime",
        }
    }
}

pub trait OcrWorker {
    fn recognize(&mut self, png: &[u8]) -> Result<Vec<RawOccurrence>, OcrFailure>;
}

/// Open the installed local reader for still-image callers outside a screen segment.
pub fn installed_ocr_worker() -> Result<Option<Box<dyn OcrWorker>>, OcrFailure> {
    ResolvedWorkerFactory::new(&InstalledHelperResolver).create()
}

/// Native text in the order reported by the local OCR reader.
pub fn read_still_ocr_text(worker: &mut dyn OcrWorker, png: &[u8]) -> Result<String, OcrFailure> {
    let mut words = worker.recognize(png)?;
    words.sort_by_key(|r| r.line_order);
    Ok(words
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n"))
}

pub trait OcrWorkerFactory {
    fn recipe_id(&mut self) -> String;
    fn create(&mut self) -> Result<Option<Box<dyn OcrWorker>>, OcrFailure>;

    /// Sidecar detail for a helper that cannot be loaded. Desktop leaves this unset.
    fn unavailable_detail(&mut self) -> Option<&'static str> {
        None
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HelperReadyIdentity {
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
    pub fn cache_id(&self) -> String {
        serde_json::to_vec(self)
            .map(|bytes| hex(&Sha256::digest(bytes)))
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedHelper {
    pub executable: PathBuf,
    pub asset_dir: PathBuf,
    pub runtime_root: PathBuf,
    pub runtime_library: PathBuf,
    pub expected_ready: HelperReadyIdentity,
}

/// The distribution crate supplies this boundary when it can resolve an
/// installed member. Until then, callers record an unavailable OCR gap.
pub trait HelperResolver {
    fn resolve(&self) -> Option<ResolvedHelper>;
}

pub struct InstalledHelperResolver;

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
        resolve_helper_bundle(bundle)
    }
}

fn resolve_helper_bundle(
    bundle: solstone_core_installed_payload::CurrentScreenOcrBundle,
) -> Option<ResolvedHelper> {
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

pub struct ResolvedWorkerFactory<'a> {
    resolver: &'a dyn HelperResolver,
    resolved: Option<Option<ResolvedHelper>>,
}

impl<'a> ResolvedWorkerFactory<'a> {
    pub fn new(resolver: &'a dyn HelperResolver) -> Self {
        Self {
            resolver,
            resolved: None,
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn from_executable(
        resolver: &'a dyn HelperResolver,
        executable: &Path,
    ) -> Result<Self, String> {
        let bundle = solstone_core_installed_payload::resolve_screen_ocr_bundle_from_executable(
            executable,
            solstone_core_installed_payload::compiled_target(),
            cfg!(target_env = "gnu"),
        )
        .map_err(|error| error.to_string())?;
        let helper = resolve_helper_bundle(bundle).ok_or("invalid screen OCR bundle")?;
        Ok(Self {
            resolver,
            resolved: Some(Some(helper)),
        })
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
pub enum WorkerHeader {
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

pub struct ProcessWorker {
    child: Option<Child>,
    requests: Option<SyncSender<(WorkerHeader, Vec<u8>)>>,
    messages: Option<Receiver<WorkerFrame>>,
    stderr_overflow: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    request_number: u64,
    failed: bool,
    inference_timeout: Duration,
}

impl ProcessWorker {
    pub fn launch(helper: ResolvedHelper) -> Result<Self, OcrFailure> {
        Self::launch_with_timeouts(helper, STARTUP_TIMEOUT, INFERENCE_TIMEOUT)
    }

    pub fn launch_with_timeouts(
        helper: ResolvedHelper,
        startup_timeout: Duration,
        inference_timeout: Duration,
    ) -> Result<Self, OcrFailure> {
        let child = Self::spawn_child(&helper)?;
        Self::from_child(
            child,
            helper.expected_ready,
            startup_timeout,
            inference_timeout,
        )
    }

    pub fn spawn_child(helper: &ResolvedHelper) -> Result<Child, OcrFailure> {
        if !helper.executable.is_absolute()
            || !helper.asset_dir.is_absolute()
            || !helper.runtime_root.is_absolute()
            || !helper.runtime_library.is_absolute()
            || !valid_ready_identity(&helper.expected_ready)
        {
            return Err(OcrFailure::Unavailable);
        }
        Command::new(&helper.executable)
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
            .map_err(|_| OcrFailure::Unavailable)
    }

    pub fn from_child(
        mut child: Child,
        expected_ready: HelperReadyIdentity,
        startup_timeout: Duration,
        inference_timeout: Duration,
    ) -> Result<Self, OcrFailure> {
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
            inference_timeout,
        };
        let ready = worker.receive(startup_timeout)?;
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
        if !valid_ready_identity(&identity) || identity != expected_ready {
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

    pub fn is_terminated_after_failure(&self) -> bool {
        self.failed && self.child.is_none() && self.threads.is_empty()
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
        let (header, body) = self.receive(self.inference_timeout)?;
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

pub fn response_binding_matches(header: &WorkerHeader, request_id: &str, png_sha256: &str) -> bool {
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

pub fn terminate_child(child: &mut Child) {
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

pub fn read_worker_message(
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

pub fn write_worker_message(
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

pub fn decode_worker_response(
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

pub fn validate_png(png: &[u8]) -> Result<(u32, u32), &'static str> {
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

pub fn png_sha256(png: &[u8]) -> String {
    hex(&Sha256::digest(png))
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
