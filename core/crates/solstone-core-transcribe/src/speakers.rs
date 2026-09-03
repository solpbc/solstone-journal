// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded subprocess adaptation for native speaker analysis.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::sync::mpsc::{self, Receiver, TryRecvError};
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Instant;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};
use solstone_core_observe_audio::{AudioError, write_f32le_exclusive};
#[cfg(unix)]
use solstone_core_system::process::{
    BoxedTerminateFn, Disposition, LaunchAuthority, LaunchError, launch,
};

use crate::speakers_installation::validate_speakers_analyze_runtime;

const REQUEST_SCHEMA: &str = "solstone-speaker-analyze-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-speaker-analyze-response-v1";
const ERROR_SCHEMA: &str = "solstone-speaker-analyze-error-v1";
const TEMP_ROOT: &str = "/var/tmp";
const TEMP_PREFIX: &str = "solstone-speakers-analyze-";
#[cfg(unix)]
const TEMP_DIR_MODE: u32 = 0o700;
const WESPEAKER_EMBEDDING_WIDTH: usize = 256;
const ENCODER_ID: &str = "wespeaker-resnet34-256";
const PAYLOAD_FORMAT: &str = "raw-f32le-row-major-v1";
const PAYLOAD_DTYPE: &str = "float32-le";
const SPEAKER_ANALYSIS_FAILURE_PATH: &str = "native";

/// Native helper invocation limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpeakersAnalyzeBudget {
    pub(crate) timeout: Duration,
    pub(crate) stdout_limit_bytes: usize,
    pub(crate) stderr_limit_bytes: usize,
    pub(crate) terminate_grace: Duration,
    pub(crate) kill_grace: Duration,
}

impl Default for SpeakersAnalyzeBudget {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2400),
            stdout_limit_bytes: 1024 * 1024,
            stderr_limit_bytes: 64 * 1024,
            terminate_grace: Duration::from_secs(5),
            kill_grace: Duration::from_secs(5),
        }
    }
}

/// Validated raw statement-embedding sidecar metadata.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpeakerEmbeddingPayload {
    pub(crate) payload: Vec<u8>,
    pub(crate) statement_ids: Vec<i64>,
    pub(crate) durations_s: Vec<f64>,
    pub(crate) encoder: String,
}

/// Validated native speaker-analysis result for later transcript publication.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpeakerAnalyzeResult {
    pub(crate) statements: Vec<Map<String, Value>>,
    pub(crate) embedding_payload: Option<SpeakerEmbeddingPayload>,
    pub(crate) speaker_evidence: String,
    pub(crate) speaker_evidence_multi_fraction: f64,
    pub(crate) overlap_fraction: f64,
    pub(crate) statement_labels: Option<Vec<Option<i64>>>,
}

/// Content-free attribution for a failed native speaker-analysis attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeakerAnalyzeError {
    pub path: PathBuf,
    pub stage: String,
    pub reason: String,
    pub native_exit_code: Option<i32>,
}

impl SpeakerAnalyzeError {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        stage: impl Into<String>,
        reason: impl Into<String>,
        native_exit_code: Option<i32>,
    ) -> Self {
        let reason = sanitize_reason(&reason.into());
        Self {
            path: path.into(),
            stage: stage.into(),
            reason,
            native_exit_code,
        }
    }

    /// Content-free fields for the eventual `observe.transcribed` event.
    pub fn event_fields(&self) -> Map<String, Value> {
        let mut fields = Map::from_iter([
            (
                "speaker_analysis_failure_path".to_owned(),
                Value::String(SPEAKER_ANALYSIS_FAILURE_PATH.to_owned()),
            ),
            (
                "speaker_analysis_failure_stage".to_owned(),
                Value::String(self.stage.clone()),
            ),
            (
                "speaker_analysis_failure_reason".to_owned(),
                Value::String(sanitize_reason(&self.reason)),
            ),
        ]);
        if let Some(exit_code) = self.native_exit_code {
            fields.insert(
                "speaker_analysis_failure_native_exit_code".to_owned(),
                Value::from(exit_code),
            );
        }
        fields
    }
}

impl fmt::Display for SpeakerAnalyzeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "speaker analysis failed: {}/{}",
            self.stage, self.reason
        )
    }
}

impl std::error::Error for SpeakerAnalyzeError {}

/// Create the private temporary directory used for one helper invocation.
pub(crate) fn create_speakers_analyze_temp_dir(raw_path: &Path) -> io::Result<PathBuf> {
    create_speakers_analyze_temp_dir_in(raw_path, Path::new(TEMP_ROOT), std::process::id())
}

/// Remove stale helper directories older than one day.
pub(crate) fn sweep_stale_speakers_analyze_dirs(max_age: Duration) -> usize {
    sweep_stale_speakers_analyze_dirs_at(Path::new(TEMP_ROOT), max_age, SystemTime::now())
}

/// Run speaker analysis through the isolated sibling helper process.
#[allow(clippy::too_many_arguments)]
#[cfg(unix)]
pub(crate) fn analyze_speakers(
    raw_path: &Path,
    full_audio: &[f32],
    statement_audio: &[f32],
    reduced_audio: Option<&[f32]>,
    statements_pre_restore: &[Map<String, Value>],
    statements_restored: &[Map<String, Value>],
    sample_rate: u32,
    min_statement_duration: f64,
) -> Result<SpeakerAnalyzeResult, SpeakerAnalyzeError> {
    let installation = validate_speakers_analyze_runtime().map_err(|error| {
        SpeakerAnalyzeError::new(
            raw_path,
            "request",
            error.message().unwrap_or("speakers-installation-failed"),
            None,
        )
    })?;
    let temporary = create_speakers_analyze_temp_dir(raw_path)
        .map_err(|error| SpeakerAnalyzeError::new(raw_path, "request", error.to_string(), None))?;

    with_cleaned_temp_dir(raw_path, &temporary, || {
        analyze_in_temp_dir(
            raw_path,
            &temporary,
            full_audio,
            statement_audio,
            reduced_audio,
            statements_pre_restore,
            statements_restored,
            sample_rate,
            min_statement_duration,
            &installation.wespeaker_model,
            &installation.pyannote_model,
            |request| {
                invoke_speakers_analyze_helper(
                    &installation.helper,
                    request,
                    raw_path,
                    SpeakersAnalyzeBudget::default(),
                )
            },
        )
    })
}

