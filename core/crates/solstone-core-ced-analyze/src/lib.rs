// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native ced.cpp sound-tag classification command contract.
//!
//! `solstone-core-ced-sys` `dlopen`s a dynamically-linked glibc shared object
//! (`libced.so`). A `musl-static`-lane process has no in-process dynamic
//! loader and can never satisfy that call -- see Brief D
//! (`vpe/workspace/archived/wave8-suze-owner-journal-burn-in-260831/brief-d-ced-out-of-process.md`)
//! for the root cause, measured on a shipped build. This crate is the
//! `zig-gnu-2.27` sibling that owns the boundary instead, mirroring
//! `solstone-core-speakers-analyze` and `solstone-core-vad-analyze`: a small
//! JSON request/response contract over stdin/stdout, invoked out of process
//! by `solstone-core-local` (readiness probing) and `solstone-core-sound-tags`
//! (actual classification during transcription).
//!
//! Two commands, dispatched by argv exactly like
//! `solstone-core-speakers-analyze`'s `discovery-cluster` token:
//! - bare invocation runs [`run_classify_request`]: open the engine and
//!   model once, classify every requested audio window, and report a
//!   per-window outcome so one bad window does not fail the whole file (the
//!   caller keeps the existing best-effort/aggregate behavior).
//! - `probe` runs [`run_probe_request`]: open the engine and load the model
//!   only. This is the whole of what today's in-process
//!   `CedLibrary::open` + `load_model` readiness check does; moving it here
//!   is the fix, not a redesign of what is checked.

use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_ced_sys::{CedContext, CedLibrary};

pub const PROBE_COMMAND: &str = "probe";
pub const REQUEST_SCHEMA: &str = "solstone-ced-request-v1";
pub const RESPONSE_SCHEMA: &str = "solstone-ced-response-v1";
pub const PROBE_REQUEST_SCHEMA: &str = "solstone-ced-probe-request-v1";
pub const PROBE_RESPONSE_SCHEMA: &str = "solstone-ced-probe-response-v1";
pub const ERROR_SCHEMA: &str = "solstone-ced-error-v1";
pub const USAGE: &str = "Usage: solstone-core-ced-analyze < request.json > response.json\n       solstone-core-ced-analyze probe < probe-request.json > probe-response.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Run,
    Probe,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    UnexpectedArgument { argument: String },
}

impl fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedArgument { argument } => {
                write!(formatter, "{USAGE}; unexpected argument {argument:?}")
            }
        }
    }
}

impl Error for UsageError {}

pub fn evaluate_args(args: &[OsString]) -> Result<Command, UsageError> {
    match args {
        [] => Ok(Command::Run),
        [argument] if argument == PROBE_COMMAND => Ok(Command::Probe),
        [argument, ..] => Err(UsageError::UnexpectedArgument {
            argument: argument.to_string_lossy().into_owned(),
        }),
    }
}

/// Errors from the classify and probe command contracts.
///
/// `LibraryUnloadable` covers every way `CedLibrary::open` can fail --
/// `dlopen` failure, a missing required symbol, and an ABI mismatch alike --
/// because every reader of this crate's readiness caller
/// (`solstone-core-local::install::ced_readiness`) collapses all three into
/// one `Unloadable` cause today; the detail string keeps the specific reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalyzeError {
    MalformedRequest {
        detail: String,
    },
    UnknownSchema {
        schema: String,
        expected: &'static str,
    },
    LibraryUnreadable {
        path: String,
    },
    LibraryUnloadable {
        path: String,
        detail: String,
    },
    ModelUnreadable {
        path: String,
    },
    ModelLoadFailed {
        path: String,
        detail: String,
    },
    AudioUnreadable {
        path: String,
        detail: String,
    },
    AudioInvalid {
        path: String,
        detail: String,
    },
    AudioNonFinite {
        path: String,
        index: usize,
    },
    WindowOutOfRange {
        index: usize,
        start_sample: usize,
        end_sample: usize,
        audio_len_samples: usize,
    },
    Internal {
        detail: String,
    },
}

