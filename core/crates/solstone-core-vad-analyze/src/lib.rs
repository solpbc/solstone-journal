// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg_attr(all(not(feature = "runtime"), not(test)), allow(dead_code))]

//! Native Silero VAD v6 analysis command contract.
//!
//! Production settings match the retired Python port: `threshold=0.3`,
//! `min_silence_duration_ms=1000`, and the `VadOptions` defaults for
//! `min_speech_duration_ms` (0) and `speech_pad_ms` (400). SessionOptions
//! are pinned as Rust source text only; there is no live Python reference.
//!
//! The wire contract is narrower than `VadOptions` on purpose. `neg_threshold`
//! is always derived (`max(threshold - 0.15, 0.01)`) and `max_speech_duration_s`
//! is always `inf`, so neither is expressible in a request and those branches
//! are not ported. The sampling rate is fixed at 16 kHz raw mono `f32le`.

#[cfg(feature = "runtime")]
use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
#[cfg(feature = "runtime")]
use std::fs;
#[cfg(feature = "runtime")]
use std::io::Read;
#[cfg(feature = "runtime")]
use std::path::Path;

#[cfg(feature = "runtime")]
use ort::ep::CPU;
#[cfg(feature = "runtime")]
use ort::session::Session;
#[cfg(feature = "runtime")]
use ort::value::{Tensor, TensorElementType, ValueType};
use serde_json::{Map, Value, json};
#[cfg(feature = "runtime")]
use sha2::{Digest, Sha256};

pub mod locate;

pub const REQUEST_SCHEMA: &str = "solstone-vad-request-v1";
pub const RESPONSE_SCHEMA: &str = "solstone-vad-response-v1";
pub const ERROR_SCHEMA: &str = "solstone-vad-error-v1";
pub const USAGE: &str = "Usage: solstone-core-vad-analyze < request.json > response.json";

/// Fixed capture sample rate; the contract carries no sample-rate field.
pub const SAMPLE_RATE_HZ: u32 = 16000;

/// sha256 of the only Silero VAD graph this helper accepts.
pub const SILERO_VAD_V6_SHA256: &str =
    "4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2";

const WINDOW_SIZE_SAMPLES: usize = 512;
const CONTEXT_SIZE_SAMPLES: usize = 64;
const ROW_SIZE_SAMPLES: usize = CONTEXT_SIZE_SAMPLES + WINDOW_SIZE_SAMPLES;
#[cfg(feature = "runtime")]
const LSTM_STATE_SIZE: usize = 128;
#[cfg(feature = "runtime")]
const ENCODER_BATCH_SIZE: usize = 10000;

#[cfg(feature = "runtime")]
const INPUT_AUDIO_NAME: &str = "input";
#[cfg(feature = "runtime")]
const INPUT_H_NAME: &str = "h";
#[cfg(feature = "runtime")]
const INPUT_C_NAME: &str = "c";
#[cfg(feature = "runtime")]
const OUTPUT_PROBS_NAME: &str = "speech_probs";
#[cfg(feature = "runtime")]
const OUTPUT_HN_NAME: &str = "hn";
#[cfg(feature = "runtime")]
const OUTPUT_CN_NAME: &str = "cn";

const DEFAULT_THRESHOLD: f64 = 0.3;
const DEFAULT_MIN_SPEECH_DURATION_MS: u32 = 0;
const DEFAULT_MIN_SILENCE_DURATION_MS: u32 = 1000;
const DEFAULT_SPEECH_PAD_MS: u32 = 400;

/// Where the loader records every shared object this process has mapped.
#[cfg(feature = "runtime")]
const PROCESS_MAPS_PATH: &str = "/proc/self/maps";
/// SONAME stem of the ONNX Runtime shared object the helper links against.
#[cfg(feature = "runtime")]
const ONNX_RUNTIME_LIBRARY_PREFIX: &str = "libonnxruntime.so.";

const OPTION_THRESHOLD: &str = "threshold";
const OPTION_MIN_SPEECH_DURATION_MS: &str = "min_speech_duration_ms";
const OPTION_MIN_SILENCE_DURATION_MS: &str = "min_silence_duration_ms";
const OPTION_SPEECH_PAD_MS: &str = "speech_pad_ms";

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

pub fn evaluate_args(args: &[OsString]) -> Result<(), UsageError> {
    match args {
        [] => Ok(()),
        [argument, ..] => Err(UsageError::UnexpectedArgument {
            argument: argument.to_string_lossy().into_owned(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VadError {
    MalformedRequest {
        detail: String,
    },
    UnknownSchema {
        schema: String,
    },
    ModelIdentityMismatch {
        path: String,
        expected: &'static str,
        actual: String,
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
    ModelUnreadable {
        path: String,
        detail: String,
    },
    ModelInvalid {
        path: String,
        detail: String,
    },
    ModelIoMismatch {
        detail: String,
    },
    ProviderUnavailable {
        detail: String,
    },
    OnnxRuntime {
        detail: String,
    },
    Internal {
        detail: String,
    },
}

impl VadError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::MalformedRequest { .. } => "malformed-request",
            Self::UnknownSchema { .. } => "unknown-schema",
            Self::ModelIdentityMismatch { .. } => "model-identity-mismatch",
            Self::AudioUnreadable { .. } => "audio-unreadable",
            Self::AudioInvalid { .. } => "audio-invalid",
            Self::AudioNonFinite { .. } => "audio-non-finite",
            Self::ModelUnreadable { .. } => "model-unreadable",
            Self::ModelInvalid { .. } => "model-invalid",
            Self::ModelIoMismatch { .. } => "model-io-mismatch",
            Self::ProviderUnavailable { .. } => "provider-unavailable",
            Self::OnnxRuntime { .. } => "onnx-runtime-error",
            Self::Internal { .. } => "internal-error",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            // A structurally valid but wrong model is a caller/config error, not
            // a host-environment failure, so it shares the usage exit code.
            Self::MalformedRequest { .. }
            | Self::UnknownSchema { .. }
            | Self::ModelIdentityMismatch { .. } => 64,
            Self::AudioUnreadable { .. }
            | Self::AudioInvalid { .. }
            | Self::AudioNonFinite { .. }
            | Self::ModelUnreadable { .. }
            | Self::ModelInvalid { .. }
            | Self::ModelIoMismatch { .. } => 69,
            Self::ProviderUnavailable { .. } | Self::OnnxRuntime { .. } | Self::Internal { .. } => {
                75
            }
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::MalformedRequest { detail } => detail.clone(),
            Self::UnknownSchema { schema } => {
                format!("request schema {schema:?} is not {REQUEST_SCHEMA:?}")
            }
            Self::ModelIdentityMismatch {
                path,
                expected,
                actual,
            } => format!(
                "model {path:?} has sha256 {actual}, not the pinned Silero VAD v6 digest {expected}"
            ),
            Self::AudioUnreadable { path, detail } => {
                format!("audio path {path:?} is unreadable: {detail}")
            }
            Self::AudioInvalid { path, detail } => {
                format!("audio path {path:?} is not raw little-endian f32 mono: {detail}")
            }
            Self::AudioNonFinite { path, index } => {
                format!("audio path {path:?} contains non-finite sample at index {index}")
            }
            Self::ModelUnreadable { path, detail } => {
                format!(
                    "models.silero_vad_onnx_path is missing or unreadable at {path:?}: {detail}"
                )
            }
            Self::ModelInvalid { path, detail } => {
                format!("model {path:?} could not be opened as an ONNX model: {detail}")
            }
            Self::ModelIoMismatch { detail } => {
                format!("model has unsupported ONNX input/output shape: {detail}")
            }
            Self::ProviderUnavailable { detail } => detail.clone(),
            Self::OnnxRuntime { detail } => detail.clone(),
            Self::Internal { detail } => detail.clone(),
        }
    }
}

impl fmt::Display for VadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail())
    }
}