/// Windows has no admitted speaker-helper process transport yet. Refuse before
/// creating a temporary sidecar so the required capability is visibly degraded.
#[allow(clippy::too_many_arguments)]
#[cfg(not(unix))]
pub(crate) fn analyze_speakers(
    raw_path: &Path,
    _full_audio: &[f32],
    _statement_audio: &[f32],
    _reduced_audio: Option<&[f32]>,
    _statements_pre_restore: &[Map<String, Value>],
    _statements_restored: &[Map<String, Value>],
    _sample_rate: u32,
    _min_statement_duration: f64,
) -> Result<SpeakerAnalyzeResult, SpeakerAnalyzeError> {
    Err(SpeakerAnalyzeError::new(
        raw_path,
        "invoke",
        "platform-unsupported",
        None,
    ))
}

fn with_cleaned_temp_dir<T, F>(
    raw_path: &Path,
    temporary: &Path,
    action: F,
) -> Result<T, SpeakerAnalyzeError>
where
    F: FnOnce() -> Result<T, SpeakerAnalyzeError>,
{
    let result = action();
    let cleanup = fs::remove_dir_all(temporary);
    match result {
        Err(error) => Err(error),
        Ok(result) => cleanup.map(|()| result).map_err(|error| {
            SpeakerAnalyzeError::new(raw_path, "cleanup", error.to_string(), None)
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze_in_temp_dir<F>(
    raw_path: &Path,
    temporary: &Path,
    full_audio: &[f32],
    statement_audio: &[f32],
    reduced_audio: Option<&[f32]>,
    statements_pre_restore: &[Map<String, Value>],
    statements_restored: &[Map<String, Value>],
    sample_rate: u32,
    min_statement_duration: f64,
    wespeaker_model: &Path,
    pyannote_model: &Path,
    invoke: F,
) -> Result<SpeakerAnalyzeResult, SpeakerAnalyzeError>
where
    F: FnOnce(&[u8]) -> Result<HelperInvocationResult, SpeakerAnalyzeError>,
{
    let (request, payload_path) = build_request(
        raw_path,
        temporary,
        full_audio,
        reduced_audio,
        statements_pre_restore,
        statements_restored,
        sample_rate,
        wespeaker_model,
        pyannote_model,
    )?;
    let request_ids = statement_ids(raw_path, statements_pre_restore)?;
    let expected_ids = admitted_statement_ids(
        raw_path,
        statement_audio,
        statements_pre_restore,
        sample_rate,
        min_statement_duration,
    )?;
    let request = serde_json::to_vec(&request)
        .map_err(|error| SpeakerAnalyzeError::new(raw_path, "request", error.to_string(), None))?;
    let completed = invoke(&request)?;
    raise_for_returncode(raw_path, &completed)?;
    let response: Value = serde_json::from_str(&completed.stdout).map_err(|_error| {
        SpeakerAnalyzeError::new(raw_path, "parse", "malformed-response", None)
    })?;
    accepted_result_from_response(
        raw_path,
        &response,
        &payload_path,
        statements_restored,
        &expected_ids,
        &request_ids,
        sample_rate,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_request(
    raw_path: &Path,
    temporary: &Path,
    full_audio: &[f32],
    reduced_audio: Option<&[f32]>,
    statements_pre_restore: &[Map<String, Value>],
    statements_restored: &[Map<String, Value>],
    sample_rate: u32,
    wespeaker_model: &Path,
    pyannote_model: &Path,
) -> Result<(Value, PathBuf), SpeakerAnalyzeError> {
    let full_path = temporary.join("full-audio.f32le");
    write_audio_sidecar(raw_path, &full_path, full_audio)?;
    let reduced_path = reduced_audio
        .map(|audio| {
            let path = temporary.join("reduced-audio.f32le");
            write_audio_sidecar(raw_path, &path, audio).map(|()| path)
        })
        .transpose()?;
    let statement_spans = spans_from_statements(raw_path, statements_pre_restore)?;
    let diarization_spans = spans_from_statements(raw_path, statements_restored)?;
    ensure_span_parity(raw_path, &statement_spans, &diarization_spans)?;
    let payload_path = temporary.join("statement-embeddings.f32le");
    Ok((
        json!({
            "schema": REQUEST_SCHEMA,
            "sample_rate_hz": sample_rate,
            "full_audio_f32le_path": full_path,
            "reduced_audio_f32le_path": reduced_path,
            "output_payload_f32le_path": payload_path,
            "interval_embedding_payload_f32le_path": Value::Null,
            "models": {
                "pyannote_segmentation_onnx_path": pyannote_model,
                "wespeaker_onnx_path": wespeaker_model,
            },
            "statement_embedding": { "spans": statement_spans },
            "diarization": { "spans": diarization_spans },
        }),
        payload_path,
    ))
}

fn write_audio_sidecar(
    raw_path: &Path,
    path: &Path,
    audio: &[f32],
) -> Result<(), SpeakerAnalyzeError> {
    write_f32le_exclusive(path, audio).map_err(|error| {
        let error = remove_partial_sidecar(path, error);
        SpeakerAnalyzeError::new(raw_path, "request", error.to_string(), None)
    })
}

fn remove_partial_sidecar(path: &Path, error: AudioError) -> AudioError {
    if !matches!(error, AudioError::SidecarCreate { .. }) {
        let _ = fs::remove_file(path);
    }
    error
}

#[cfg(unix)]
pub(crate) fn invoke_speakers_analyze_helper(
    binary: &Path,
    request: &[u8],
    raw_path: &Path,
    budget: SpeakersAnalyzeBudget,
) -> Result<HelperInvocationResult, SpeakerAnalyzeError> {
    let authority = launch(
        Disposition::IndependentBoundedHelper {
            timeout: budget.timeout,
        },
        || {
            Command::new(binary)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()
        },
        speakers_terminate_fn(budget.terminate_grace),
    )
    .map_err(|error| SpeakerAnalyzeError::new(raw_path, "invoke", error.to_string(), None))?;
    invoke_child(authority, request, raw_path, budget)
}

#[cfg(not(unix))]
pub(crate) fn invoke_speakers_analyze_helper(
    binary: &Path,
    request: &[u8],
    raw_path: &Path,
    budget: SpeakersAnalyzeBudget,
) -> Result<HelperInvocationResult, SpeakerAnalyzeError> {
    let _ = (binary, request, budget);
    Err(SpeakerAnalyzeError::new(
        raw_path,
        "invoke",
        "platform-unsupported",
        None,
    ))
}

#[cfg(unix)]
fn process_group_exited(pgid: rustix::process::Pid) -> io::Result<bool> {
    loop {
        match rustix::process::waitid(
            rustix::process::WaitId::Pid(pgid),
            rustix::process::WaitIdOptions::EXITED
                | rustix::process::WaitIdOptions::NOHANG
                | rustix::process::WaitIdOptions::NOWAIT,
        ) {
            Ok(status) => return Ok(status.is_some()),
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
}

#[cfg(unix)]
fn signal_process_group(
    pgid: rustix::process::Pid,
    signal: rustix::process::Signal,
) -> io::Result<()> {
    match rustix::process::kill_process_group(pgid, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(unix)]
fn wait_for_process_group_exit(pgid: rustix::process::Pid, grace: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + grace;
    loop {
        if process_group_exited(pgid)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn speakers_terminate_fn(terminate_grace: Duration) -> BoxedTerminateFn {
    Box::new(move |child, _timeout| {
        let Some(pgid) = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            return child.kill().map_err(LaunchError::Terminate);
        };
        if process_group_exited(pgid).map_err(LaunchError::Terminate)? {
            return signal_process_group(pgid, rustix::process::Signal::KILL)
                .map_err(LaunchError::Terminate);
        }
        signal_process_group(pgid, rustix::process::Signal::TERM)
            .map_err(LaunchError::Terminate)?;
        if !wait_for_process_group_exit(pgid, terminate_grace).map_err(LaunchError::Terminate)? {
            signal_process_group(pgid, rustix::process::Signal::KILL)
                .map_err(LaunchError::Terminate)?;
        }
        Ok(())
    })
}

#[cfg(unix)]
struct SpeakerChild {
    authority: LaunchAuthority,
    pgid: rustix::process::Pid,
}

#[cfg(unix)]
impl SpeakerChild {
    fn new(authority: LaunchAuthority) -> io::Result<Self> {
        let pgid = i32::try_from(authority.pid())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| io::Error::other("invalid child pid"))?;
        Ok(Self { authority, pgid })
    }

    fn observe_exit(&mut self) -> io::Result<bool> {
        process_group_exited(self.pgid)
    }
}

#[cfg(unix)]
fn invoke_child(
    authority: LaunchAuthority,
    request: &[u8],
    raw_path: &Path,
    budget: SpeakersAnalyzeBudget,
) -> Result<HelperInvocationResult, SpeakerAnalyzeError> {
    let mut child = SpeakerChild::new(authority)
        .map_err(|error| SpeakerAnalyzeError::new(raw_path, "invoke", error.to_string(), None))?;
    let stdin = child
        .authority
        .take_stdin()
        .ok_or_else(|| SpeakerAnalyzeError::new(raw_path, "invoke", "stdin-unavailable", None))?;
    let stdout = child
        .authority
        .take_stdout()
        .ok_or_else(|| SpeakerAnalyzeError::new(raw_path, "invoke", "stdout-unavailable", None))?;
    let stderr = child
        .authority
        .take_stderr()
        .ok_or_else(|| SpeakerAnalyzeError::new(raw_path, "invoke", "stderr-unavailable", None))?;
    let (stdin_tx, stdin_rx) = mpsc::channel();
    let request = request.to_vec();
    thread::spawn(move || {
        let result: io::Result<()> = {
            let mut stdin = stdin;
            stdin.write_all(&request)
        };
        let _ = stdin_tx.send(result);
    });
    let stdout_rx = capture_stream(stdout, budget.stdout_limit_bytes);
    let stderr_rx = capture_stream(stderr, budget.stderr_limit_bytes);
    let mut stdout_capture = None;
    let mut stderr_capture = None;
    let deadline = Instant::now() + budget.timeout;
    let status = loop {
        if Instant::now() >= deadline {
            let _ = child.authority.terminate(budget.terminate_grace);
            let exit_code = child.authority.wait().ok();
            return Err(SpeakerAnalyzeError::new(
                raw_path, "invoke", "timeout", exit_code,
            ));
        }
        if let Some(capture) = poll_capture(&stdout_rx, "stdout", &mut child, budget, raw_path)? {
            stdout_capture = Some(capture);
        }
        if let Some(capture) = poll_capture(&stderr_rx, "stderr", &mut child, budget, raw_path)? {
            stderr_capture = Some(capture);
        }
        if child.observe_exit().map_err(|error| {
            SpeakerAnalyzeError::new(raw_path, "invoke", error.to_string(), None)
        })? {
            let _ = child.authority.terminate(budget.terminate_grace);
            break child.authority.wait().map_err(|error| {
                SpeakerAnalyzeError::new(raw_path, "invoke", error.to_string(), None)
            })?;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = receive_capture(stdout_capture, stdout_rx, raw_path, "stdout")?;
    let stderr = receive_capture(stderr_capture, stderr_rx, raw_path, "stderr")?;
    match stdin_rx.recv() {
        Ok(Ok(())) | Err(_) => {}
        Ok(Err(_error)) => {
            return Err(SpeakerAnalyzeError::new(
                raw_path,
                "invoke",
                "stdin-write-failed",
                Some(status),
            ));
        }
    }
    Ok(HelperInvocationResult {
        returncode: status,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

#[cfg(unix)]
fn capture_stream<R>(mut reader: R, limit: usize) -> Receiver<Result<Vec<u8>, CaptureError>>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 8192];
        let result = loop {
            match reader.read(&mut buffer) {
                Ok(0) => break Ok(output),
                Ok(count) if output.len().saturating_add(count) > limit => {
                    break Err(CaptureError::TooLarge);
                }
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) => break Err(CaptureError::Io(error)),
            }
        };
        let _ = sender.send(result);
    });
    receiver
}

#[cfg(unix)]
fn poll_capture(
    receiver: &Receiver<Result<Vec<u8>, CaptureError>>,
    stream: &str,
    child: &mut SpeakerChild,
    budget: SpeakersAnalyzeBudget,
    raw_path: &Path,
) -> Result<Option<Result<Vec<u8>, CaptureError>>, SpeakerAnalyzeError> {
    match receiver.try_recv() {
        Ok(Err(CaptureError::TooLarge)) => {
            let _ = child.authority.terminate(budget.terminate_grace);
            let exit_code = child.authority.wait().ok();
            Err(SpeakerAnalyzeError::new(
                raw_path,
                "invoke",
                format!("{stream}-too-large"),
                exit_code,
            ))
        }
        Ok(Err(CaptureError::Io(error))) => {
            let _ = child.authority.terminate(budget.terminate_grace);
            Err(SpeakerAnalyzeError::new(
                raw_path,
                "invoke",
                error.to_string(),
                None,
            ))
        }
        Ok(capture) => Ok(Some(capture)),
        Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => Ok(None),
    }
}

#[cfg(unix)]
fn receive_capture(
    captured: Option<Result<Vec<u8>, CaptureError>>,
    receiver: Receiver<Result<Vec<u8>, CaptureError>>,
    raw_path: &Path,
    stream: &str,
) -> Result<Vec<u8>, SpeakerAnalyzeError> {
    let capture = match captured {
        Some(capture) => capture,
        None => receiver.recv().map_err(|_| {
            SpeakerAnalyzeError::new(raw_path, "invoke", format!("{stream}-capture-failed"), None)
        })?,
    };
    match capture {
        Ok(bytes) => Ok(bytes),
        Err(CaptureError::TooLarge) => Err(SpeakerAnalyzeError::new(
            raw_path,
            "invoke",
            format!("{stream}-too-large"),
            None,
        )),
        Err(CaptureError::Io(error)) => Err(SpeakerAnalyzeError::new(
            raw_path,
            "invoke",
            error.to_string(),
            None,
        )),
    }
}

fn raise_for_returncode(
    raw_path: &Path,
    completed: &HelperInvocationResult,
) -> Result<(), SpeakerAnalyzeError> {
    if completed.returncode == 0 {
        return Ok(());
    }
    let reason = if completed.returncode < 0 {
        format!("signal-{}", completed.returncode.unsigned_abs())
    } else {
        helper_reason(&completed.stderr).unwrap_or_else(|| format!("exit-{}", completed.returncode))
    };
    Err(SpeakerAnalyzeError::new(
        raw_path,
        "invoke",
        reason,
        Some(completed.returncode),
    ))
}

fn helper_reason(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        let object = serde_json::from_str::<Value>(line)
            .ok()?
            .as_object()?
            .clone();
        (object.get("schema").and_then(Value::as_str) == Some(ERROR_SCHEMA))
            .then(|| {
                object
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten()
    })
}

fn spans_from_statements(
    raw_path: &Path,
    statements: &[Map<String, Value>],
) -> Result<Vec<Value>, SpeakerAnalyzeError> {
    let mut seen = BTreeSet::new();
    statements
        .iter()
        .map(|statement| {
            let id = statement_id(raw_path, statement)?;
            if !seen.insert(id) {
                return Err(SpeakerAnalyzeError::new(
                    raw_path,
                    "request",
                    "duplicate-statement-id",
                    None,
                ));
            }
            Ok(json!({
                "statement_id": id,
                "start_s": optional_finite(statement.get("start")),
                "end_s": optional_finite(statement.get("end")),
            }))
        })
        .collect()
}

fn ensure_span_parity(
    raw_path: &Path,
    statement_spans: &[Value],
    diarization_spans: &[Value],
) -> Result<(), SpeakerAnalyzeError> {
    if statement_spans.len() != diarization_spans.len() {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "request",
            "span-parity-length",
            None,
        ));
    }
    for (left, right) in statement_spans.iter().zip(diarization_spans) {
        if left.get("statement_id") != right.get("statement_id") {
            return Err(SpeakerAnalyzeError::new(
                raw_path,
                "request",
                "span-parity-statement-id",
                None,
            ));
        }
    }
    Ok(())
}

fn admitted_statement_ids(
    raw_path: &Path,
    audio: &[f32],
    statements: &[Map<String, Value>],
    sample_rate: u32,
    min_statement_duration: f64,
) -> Result<Vec<i64>, SpeakerAnalyzeError> {
    if sample_rate == 0 || !min_statement_duration.is_finite() || min_statement_duration < 0.0 {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "request",
            "invalid-admission-input",
            None,
        ));
    }
    let duration = audio.len() as f64 / f64::from(sample_rate);
    let mut admitted = Vec::new();
    for statement in statements {
        let (Some(start), Some(end)) = (
            optional_finite(statement.get("start")),
            optional_finite(statement.get("end")),
        ) else {
            continue;
        };
        let start = start.clamp(0.0, duration);
        let end = end.clamp(0.0, duration);
        if end - start < min_statement_duration {
            continue;
        }
        let start_sample = (start * f64::from(sample_rate)) as usize;
        let end_sample = (end * f64::from(sample_rate)) as usize;
        if end_sample.saturating_sub(start_sample)
            < (min_statement_duration * f64::from(sample_rate)) as usize
        {
            continue;
        }
        admitted.push(statement_id(raw_path, statement)?);
    }
    if admitted.len() != admitted.iter().collect::<BTreeSet<_>>().len() {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "request",
            "duplicate-admitted-statement-id",
            None,
        ));
    }
    Ok(admitted)
}

fn accepted_result_from_response(
    raw_path: &Path,
    response: &Value,
    payload_path: &Path,
    statements_restored: &[Map<String, Value>],
    expected_ids: &[i64],
    request_ids: &[i64],
    sample_rate: u32,
) -> Result<SpeakerAnalyzeResult, SpeakerAnalyzeError> {
    let object = response
        .as_object()
        .ok_or_else(|| SpeakerAnalyzeError::new(raw_path, "parse", "response-not-object", None))?;
    if object.get("schema").and_then(Value::as_str) != Some(RESPONSE_SCHEMA) {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "parse",
            "unknown-schema",
            None,
        ));
    }
    if object.get("sample_rate_hz").and_then(Value::as_u64) != Some(u64::from(sample_rate)) {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "sample-rate-mismatch",
            None,
        ));
    }
    for key in [
        "inputs",
        "statement_embeddings",
        "pyannote",
        "evidence",
        "diarization",
    ] {
        if !object.contains_key(key) {
            return Err(SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("missing-{}", reason_key(key)),
                None,
            ));
        }
    }
    validate_inputs(raw_path, object, request_ids)?;
    validate_pyannote(raw_path, object)?;
    validate_diarization_keys(raw_path, object)?;
    let embeddings = required_object(raw_path, object, "statement_embeddings")?;
    required_one_of(raw_path, embeddings, "audio_buffer", &["full", "reduced"])?;
    required_literal(raw_path, embeddings, "encoder", ENCODER_ID)?;
    required_literal(raw_path, embeddings, "payload_format", PAYLOAD_FORMAT)?;
    if embeddings.get("payload_path").and_then(Value::as_str) != payload_path.to_str() {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "invalid-payload-path",
            None,
        ));
    }
    required_literal(raw_path, embeddings, "dtype", PAYLOAD_DTYPE)?;
    let statement_ids = required_int_list(raw_path, embeddings, "statement_ids")?;
    if statement_ids.len() != statement_ids.iter().collect::<BTreeSet<_>>().len() {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "duplicate-statement-id",
            None,
        ));
    }
    if statement_ids.iter().any(|id| !request_ids.contains(id)) {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "foreign-statement-id",
            None,
        ));
    }
    if statement_ids != expected_ids {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "statement-id-divergence",
            None,
        ));
    }
    let durations = required_finite_float_list(raw_path, embeddings, "durations_s")?;
    let rows = statement_ids.len();
    if durations.len() != rows {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "duration-count-mismatch",
            None,
        ));
    }
    if embeddings.get("shape") != Some(&json!([rows, WESPEAKER_EMBEDDING_WIDTH])) {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "embedding-shape-mismatch",
            None,
        ));
    }
    let expected_bytes = rows
        .checked_mul(WESPEAKER_EMBEDDING_WIDTH)
        .and_then(|values| values.checked_mul(4))
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(raw_path, "payload", "embedding-byte-count-overflow", None)
        })?;
    if required_usize(raw_path, embeddings, "byte_count")? != expected_bytes {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "embedding-byte-count-mismatch",
            None,
        ));
    }
    if required_usize(raw_path, embeddings, "admitted_count")? != rows {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "embedding-admitted-count-mismatch",
            None,
        ));
    }
    if required_usize(raw_path, embeddings, "skipped_count")?
        != request_ids.len().saturating_sub(rows)
    {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "embedding-skipped-count-mismatch",
            None,
        ));
    }
    let payload = read_payload_bytes(raw_path, payload_path, expected_bytes)?;
    if !payload
        .chunks_exact(4)
        .all(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")).is_finite())
    {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "nonfinite-embedding",
            None,
        ));
    }
    let embedding_payload = (!statement_ids.is_empty()).then_some(SpeakerEmbeddingPayload {
        payload,
        statement_ids,
        durations_s: durations,
        encoder: ENCODER_ID.to_owned(),
    });
    let evidence = required_object(raw_path, object, "evidence")?;
    let speaker_evidence = required_string(raw_path, evidence, "speaker_evidence")?;
    if !matches!(speaker_evidence, "none" | "single" | "multi") {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "unknown-speaker-evidence",
            None,
        ));
    }
    let overlap_fraction = fraction(raw_path, evidence, "overlap_fraction")?;
    let speaker_evidence_multi_fraction = fraction(raw_path, evidence, "multi_window_fraction")?;
    fraction(raw_path, evidence, "mean_window_overlap_share")?;
    let labels = statement_labels(raw_path, object)?;
    let mut statements = statements_restored.to_vec();
    if let Some(labels) = &labels {
        if labels.len() != statements.len() {
            return Err(SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                "statement-label-count-mismatch",
                None,
            ));
        }
        for (statement, label) in statements.iter_mut().zip(labels) {
            if let Some(label) = label {
                statement.insert("speaker".to_owned(), Value::from(*label));
            }
        }
    }
    Ok(SpeakerAnalyzeResult {
        statements,
        embedding_payload,
        speaker_evidence: speaker_evidence.to_owned(),
        speaker_evidence_multi_fraction,
        overlap_fraction,
        statement_labels: labels,
    })
}