impl AnalyzeError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::MalformedRequest { .. } => "malformed-request",
            Self::UnknownSchema { .. } => "unknown-schema",
            Self::LibraryUnreadable { .. } => "library-unreadable",
            Self::LibraryUnloadable { .. } => "library-unloadable",
            Self::ModelUnreadable { .. } => "model-unreadable",
            Self::ModelLoadFailed { .. } => "model-load-failed",
            Self::AudioUnreadable { .. } => "audio-unreadable",
            Self::AudioInvalid { .. } => "audio-invalid",
            Self::AudioNonFinite { .. } => "audio-non-finite",
            Self::WindowOutOfRange { .. } => "window-out-of-range",
            Self::Internal { .. } => "internal-error",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::MalformedRequest { .. } | Self::UnknownSchema { .. } => 64,
            Self::LibraryUnreadable { .. }
            | Self::LibraryUnloadable { .. }
            | Self::ModelUnreadable { .. }
            | Self::ModelLoadFailed { .. }
            | Self::AudioUnreadable { .. }
            | Self::AudioInvalid { .. }
            | Self::AudioNonFinite { .. }
            | Self::WindowOutOfRange { .. } => 69,
            Self::Internal { .. } => 75,
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::MalformedRequest { detail } => detail.clone(),
            Self::UnknownSchema { schema, expected } => {
                // Name the schema THIS invocation accepts, not both. The old
                // wording listed the rejected schema among the accepted ones,
                // which read as a contradiction and hid a real argv/schema
                // mismatch: a probe request sent to a bare (classify)
                // invocation was reported as "not X or Y" while being Y.
                format!("request schema {schema:?} is not {expected:?} for this invocation")
            }
            Self::LibraryUnreadable { path } => {
                format!("models.ced_library_path is missing or unreadable at {path:?}")
            }
            Self::LibraryUnloadable { path, detail } => {
                format!("ced engine {path:?} could not be loaded: {detail}")
            }
            Self::ModelUnreadable { path } => {
                format!("models.ced_model_path is missing or unreadable at {path:?}")
            }
            Self::ModelLoadFailed { path, detail } => {
                format!("ced model {path:?} could not be loaded: {detail}")
            }
            Self::AudioUnreadable { path, detail } => {
                format!("audio path {path:?} is unreadable: {detail}")
            }
            Self::AudioInvalid { path, detail } => {
                format!("audio path {path:?} is not raw little-endian f32 mono: {detail}")
            }
            Self::AudioNonFinite { path, index } => {
                format!("audio path {path:?} contains non-finite sample at index {index}")
            }
            Self::WindowOutOfRange {
                index,
                start_sample,
                end_sample,
                audio_len_samples,
            } => {
                format!(
                    "windows[{index}] range {start_sample}..{end_sample} is invalid for {audio_len_samples} decoded samples"
                )
            }
            Self::Internal { detail } => detail.clone(),
        }
    }
}

impl fmt::Display for AnalyzeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail())
    }
}

impl Error for AnalyzeError {}

pub fn error_json_line(reason: &str, detail: &str) -> String {
    serde_json::to_string(&json!({
        "schema": ERROR_SCHEMA,
        "reason": reason,
        "detail": detail,
    }))
    .expect("error JSON serialization")
}

pub fn error_line_for_usage(error: &UsageError) -> String {
    error_json_line("usage", &error.to_string())
}