impl Error for VadError {}

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

pub fn error_line_for_vad_error(error: &VadError) -> String {
    error_json_line(error.reason(), &error.detail())
}

/// Options reachable through the wire contract.
///
/// `neg_threshold` and `max_speech_duration_s` from the Python reference are
/// deliberately absent; see the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct VadOptions {
    pub threshold: f64,
    pub min_speech_duration_ms: u32,
    pub min_silence_duration_ms: u32,
    pub speech_pad_ms: u32,
}

impl Default for VadOptions {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            min_speech_duration_ms: DEFAULT_MIN_SPEECH_DURATION_MS,
            min_silence_duration_ms: DEFAULT_MIN_SILENCE_DURATION_MS,
            speech_pad_ms: DEFAULT_SPEECH_PAD_MS,
        }
    }
}

/// One speech span in unpadded source-sample indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechChunk {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Request {
    audio_f32le_path: String,
    silero_vad_onnx_path: String,
    min_speech_seconds: f64,
    options: VadOptions,
}

#[cfg(feature = "runtime")]
pub fn run_request(input: &str) -> Result<Value, VadError> {
    let request = parse_request(input)?;
    let audio = read_audio_f32le(&request.audio_f32le_path)?;
    let speech_probs = speech_probabilities(&audio, Path::new(&request.silero_vad_onnx_path))?;

    let speech = find_speech_timestamps(&speech_probs, audio.len(), &request.options);
    Ok(response_value(&request, audio.len(), &speech))
}

/// One Silero speech probability per encoder window for `audio`.
///
/// This is the probability path `run_request` runs; span extraction is the only
/// thing layered on top of it. Exposed so a probability oracle can be compared
/// against the raw per-window sequence rather than against post-processed spans,
/// where padding and merging would hide a per-window divergence.
#[cfg(feature = "runtime")]
pub fn speech_probabilities(audio: &[f32], model_path: &Path) -> Result<Vec<f32>, VadError> {
    check_model_identity(&model_path.to_string_lossy())?;
    let (rows, window_count) = windows_with_context(audio);
    let mut session = SileroVadSession::open(model_path)?;
    session.speech_probs(&rows, window_count)
}

/// Version of the ONNX Runtime shared object this process actually loaded.
///
/// ONNX Runtime reports its own version through `OrtApiBase::GetVersionString`,
/// a raw C function pointer, and the workspace forbids `unsafe_code`, so that
/// call is out of reach; `ort::info()` carries the build's git commit but no
/// version. What remains is the loader's own record: the linked
/// `libonnxruntime.so.1` SONAME resolves to a versioned real path, and
/// `/proc/self/maps` names that resolved path for every mapped object. Reading
/// it reports the library in force rather than a compile-time expectation.
#[cfg(feature = "runtime")]
pub fn loaded_onnx_runtime_version() -> Result<String, VadError> {
    let maps = fs::read_to_string(PROCESS_MAPS_PATH).map_err(|error| VadError::Internal {
        detail: format!("could not read {PROCESS_MAPS_PATH}: {error}"),
    })?;
    let versions = maps
        .lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .filter_map(|path| Path::new(path).file_name()?.to_str())
        .filter_map(|name| name.strip_prefix(ONNX_RUNTIME_LIBRARY_PREFIX))
        .filter(|version| is_dotted_release_version(version))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    match versions.len() {
        1 => Ok(versions.into_iter().next().expect("one version")),
        0 => Err(VadError::Internal {
            detail: format!(
                "no {ONNX_RUNTIME_LIBRARY_PREFIX}<version> mapping is present in \
                 {PROCESS_MAPS_PATH}; the ONNX Runtime version cannot be determined"
            ),
        }),
        _ => Err(VadError::Internal {
            detail: format!(
                "{PROCESS_MAPS_PATH} names more than one ONNX Runtime version: {versions:?}"
            ),
        }),
    }
}

/// True for a `major.minor.patch` string of decimal components.
///
/// The SONAME symlink `libonnxruntime.so.1` shares the version prefix, so a
/// looser test would report `1` as the runtime version.
fn is_dotted_release_version(version: &str) -> bool {
    let mut components = 0;
    for component in version.split('.') {
        if component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        components += 1;
    }
    components == 3
}

fn response_value(request: &Request, audio_length_samples: usize, speech: &[SpeechChunk]) -> Value {
    let sample_rate = f64::from(SAMPLE_RATE_HZ);
    let duration = audio_length_samples as f64 / sample_rate;
    // Matches solstone/observe/vad.py::run_vad: sum the sample spans of the
    // returned chunks, then compare the seconds against min_speech_seconds.
    let speech_samples: usize = speech.iter().map(|chunk| chunk.end - chunk.start).sum();
    let speech_duration = speech_samples as f64 / sample_rate;
    json!({
        "schema": RESPONSE_SCHEMA,
        "duration": duration,
        "min_speech_seconds": request.min_speech_seconds,
        "speech_duration": speech_duration,
        "has_speech": speech_duration >= request.min_speech_seconds,
        "speech": speech
            .iter()
            .map(|chunk| json!({"start": chunk.start, "end": chunk.end}))
            .collect::<Vec<_>>(),
    })
}