fn validate_inputs(
    raw_path: &Path,
    response: &Map<String, Value>,
    request_ids: &[i64],
) -> Result<(), SpeakerAnalyzeError> {
    let inputs = required_object(raw_path, response, "inputs")?;
    for section_name in ["statement_embedding", "diarization"] {
        let section = required_object(raw_path, inputs, section_name)?;
        if required_int_list(raw_path, section, "statement_ids")? != request_ids {
            return Err(SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("{}-input-id-mismatch", reason_key(section_name)),
                None,
            ));
        }
        let spans = section
            .get("spans_s")
            .and_then(Value::as_array)
            .filter(|spans| spans.len() == request_ids.len())
            .ok_or_else(|| {
                SpeakerAnalyzeError::new(
                    raw_path,
                    "payload",
                    format!("invalid-{}-spans", reason_key(section_name)),
                    None,
                )
            })?;
        for span in spans {
            let values = span
                .as_array()
                .filter(|values| values.len() == 2)
                .ok_or_else(|| {
                    SpeakerAnalyzeError::new(
                        raw_path,
                        "payload",
                        format!("invalid-{}-spans", reason_key(section_name)),
                        None,
                    )
                })?;
            if values
                .iter()
                .any(|value| !value.is_null() && !optional_finite(Some(value)).is_some())
            {
                return Err(SpeakerAnalyzeError::new(
                    raw_path,
                    "payload",
                    format!("invalid-{}-spans", reason_key(section_name)),
                    None,
                ));
            }
        }
    }
    Ok(())
}