pub fn error_line_for_analyze_error(error: &AnalyzeError) -> String {
    error_json_line(error.reason(), &error.detail())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelPaths {
    ced_library_path: String,
    ced_model_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeRequest {
    models: ModelPaths,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Window {
    start_sample: usize,
    end_sample: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassifyRequest {
    models: ModelPaths,
    audio_f32le_path: String,
    sample_rate_hz: i32,
    top_k: i32,
    windows: Vec<Window>,
}

/// Open the engine and load the model only; used by both commands.
///
/// This is exactly the pair of calls
/// `solstone-core-local::install::ced_readiness::probe_integrity_and_load`
/// used to make in-process before this crate existed.
fn open_and_load<'library>(
    library: &'library CedLibrary,
    model_path: &str,
) -> Result<CedContext<'library>, AnalyzeError> {
    if !Path::new(model_path).is_file() {
        return Err(AnalyzeError::ModelUnreadable {
            path: model_path.to_owned(),
        });
    }
    library
        .load_model(Path::new(model_path))
        .map_err(|error| AnalyzeError::ModelLoadFailed {
            path: model_path.to_owned(),
            detail: error.to_string(),
        })
}

fn open_library(library_path: &str) -> Result<CedLibrary, AnalyzeError> {
    if !Path::new(library_path).is_file() {
        return Err(AnalyzeError::LibraryUnreadable {
            path: library_path.to_owned(),
        });
    }
    CedLibrary::open(Path::new(library_path)).map_err(|error| AnalyzeError::LibraryUnloadable {
        path: library_path.to_owned(),
        detail: error.to_string(),
    })
}

/// Probe command: open the engine and load the model, report only whether
/// that succeeded. No audio is read or classified.
pub fn run_probe_request(input: &str) -> Result<Value, AnalyzeError> {
    let request = parse_probe_request(input)?;
    let library = open_library(&request.models.ced_library_path)?;
    let _context = open_and_load(&library, &request.models.ced_model_path)?;
    Ok(json!({
        "schema": PROBE_RESPONSE_SCHEMA,
        "ok": true,
    }))
}

/// Run (classify) command: open the engine and model once, then classify
/// every requested window. A single window's classify failure is reported
/// per-window, not as a whole-request failure, so the caller can keep
/// successful windows exactly as it does today.
pub fn run_classify_request(input: &str) -> Result<Value, AnalyzeError> {
    let request = parse_classify_request(input)?;
    let audio = read_audio_f32le(&request.audio_f32le_path)?;
    // Validate every window's structural shape against the decoded audio
    // before touching the engine at all: a doomed request should not pay for
    // a dlopen and model load first.
    for (index, window) in request.windows.iter().enumerate() {
        if window.start_sample > window.end_sample || window.end_sample > audio.len() {
            return Err(AnalyzeError::WindowOutOfRange {
                index,
                start_sample: window.start_sample,
                end_sample: window.end_sample,
                audio_len_samples: audio.len(),
            });
        }
    }
    let library = open_library(&request.models.ced_library_path)?;
    let context = open_and_load(&library, &request.models.ced_model_path)?;

    let mut windows = Vec::with_capacity(request.windows.len());
    for window in &request.windows {
        windows.push(classify_window(
            &context,
            &audio[window.start_sample..window.end_sample],
            request.sample_rate_hz,
            request.top_k,
        ));
    }
    Ok(json!({
        "schema": RESPONSE_SCHEMA,
        "windows": windows,
    }))
}

fn classify_window(
    context: &CedContext<'_>,
    samples: &[f32],
    sample_rate_hz: i32,
    top_k: i32,
) -> Value {
    match context.classify_pcm_json(samples, sample_rate_hz, top_k) {
        Ok(raw) => match parse_ced_tags(&raw) {
            Ok(tags) => json!({ "ok": true, "tags": tags }),
            Err(detail) => json!({
                "ok": false,
                "reason": "invalid-classify-output",
                "detail": detail,
            }),
        },
        Err(error) => json!({
            "ok": false,
            "reason": "classify-failed",
            "detail": error.to_string(),
        }),
    }
}

/// Parse ced.cpp's raw `[{"label":...,"score":...}, ...]` classify output.
///
/// Ported unchanged from `solstone-core-sound-tags::parse_classify_json`,
/// which owned this parsing while the classify call itself was in-process.
/// The `BTreeMap<String, f64>` output is the exact input shape
/// `solstone-core-sound-tags::aggregate` already takes, so the caller needs
/// no format-specific parsing of its own once this crate validates the wire.
fn parse_ced_tags(raw: &str) -> Result<BTreeMap<String, f64>, String> {
    let data: Value = serde_json::from_str(raw)
        .map_err(|error| format!("ced classify JSON was invalid: {error}"))?;
    let entries = data
        .as_array()
        .ok_or_else(|| "ced classify JSON must be an array".to_owned())?;
    let mut tags: BTreeMap<String, f64> = BTreeMap::new();
    for item in entries {
        let object = item
            .as_object()
            .ok_or_else(|| "ced classify JSON entries must be objects".to_owned())?;
        let label = object
            .get("label")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "ced classify JSON entry label must be a non-empty string".to_owned())?;
        let score = object
            .get("score")
            .and_then(Value::as_f64)
            .ok_or_else(|| "ced classify JSON entry score must be numeric".to_owned())?;
        tags.entry(label.to_owned())
            .and_modify(|current| *current = current.max(score))
            .or_insert(score);
    }
    Ok(tags)
}

fn parse_probe_request(input: &str) -> Result<ProbeRequest, AnalyzeError> {
    let value: Value =
        serde_json::from_str(input).map_err(|error| AnalyzeError::MalformedRequest {
            detail: format!("request body is not valid JSON: {error}"),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| malformed("request body must be a JSON object"))?;
    let schema = required_string(object, "schema")?;
    if schema != PROBE_REQUEST_SCHEMA {
        return Err(AnalyzeError::UnknownSchema {
            schema: schema.to_string(),
            expected: PROBE_REQUEST_SCHEMA,
        });
    }
    Ok(ProbeRequest {
        models: parse_models(object)?,
    })
}

fn parse_classify_request(input: &str) -> Result<ClassifyRequest, AnalyzeError> {
    let value: Value =
        serde_json::from_str(input).map_err(|error| AnalyzeError::MalformedRequest {
            detail: format!("request body is not valid JSON: {error}"),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| malformed("request body must be a JSON object"))?;
    let schema = required_string(object, "schema")?;
    if schema != REQUEST_SCHEMA {
        return Err(AnalyzeError::UnknownSchema {
            schema: schema.to_string(),
            expected: REQUEST_SCHEMA,
        });
    }
    let models = parse_models(object)?;
    let audio_f32le_path = required_string(object, "audio_f32le_path")?.to_string();
    let sample_rate_hz = required_i32(object, "sample_rate_hz")?;
    let top_k = required_i32(object, "top_k")?;
    let windows = parse_windows(object)?;
    Ok(ClassifyRequest {
        models,
        audio_f32le_path,
        sample_rate_hz,
        top_k,
        windows,
    })
}

fn parse_models(object: &Map<String, Value>) -> Result<ModelPaths, AnalyzeError> {
    let models = required_object(object, "models")?;
    Ok(ModelPaths {
        ced_library_path: required_string(models, "ced_library_path")?.to_string(),
        ced_model_path: required_string(models, "ced_model_path")?.to_string(),
    })
}

fn parse_windows(object: &Map<String, Value>) -> Result<Vec<Window>, AnalyzeError> {
    let windows = object
        .get("windows")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("windows must be an array"))?;
    windows
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let object = value
                .as_object()
                .ok_or_else(|| malformed(format!("windows[{index}] must be an object")))?;
            Ok(Window {
                start_sample: required_usize(object, "start_sample", index)?,
                end_sample: required_usize(object, "end_sample", index)?,
            })
        })
        .collect()
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Map<String, Value>, AnalyzeError> {
    object
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| malformed(format!("{field} must be an object")))
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, AnalyzeError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| malformed(format!("{field} must be a non-empty string")))
}