fn parse_request(input: &str) -> Result<Request, VadError> {
    let value: Value = serde_json::from_str(input).map_err(|error| VadError::MalformedRequest {
        detail: format!("request body is not valid JSON: {error}"),
    })?;
    let object = value
        .as_object()
        .ok_or_else(|| malformed("request body must be a JSON object"))?;
    let schema = required_string(object, "schema")?;
    if schema != REQUEST_SCHEMA {
        return Err(VadError::UnknownSchema {
            schema: schema.to_string(),
        });
    }

    let audio_f32le_path = required_string(object, "audio_f32le_path")?.to_string();
    let models = required_object(object, "models")?;
    let silero_vad_onnx_path = required_string(models, "silero_vad_onnx_path")?.to_string();
    let min_speech_seconds = required_f64(object, "min_speech_seconds")?;
    if min_speech_seconds < 0.0 {
        return Err(malformed("min_speech_seconds must be non-negative"));
    }
    let options = parse_options(object)?;

    Ok(Request {
        audio_f32le_path,
        silero_vad_onnx_path,
        min_speech_seconds,
        options,
    })
}

fn parse_options(object: &Map<String, Value>) -> Result<VadOptions, VadError> {
    let defaults = VadOptions::default();
    let options = match object.get("options") {
        None | Some(Value::Null) => return Ok(defaults),
        Some(value) => value
            .as_object()
            .ok_or_else(|| malformed("options must be an object"))?,
    };
    for key in options.keys() {
        if ![
            OPTION_THRESHOLD,
            OPTION_MIN_SPEECH_DURATION_MS,
            OPTION_MIN_SILENCE_DURATION_MS,
            OPTION_SPEECH_PAD_MS,
        ]
        .contains(&key.as_str())
        {
            return Err(malformed(format!("options has unknown field {key:?}")));
        }
    }

    Ok(VadOptions {
        threshold: optional_f64(options, OPTION_THRESHOLD, defaults.threshold)?,
        min_speech_duration_ms: optional_u32(
            options,
            OPTION_MIN_SPEECH_DURATION_MS,
            defaults.min_speech_duration_ms,
        )?,
        min_silence_duration_ms: optional_u32(
            options,
            OPTION_MIN_SILENCE_DURATION_MS,
            defaults.min_silence_duration_ms,
        )?,
        speech_pad_ms: optional_u32(options, OPTION_SPEECH_PAD_MS, defaults.speech_pad_ms)?,
    })
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a Map<String, Value>, VadError> {
    object
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| malformed(format!("{field} must be an object")))
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, VadError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| malformed(format!("{field} must be a non-empty string")))
}

fn required_f64(object: &Map<String, Value>, field: &'static str) -> Result<f64, VadError> {
    object
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| malformed(format!("{field} must be a finite number")))
}

fn optional_f64(
    object: &Map<String, Value>,
    field: &'static str,
    default: f64,
) -> Result<f64, VadError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(_) => required_f64(object, field)
            .map_err(|_error| malformed(format!("options.{field} must be a finite number"))),
    }
}

fn optional_u32(
    object: &Map<String, Value>,
    field: &'static str,
    default: u32,
) -> Result<u32, VadError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                malformed(format!(
                    "options.{field} must be a non-negative integer that fits in u32"
                ))
            }),
    }
}

fn malformed(detail: impl Into<String>) -> VadError {
    VadError::MalformedRequest {
        detail: detail.into(),
    }
}