fn validate_pyannote(
    raw_path: &Path,
    response: &Map<String, Value>,
) -> Result<(), SpeakerAnalyzeError> {
    let pyannote = required_object(raw_path, response, "pyannote")?;
    let stats = pyannote
        .get("window_stats")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(raw_path, "payload", "invalid-pyannote-window-stats", None)
        })?;
    for stat in stats {
        let stat = stat.as_object().ok_or_else(|| {
            SpeakerAnalyzeError::new(raw_path, "payload", "invalid-pyannote-window-stats", None)
        })?;
        for key in ["speech_frames", "active_slot_count", "overlap_frames"] {
            if stat.get(key).and_then(Value::as_u64).is_none() {
                return Err(SpeakerAnalyzeError::new(
                    raw_path,
                    "payload",
                    "invalid-pyannote-window-stats",
                    None,
                ));
            }
        }
    }
    Ok(())
}

fn validate_diarization_keys(
    raw_path: &Path,
    response: &Map<String, Value>,
) -> Result<(), SpeakerAnalyzeError> {
    let diarization = required_object(raw_path, response, "diarization")?;
    for key in [
        "intervals",
        "valid_intervals",
        "interval_embeddings",
        "cluster_labels",
        "statement_labels",
        "silhouette_k",
        "effective_k",
    ] {
        if !diarization.contains_key(key) {
            return Err(SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("missing-diarization-{}", reason_key(key)),
                None,
            ));
        }
    }
    if !diarization["interval_embeddings"].is_null() {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "unexpected-interval-embeddings",
            None,
        ));
    }
    Ok(())
}