fn required_i32(object: &Map<String, Value>, field: &'static str) -> Result<i32, AnalyzeError> {
    let value = object
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| malformed(format!("{field} must be an integer")))?;
    i32::try_from(value).map_err(|_error| malformed(format!("{field} is out of range for i32")))
}

fn required_usize(
    object: &Map<String, Value>,
    field: &'static str,
    index: usize,
) -> Result<usize, AnalyzeError> {
    let value = object.get(field).and_then(Value::as_u64).ok_or_else(|| {
        malformed(format!(
            "windows[{index}].{field} must be a non-negative integer"
        ))
    })?;
    usize::try_from(value).map_err(|_error| {
        malformed(format!(
            "windows[{index}].{field} is out of range for usize"
        ))
    })
}

fn malformed(detail: impl Into<String>) -> AnalyzeError {
    AnalyzeError::MalformedRequest {
        detail: detail.into(),
    }
}

fn read_audio_f32le(path: &str) -> Result<Vec<f32>, AnalyzeError> {
    let mut file = fs::File::open(path).map_err(|error| AnalyzeError::AudioUnreadable {
        path: path.to_string(),
        detail: error.to_string(),
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| AnalyzeError::AudioUnreadable {
            path: path.to_string(),
            detail: error.to_string(),
        })?;
    if !bytes.len().is_multiple_of(4) {
        return Err(AnalyzeError::AudioInvalid {
            path: path.to_string(),
            detail: format!("byte length {} is not divisible by 4", bytes.len()),
        });
    }
    let mut audio = Vec::with_capacity(bytes.len() / 4);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let sample = f32::from_le_bytes(chunk.try_into().expect("four bytes"));
        if !sample.is_finite() {
            return Err(AnalyzeError::AudioNonFinite {
                path: path.to_string(),
                index,
            });
        }
        audio.push(sample);
    }
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;

    // No process-spawning helper lives here on purpose. The CI topology
    // validator's routine-boundary scanner (`solstone-core-repository-contracts::ci::
    // scan_routine_boundaries`) walks every `src/` file -- including
    // `#[cfg(test)]` code -- and refuses a `std::process::Command::new` call
    // reachable from the routine `--lib` unit harness (it does not walk
    // `tests/`, which is registered and gated separately). A `compile_stub`
    // helper here previously spawned `cc` and was flagged as
    // `tests::compile_stub::process`. Every test that needed a real compiled
    // `libced.so` -- and therefore a real subprocess to build it -- moved to
    // `tests/ced_oracles.rs`, which already has its own `compile_stub` and
    // spawns the real `solstone-core-ced-analyze` binary as a subprocess
    // anyway: `real_subprocess_probe_succeeds_against_a_loadable_stub`,
    // `real_subprocess_probe_reports_unloadable_for_a_wrong_abi_stub` (now
    // also asserting the ABI-mismatch detail text),
    // `real_subprocess_probe_reports_model_load_failed_for_null_load_marker`
    // (new), and `real_subprocess_classify_matches_the_two_window_contract`
    // (now also asserting the failed window's `reason`). Every assertion
    // that lived in the four removed tests
    // (`probe_wrong_abi_reports_library_unloadable`,
    // `probe_null_load_reports_model_load_failed`,
    // `probe_succeeds_against_a_loadable_stub_and_real_model_bytes`,
    // `classify_reports_per_window_success_and_failure_without_failing_the_request`)
    // still exists in `tests/ced_oracles.rs`; only the process boundary
    // moved to a target the scanner already excludes, along with the
    // in-process call becoming a real subprocess call to the compiled
    // binary -- a strictly stronger proof, not a weaker one, of the same
    // outcomes.

    fn request_string(value: Value) -> String {
        serde_json::to_string(&value).expect("request JSON")
    }

    fn write_f32le(path: &Path, values: &[f32]) {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(path, bytes).expect("write audio");
    }

    #[test]
    fn argv_accepts_bare_and_probe_and_rejects_unknown() {
        assert_eq!(evaluate_args(&[]), Ok(Command::Run));
        assert_eq!(
            evaluate_args(&[OsString::from(PROBE_COMMAND)]),
            Ok(Command::Probe)
        );
        let error = evaluate_args(&[OsString::from("--help")]).unwrap_err();
        assert!(error_line_for_usage(&error).contains("\"reason\":\"usage\""));
    }

    #[test]
    fn probe_request_with_unknown_schema_is_rejected() {
        let error = run_probe_request(&request_string(json!({
            "schema": "solstone-ced-probe-request-v2",
            "models": {"ced_library_path": "/x", "ced_model_path": "/y"},
        })))
        .unwrap_err();
        assert_eq!(error.reason(), "unknown-schema");
        assert_eq!(error.exit_code(), 64);
    }

    #[test]
    fn probe_missing_library_reports_library_unreadable() {
        let directory = tempfile::tempdir().unwrap();
        let error = run_probe_request(&request_string(json!({
            "schema": PROBE_REQUEST_SCHEMA,
            "models": {
                "ced_library_path": directory.path().join("missing.so"),
                "ced_model_path": directory.path().join("missing.gguf"),
            },
        })))
        .unwrap_err();
        assert_eq!(error.reason(), "library-unreadable");
        assert_eq!(error.exit_code(), 69);
    }

    #[test]
    fn probe_garbage_library_reports_library_unloadable() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("libced.so");
        fs::write(&library, b"not an ELF shared object").unwrap();
        let model = directory.path().join("model.gguf");
        fs::write(&model, b"model").unwrap();
        let error = run_probe_request(&request_string(json!({
            "schema": PROBE_REQUEST_SCHEMA,
            "models": {"ced_library_path": library, "ced_model_path": model},
        })))
        .unwrap_err();
        assert_eq!(error.reason(), "library-unloadable");
        assert_eq!(error.exit_code(), 69);
    }

    #[test]
    fn classify_request_with_unknown_schema_is_rejected() {
        let error = run_classify_request(&request_string(json!({
            "schema": "solstone-ced-request-v2",
            "models": {"ced_library_path": "/x", "ced_model_path": "/y"},
            "audio_f32le_path": "/z",
            "sample_rate_hz": 16000,
            "top_k": 0,
            "windows": [],
        })))
        .unwrap_err();
        assert_eq!(error.reason(), "unknown-schema");
        assert_eq!(error.exit_code(), 64);
    }

    #[test]
    fn classify_window_out_of_range_is_rejected_before_touching_a_missing_engine() {
        let directory = tempfile::tempdir().unwrap();
        let audio_path = directory.path().join("audio.f32le");
        write_f32le(&audio_path, &[0.0; 16_000]);
        let error = run_classify_request(&request_string(json!({
            "schema": REQUEST_SCHEMA,
            "models": {
                "ced_library_path": directory.path().join("missing.so"),
                "ced_model_path": directory.path().join("missing.gguf"),
            },
            "audio_f32le_path": audio_path,
            "sample_rate_hz": 16000,
            "top_k": 0,
            "windows": [{"start_sample": 0, "end_sample": 32_000}],
        })))
        .unwrap_err();
        // The audio is read and decoded before the engine is opened, so a
        // structurally-bad request surfaces its own problem (audio window
        // exceeds the decoded length) rather than reporting the missing
        // engine as though the window were fine.
        assert_eq!(error.reason(), "window-out-of-range");
    }

    #[test]
    fn parse_ced_tags_dedupes_a_label_to_its_max_score() {
        let tags =
            parse_ced_tags(r#"[{"label":"Music","score":0.2},{"label":"Music","score":0.7}]"#)
                .unwrap();
        assert_eq!(tags.get("Music"), Some(&0.7));
    }

    #[test]
    fn parse_ced_tags_rejects_empty_label() {
        let error = parse_ced_tags(r#"[{"label":"","score":0.2}]"#).unwrap_err();
        assert!(error.contains("non-empty string"), "{error}");
    }
}
