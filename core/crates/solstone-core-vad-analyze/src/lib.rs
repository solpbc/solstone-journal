// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg_attr(all(not(feature = "runtime"), not(test)), allow(dead_code))]

//! Native FireRed offline voice activity detection.
//!
//! Mono 16 kHz float audio is quantized to PCM16 precision, converted to raw
//! Kaldi filterbanks and globally normalized before inference. Bounded model
//! calls include the graph's full temporal context; only owned frame outputs
//! are concatenated before the timestamp policy runs at a 10 ms stride.
//!
//! The wire contract derives its negative threshold as
//! `max(threshold - 0.15, 0.01)` and imposes no maximum speech duration.

#[cfg(all(feature = "runtime", not(windows)))]
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

mod features;
pub mod locate;

pub const REQUEST_SCHEMA: &str = "solstone-vad-request-v2";
pub const RESPONSE_SCHEMA: &str = "solstone-vad-response-v1";
pub const ERROR_SCHEMA: &str = "solstone-vad-error-v1";
pub const USAGE: &str = "Usage: solstone-core-vad-analyze < request.json > response.json";

/// Fixed capture sample rate; the contract carries no sample-rate field.
pub const SAMPLE_RATE_HZ: u32 = 16000;

/// SHA-256 of the admitted offline FireRed graph.
pub const FIRERED_VAD_SHA256: &str =
    "a226e68e695548754a9ab5480a55bbcb8522aa69111cb97e4f093ddce63ad5dc";

/// SHA-256 of the original CMVN bytes embedded into this helper.
pub const FIRERED_VAD_CMVN_SHA256: &str =
    "c87f6f13edf0f0ec7535ddfc9cc3387d9268cb234b70182d566c5e2edf3ca473";

const FRAME_SHIFT_SAMPLES: usize = 160;
const OWNED_FRAMES_PER_BLOCK: usize = 30000;
const LEFT_CONTEXT_FRAMES: usize = 152;
const RIGHT_CONTEXT_FRAMES: usize = 160;
#[cfg(feature = "runtime")]
const INPUT_FEATURES_NAME: &str = "feat";
#[cfg(feature = "runtime")]
const OUTPUT_PROBS_NAME: &str = "probs";

const DEFAULT_THRESHOLD: f64 = 0.3;
const DEFAULT_MIN_SPEECH_DURATION_MS: u32 = 0;
const DEFAULT_MIN_SILENCE_DURATION_MS: u32 = 1000;
const DEFAULT_SPEECH_PAD_MS: u32 = 400;

/// Where the loader records every shared object this process has mapped.
#[cfg(all(feature = "runtime", not(windows)))]
const PROCESS_MAPS_PATH: &str = "/proc/self/maps";
/// SONAME stem of the ONNX Runtime shared object the helper links against.
#[cfg(all(feature = "runtime", not(windows)))]
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
                "model asset {path:?} has sha256 {actual}, not the pinned VAD asset digest {expected}"
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
                    "models.firered_vad_onnx_path is missing or unreadable at {path:?}: {detail}"
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
/// Negative threshold is derived from `threshold`; maximum duration is unbounded.
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

/// One speech span in original source-sample indices, including speech padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechChunk {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq)]
struct Request {
    audio_f32le_path: String,
    firered_vad_onnx_path: String,
    min_speech_seconds: f64,
    options: VadOptions,
}

#[cfg(feature = "runtime")]
pub fn run_request(input: &str) -> Result<Value, VadError> {
    let request = parse_request(input)?;
    let audio = read_audio_f32le(&request.audio_f32le_path)?;
    let speech_probs = speech_probabilities(&audio, Path::new(&request.firered_vad_onnx_path))?;

    let speech = find_speech_timestamps(&speech_probs, audio.len(), &request.options);
    Ok(response_value(&request, audio.len(), &speech))
}