fn statement_labels(
    raw_path: &Path,
    response: &Map<String, Value>,
) -> Result<Option<Vec<Option<i64>>>, SpeakerAnalyzeError> {
    let diarization = required_object(raw_path, response, "diarization")?;
    let Some(value) = diarization.get("statement_labels") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let labels = value.as_array().ok_or_else(|| {
        SpeakerAnalyzeError::new(raw_path, "payload", "invalid-statement-labels", None)
    })?;
    let labels = labels
        .iter()
        .map(|value| match value {
            Value::Null => Ok(None),
            _ => value
                .as_i64()
                .filter(|label| *label > 0)
                .map(Some)
                .ok_or_else(|| {
                    SpeakerAnalyzeError::new(raw_path, "payload", "invalid-statement-labels", None)
                }),
        })
        .collect::<Result<Vec<Option<i64>>, _>>()?;
    Ok(Some(labels))
}

fn read_payload_bytes(
    raw_path: &Path,
    path: &Path,
    expected: usize,
) -> Result<Vec<u8>, SpeakerAnalyzeError> {
    let actual = fs::metadata(path)
        .map_err(|_error| {
            SpeakerAnalyzeError::new(raw_path, "payload", "embedding-payload-missing", None)
        })?
        .len();
    if actual != expected as u64 {
        return Err(SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            "embedding-payload-size-mismatch",
            None,
        ));
    }
    fs::read(path).map_err(|_error| {
        SpeakerAnalyzeError::new(raw_path, "payload", "embedding-payload-missing", None)
    })
}