#[cfg(feature = "runtime")]
fn read_audio_f32le(path: &str) -> Result<Vec<f32>, VadError> {
    let metadata = fs::metadata(path).map_err(|error| VadError::AudioUnreadable {
        path: path.to_string(),
        detail: error.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(VadError::AudioInvalid {
            path: path.to_string(),
            detail: "path is not a regular file".to_string(),
        });
    }
    let mut file = fs::File::open(path).map_err(|error| VadError::AudioUnreadable {
        path: path.to_string(),
        detail: error.to_string(),
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| VadError::AudioUnreadable {
            path: path.to_string(),
            detail: error.to_string(),
        })?;
    if !bytes.len().is_multiple_of(size_of::<f32>()) {
        return Err(VadError::AudioInvalid {
            path: path.to_string(),
            detail: format!("byte length {} is not divisible by 4", bytes.len()),
        });
    }
    if bytes.is_empty() {
        return Err(VadError::AudioInvalid {
            path: path.to_string(),
            detail: "audio is empty; VAD needs at least one sample".to_string(),
        });
    }
    let mut audio = Vec::with_capacity(bytes.len() / size_of::<f32>());
    for (index, chunk) in bytes.chunks_exact(size_of::<f32>()).enumerate() {
        let sample = f32::from_le_bytes(chunk.try_into().expect("four bytes"));
        if !sample.is_finite() {
            return Err(VadError::AudioNonFinite {
                path: path.to_string(),
                index,
            });
        }
        audio.push(sample);
    }
    Ok(audio)
}

#[cfg(feature = "runtime")]
fn check_model_identity(path: &str) -> Result<(), VadError> {
    let metadata = fs::metadata(path).map_err(|error| VadError::ModelUnreadable {
        path: path.to_string(),
        detail: error.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(VadError::ModelUnreadable {
            path: path.to_string(),
            detail: "path is not a regular file".to_string(),
        });
    }
    let mut file = fs::File::open(path).map_err(|error| VadError::ModelUnreadable {
        path: path.to_string(),
        detail: error.to_string(),
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| VadError::ModelUnreadable {
                path: path.to_string(),
                detail: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if actual != SILERO_VAD_V6_SHA256 {
        return Err(VadError::ModelIdentityMismatch {
            path: path.to_string(),
            expected: SILERO_VAD_V6_SHA256,
            actual,
        });
    }
    Ok(())
}

/// Builds the `(window_count, 576)` row-major encoder input from raw audio.
///
/// Reproduces `SileroVADModel.__call__` padding and context mechanics.
pub fn windows_with_context(audio: &[f32]) -> (Vec<f32>, usize) {
    // The reference pads with `window_size_samples - len % window_size_samples`,
    // so an exact multiple gets a full extra 512-sample window rather than none.
    let pad = WINDOW_SIZE_SAMPLES - audio.len() % WINDOW_SIZE_SAMPLES;
    let mut padded = Vec::with_capacity(audio.len() + pad);
    padded.extend_from_slice(audio);
    padded.resize(audio.len() + pad, 0.0);

    let window_count = padded.len() / WINDOW_SIZE_SAMPLES;
    // The reference takes `context` as a *view* into the batched audio, zeroes
    // its last row in place, and only then rotates it:
    //     context = batched_audio[..., -64:]; context[-1] = 0
    //     context = np.roll(context, 1, 0)
    // Because the write lands in the shared buffer, it has two visible effects:
    // the final window's own trailing 64 samples become zero, and the rotated
    // row 0 (the context for the first window) is that zeroed row.
    let tail = padded.len() - CONTEXT_SIZE_SAMPLES;
    padded[tail..].fill(0.0);

    let mut rows = vec![0.0_f32; window_count * ROW_SIZE_SAMPLES];
    for window in 0..window_count {
        let row = window * ROW_SIZE_SAMPLES;
        let window_start = window * WINDOW_SIZE_SAMPLES;
        if window > 0 {
            // np.roll(..., 1, 0): row i carries the previous window's tail.
            rows[row..row + CONTEXT_SIZE_SAMPLES]
                .copy_from_slice(&padded[window_start - CONTEXT_SIZE_SAMPLES..window_start]);
        }
        rows[row + CONTEXT_SIZE_SAMPLES..row + ROW_SIZE_SAMPLES]
            .copy_from_slice(&padded[window_start..window_start + WINDOW_SIZE_SAMPLES]);
    }
    (rows, window_count)
}

fn samples_for_ms(milliseconds: u32) -> i64 {
    // Exact for every u32 ms at 16 kHz: 16000 * ms / 1000 == 16 * ms.
    (i64::from(SAMPLE_RATE_HZ) * i64::from(milliseconds)) / 1000
}

/// Ports `get_speech_timestamps` for the contract's reachable option space.
pub fn find_speech_timestamps(
    speech_probs: &[f32],
    audio_length_samples: usize,
    options: &VadOptions,
) -> Vec<SpeechChunk> {
    let window_size_samples = WINDOW_SIZE_SAMPLES as i64;
    let min_speech_samples = samples_for_ms(options.min_speech_duration_ms);
    let speech_pad_samples = samples_for_ms(options.speech_pad_ms);
    let min_silence_samples = samples_for_ms(options.min_silence_duration_ms);
    let audio_length_samples = audio_length_samples as i64;
    // The contract has no explicit neg_threshold, so the reference default is
    // always the one in force.
    let neg_threshold = (options.threshold - 0.15).max(0.01);

    let mut triggered = false;
    let mut speeches: Vec<(i64, i64)> = Vec::new();
    let mut current_start: Option<i64> = None;
    // 0 doubles as "unset", exactly as the reference's falsy `temp_end`.
    let mut temp_end: i64 = 0;

    for (index, probability) in speech_probs.iter().enumerate() {
        // numpy promotes the float32 probability before comparing it against the
        // Python float threshold; compare in f64 for the same decisions.
        let probability = f64::from(*probability);
        let window_position = window_size_samples * index as i64;

        if probability >= options.threshold && temp_end != 0 {
            temp_end = 0;
            // The reference also maintains prev_end/next_start here. Both are
            // read only by the max_speech_duration_s branch, which this contract
            // cannot reach, so they are not ported.
        }

        if probability >= options.threshold && !triggered {
            triggered = true;
            current_start = Some(window_position);
            continue;
        }

        // The reference's max_speech_duration_s split branch is not ported:
        // max_speech_samples is `inf` for every request this contract accepts,
        // so `(512 * i) - start > max_speech_samples` is never true.

        if probability < neg_threshold && triggered {
            if temp_end == 0 {
                temp_end = window_position;
            }
            if window_position - temp_end < min_silence_samples {
                continue;
            }
            let start = current_start.expect("triggered speech always has a start");
            if temp_end - start > min_speech_samples {
                speeches.push((start, temp_end));
            }
            current_start = None;
            temp_end = 0;
            triggered = false;
        }
    }

    if let Some(start) = current_start
        && audio_length_samples - start > min_speech_samples
    {
        // The trailing segment closes on the unpadded audio length, never on the
        // zero-padded length the encoder saw.
        speeches.push((start, audio_length_samples));
    }

    for index in 0..speeches.len() {
        if index == 0 {
            speeches[0].0 = (speeches[0].0 - speech_pad_samples).max(0);
        }
        // The reference splits the inter-chunk silence in half when
        // `silence_duration < 2 * speech_pad_samples`. That branch is not ported:
        // closing a chunk requires at least `min_silence_samples` of silence plus
        // one more window, and at production settings 16000 + 512 already exceeds
        // `2 * speech_pad_samples` (12800), so only the symmetric branch runs.
        speeches[index].1 = (speeches[index].1 + speech_pad_samples).min(audio_length_samples);
        if index + 1 < speeches.len() {
            speeches[index + 1].0 = (speeches[index + 1].0 - speech_pad_samples).max(0);
        }
    }

    speeches
        .into_iter()
        .map(|(start, end)| SpeechChunk {
            start: start as usize,
            end: end as usize,
        })
        .collect()
}

#[cfg(feature = "runtime")]
struct SileroVadSession {
    session: Session,
}

#[cfg(feature = "runtime")]
impl SileroVadSession {
    fn open(model_path: &Path) -> Result<Self, VadError> {
        let builder = Session::builder().map_err(|error| VadError::ProviderUnavailable {
            detail: format!("ONNX Runtime session builder is unavailable: {error}"),
        })?;
        // Mirrors the reference session options: single-threaded intra/inter op
        // execution, CPU execution provider, CPU memory arena disabled.
        let mut builder = builder
            .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
            .map_err(session_option_error)?
            .with_intra_threads(1)
            .map_err(session_option_error)?
            .with_inter_threads(1)
            .map_err(session_option_error)?;
        let session =
            builder
                .commit_from_file(model_path)
                .map_err(|error| VadError::ModelInvalid {
                    path: model_path.to_string_lossy().into_owned(),
                    detail: error.to_string(),
                })?;
        check_session_io(&session)?;
        Ok(Self { session })
    }

    fn speech_probs(&mut self, rows: &[f32], window_count: usize) -> Result<Vec<f32>, VadError> {
        let mut hidden = vec![0.0_f32; LSTM_STATE_SIZE];
        let mut cell = vec![0.0_f32; LSTM_STATE_SIZE];
        let mut probabilities = Vec::with_capacity(window_count);
        let mut offset = 0;
        while offset < window_count {
            // The LSTM state deliberately carries across encoder batches; the
            // reference feeds each batch the previous batch's hn/cn.
            let batch = (window_count - offset).min(ENCODER_BATCH_SIZE);
            let slice = &rows[offset * ROW_SIZE_SAMPLES..(offset + batch) * ROW_SIZE_SAMPLES];
            let audio_input = tensor_2d(batch, ROW_SIZE_SAMPLES, slice)?;
            let hidden_input = tensor_state(&hidden)?;
            let cell_input = tensor_state(&cell)?;
            let mut outputs = self
                .session
                .run(ort::inputs![
                    INPUT_AUDIO_NAME => audio_input,
                    INPUT_H_NAME => hidden_input,
                    INPUT_C_NAME => cell_input,
                ])
                .map_err(|error| VadError::OnnxRuntime {
                    detail: format!("silero vad inference failed: {error}"),
                })?;
            let batch_probabilities = extract_values(&mut outputs, OUTPUT_PROBS_NAME, &[batch])?;
            probabilities.extend_from_slice(&batch_probabilities);
            hidden = extract_values(&mut outputs, OUTPUT_HN_NAME, &[1, 1, LSTM_STATE_SIZE])?;
            cell = extract_values(&mut outputs, OUTPUT_CN_NAME, &[1, 1, LSTM_STATE_SIZE])?;
            offset += batch;
        }
        Ok(probabilities)
    }
}

#[cfg(feature = "runtime")]
fn session_option_error<R>(error: ort::Error<R>) -> VadError {
    VadError::ProviderUnavailable {
        detail: format!("ONNX Runtime session options could not be applied: {error}"),
    }
}

#[cfg(feature = "runtime")]
fn tensor_2d(rows: usize, cols: usize, values: &[f32]) -> Result<Tensor<f32>, VadError> {
    Tensor::from_array(([rows, cols], values.to_vec().into_boxed_slice())).map_err(|error| {
        VadError::Internal {
            detail: format!("could not build a [{rows}, {cols}] input tensor: {error}"),
        }
    })
}

#[cfg(feature = "runtime")]
fn tensor_state(values: &[f32]) -> Result<Tensor<f32>, VadError> {
    Tensor::from_array((
        [1_usize, 1_usize, LSTM_STATE_SIZE],
        values.to_vec().into_boxed_slice(),
    ))
    .map_err(|error| VadError::Internal {
        detail: format!("could not build a [1, 1, {LSTM_STATE_SIZE}] state tensor: {error}"),
    })
}

#[cfg(feature = "runtime")]
fn extract_values(
    outputs: &mut ort::session::SessionOutputs<'_>,
    name: &'static str,
    expected_shape: &[usize],
) -> Result<Vec<f32>, VadError> {
    let output = outputs
        .remove(name)
        .ok_or_else(|| VadError::ModelIoMismatch {
            detail: format!("run produced no {name:?} output"),
        })?;
    let (shape, values) =
        output
            .try_extract_tensor::<f32>()
            .map_err(|error| VadError::ModelIoMismatch {
                detail: format!("{name:?} output is not a float32 tensor: {error}"),
            })?;
    let expected = expected_shape
        .iter()
        .map(|dimension| *dimension as i64)
        .collect::<Vec<_>>();
    if shape[..] != expected[..] {
        return Err(VadError::ModelIoMismatch {
            detail: format!("{name:?} output shape {shape} is not {expected_shape:?}"),
        });
    }
    Ok(values.to_vec())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "runtime")]
enum ExpectedDim {
    Any,
    Exact(i64),
}

#[cfg(feature = "runtime")]
fn check_session_io(session: &Session) -> Result<(), VadError> {
    let inputs = session.inputs();
    let outputs = session.outputs();
    if inputs.len() != 3 {
        return Err(VadError::ModelIoMismatch {
            detail: format!("expected three inputs, got {}", inputs.len()),
        });
    }
    if outputs.len() != 3 {
        return Err(VadError::ModelIoMismatch {
            detail: format!("expected three outputs, got {}", outputs.len()),
        });
    }
    let state_shape = [
        ExpectedDim::Exact(1),
        ExpectedDim::Exact(1),
        ExpectedDim::Exact(LSTM_STATE_SIZE as i64),
    ];
    check_tensor(
        "input",
        inputs[0].name(),
        inputs[0].dtype(),
        INPUT_AUDIO_NAME,
        &[
            ExpectedDim::Any,
            ExpectedDim::Exact(ROW_SIZE_SAMPLES as i64),
        ],
    )?;
    check_tensor(
        "input",
        inputs[1].name(),
        inputs[1].dtype(),
        INPUT_H_NAME,
        &state_shape,
    )?;
    check_tensor(
        "input",
        inputs[2].name(),
        inputs[2].dtype(),
        INPUT_C_NAME,
        &state_shape,
    )?;
    check_tensor(
        "output",
        outputs[0].name(),
        outputs[0].dtype(),
        OUTPUT_PROBS_NAME,
        &[ExpectedDim::Any],
    )?;
    check_tensor(
        "output",
        outputs[1].name(),
        outputs[1].dtype(),
        OUTPUT_HN_NAME,
        &state_shape,
    )?;
    check_tensor(
        "output",
        outputs[2].name(),
        outputs[2].dtype(),
        OUTPUT_CN_NAME,
        &state_shape,
    )?;
    Ok(())
}

#[cfg(feature = "runtime")]
fn check_tensor(
    label: &str,
    name: &str,
    value_type: &ValueType,
    expected_name: &str,
    expected_shape: &[ExpectedDim],
) -> Result<(), VadError> {
    if name != expected_name {
        return Err(VadError::ModelIoMismatch {
            detail: format!("{label} name {name:?} is not {expected_name:?}"),
        });
    }
    let ValueType::Tensor { ty, shape, .. } = value_type else {
        return Err(VadError::ModelIoMismatch {
            detail: format!("{label} {name:?} is not a tensor"),
        });
    };
    if *ty != TensorElementType::Float32 {
        return Err(VadError::ModelIoMismatch {
            detail: format!("{label} {name:?} is {ty}, not float32"),
        });
    }
    if shape.len() != expected_shape.len() {
        return Err(VadError::ModelIoMismatch {
            detail: format!("{label} {name:?} shape {shape} has wrong rank"),
        });
    }
    for (index, (actual, expected)) in shape.iter().zip(expected_shape).enumerate() {
        match expected {
            ExpectedDim::Any => {}
            ExpectedDim::Exact(value) if actual == value => {}
            ExpectedDim::Exact(value) => {
                return Err(VadError::ModelIoMismatch {
                    detail: format!("{label} {name:?} dim {index} is {actual}, not {value}"),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "full-tests")]
    use std::path::PathBuf;
    #[cfg(feature = "full-tests")]
    use std::process;
    #[cfg(feature = "full-tests")]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(feature = "full-tests")]
    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    #[cfg(feature = "full-tests")]
    fn committed_model_path() -> PathBuf {
        repo_root().join("core/models/assets/silero_vad_v6.onnx")
    }

    #[cfg(feature = "full-tests")]
    struct TestDir {
        root: PathBuf,
    }

    #[cfg(feature = "full-tests")]
    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "solstone-vad-analyze-test-{}-{nonce}",
                process::id()
            ));
            fs::create_dir(&root).expect("create test dir");
            Self { root }
        }

        fn path(&self, name: &str) -> String {
            self.root.join(name).to_string_lossy().into_owned()
        }
    }

    #[cfg(feature = "full-tests")]
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(feature = "full-tests")]
    fn write_f32le(path: &str, values: &[f32]) {
        let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(path, bytes).expect("write audio");
    }

    fn base_request(audio_path: &str, model_path: &str) -> Value {
        json!({
            "schema": REQUEST_SCHEMA,
            "audio_f32le_path": audio_path,
            "models": {"silero_vad_onnx_path": model_path},
            "min_speech_seconds": 0.5,
        })
    }

    fn request_string(value: Value) -> String {
        serde_json::to_string(&value).expect("request JSON")
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    mod routine_request {
        use super::*;

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_defaults_match_production_run_vad_settings() {
            let request = parse_request(&request_string(base_request("/tmp/a.f32", "/tmp/m.onnx")))
                .expect("ok");

            assert_eq!(
                request.options,
                VadOptions {
                    threshold: 0.3,
                    min_speech_duration_ms: 0,
                    min_silence_duration_ms: 1000,
                    speech_pad_ms: 400,
                }
            );
            assert_eq!(request.min_speech_seconds, 0.5);
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_options_override_individual_fields() {
            let mut value = base_request("/tmp/a.f32", "/tmp/m.onnx");
            value["options"] = json!({"threshold": 0.5, "speech_pad_ms": 0});

            let request = parse_request(&request_string(value)).expect("ok");

            assert_eq!(request.options.threshold, 0.5);
            assert_eq!(request.options.speech_pad_ms, 0);
            assert_eq!(request.options.min_silence_duration_ms, 1000);
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_with_unknown_schema_is_rejected() {
            let mut value = base_request("/tmp/a.f32", "/tmp/m.onnx");
            value["schema"] = json!("solstone-vad-request-v2");

            let error = parse_request(&request_string(value)).unwrap_err();

            assert_eq!(error.reason(), "unknown-schema");
            assert_eq!(error.exit_code(), 64);
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_body_that_is_not_json_is_rejected() {
            let error = parse_request("not json").unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
            assert_eq!(error.exit_code(), 64);
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_body_that_is_not_an_object_is_rejected() {
            let error = parse_request("[]").unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_without_model_path_is_rejected() {
            let mut value = base_request("/tmp/a.f32", "/tmp/m.onnx");
            value["models"] = json!({});

            let error = parse_request(&request_string(value)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
            assert!(error.detail().contains("silero_vad_onnx_path"));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_with_empty_audio_path_is_rejected() {
            let mut value = base_request("", "/tmp/m.onnx");
            value["audio_f32le_path"] = json!("");

            let error = parse_request(&request_string(value)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_with_negative_min_speech_seconds_is_rejected() {
            let mut value = base_request("/tmp/a.f32", "/tmp/m.onnx");
            value["min_speech_seconds"] = json!(-1.0);

            let error = parse_request(&request_string(value)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_with_unknown_option_field_is_rejected() {
            let mut value = base_request("/tmp/a.f32", "/tmp/m.onnx");
            value["options"] = json!({"min_silence_ms": 500});

            let error = parse_request(&request_string(value)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
            assert!(error.detail().contains("min_silence_ms"));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_with_non_integer_option_is_rejected() {
            let mut value = base_request("/tmp/a.f32", "/tmp/m.onnx");
            value["options"] = json!({"speech_pad_ms": -5});

            let error = parse_request(&request_string(value)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_filesystem {
        use super::*;

        #[cfg(feature = "full-tests")]
        #[test]
        fn missing_audio_path_reports_audio_unreadable() {
            let dir = TestDir::new();

            let error = read_audio_f32le(&dir.path("missing.f32")).unwrap_err();

            assert_eq!(error.reason(), "audio-unreadable");
            assert_eq!(error.exit_code(), 69);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn directory_audio_path_reports_audio_invalid() {
            let dir = TestDir::new();
            let nested = dir.path("nested");
            fs::create_dir(&nested).expect("create dir");

            let error = read_audio_f32le(&nested).unwrap_err();

            assert_eq!(error.reason(), "audio-invalid");
            assert_eq!(error.exit_code(), 69);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn audio_byte_length_not_divisible_by_four_reports_audio_invalid() {
            let dir = TestDir::new();
            let path = dir.path("odd.f32");
            fs::write(&path, [0_u8, 1, 2]).expect("write");

            let error = read_audio_f32le(&path).unwrap_err();

            assert_eq!(error.reason(), "audio-invalid");
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn zero_length_audio_reports_audio_invalid() {
            let dir = TestDir::new();
            let path = dir.path("empty.f32");
            fs::write(&path, []).expect("write");

            let error = read_audio_f32le(&path).unwrap_err();

            assert_eq!(error.reason(), "audio-invalid");
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn non_finite_audio_sample_reports_audio_non_finite() {
            let dir = TestDir::new();
            let path = dir.path("nan.f32");
            write_f32le(&path, &[0.0, 0.5, f32::NAN]);

            let error = read_audio_f32le(&path).unwrap_err();

            assert_eq!(error.reason(), "audio-non-finite");
            assert_eq!(error.exit_code(), 69);
            assert!(error.detail().contains("index 2"));
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn missing_model_reports_model_unreadable() {
            let dir = TestDir::new();

            let error = check_model_identity(&dir.path("missing.onnx")).unwrap_err();

            assert_eq!(error.reason(), "model-unreadable");
            assert_eq!(error.exit_code(), 69);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn wrong_model_bytes_report_model_identity_mismatch() {
            let dir = TestDir::new();
            let path = dir.path("wrong.onnx");
            fs::write(&path, b"not the silero graph").expect("write");

            let error = check_model_identity(&path).unwrap_err();

            assert_eq!(error.reason(), "model-identity-mismatch");
            assert_eq!(error.exit_code(), 64);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn committed_silero_model_matches_the_pinned_digest() {
            check_model_identity(&committed_model_path().to_string_lossy()).expect("pinned digest");
        }
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    mod routine_algorithms {
        use super::*;

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn exact_multiple_audio_length_pads_a_full_extra_window() {
            let audio = vec![1.0_f32; 2 * WINDOW_SIZE_SAMPLES];

            let (rows, window_count) = windows_with_context(&audio);

            assert_eq!(window_count, 3);
            assert_eq!(rows.len(), 3 * ROW_SIZE_SAMPLES);
            // The padded third window is all zeros.
            assert!(
                rows[2 * ROW_SIZE_SAMPLES + CONTEXT_SIZE_SAMPLES..3 * ROW_SIZE_SAMPLES]
                    .iter()
                    .all(|value| *value == 0.0)
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn partial_final_window_pads_only_the_remainder() {
            let audio = vec![1.0_f32; WINDOW_SIZE_SAMPLES + 100];

            let (_rows, window_count) = windows_with_context(&audio);

            assert_eq!(window_count, 2);
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn first_window_context_is_zero_and_later_context_is_the_previous_tail() {
            let audio: Vec<f32> = (0..3 * WINDOW_SIZE_SAMPLES).map(|i| i as f32).collect();

            let (rows, window_count) = windows_with_context(&audio);

            assert_eq!(window_count, 4);
            assert!(
                rows[0..CONTEXT_SIZE_SAMPLES]
                    .iter()
                    .all(|value| *value == 0.0)
            );
            for window in 1..window_count {
                let row = window * ROW_SIZE_SAMPLES;
                let previous = (window - 1) * ROW_SIZE_SAMPLES;
                assert_eq!(
                    rows[row..row + CONTEXT_SIZE_SAMPLES],
                    rows[previous + ROW_SIZE_SAMPLES - CONTEXT_SIZE_SAMPLES
                        ..previous + ROW_SIZE_SAMPLES]
                );
            }
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn in_place_context_zeroing_clears_the_final_window_tail() {
            // The reference's `context[-1] = 0` writes through a view of the padded
            // audio, so the last window's own trailing 64 samples are zeroed before
            // the encoder ever sees them.
            let audio: Vec<f32> = vec![1.0; 2 * WINDOW_SIZE_SAMPLES - 1];

            let (rows, window_count) = windows_with_context(&audio);

            assert_eq!(window_count, 2);
            let last_window = ROW_SIZE_SAMPLES + CONTEXT_SIZE_SAMPLES;
            let tail_start = 2 * ROW_SIZE_SAMPLES - CONTEXT_SIZE_SAMPLES;
            assert!(
                rows[tail_start..2 * ROW_SIZE_SAMPLES]
                    .iter()
                    .all(|value| *value == 0.0)
            );
            // Samples before the zeroed tail are untouched.
            assert_eq!(rows[last_window], 1.0);
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn single_window_audio_gets_zero_context_and_zeroed_tail() {
            let audio = vec![1.0_f32; 200];

            let (rows, window_count) = windows_with_context(&audio);

            assert_eq!(window_count, 1);
            assert!(
                rows[0..CONTEXT_SIZE_SAMPLES]
                    .iter()
                    .all(|value| *value == 0.0)
            );
            assert!(
                rows[ROW_SIZE_SAMPLES - CONTEXT_SIZE_SAMPLES..ROW_SIZE_SAMPLES]
                    .iter()
                    .all(|value| *value == 0.0)
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn neg_threshold_default_tracks_threshold_minus_fifteen_hundredths() {
            // A probability in [neg_threshold, threshold) keeps a triggered segment
            // open, which is the only way the derived neg_threshold is observable.
            let options = VadOptions {
                threshold: 0.3,
                min_speech_duration_ms: 0,
                min_silence_duration_ms: 1000,
                speech_pad_ms: 0,
            };
            let mut probs = vec![0.9_f32];
            probs.extend(std::iter::repeat_n(0.2_f32, 60));

            let speech = find_speech_timestamps(&probs, 61 * WINDOW_SIZE_SAMPLES, &options);

            assert_eq!(
                speech,
                vec![SpeechChunk {
                    start: 0,
                    end: 61 * WINDOW_SIZE_SAMPLES,
                }]
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn probability_below_neg_threshold_for_long_enough_closes_the_segment() {
            let options = VadOptions {
                threshold: 0.3,
                min_speech_duration_ms: 0,
                min_silence_duration_ms: 1000,
                speech_pad_ms: 0,
            };
            let mut probs = vec![0.9_f32; 10];
            probs.extend(std::iter::repeat_n(0.0_f32, 60));
            probs.extend(std::iter::repeat_n(0.9_f32, 10));

            let speech = find_speech_timestamps(&probs, 80 * WINDOW_SIZE_SAMPLES, &options);

            assert_eq!(
                speech,
                vec![
                    SpeechChunk {
                        start: 0,
                        end: 10 * WINDOW_SIZE_SAMPLES,
                    },
                    SpeechChunk {
                        start: 70 * WINDOW_SIZE_SAMPLES,
                        end: 80 * WINDOW_SIZE_SAMPLES,
                    },
                ]
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn silence_shorter_than_min_silence_does_not_split_a_segment() {
            let options = VadOptions {
                threshold: 0.3,
                min_speech_duration_ms: 0,
                min_silence_duration_ms: 1000,
                speech_pad_ms: 0,
            };
            let mut probs = vec![0.9_f32; 5];
            // 20 windows of silence is 10240 samples, under the 16000-sample floor.
            probs.extend(std::iter::repeat_n(0.0_f32, 20));
            probs.extend(std::iter::repeat_n(0.9_f32, 5));

            let speech = find_speech_timestamps(&probs, 30 * WINDOW_SIZE_SAMPLES, &options);

            assert_eq!(
                speech,
                vec![SpeechChunk {
                    start: 0,
                    end: 30 * WINDOW_SIZE_SAMPLES,
                }]
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn trailing_segment_and_padding_clamp_to_unpadded_audio_length() {
            let options = VadOptions::default();
            // 40 windows of probabilities against a shorter unpadded audio length.
            let probs = vec![0.9_f32; 40];
            let audio_length = 40 * WINDOW_SIZE_SAMPLES - 300;

            let speech = find_speech_timestamps(&probs, audio_length, &options);

            assert_eq!(
                speech,
                vec![SpeechChunk {
                    start: 0,
                    end: audio_length,
                }]
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn speech_pad_extends_both_sides_and_clamps_at_zero() {
            let options = VadOptions {
                threshold: 0.3,
                min_speech_duration_ms: 0,
                min_silence_duration_ms: 1000,
                speech_pad_ms: 400,
            };
            let mut probs = vec![0.0_f32; 40];
            probs.extend(std::iter::repeat_n(0.9_f32, 10));
            probs.extend(std::iter::repeat_n(0.0_f32, 60));
            let audio_length = 110 * WINDOW_SIZE_SAMPLES;

            let speech = find_speech_timestamps(&probs, audio_length, &options);

            assert_eq!(
                speech,
                vec![SpeechChunk {
                    start: 40 * WINDOW_SIZE_SAMPLES - 6400,
                    end: 50 * WINDOW_SIZE_SAMPLES + 6400,
                }]
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn min_speech_duration_filters_short_segments() {
            let options = VadOptions {
                threshold: 0.3,
                min_speech_duration_ms: 1000,
                min_silence_duration_ms: 1000,
                speech_pad_ms: 0,
            };
            let mut probs = vec![0.9_f32; 5];
            probs.extend(std::iter::repeat_n(0.0_f32, 60));

            let speech = find_speech_timestamps(&probs, 65 * WINDOW_SIZE_SAMPLES, &options);

            assert!(speech.is_empty());
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn all_silence_yields_no_speech() {
            let probs = vec![0.0_f32; 50];

            let speech =
                find_speech_timestamps(&probs, 50 * WINDOW_SIZE_SAMPLES, &VadOptions::default());

            assert!(speech.is_empty());
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn response_reports_speech_duration_and_has_speech_from_chunk_spans() {
            let request = Request {
                audio_f32le_path: "/tmp/a.f32".to_string(),
                silero_vad_onnx_path: "/tmp/m.onnx".to_string(),
                min_speech_seconds: 1.0,
                options: VadOptions::default(),
            };
            let speech = vec![
                SpeechChunk {
                    start: 0,
                    end: 8000,
                },
                SpeechChunk {
                    start: 16000,
                    end: 24000,
                },
            ];

            let response = response_value(&request, 48000, &speech);

            assert_eq!(response["schema"], json!(RESPONSE_SCHEMA));
            assert_eq!(response["duration"], json!(3.0));
            assert_eq!(response["speech_duration"], json!(1.0));
            assert_eq!(response["min_speech_seconds"], json!(1.0));
            assert_eq!(response["has_speech"], json!(true));
            assert_eq!(response["speech"][1]["start"], json!(16000));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn response_has_speech_is_false_below_the_minimum() {
            let request = Request {
                audio_f32le_path: "/tmp/a.f32".to_string(),
                silero_vad_onnx_path: "/tmp/m.onnx".to_string(),
                min_speech_seconds: 1.0,
                options: VadOptions::default(),
            };
            let speech = vec![SpeechChunk {
                start: 0,
                end: 15999,
            }];

            let response = response_value(&request, 48000, &speech);

            assert_eq!(response["has_speech"], json!(false));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn error_envelope_carries_schema_reason_and_detail() {
            let line = error_line_for_vad_error(&VadError::Internal {
                detail: "boom".to_string(),
            });
            let value: Value = serde_json::from_str(&line).expect("error JSON");

            assert_eq!(value["schema"], json!(ERROR_SCHEMA));
            assert_eq!(value["reason"], json!("internal-error"));
            assert_eq!(value["detail"], json!("boom"));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn argv_rejects_unexpected_arguments_as_usage() {
            let error = evaluate_args(&[OsString::from("--help")]).unwrap_err();
            let line = error_line_for_usage(&error);

            assert!(line.contains("\"reason\":\"usage\""));
            assert!(line.contains("Usage: solstone-core-vad-analyze"));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn argv_accepts_a_bare_invocation() {
            assert_eq!(evaluate_args(&[]), Ok(()));
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_native {
        use super::*;

        #[cfg(feature = "full-tests")]
        #[test]
        fn committed_silero_model_opens_with_the_expected_graph_io() {
            let session = SileroVadSession::open(&committed_model_path()).expect("session");

            assert_eq!(session.session.inputs().len(), 3);
            assert_eq!(session.session.outputs().len(), 3);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn silence_through_the_committed_model_reports_no_speech() {
            let audio = vec![0.0_f32; SAMPLE_RATE_HZ as usize];
            let (rows, window_count) = windows_with_context(&audio);
            let mut session = SileroVadSession::open(&committed_model_path()).expect("session");

            let probs = session.speech_probs(&rows, window_count).expect("probs");

            assert_eq!(probs.len(), window_count);
            assert!(probs.iter().all(|value| value.is_finite()));
            assert!(probs.iter().all(|value| *value < 0.3));
            assert!(find_speech_timestamps(&probs, audio.len(), &VadOptions::default()).is_empty());
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn run_request_on_silence_returns_a_well_formed_response() {
            let dir = TestDir::new();
            let audio_path = dir.path("silence.f32");
            write_f32le(&audio_path, &vec![0.0_f32; SAMPLE_RATE_HZ as usize]);
            let request = base_request(&audio_path, &committed_model_path().to_string_lossy());

            let response = run_request(&request_string(request)).expect("response");

            assert_eq!(response["schema"], json!(RESPONSE_SCHEMA));
            assert_eq!(response["duration"], json!(1.0));
            assert_eq!(response["speech_duration"], json!(0.0));
            assert_eq!(response["has_speech"], json!(false));
            assert_eq!(response["speech"], json!([]));
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn loaded_onnx_runtime_version_is_a_dotted_release_version() {
            // Opening a session forces the runtime to be in use before the mapping
            // table is read, so this cannot pass against an unloaded library.
            SileroVadSession::open(&committed_model_path()).expect("session");

            let version = loaded_onnx_runtime_version().expect("loaded ONNX Runtime version");

            assert!(
                is_dotted_release_version(&version),
                "{version:?} is not a major.minor.patch version"
            );
        }
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    mod routine_version {
        use super::*;

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn dotted_release_version_rejects_the_soname_suffix_and_junk() {
            assert!(is_dotted_release_version("1.25.0"));
            assert!(!is_dotted_release_version("1"));
            assert!(!is_dotted_release_version("1.25"));
            assert!(!is_dotted_release_version("1.25.0.1"));
            assert!(!is_dotted_release_version("1.25.0-rc1"));
            assert!(!is_dotted_release_version("1..0"));
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_preflight {
        use super::*;

        #[cfg(feature = "full-tests")]
        #[test]
        fn run_request_with_a_wrong_model_reports_identity_mismatch_before_opening_it() {
            let dir = TestDir::new();
            let audio_path = dir.path("silence.f32");
            write_f32le(&audio_path, &vec![0.0_f32; 1024]);
            let model_path = dir.path("wrong.onnx");
            fs::write(&model_path, b"not onnx").expect("write");
            let request = base_request(&audio_path, &model_path);

            let error = run_request(&request_string(request)).unwrap_err();

            assert_eq!(error.reason(), "model-identity-mismatch");
            assert_eq!(error.exit_code(), 64);
        }
    }
}