/// One FireRed speech probability per complete 25 ms frame at a 10 ms stride.
///
/// The same path backs `run_request`. No state persists between recordings.
#[cfg(feature = "runtime")]
pub fn speech_probabilities(audio: &[f32], model_path: &Path) -> Result<Vec<f32>, VadError> {
    check_model_identity(&model_path.to_string_lossy())?;
    let features = features::prepare_features(audio)?;
    if features.frames() == 0 {
        return Ok(Vec::new());
    }
    let mut session = FireRedVadSession::open(model_path)?;
    session.speech_probs(&features)
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
#[cfg(all(feature = "runtime", not(windows)))]
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
#[cfg(any(not(windows), test))]
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
    let firered_vad_onnx_path = required_string(models, "firered_vad_onnx_path")?.to_string();
    let min_speech_seconds = required_f64(object, "min_speech_seconds")?;
    if min_speech_seconds < 0.0 {
        return Err(malformed("min_speech_seconds must be non-negative"));
    }
    let options = parse_options(object)?;

    Ok(Request {
        audio_f32le_path,
        firered_vad_onnx_path,
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
    if actual != FIRERED_VAD_SHA256 {
        return Err(VadError::ModelIdentityMismatch {
            path: path.to_string(),
            expected: FIRERED_VAD_SHA256,
            actual,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeatureBlock {
    owned: std::ops::Range<usize>,
    context: std::ops::Range<usize>,
}

fn feature_blocks(frames: usize) -> impl Iterator<Item = FeatureBlock> {
    (0..frames)
        .step_by(OWNED_FRAMES_PER_BLOCK)
        .map(move |first| {
            let end = first + (frames - first).min(OWNED_FRAMES_PER_BLOCK);
            FeatureBlock {
                owned: first..end,
                context: first.saturating_sub(LEFT_CONTEXT_FRAMES)
                    ..end.saturating_add(RIGHT_CONTEXT_FRAMES).min(frames),
            }
        })
}

fn samples_for_ms(milliseconds: u32) -> i64 {
    // Exact for every u32 ms at 16 kHz: 16000 * ms / 1000 == 16 * ms.
    (i64::from(SAMPLE_RATE_HZ) * i64::from(milliseconds)) / 1000
}

/// Applies the timestamp state machine to the complete recording's frame posteriors.
pub fn find_speech_timestamps(
    speech_probs: &[f32],
    audio_length_samples: usize,
    options: &VadOptions,
) -> Vec<SpeechChunk> {
    let window_size_samples = FRAME_SHIFT_SAMPLES as i64;
    let min_speech_samples = samples_for_ms(options.min_speech_duration_ms);
    let speech_pad_samples = samples_for_ms(options.speech_pad_ms);
    let min_silence_samples = samples_for_ms(options.min_silence_duration_ms);
    let audio_length_samples = audio_length_samples as i64;
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
        }

        if probability >= options.threshold && !triggered {
            triggered = true;
            current_start = Some(window_position);
            continue;
        }

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
        // The trailing segment closes at the original recording's end.
        speeches.push((start, audio_length_samples));
    }

    for index in 0..speeches.len() {
        if index == 0 {
            speeches[0].0 = (speeches[0].0 - speech_pad_samples).max(0);
        }
        if index + 1 < speeches.len() {
            let silence = speeches[index + 1].0 - speeches[index].1;
            if silence < 2 * speech_pad_samples {
                // Share a short gap instead of double-counting padded samples.
                let boundary = speeches[index].1 + silence / 2;
                speeches[index].1 = boundary;
                speeches[index + 1].0 = boundary;
            } else {
                speeches[index].1 += speech_pad_samples;
                speeches[index + 1].0 -= speech_pad_samples;
            }
        } else {
            speeches[index].1 = (speeches[index].1 + speech_pad_samples).min(audio_length_samples);
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
struct FireRedVadSession {
    session: Session,
}

#[cfg(feature = "runtime")]
impl FireRedVadSession {
    fn open(model_path: &Path) -> Result<Self, VadError> {
        #[cfg(windows)]
        let admitted_model =
            solstone_core_speakers_onnx::windows_runtime::bootstrap_windows_vad_model(model_path)
                .map_err(|detail| VadError::OnnxRuntime { detail })?;
        #[cfg(windows)]
        let model_path = admitted_model.as_path();
        let builder = Session::builder().map_err(|error| VadError::ProviderUnavailable {
            detail: format!("ONNX Runtime session builder is unavailable: {error}"),
        })?;
        let mut builder = builder
            .with_execution_providers([CPU::default().with_arena_allocator(false).build()])
            .map_err(session_option_error)?
            .with_intra_threads(1)
            .map_err(session_option_error)?
            .with_inter_threads(1)
            .map_err(session_option_error)?
            .with_parallel_execution(false)
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

    fn infer(&mut self, values: &[f32], frames: usize) -> Result<Vec<f32>, VadError> {
        let input = Tensor::from_array((
            [1, frames, features::FEATURE_BINS],
            values.to_vec().into_boxed_slice(),
        ))
        .map_err(|error| VadError::Internal {
            detail: format!("could not build FireRed input tensor: {error}"),
        })?;
        let mut outputs = self
            .session
            .run(ort::inputs![INPUT_FEATURES_NAME => input])
            .map_err(|error| VadError::OnnxRuntime {
                detail: format!("FireRed VAD inference failed: {error}"),
            })?;
        let probabilities = extract_values(&mut outputs, OUTPUT_PROBS_NAME, &[1, frames, 1])?;
        validate_probabilities(&probabilities, frames)?;
        Ok(probabilities)
    }

    fn speech_probs(
        &mut self,
        features: &solstone_core_speakers::FeatureMatrix,
    ) -> Result<Vec<f32>, VadError> {
        let mut probabilities = Vec::with_capacity(features.frames());
        for block in feature_blocks(features.frames()) {
            let input = &features.data()[block.context.start * features::FEATURE_BINS
                ..block.context.end * features::FEATURE_BINS];
            let output = self.infer(input, block.context.len())?;
            let crop_start = block.owned.start - block.context.start;
            probabilities.extend_from_slice(&output[crop_start..crop_start + block.owned.len()]);
        }
        Ok(probabilities)
    }
}

fn validate_probabilities(values: &[f32], frames: usize) -> Result<(), VadError> {
    if values.len() != frames {
        return Err(VadError::ModelIoMismatch {
            detail: format!(
                "FireRed returned {} probabilities for {frames} frames",
                values.len()
            ),
        });
    }
    if let Some((index, value)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite() || !(0.0..=1.0).contains(*value))
    {
        return Err(VadError::ModelIoMismatch {
            detail: format!("FireRed probability at frame {index} is invalid: {value}"),
        });
    }
    Ok(())
}

#[cfg(feature = "runtime")]
fn session_option_error<R>(error: ort::Error<R>) -> VadError {
    VadError::ProviderUnavailable {
        detail: format!("ONNX Runtime session options could not be applied: {error}"),
    }
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
    if inputs.len() != 1 || outputs.len() != 1 {
        return Err(VadError::ModelIoMismatch {
            detail: format!(
                "expected one input and output, got {} inputs and {} outputs",
                inputs.len(),
                outputs.len()
            ),
        });
    }
    check_tensor(
        "input",
        inputs[0].name(),
        inputs[0].dtype(),
        INPUT_FEATURES_NAME,
        &[
            ExpectedDim::Any,
            ExpectedDim::Any,
            ExpectedDim::Exact(features::FEATURE_BINS as i64),
        ],
    )?;
    check_tensor(
        "output",
        outputs[0].name(),
        outputs[0].dtype(),
        OUTPUT_PROBS_NAME,
        &[ExpectedDim::Any, ExpectedDim::Any, ExpectedDim::Exact(1)],
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
mod tests;

#[cfg(all(windows, feature = "runtime"))]
pub fn loaded_onnx_runtime_version() -> Result<String, VadError> {
    solstone_core_speakers_onnx::windows_runtime::loaded_windows_onnx_version()
        .map_err(|detail| VadError::OnnxRuntime { detail })
}