fn required_object<'a>(
    raw_path: &Path,
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>, SpeakerAnalyzeError> {
    object.get(key).and_then(Value::as_object).ok_or_else(|| {
        SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            format!("missing-{}", reason_key(key)),
            None,
        )
    })
}

fn required_string<'a>(
    raw_path: &Path,
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, SpeakerAnalyzeError> {
    object.get(key).and_then(Value::as_str).ok_or_else(|| {
        SpeakerAnalyzeError::new(
            raw_path,
            "payload",
            format!("invalid-{}", reason_key(key)),
            None,
        )
    })
}

fn required_literal(
    raw_path: &Path,
    object: &Map<String, Value>,
    key: &str,
    expected: &str,
) -> Result<(), SpeakerAnalyzeError> {
    (object.get(key).and_then(Value::as_str) == Some(expected))
        .then_some(())
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("invalid-{}", reason_key(key)),
                None,
            )
        })
}

fn required_one_of(
    raw_path: &Path,
    object: &Map<String, Value>,
    key: &str,
    expected: &[&str],
) -> Result<(), SpeakerAnalyzeError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| expected.contains(&value))
        .then_some(())
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("invalid-{}", reason_key(key)),
                None,
            )
        })
}

fn required_int_list(
    raw_path: &Path,
    object: &Map<String, Value>,
    key: &str,
) -> Result<Vec<i64>, SpeakerAnalyzeError> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("invalid-{}", reason_key(key)),
                None,
            )
        })?
        .iter()
        .map(|value| {
            value.as_i64().ok_or_else(|| {
                SpeakerAnalyzeError::new(
                    raw_path,
                    "payload",
                    format!("invalid-{}", reason_key(key)),
                    None,
                )
            })
        })
        .collect()
}

fn required_finite_float_list(
    raw_path: &Path,
    object: &Map<String, Value>,
    key: &str,
) -> Result<Vec<f64>, SpeakerAnalyzeError> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("invalid-{}", reason_key(key)),
                None,
            )
        })?
        .iter()
        .map(|value| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    SpeakerAnalyzeError::new(
                        raw_path,
                        "payload",
                        format!("invalid-{}", reason_key(key)),
                        None,
                    )
                })
        })
        .collect()
}

fn required_usize(
    raw_path: &Path,
    object: &Map<String, Value>,
    key: &str,
) -> Result<usize, SpeakerAnalyzeError> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("invalid-{}", reason_key(key)),
                None,
            )
        })
}

fn fraction(
    raw_path: &Path,
    object: &Map<String, Value>,
    key: &str,
) -> Result<f64, SpeakerAnalyzeError> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
        .ok_or_else(|| {
            SpeakerAnalyzeError::new(
                raw_path,
                "payload",
                format!("invalid-{}", reason_key(key)),
                None,
            )
        })
}

fn statement_ids(
    raw_path: &Path,
    statements: &[Map<String, Value>],
) -> Result<Vec<i64>, SpeakerAnalyzeError> {
    statements
        .iter()
        .map(|statement| statement_id(raw_path, statement))
        .collect()
}

fn statement_id(
    raw_path: &Path,
    statement: &Map<String, Value>,
) -> Result<i64, SpeakerAnalyzeError> {
    statement
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| SpeakerAnalyzeError::new(raw_path, "request", "invalid-statement-id", None))
}

fn optional_finite(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn reason_key(key: &str) -> String {
    key.replace('_', "-")
}

fn sanitize_reason(reason: &str) -> String {
    let valid = !reason.is_empty()
        && reason
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        reason.to_owned()
    } else {
        "invalid-helper-reason".to_owned()
    }
}

fn create_speakers_analyze_temp_dir_in(
    raw_path: &Path,
    root: &Path,
    pid: u32,
) -> io::Result<PathBuf> {
    let day = raw_path
        .ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("x");
    let segment = raw_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("x");
    let source = raw_path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("x");
    let prefix = format!(
        "{TEMP_PREFIX}{}-{}-{}-{pid}-",
        safe_temp_part(day),
        safe_temp_part(segment),
        safe_temp_part(source)
    );
    let directory = tempfile::Builder::new().prefix(&prefix).tempdir_in(root)?;
    let path = directory.keep();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(TEMP_DIR_MODE))?;
    }
    Ok(path)
}

fn sweep_stale_speakers_analyze_dirs_at(root: &Path, max_age: Duration, now: SystemTime) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let stale = path.is_dir()
                && entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX)
                && entry
                    .metadata()
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .is_some_and(|modified| {
                        now.duration_since(modified).is_ok_and(|age| age > max_age)
                    });
            stale.then_some(path)
        })
        .filter(|path| {
            let _ = fs::remove_dir_all(path);
            !path.exists()
        })
        .count()
}

fn safe_temp_part(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    if cleaned.is_empty() {
        "x".to_owned()
    } else {
        cleaned
    }
}

#[derive(Debug)]
pub(crate) struct HelperInvocationResult {
    pub(crate) returncode: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[cfg(unix)]
#[derive(Debug)]
enum CaptureError {
    TooLarge,
    Io(io::Error),
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use serde_json::{Map, Value, json};

    use super::{
        RESPONSE_SCHEMA, SpeakerAnalyzeError, SpeakersAnalyzeBudget, accepted_result_from_response,
        admitted_statement_ids, create_speakers_analyze_temp_dir_in, remove_partial_sidecar,
        sweep_stale_speakers_analyze_dirs_at, with_cleaned_temp_dir,
    };
    use crate::TranscribeError;

    #[test]
    fn production_speaker_budget_preserves_timeout_caps_and_signal_graces() {
        assert_eq!(
            SpeakersAnalyzeBudget::default(),
            SpeakersAnalyzeBudget {
                timeout: Duration::from_secs(2400),
                stdout_limit_bytes: 1024 * 1024,
                stderr_limit_bytes: 64 * 1024,
                terminate_grace: Duration::from_secs(5),
                kill_grace: Duration::from_secs(5),
            }
        );
    }

    #[test]
    fn temporary_directory_has_expected_name_and_permissions() {
        let root = tempfile::tempdir().unwrap();
        let raw = Path::new("/journal/chronicle/20260101/segment/a source.wav");
        let path = create_speakers_analyze_temp_dir_in(raw, root.path(), 42).unwrap();

        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("solstone-speakers-analyze-chronicle-segment-a_source-42-")
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn stale_sweep_removes_old_directory_and_keeps_fresh_one() {
        let root = tempfile::tempdir().unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(100_000);
        let old = root.path().join("solstone-speakers-analyze-old");
        let fresh = root.path().join("solstone-speakers-analyze-fresh");
        let unrelated = root.path().join("other-dir");
        let leftover = root.path().join("readme.txt");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&fresh).unwrap();
        fs::create_dir(&unrelated).unwrap();
        fs::write(&leftover, b"keep").unwrap();
        fs::File::open(&old)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        fs::File::open(&fresh)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(now - Duration::from_secs(1)))
            .unwrap();

        assert_eq!(
            sweep_stale_speakers_analyze_dirs_at(root.path(), Duration::from_secs(86_400), now),
            1
        );
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(unrelated.exists());
        assert!(leftover.exists());
    }

    #[test]
    fn analysis_temp_directory_is_cleaned_after_injected_invocation_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let invocation = temporary.path().join("solstone-speakers-analyze-test");
        fs::create_dir(&invocation).unwrap();
        let raw = Path::new("input.wav");
        let result = with_cleaned_temp_dir(raw, &invocation, || {
            Err::<(), _>(SpeakerAnalyzeError::new(raw, "invoke", "injected", None))
        });
        assert!(result.is_err());
        assert!(!invocation.exists());
    }

    #[test]
    fn f32le_sidecar_is_written_exclusively() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("audio.f32le");
        solstone_core_observe_audio::write_f32le_exclusive(&path, &[1.0, -0.5]).unwrap();
        assert_eq!(fs::read(path).unwrap(), [0, 0, 128, 63, 0, 0, 0, 191]);
    }

    #[test]
    fn sidecar_write_failure_removes_partial_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial.f32le");
        fs::write(&path, b"partial").unwrap();
        let _ = remove_partial_sidecar(
            &path,
            solstone_core_observe_audio::AudioError::SidecarSync {
                path: path.clone(),
                source: std::io::Error::other("injected"),
            },
        );
        assert!(!path.exists());
    }

    #[test]
    fn admission_clamps_both_boundaries_and_drops_short_statement() {
        let raw = Path::new("input.wav");
        let statements = vec![statement(1, -1.0, 2.0), statement(2, 0.1, 0.15)];
        assert_eq!(
            admitted_statement_ids(raw, &[0.0; 16_000], &statements, 16_000, 0.1).unwrap(),
            vec![1]
        );
    }

    #[test]
    fn admission_rejects_duplicate_ids() {
        let raw = Path::new("input.wav");
        let statements = vec![statement(1, 0.0, 0.5), statement(1, 0.5, 1.0)];
        assert_eq!(
            admitted_statement_ids(raw, &[0.0; 16_000], &statements, 16_000, 0.1)
                .unwrap_err()
                .reason,
            "duplicate-admitted-statement-id"
        );
    }

    #[test]
    fn response_rejects_statement_id_divergence() {
        let directory = tempfile::tempdir().unwrap();
        let payload = directory.path().join("payload.f32le");
        fs::write(&payload, []).unwrap();
        let response = valid_response(&payload, &[7], 1024);
        assert_eq!(
            accepted_with_ids(&response, &payload, &[], &[7])
                .unwrap_err()
                .reason,
            "statement-id-divergence"
        );
    }

    #[test]
    fn response_checks_declared_embedding_byte_count() {
        let directory = tempfile::tempdir().unwrap();
        let payload = directory.path().join("payload.f32le");
        fs::write(&payload, [0_u8; 1024]).unwrap();
        let mut response = valid_response(&payload, &[1], 1024);
        response["statement_embeddings"]["byte_count"] = json!(4);
        assert_eq!(
            accepted(&response, &payload, &[1]).unwrap_err().reason,
            "embedding-byte-count-mismatch"
        );
    }

    #[test]
    fn response_checks_actual_embedding_payload_size() {
        let directory = tempfile::tempdir().unwrap();
        let payload = directory.path().join("payload.f32le");
        fs::write(&payload, [0_u8; 4]).unwrap();
        let response = valid_response(&payload, &[1], 1024);
        assert_eq!(
            accepted(&response, &payload, &[1]).unwrap_err().reason,
            "embedding-payload-size-mismatch"
        );
    }

    #[test]
    fn event_fields_use_verbatim_names_and_sanitize_reason() {
        let error = SpeakerAnalyzeError::new("input.wav", "invoke", "BAD reason", Some(75));
        let fields = error.event_fields();
        assert_eq!(fields["speaker_analysis_failure_path"], "native");
        assert_eq!(fields["speaker_analysis_failure_stage"], "invoke");
        assert_eq!(
            fields["speaker_analysis_failure_reason"],
            "invalid-helper-reason"
        );
        assert_eq!(fields["speaker_analysis_failure_native_exit_code"], 75);
        let known = SpeakerAnalyzeError::new("input.wav", "parse", "malformed-response", None);
        assert!(
            !known
                .event_fields()
                .contains_key("speaker_analysis_failure_native_exit_code")
        );
    }

    #[test]
    fn speaker_analysis_failures_always_exit_one() {
        for reason in ["timeout", "malformed-response", "exit-69"] {
            assert_eq!(
                TranscribeError::SpeakerAnalysis(SpeakerAnalyzeError::new(
                    "input.wav",
                    "invoke",
                    reason,
                    Some(69)
                ))
                .exit_code(),
                1
            );
        }
    }

    fn accepted(
        response: &Value,
        payload: &Path,
        ids: &[i64],
    ) -> Result<super::SpeakerAnalyzeResult, SpeakerAnalyzeError> {
        accepted_result_from_response(
            Path::new("input.wav"),
            response,
            payload,
            &[],
            ids,
            ids,
            16_000,
        )
    }

    fn accepted_with_ids(
        response: &Value,
        payload: &Path,
        expected_ids: &[i64],
        request_ids: &[i64],
    ) -> Result<super::SpeakerAnalyzeResult, SpeakerAnalyzeError> {
        accepted_result_from_response(
            Path::new("input.wav"),
            response,
            payload,
            &[],
            expected_ids,
            request_ids,
            16_000,
        )
    }

    fn statement(id: i64, start: f64, end: f64) -> Map<String, Value> {
        Map::from_iter([
            ("id".to_owned(), Value::from(id)),
            ("start".to_owned(), Value::from(start)),
            ("end".to_owned(), Value::from(end)),
        ])
    }

    fn valid_response(payload: &Path, ids: &[i64], byte_count: usize) -> Value {
        let spans = ids.iter().map(|_id| json!([0.0, 0.5])).collect::<Vec<_>>();
        json!({
            "schema": RESPONSE_SCHEMA, "sample_rate_hz": 16_000,
            "inputs": {
                "statement_embedding": {"statement_ids": ids, "spans_s": spans},
                "diarization": {"statement_ids": ids, "spans_s": ids.iter().map(|_| json!([0.0, 0.5])).collect::<Vec<_>>()},
            },
            "statement_embeddings": {
                "audio_buffer":"full", "encoder":"wespeaker-resnet34-256",
                "payload_format":"raw-f32le-row-major-v1", "payload_path": payload,
                "dtype":"float32-le", "statement_ids": ids,
                "durations_s": ids.iter().map(|_| 0.5).collect::<Vec<_>>(),
                "shape":[ids.len(), 256], "byte_count": byte_count,
                "admitted_count":ids.len(), "skipped_count":0,
            },
            "pyannote":{"window_stats":[]},
            "evidence":{"speaker_evidence":"none", "multi_window_fraction":0.0, "mean_window_overlap_share":0.0, "overlap_fraction":0.0},
            "diarization":{"intervals":null,"valid_intervals":null,"interval_embeddings":null,"cluster_labels":null,"statement_labels":null,"silhouette_k":null,"effective_k":null},
        })
    }
}
