// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg_attr(all(not(feature = "runtime"), not(test)), allow(dead_code))]

//! Native one-record speaker analysis command contract.
//!
//! Scalar and vector response fields such as `statement_ids`, `durations_s`,
//! `encoder`, `evidence.*`, `pyannote.window_stats`, and `diarization.*` map
//! directly onto the differential bundle vocabulary in
//! `tests/verify_speaker_differential.py:82-118`.
//!
//! Matrix-valued bundle fields (`statement_embeddings.embeddings` and
//! `diarization.interval_embeddings`) are represented as payload descriptors
//! because the v1 contract keeps binary matrices out of stdout. A bundle emitter
//! loads the named payload using the reported shape, dtype, and row id/index
//! lists; this difference is structural rather than a field-name oversight.

use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
#[cfg(feature = "runtime")]
use std::io::Write;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_speakers::diarization::{
    DiarizationError, SentenceTiming, SpeakerInterval, assign_sentences,
    cluster_embeddings as cluster_diarization_embeddings,
};
#[cfg(feature = "runtime")]
use solstone_core_speakers::diarization::{FrameLogProbs, MIN_INTERVAL_S, find_intervals};
use solstone_core_speakers::discovery::{
    DiscoveryClusteringError, cluster_embeddings as cluster_discovery_embeddings,
};
#[cfg(feature = "runtime")]
use solstone_core_speakers::{
    PYANNOTE_CLASS_COUNT, PYANNOTE_DIARIZE_STRIDE_S, SpeakerEvidence, SpeakerFeatureError,
    SpeakerSegmentationError, SpeakerWindowStats, admit_statement_features,
    compute_wespeaker_filterbank_cmn, decide_speaker_evidence, run_pyannote_segmentation_pass,
};
use solstone_core_speakers::{StatementSpan, WESPEAKER_EMBEDDING_SIZE, WESPEAKER_SAMPLE_RATE_HZ};
#[cfg(feature = "runtime")]
use solstone_core_speakers_onnx::{
    PlatformDescriptor, PyannoteSegmenter, SpeakerOnnxError, WespeakerEmbedder,
    default_speaker_execution_providers,
};

pub const REQUEST_SCHEMA: &str = "solstone-speaker-analyze-request-v1";
pub const RESPONSE_SCHEMA: &str = "solstone-speaker-analyze-response-v1";
pub const ERROR_SCHEMA: &str = "solstone-speaker-analyze-error-v1";
pub const DISCOVERY_CLUSTER_REQUEST_SCHEMA: &str = "solstone-speaker-discovery-cluster-request-v1";
pub const DISCOVERY_CLUSTER_RESPONSE_SCHEMA: &str =
    "solstone-speaker-discovery-cluster-response-v1";
pub const USAGE: &str = "Usage: solstone-core-speakers-analyze < request.json > response.json\n       solstone-core-speakers-analyze discovery-cluster < request.json > response.json";

const PAYLOAD_FORMAT: &str = "raw-f32le-row-major-v1";
const DTYPE_F32LE: &str = "float32-le";
const ENCODER: &str = "wespeaker-resnet34-256";
const DISCOVERY_CLUSTER_COMMAND: &str = "discovery-cluster";
const DISCOVERY_CLUSTER_ALGORITHM: &str = "hdbscan-eom-euclidean-f64-prim-mst";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Run,
    DiscoveryCluster,
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
        [argument] if argument == DISCOVERY_CLUSTER_COMMAND => Ok(Command::DiscoveryCluster),
        [argument, ..] => Err(UsageError::UnexpectedArgument {
            argument: argument.to_string_lossy().into_owned(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalyzeError {
    MalformedRequest {
        detail: String,
    },
    UnknownSchema {
        schema: String,
    },
    AudioUnreadable {
        path: String,
        detail: String,
    },
    AudioInvalid {
        path: String,
        detail: String,
    },
    UnsupportedSampleRate {
        expected: u32,
        actual: u32,
    },
    AudioNonFinite {
        path: String,
        index: usize,
    },
    PayloadUnreadable {
        path: String,
        detail: String,
    },
    PayloadInvalid {
        path: String,
        detail: String,
    },
    PayloadNonFinite {
        path: String,
        row: usize,
        col: usize,
    },
    ModelUnreadable {
        field: &'static str,
        path: String,
    },
    ModelInvalid {
        field: &'static str,
        detail: String,
    },
    ModelIoMismatch {
        field: &'static str,
        detail: String,
    },
    ProviderUnavailable {
        detail: String,
    },
    OnnxRuntime {
        detail: String,
    },
    OutputUnwritable {
        path: String,
        detail: String,
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
            Self::AudioUnreadable { .. } => "audio-unreadable",
            Self::AudioInvalid { .. } => "audio-invalid",
            Self::UnsupportedSampleRate { .. } => "unsupported-sample-rate",
            Self::AudioNonFinite { .. } => "audio-non-finite",
            Self::PayloadUnreadable { .. } => "payload-unreadable",
            Self::PayloadInvalid { .. } => "payload-invalid",
            Self::PayloadNonFinite { .. } => "payload-non-finite",
            Self::ModelUnreadable { .. } => "model-unreadable",
            Self::ModelInvalid { .. } => "model-invalid",
            Self::ModelIoMismatch { .. } => "model-io-mismatch",
            Self::ProviderUnavailable { .. } => "provider-unavailable",
            Self::OnnxRuntime { .. } => "onnx-runtime-error",
            Self::OutputUnwritable { .. } => "output-unwritable",
            Self::Internal { .. } => "internal-error",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::MalformedRequest { .. } | Self::UnknownSchema { .. } => 64,
            Self::AudioUnreadable { .. }
            | Self::AudioInvalid { .. }
            | Self::UnsupportedSampleRate { .. }
            | Self::AudioNonFinite { .. }
            | Self::PayloadUnreadable { .. }
            | Self::PayloadInvalid { .. }
            | Self::PayloadNonFinite { .. }
            | Self::ModelUnreadable { .. }
            | Self::ModelInvalid { .. }
            | Self::ModelIoMismatch { .. } => 69,
            Self::ProviderUnavailable { .. }
            | Self::OnnxRuntime { .. }
            | Self::OutputUnwritable { .. }
            | Self::Internal { .. } => 75,
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::MalformedRequest { detail } => detail.clone(),
            Self::UnknownSchema { schema } => {
                format!("request schema {schema:?} is not {REQUEST_SCHEMA:?}")
            }
            Self::AudioUnreadable { path, detail } => {
                format!("audio path {path:?} is unreadable: {detail}")
            }
            Self::AudioInvalid { path, detail } => {
                format!("audio path {path:?} is not raw little-endian f32 mono: {detail}")
            }
            Self::UnsupportedSampleRate { expected, actual } => {
                format!("unsupported sample rate: expected {expected}, got {actual}")
            }
            Self::AudioNonFinite { path, index } => {
                format!("audio path {path:?} contains non-finite sample at index {index}")
            }
            Self::PayloadUnreadable { path, detail } => {
                format!("payload path {path:?} is unreadable: {detail}")
            }
            Self::PayloadInvalid { path, detail } => {
                format!("payload path {path:?} is not raw little-endian f32 row-major: {detail}")
            }
            Self::PayloadNonFinite { path, row, col } => {
                format!("payload path {path:?} contains non-finite value at row={row} col={col}")
            }
            Self::ModelUnreadable { field, path } => {
                format!(
                    "{field} is missing or unreadable at {path:?}; provide a readable ONNX model path"
                )
            }
            Self::ModelInvalid { field, detail } => {
                format!("{field} could not be opened as an ONNX model: {detail}")
            }
            Self::ModelIoMismatch { field, detail } => {
                format!("{field} has unsupported ONNX input/output shape: {detail}")
            }
            Self::ProviderUnavailable { detail } => detail.clone(),
            Self::OnnxRuntime { detail } => detail.clone(),
            Self::OutputUnwritable { path, detail } => {
                format!(
                    "output payload path {path:?} is unwritable: {detail}; non-zero exit means payloads must not be trusted"
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

#[cfg(feature = "runtime")]
pub fn run_request(input: &str) -> Result<Value, AnalyzeError> {
    run_command_request(Command::Run, input)
}

#[cfg(feature = "runtime")]
pub fn run_command_request(command: Command, input: &str) -> Result<Value, AnalyzeError> {
    match command {
        Command::Run => run_analyze_request(input),
        Command::DiscoveryCluster => run_discovery_cluster_request(input),
    }
}

#[cfg(feature = "runtime")]
fn run_analyze_request(input: &str) -> Result<Value, AnalyzeError> {
    let request = parse_request(input)?;
    analyze_request(&request)
}

#[derive(Debug, Clone, PartialEq)]
struct Request {
    sample_rate_hz: u32,
    full_audio_f32le_path: String,
    reduced_audio_f32le_path: Option<String>,
    pyannote_segmentation_onnx_path: String,
    wespeaker_onnx_path: String,
    output_payload_f32le_path: String,
    interval_embedding_payload_f32le_path: Option<String>,
    statement_spans: Vec<StatementSpan>,
    diarization_spans: Vec<StatementSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveryClusterRequest {
    embeddings_f32le_path: String,
    rows: usize,
    cols: usize,
    byte_count: usize,
    min_cluster_size: usize,
    min_samples: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatementAudioBuffer {
    Full,
    Reduced,
}

impl StatementAudioBuffer {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Reduced => "reduced",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PayloadWrite {
    path: String,
    bytes: Vec<u8>,
}

fn validate_sample_rate(sample_rate_hz: u32) -> Result<(), AnalyzeError> {
    if sample_rate_hz != WESPEAKER_SAMPLE_RATE_HZ {
        return Err(AnalyzeError::UnsupportedSampleRate {
            expected: WESPEAKER_SAMPLE_RATE_HZ,
            actual: sample_rate_hz,
        });
    }
    Ok(())
}

#[cfg(feature = "runtime")]
fn analyze_request(request: &Request) -> Result<Value, AnalyzeError> {
    validate_sample_rate(request.sample_rate_hz)?;

    let full_audio = read_audio_f32le(&request.full_audio_f32le_path)?;
    let reduced_audio = match &request.reduced_audio_f32le_path {
        Some(path) => Some(read_audio_f32le(path)?),
        None => None,
    };
    preflight_model_path(
        "pyannote_segmentation_onnx_path",
        &request.pyannote_segmentation_onnx_path,
    )?;
    preflight_model_path("wespeaker_onnx_path", &request.wespeaker_onnx_path)?;

    let providers = default_speaker_execution_providers(PlatformDescriptor::current());
    let mut segmenter = PyannoteSegmenter::open(
        Path::new(&request.pyannote_segmentation_onnx_path),
        &providers,
    )
    .map_err(|error| map_open_onnx_error("pyannote_segmentation_onnx_path", error))?;
    // Python uses two Wespeaker sessions for statements and diarization; one
    // shared session is behavior-neutral because calls are serialized here and
    // the model carries no per-role state.
    let mut embedder = WespeakerEmbedder::open(Path::new(&request.wespeaker_onnx_path), &providers)
        .map_err(|error| map_open_onnx_error("wespeaker_onnx_path", error))?;

    let statement_audio_buffer = statement_audio_buffer_for_request(request);
    let statement_audio = match statement_audio_buffer {
        StatementAudioBuffer::Reduced => reduced_audio
            .as_ref()
            .expect("reduced audio is loaded when request has a reduced path")
            .as_slice(),
        StatementAudioBuffer::Full => full_audio.as_slice(),
    };

    let statement_admission = admit_statement_features(
        statement_audio,
        request.sample_rate_hz,
        &request.statement_spans,
    )
    .map_err(|error| map_feature_error(&request.full_audio_f32le_path, error))?;
    let mut statement_embedding_values =
        Vec::with_capacity(statement_admission.admitted.len() * WESPEAKER_EMBEDDING_SIZE);
    let mut statement_ids = Vec::with_capacity(statement_admission.admitted.len());
    let mut statement_durations = Vec::with_capacity(statement_admission.admitted.len());
    for admitted in &statement_admission.admitted {
        let embedding = embedder
            .embed(&admitted.features)
            .map_err(|error| map_runtime_onnx_error("wespeaker statement embedding", error))?;
        statement_embedding_values.extend_from_slice(embedding.values());
        statement_ids.push(admitted.statement_id);
        statement_durations.push(admitted.duration_s);
    }

    let pyannote_result = run_pyannote_segmentation_pass(
        &full_audio,
        request.sample_rate_hz,
        PYANNOTE_DIARIZE_STRIDE_S,
        |_window_index, audio_window| segmenter.infer_window(audio_window),
    )
    .map_err(|error| map_segmentation_error(&request.full_audio_f32le_path, error))?;
    let evidence = decide_speaker_evidence(
        pyannote_result.overlap_fraction,
        &pyannote_result.window_stats,
    );

    let mut payloads = Vec::new();
    let statement_payload = embedding_payload_bytes(&statement_embedding_values);
    let statement_byte_count = statement_payload.len();
    payloads.push(PayloadWrite {
        path: request.output_payload_f32le_path.clone(),
        bytes: statement_payload,
    });

    let statement_embeddings = statement_embeddings_value(
        request,
        statement_audio_buffer,
        statement_ids,
        statement_durations,
        statement_embedding_values.len() / WESPEAKER_EMBEDDING_SIZE,
        statement_byte_count,
        statement_admission.skipped_count,
    );

    let (diarization, interval_payload) = if evidence.speaker_evidence != SpeakerEvidence::Multi {
        (gate_declined_diarization_value(), None)
    } else {
        let log_probs = FrameLogProbs::from_row_major(
            pyannote_result.avg_log_probs.data(),
            PYANNOTE_CLASS_COUNT,
        )
        .map_err(map_diarization_error)?;
        let intervals = find_intervals(log_probs, full_audio.len());
        if intervals.is_empty() {
            (no_intervals_diarization_value(), None)
        } else {
            let mut valid_intervals = Vec::new();
            let mut interval_indices = Vec::new();
            let mut interval_embedding_values = Vec::new();
            for (interval_index, interval) in intervals.iter().enumerate() {
                let Some(features) =
                    interval_features(&full_audio, request.sample_rate_hz, interval).map_err(
                        |error| map_feature_error(&request.full_audio_f32le_path, error),
                    )?
                else {
                    continue;
                };
                let embedding = embedder.embed(&features).map_err(|error| {
                    map_runtime_onnx_error("wespeaker interval embedding", error)
                })?;
                valid_intervals.push(*interval);
                interval_indices.push(interval_index);
                interval_embedding_values.extend_from_slice(embedding.values());
            }
            diarization_with_interval_embeddings(
                &intervals,
                &valid_intervals,
                &interval_indices,
                &interval_embedding_values,
                &request.diarization_spans,
                request.interval_embedding_payload_f32le_path.as_deref(),
            )?
        }
    };
    if let Some(payload) = interval_payload {
        payloads.push(payload);
    }

    let response = json!({
        "schema": RESPONSE_SCHEMA,
        "sample_rate_hz": request.sample_rate_hz,
        "inputs": {
            "statement_embedding": {
                "statement_ids": span_ids_value(&request.statement_spans),
                "spans_s": spans_value(&request.statement_spans),
            },
            "diarization": {
                "statement_ids": span_ids_value(&request.diarization_spans),
                "spans_s": spans_value(&request.diarization_spans),
            },
        },
        "statement_embeddings": statement_embeddings,
        "pyannote": {
            "window_stats": window_stats_value(&pyannote_result.window_stats),
        },
        "evidence": {
            "speaker_evidence": evidence.speaker_evidence.as_str(),
            "multi_window_fraction": evidence.multi_window_fraction,
            "mean_window_overlap_share": evidence.mean_window_overlap_share,
            "overlap_fraction": pyannote_result.overlap_fraction,
        },
        "diarization": diarization,
    });

    write_payloads(&payloads)?;
    Ok(response)
}

fn run_discovery_cluster_request(input: &str) -> Result<Value, AnalyzeError> {
    let request = parse_discovery_cluster_request(input)?;
    let embeddings = read_embedding_payload_f32le(&request)?;
    let labels = cluster_discovery_embeddings(
        &embeddings,
        request.rows,
        request.cols,
        request.min_cluster_size,
        request.min_samples,
    )
    .map_err(|error| map_discovery_clustering_error(&request.embeddings_f32le_path, error))?;
    Ok(discovery_cluster_response_value(&request, &labels))
}

fn discovery_cluster_response_value(
    request: &DiscoveryClusterRequest,
    labels: &[Option<usize>],
) -> Value {
    let noise_count = labels.iter().filter(|label| label.is_none()).count();
    let cluster_count = labels
        .iter()
        .filter_map(|label| *label)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let labels = labels
        .iter()
        .map(|label| label.map_or(-1_i64, |label| label as i64))
        .collect::<Vec<_>>();
    json!({
        "schema": DISCOVERY_CLUSTER_RESPONSE_SCHEMA,
        "labels": labels,
        "cluster_count": cluster_count,
        "noise_count": noise_count,
        "parameters": {
            "min_cluster_size": request.min_cluster_size,
            "min_samples": request.min_samples,
        },
        "algorithm": DISCOVERY_CLUSTER_ALGORITHM,
    })
}

fn statement_audio_buffer_for_request(request: &Request) -> StatementAudioBuffer {
    if request.reduced_audio_f32le_path.is_some() {
        StatementAudioBuffer::Reduced
    } else {
        StatementAudioBuffer::Full
    }
}

fn parse_request(input: &str) -> Result<Request, AnalyzeError> {
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
        });
    }

    let sample_rate_hz = required_u32(object, "sample_rate_hz")?;
    let full_audio_f32le_path = required_string(object, "full_audio_f32le_path")?.to_string();
    let reduced_audio_f32le_path = optional_string(object, "reduced_audio_f32le_path")?;
    let output_payload_f32le_path =
        required_string(object, "output_payload_f32le_path")?.to_string();
    let interval_embedding_payload_f32le_path =
        optional_string(object, "interval_embedding_payload_f32le_path")?;
    let models = required_object(object, "models")?;
    let pyannote_segmentation_onnx_path =
        required_string(models, "pyannote_segmentation_onnx_path")?.to_string();
    let wespeaker_onnx_path = required_string(models, "wespeaker_onnx_path")?.to_string();
    ensure_payload_paths_do_not_collide(
        &full_audio_f32le_path,
        reduced_audio_f32le_path.as_deref(),
        &pyannote_segmentation_onnx_path,
        &wespeaker_onnx_path,
        &output_payload_f32le_path,
        interval_embedding_payload_f32le_path.as_deref(),
    )?;
    let statement_embedding = required_object(object, "statement_embedding")?;
    let diarization = required_object(object, "diarization")?;
    let statement_spans = parse_spans(statement_embedding, "statement_embedding.spans")?;
    let diarization_spans = parse_spans(diarization, "diarization.spans")?;
    ensure_span_parity(&statement_spans, &diarization_spans)?;

    Ok(Request {
        sample_rate_hz,
        full_audio_f32le_path,
        reduced_audio_f32le_path,
        pyannote_segmentation_onnx_path,
        wespeaker_onnx_path,
        output_payload_f32le_path,
        interval_embedding_payload_f32le_path,
        statement_spans,
        diarization_spans,
    })
}

fn parse_discovery_cluster_request(input: &str) -> Result<DiscoveryClusterRequest, AnalyzeError> {
    let value: Value =
        serde_json::from_str(input).map_err(|error| AnalyzeError::MalformedRequest {
            detail: format!("request body is not valid JSON: {error}"),
        })?;
    let object = value
        .as_object()
        .ok_or_else(|| malformed("request body must be a JSON object"))?;
    let schema = required_string(object, "schema")?;
    if schema != DISCOVERY_CLUSTER_REQUEST_SCHEMA {
        return Err(AnalyzeError::UnknownSchema {
            schema: schema.to_string(),
        });
    }

    let embeddings_f32le_path = required_string(object, "embeddings_f32le_path")?.to_string();
    let (rows, cols) = required_shape(object, "shape")?;
    let byte_count = checked_payload_byte_count(rows, cols)?;
    require_literal(object, "payload_format", PAYLOAD_FORMAT)?;
    require_literal(object, "dtype", DTYPE_F32LE)?;
    let min_cluster_size = required_usize_i64(object, "min_cluster_size")?;
    let min_samples = required_usize_i64(object, "min_samples")?;

    Ok(DiscoveryClusterRequest {
        embeddings_f32le_path,
        rows,
        cols,
        byte_count,
        min_cluster_size,
        min_samples,
    })
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

fn optional_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<Option<String>, AnalyzeError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|string| !string.is_empty())
            .map(|string| Some(string.to_string()))
            .ok_or_else(|| malformed(format!("{field} must be a non-empty string or null"))),
    }
}

fn required_u32(object: &Map<String, Value>, field: &'static str) -> Result<u32, AnalyzeError> {
    let value = object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed(format!("{field} must be an integer")))?;
    u32::try_from(value).map_err(|_error| malformed(format!("{field} is out of range for u32")))
}

fn required_usize_i64(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<usize, AnalyzeError> {
    let value = object
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| malformed(format!("{field} must be an i64 integer")))?;
    if value < 0 {
        return Err(malformed(format!("{field} must be non-negative")));
    }
    usize::try_from(value).map_err(|_error| malformed(format!("{field} is out of range for usize")))
}

fn required_shape(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<(usize, usize), AnalyzeError> {
    let shape = object
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| malformed(format!("{field} must be [rows, cols]")))?;
    if shape.len() != 2 {
        return Err(malformed(format!(
            "{field} must contain exactly two integers"
        )));
    }
    let rows = usize_from_i64_value(&shape[0], "shape[0]")?;
    let cols = usize_from_i64_value(&shape[1], "shape[1]")?;
    if cols == 0 {
        return Err(malformed("shape[1] must be at least 1"));
    }
    Ok((rows, cols))
}

fn usize_from_i64_value(value: &Value, field: &'static str) -> Result<usize, AnalyzeError> {
    let value = value
        .as_i64()
        .ok_or_else(|| malformed(format!("{field} must be an i64 integer")))?;
    if value < 0 {
        return Err(malformed(format!("{field} must be non-negative")));
    }
    usize::try_from(value).map_err(|_error| malformed(format!("{field} is out of range for usize")))
}

fn checked_payload_byte_count(rows: usize, cols: usize) -> Result<usize, AnalyzeError> {
    let values = rows
        .checked_mul(cols)
        .ok_or_else(|| malformed(format!("shape overflow: rows={rows} cols={cols}")))?;
    values
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| {
            malformed(format!(
                "payload byte count overflow: rows={rows} cols={cols}"
            ))
        })
}

fn require_literal(
    object: &Map<String, Value>,
    field: &'static str,
    expected: &'static str,
) -> Result<(), AnalyzeError> {
    let actual = required_string(object, field)?;
    if actual != expected {
        return Err(malformed(format!("{field} must be {expected:?}")));
    }
    Ok(())
}

fn parse_spans(
    object: &Map<String, Value>,
    field_path: &'static str,
) -> Result<Vec<StatementSpan>, AnalyzeError> {
    let spans = object
        .get("spans")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed(format!("{field_path} must be an array")))?;
    let mut out = Vec::with_capacity(spans.len());
    for (index, value) in spans.iter().enumerate() {
        let span = value
            .as_object()
            .ok_or_else(|| malformed(format!("{field_path}[{index}] must be an object")))?;
        let statement_id = span
            .get("statement_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                malformed(format!(
                    "{field_path}[{index}].statement_id must be an integer"
                ))
            })?;
        out.push(StatementSpan {
            statement_id,
            start_s: optional_bound(span.get("start_s")),
            end_s: optional_bound(span.get("end_s")),
        });
    }
    Ok(out)
}

fn optional_bound(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

fn ensure_span_parity(
    statement_spans: &[StatementSpan],
    diarization_spans: &[StatementSpan],
) -> Result<(), AnalyzeError> {
    if statement_spans.len() != diarization_spans.len() {
        return Err(malformed(format!(
            "statement_embedding.spans length {} does not match diarization.spans length {}",
            statement_spans.len(),
            diarization_spans.len()
        )));
    }
    for (index, (left, right)) in statement_spans.iter().zip(diarization_spans).enumerate() {
        if left.statement_id != right.statement_id {
            return Err(malformed(format!(
                "statement id mismatch at index {index}: statement_embedding has {}, diarization has {}",
                left.statement_id, right.statement_id
            )));
        }
    }
    Ok(())
}

fn ensure_payload_paths_do_not_collide(
    full_audio_f32le_path: &str,
    reduced_audio_f32le_path: Option<&str>,
    pyannote_segmentation_onnx_path: &str,
    wespeaker_onnx_path: &str,
    output_payload_f32le_path: &str,
    interval_embedding_payload_f32le_path: Option<&str>,
) -> Result<(), AnalyzeError> {
    let input_paths = [
        ("full_audio_f32le_path", Some(full_audio_f32le_path)),
        ("reduced_audio_f32le_path", reduced_audio_f32le_path),
        (
            "models.pyannote_segmentation_onnx_path",
            Some(pyannote_segmentation_onnx_path),
        ),
        ("models.wespeaker_onnx_path", Some(wespeaker_onnx_path)),
    ];
    for (payload_field, payload_path) in [
        ("output_payload_f32le_path", Some(output_payload_f32le_path)),
        (
            "interval_embedding_payload_f32le_path",
            interval_embedding_payload_f32le_path,
        ),
    ] {
        let Some(payload_path) = payload_path else {
            continue;
        };
        for (input_field, input_path) in input_paths {
            let Some(input_path) = input_path else {
                continue;
            };
            if Path::new(payload_path) == Path::new(input_path) {
                return Err(payload_path_collision(payload_field, input_field));
            }
        }
    }
    if let Some(interval_embedding_payload_f32le_path) = interval_embedding_payload_f32le_path
        && Path::new(output_payload_f32le_path) == Path::new(interval_embedding_payload_f32le_path)
    {
        return Err(payload_path_collision(
            "output_payload_f32le_path",
            "interval_embedding_payload_f32le_path",
        ));
    }
    Ok(())
}

fn payload_path_collision(left_field: &'static str, right_field: &'static str) -> AnalyzeError {
    malformed(format!(
        "{left_field} must not equal {right_field}; payload writes happen after analysis and would overwrite that path"
    ))
}

fn malformed(detail: impl Into<String>) -> AnalyzeError {
    AnalyzeError::MalformedRequest {
        detail: detail.into(),
    }
}

#[cfg(feature = "runtime")]
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

fn read_embedding_payload_f32le(
    request: &DiscoveryClusterRequest,
) -> Result<Vec<f32>, AnalyzeError> {
    let metadata = fs::metadata(&request.embeddings_f32le_path).map_err(|error| {
        AnalyzeError::PayloadUnreadable {
            path: request.embeddings_f32le_path.clone(),
            detail: error.to_string(),
        }
    })?;
    if !metadata.is_file() {
        return Err(AnalyzeError::PayloadUnreadable {
            path: request.embeddings_f32le_path.clone(),
            detail: "path is not a regular file".to_string(),
        });
    }
    let actual_len = metadata.len();
    let expected_len = request.byte_count as u64;
    if actual_len != expected_len {
        return Err(AnalyzeError::PayloadInvalid {
            path: request.embeddings_f32le_path.clone(),
            detail: format!("byte length {actual_len} does not match expected {expected_len}"),
        });
    }

    let mut file = fs::File::open(&request.embeddings_f32le_path).map_err(|error| {
        AnalyzeError::PayloadUnreadable {
            path: request.embeddings_f32le_path.clone(),
            detail: error.to_string(),
        }
    })?;
    let mut bytes = Vec::with_capacity(request.byte_count);
    file.read_to_end(&mut bytes)
        .map_err(|error| AnalyzeError::PayloadUnreadable {
            path: request.embeddings_f32le_path.clone(),
            detail: error.to_string(),
        })?;
    if bytes.len() != request.byte_count {
        return Err(AnalyzeError::PayloadInvalid {
            path: request.embeddings_f32le_path.clone(),
            detail: format!(
                "byte length {} changed after metadata check; expected {}",
                bytes.len(),
                request.byte_count
            ),
        });
    }

    let mut values = Vec::with_capacity(request.rows * request.cols);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let value = f32::from_le_bytes(chunk.try_into().expect("four bytes"));
        if !value.is_finite() {
            return Err(AnalyzeError::PayloadNonFinite {
                path: request.embeddings_f32le_path.clone(),
                row: index / request.cols,
                col: index % request.cols,
            });
        }
        values.push(value);
    }
    Ok(values)
}

#[cfg(feature = "runtime")]
fn preflight_model_path(field: &'static str, path: &str) -> Result<(), AnalyzeError> {
    let metadata = fs::metadata(path).map_err(|_error| AnalyzeError::ModelUnreadable {
        field,
        path: path.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(AnalyzeError::ModelUnreadable {
            field,
            path: path.to_string(),
        });
    }
    fs::File::open(path)
        .map(|_file| ())
        .map_err(|_error| AnalyzeError::ModelUnreadable {
            field,
            path: path.to_string(),
        })
}

#[cfg(feature = "runtime")]
fn interval_features(
    audio: &[f32],
    sample_rate_hz: u32,
    interval: &SpeakerInterval,
) -> Result<Option<solstone_core_speakers::FeatureMatrix>, SpeakerFeatureError> {
    let start_sample = (interval.start_s * sample_rate_hz as f64) as usize;
    let end_sample = (interval.end_s * sample_rate_hz as f64) as usize;
    let realized_start = start_sample.min(audio.len());
    let realized_end = end_sample.min(audio.len());
    let interval_audio = if realized_start < realized_end {
        &audio[realized_start..realized_end]
    } else {
        &[]
    };
    if interval_audio.len() < (MIN_INTERVAL_S * sample_rate_hz as f64) as usize {
        return Ok(None);
    }
    let features = compute_wespeaker_filterbank_cmn(interval_audio, sample_rate_hz)?;
    if features.frames() == 0 {
        return Ok(None);
    }
    Ok(Some(features))
}

fn statement_embeddings_value(
    request: &Request,
    audio_buffer: StatementAudioBuffer,
    statement_ids: Vec<i64>,
    durations_s: Vec<f64>,
    rows: usize,
    byte_count: usize,
    skipped_count: usize,
) -> Value {
    json!({
        "audio_buffer": audio_buffer.as_str(),
        "encoder": ENCODER,
        "payload_format": PAYLOAD_FORMAT,
        "payload_path": request.output_payload_f32le_path.as_str(),
        "dtype": DTYPE_F32LE,
        "shape": [rows, WESPEAKER_EMBEDDING_SIZE],
        "byte_count": byte_count,
        "statement_ids": statement_ids,
        "durations_s": durations_s,
        // Python's skipped counter is phase-2-only and log-only
        // (main.py:661-687, returned dict at :700-705). This response reports
        // the criterion-9 count: every input span that did not become a row.
        "admitted_count": rows,
        "skipped_count": skipped_count,
    })
}

fn gate_declined_diarization_value() -> Value {
    json!({
        "intervals": Value::Null,
        "valid_intervals": Value::Null,
        "interval_embeddings": Value::Null,
        "cluster_labels": Value::Null,
        "statement_labels": Value::Null,
        "silhouette_k": Value::Null,
        "effective_k": Value::Null,
    })
}

#[cfg(feature = "runtime")]
fn no_intervals_diarization_value() -> Value {
    json!({
        "intervals": [],
        "valid_intervals": Value::Null,
        "interval_embeddings": Value::Null,
        "cluster_labels": Value::Null,
        "statement_labels": Value::Null,
        "silhouette_k": Value::Null,
        "effective_k": Value::Null,
    })
}

fn diarization_with_interval_embeddings(
    intervals: &[SpeakerInterval],
    valid_intervals: &[SpeakerInterval],
    interval_indices: &[usize],
    interval_embedding_values: &[f32],
    diarization_spans: &[StatementSpan],
    interval_payload_path: Option<&str>,
) -> Result<(Value, Option<PayloadWrite>), AnalyzeError> {
    let rows = valid_intervals.len();
    let cluster = cluster_diarization_embeddings(
        interval_embedding_values,
        rows,
        WESPEAKER_EMBEDDING_SIZE,
        None,
    )
    .map_err(map_diarization_error)?;
    let timings: Vec<SentenceTiming> = diarization_spans
        .iter()
        .map(|span| SentenceTiming {
            start_s: span.start_s,
            end_s: span.end_s,
        })
        .collect();
    let statement_labels = assign_sentences(&timings, valid_intervals, &cluster.labels)
        .map_err(map_diarization_error)?;
    let (interval_embeddings, payload) = interval_embedding_payload_value(
        interval_payload_path,
        interval_embedding_values,
        rows,
        interval_indices,
    );

    Ok((
        json!({
            "intervals": intervals_value(intervals),
            "valid_intervals": intervals_value(valid_intervals),
            "interval_embeddings": interval_embeddings,
            "cluster_labels": cluster.labels,
            "statement_labels": option_usize_vec_value(&statement_labels),
            "silhouette_k": cluster.silhouette_k,
            "effective_k": cluster.effective_k,
        }),
        payload,
    ))
}

fn interval_embedding_payload_value(
    path: Option<&str>,
    values: &[f32],
    rows: usize,
    interval_indices: &[usize],
) -> (Value, Option<PayloadWrite>) {
    let Some(path) = path else {
        return (Value::Null, None);
    };
    let bytes = embedding_payload_bytes(values);
    let byte_count = bytes.len();
    (
        json!({
            "payload_path": path,
            "payload_format": PAYLOAD_FORMAT,
            "dtype": DTYPE_F32LE,
            "shape": [rows, WESPEAKER_EMBEDDING_SIZE],
            "byte_count": byte_count,
            "interval_indices": interval_indices,
        }),
        Some(PayloadWrite {
            path: path.to_string(),
            bytes,
        }),
    )
}

fn embedding_payload_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[cfg(feature = "runtime")]
fn write_payloads(payloads: &[PayloadWrite]) -> Result<(), AnalyzeError> {
    for payload in payloads {
        let mut file =
            fs::File::create(&payload.path).map_err(|error| AnalyzeError::OutputUnwritable {
                path: payload.path.clone(),
                detail: error.to_string(),
            })?;
        file.write_all(&payload.bytes)
            .map_err(|error| AnalyzeError::OutputUnwritable {
                path: payload.path.clone(),
                detail: error.to_string(),
            })?;
    }
    Ok(())
}

#[cfg(feature = "runtime")]
fn span_ids_value(spans: &[StatementSpan]) -> Value {
    Value::Array(
        spans
            .iter()
            .map(|span| json!(span.statement_id))
            .collect::<Vec<_>>(),
    )
}

#[cfg(feature = "runtime")]
fn spans_value(spans: &[StatementSpan]) -> Value {
    Value::Array(
        spans
            .iter()
            .map(|span| {
                Value::Array(vec![
                    option_f64_value(span.start_s),
                    option_f64_value(span.end_s),
                ])
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg(feature = "runtime")]
fn option_f64_value(value: Option<f64>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

fn option_usize_vec_value(values: &[Option<usize>]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|value| value.map_or(Value::Null, |value| json!(value)))
            .collect::<Vec<_>>(),
    )
}

fn intervals_value(intervals: &[SpeakerInterval]) -> Value {
    Value::Array(
        intervals
            .iter()
            .map(|interval| {
                json!({
                    "start_s": interval.start_s,
                    "end_s": interval.end_s,
                    "local_class": interval.local_class,
                })
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg(feature = "runtime")]
fn window_stats_value(stats: &[SpeakerWindowStats]) -> Value {
    Value::Array(
        stats
            .iter()
            .map(|stats| {
                json!({
                    "speech_frames": stats.speech_frames,
                    "active_slot_count": stats.active_slot_count,
                    "overlap_frames": stats.overlap_frames,
                })
            })
            .collect::<Vec<_>>(),
    )
}

#[cfg(feature = "runtime")]
fn map_feature_error(path: &str, error: SpeakerFeatureError) -> AnalyzeError {
    match error {
        SpeakerFeatureError::UnsupportedSampleRate { expected, actual } => {
            AnalyzeError::UnsupportedSampleRate { expected, actual }
        }
        SpeakerFeatureError::NonFiniteAudioSample { index } => AnalyzeError::AudioNonFinite {
            path: path.to_string(),
            index,
        },
        SpeakerFeatureError::ShapeMismatch { .. } | SpeakerFeatureError::ShapeOverflow { .. } => {
            AnalyzeError::Internal {
                detail: error.to_string(),
            }
        }
    }
}

#[cfg(feature = "runtime")]
fn map_segmentation_error(
    path: &str,
    error: SpeakerSegmentationError<SpeakerOnnxError>,
) -> AnalyzeError {
    match error {
        SpeakerSegmentationError::UnsupportedSampleRate { expected, actual } => {
            AnalyzeError::UnsupportedSampleRate { expected, actual }
        }
        SpeakerSegmentationError::NonFiniteAudioSample { index } => AnalyzeError::AudioNonFinite {
            path: path.to_string(),
            index,
        },
        SpeakerSegmentationError::WindowLogProbShapeMismatch { .. } => {
            AnalyzeError::ModelIoMismatch {
                field: "pyannote_segmentation_onnx_path",
                detail: error.to_string(),
            }
        }
        SpeakerSegmentationError::Inference { source, .. } => {
            map_runtime_onnx_error("pyannote segmentation", source)
        }
        SpeakerSegmentationError::InvalidStride { .. }
        | SpeakerSegmentationError::ShapeOverflow { .. } => AnalyzeError::Internal {
            detail: error.to_string(),
        },
    }
}

fn map_diarization_error(error: DiarizationError) -> AnalyzeError {
    AnalyzeError::Internal {
        detail: error.to_string(),
    }
}

fn map_discovery_clustering_error(path: &str, error: DiscoveryClusteringError) -> AnalyzeError {
    match error {
        DiscoveryClusteringError::InvalidMinClusterSize { .. }
        | DiscoveryClusteringError::InvalidMinSamples { .. }
        | DiscoveryClusteringError::MinSamplesExceedsRows { .. }
        | DiscoveryClusteringError::NonUnitEmbeddingRow { .. } => AnalyzeError::PayloadInvalid {
            path: path.to_string(),
            detail: error.to_string(),
        },
        DiscoveryClusteringError::NonFiniteCoordinate { row, col } => {
            AnalyzeError::PayloadNonFinite {
                path: path.to_string(),
                row,
                col,
            }
        }
        DiscoveryClusteringError::ZeroColumns
        | DiscoveryClusteringError::ShapeOverflow { .. }
        | DiscoveryClusteringError::ShapeMismatch { .. }
        | DiscoveryClusteringError::HdbscanEmptyDataset
        | DiscoveryClusteringError::HdbscanWrongDimension { .. }
        | DiscoveryClusteringError::HdbscanNonFiniteCoordinate { .. }
        | DiscoveryClusteringError::HdbscanOutputLength { .. }
        | DiscoveryClusteringError::HdbscanInvalidLabel { .. } => AnalyzeError::Internal {
            detail: error.to_string(),
        },
    }
}

#[cfg(feature = "runtime")]
fn map_open_onnx_error(field: &'static str, error: SpeakerOnnxError) -> AnalyzeError {
    match error {
        // Reserved under the current fixed provider plan:
        // default_speaker_execution_providers(PlatformDescriptor::current()) never
        // requests CoreML on non-Apple, the only production ProviderUnavailable path.
        SpeakerOnnxError::ProviderUnavailable { .. } | SpeakerOnnxError::EmptyProviderPlan => {
            AnalyzeError::ProviderUnavailable {
                detail: error.to_string(),
            }
        }
        SpeakerOnnxError::InvalidModelIo { .. } | SpeakerOnnxError::MissingOutput { .. } => {
            AnalyzeError::ModelIoMismatch {
                field,
                detail: error.to_string(),
            }
        }
        SpeakerOnnxError::Ort { .. } => AnalyzeError::ModelInvalid {
            field,
            detail: error.to_string(),
        },
        SpeakerOnnxError::InvalidFeatureMatrix { .. }
        | SpeakerOnnxError::InvalidAudioWindow { .. } => AnalyzeError::Internal {
            detail: error.to_string(),
        },
    }
}

#[cfg(feature = "runtime")]
fn map_runtime_onnx_error(operation: &'static str, error: SpeakerOnnxError) -> AnalyzeError {
    match error {
        // Reserved under the current fixed provider plan:
        // default_speaker_execution_providers(PlatformDescriptor::current()) never
        // requests CoreML on non-Apple, the only production ProviderUnavailable path.
        SpeakerOnnxError::ProviderUnavailable { .. } | SpeakerOnnxError::EmptyProviderPlan => {
            AnalyzeError::ProviderUnavailable {
                detail: error.to_string(),
            }
        }
        SpeakerOnnxError::InvalidModelIo { .. } | SpeakerOnnxError::MissingOutput { .. } => {
            AnalyzeError::ModelIoMismatch {
                field: "speaker model",
                detail: error.to_string(),
            }
        }
        SpeakerOnnxError::Ort { .. } => AnalyzeError::OnnxRuntime {
            detail: format!("{operation} failed: {error}"),
        },
        SpeakerOnnxError::InvalidFeatureMatrix { .. }
        | SpeakerOnnxError::InvalidAudioWindow { .. } => AnalyzeError::Internal {
            detail: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "full-tests")]
    use std::process;
    #[cfg(feature = "full-tests")]
    use std::time::{SystemTime, UNIX_EPOCH};

    fn base_request() -> Value {
        json!({
            "schema": REQUEST_SCHEMA,
            "sample_rate_hz": 16000,
            "full_audio_f32le_path": "/tmp/full.f32",
            "reduced_audio_f32le_path": Value::Null,
            "models": {
                "pyannote_segmentation_onnx_path": "/models/pyannote.onnx",
                "wespeaker_onnx_path": "/models/wespeaker.onnx",
            },
            "output_payload_f32le_path": "/tmp/statements.f32",
            "statement_embedding": {
                "spans": [
                    {"statement_id": 1, "start_s": 0.0, "end_s": 0.5},
                    {"statement_id": 2, "start_s": "bad", "end_s": 1.0}
                ],
            },
            "diarization": {
                "spans": [
                    {"statement_id": 1, "start_s": 10.0, "end_s": 10.5},
                    {"statement_id": 2, "start_s": 10.6, "end_s": 11.0}
                ],
            },
        })
    }

    fn base_discovery_cluster_request(path: &str, rows: usize, cols: usize) -> Value {
        json!({
            "schema": DISCOVERY_CLUSTER_REQUEST_SCHEMA,
            "embeddings_f32le_path": path,
            "payload_format": PAYLOAD_FORMAT,
            "dtype": DTYPE_F32LE,
            "shape": [rows as i64, cols as i64],
            "min_cluster_size": 3,
            "min_samples": 2,
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
        fn parse_full_buffer_request_selects_full_statement_audio() {
            let request = parse_request(&request_string(base_request())).expect("request");

            assert_eq!(request.reduced_audio_f32le_path, None);
            assert_eq!(
                request.statement_spans[1],
                StatementSpan {
                    statement_id: 2,
                    start_s: None,
                    end_s: Some(1.0),
                }
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn parse_reduced_buffer_request_selects_reduced_statement_audio() {
            let mut request = base_request();
            request["reduced_audio_f32le_path"] = json!("/tmp/reduced.f32");

            let request = parse_request(&request_string(request)).expect("request");

            assert_eq!(
                request.reduced_audio_f32le_path,
                Some("/tmp/reduced.f32".to_string())
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn statement_embedding_uses_reduced_buffer_not_full_buffer() {
            let mut request = base_request();
            request["full_audio_f32le_path"] = json!("/tmp/full-is-not-selected.f32");
            request["reduced_audio_f32le_path"] = json!("/tmp/reduced-is-selected.f32");

            let request = parse_request(&request_string(request)).expect("request");

            assert_eq!(
                statement_audio_buffer_for_request(&request),
                StatementAudioBuffer::Reduced
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn parse_request_keeps_two_span_planes() {
            let request = parse_request(&request_string(base_request())).expect("request");

            assert_eq!(request.statement_spans[0].start_s, Some(0.0));
            assert_eq!(request.diarization_spans[0].start_s, Some(10.0));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn argv_accepts_discovery_cluster_subcommand() {
            assert_eq!(
                evaluate_args(&[OsString::from(DISCOVERY_CLUSTER_COMMAND)]),
                Ok(Command::DiscoveryCluster)
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn argv_rejects_unknown_argument_as_usage() {
            // Bare invocation is still Command::Run and no existing caller passes
            // argv today, so accepting the new discovery-cluster token is additive.
            let error = evaluate_args(&[OsString::from("--help")]).unwrap_err();
            let line = error_line_for_usage(&error);

            assert!(line.contains("\"reason\":\"usage\""));
            assert!(line.contains("Usage: solstone-core-speakers-analyze"));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn request_with_unknown_schema_is_rejected() {
            let mut request = base_request();
            request["schema"] = json!("solstone-speaker-analyze-request-v2");

            let error = parse_request(&request_string(request)).unwrap_err();

            assert_eq!(error.reason(), "unknown-schema");
            assert_eq!(error.exit_code(), 64);
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn discovery_cluster_unknown_schema_is_rejected() {
            let mut request = base_discovery_cluster_request("/not/read/embeddings.f32", 0, 2);
            request["schema"] = json!("solstone-speaker-discovery-cluster-request-v2");

            let error = run_discovery_cluster_request(&request_string(request)).unwrap_err();

            assert_eq!(error.reason(), "unknown-schema");
            assert_eq!(error.exit_code(), 64);
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_discovery {
        use super::*;

        #[cfg(feature = "full-tests")]
        #[test]
        fn discovery_cluster_missing_payload_path_reports_payload_unreadable() {
            let dir = TestDir::new();
            let request = base_discovery_cluster_request(&dir.path("missing.f32"), 6, 2);

            let error = run_command_request(Command::DiscoveryCluster, &request_string(request))
                .unwrap_err();

            assert_eq!(error.reason(), "payload-unreadable");
            assert_eq!(error.exit_code(), 69);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn discovery_cluster_byte_length_mismatch_reports_payload_invalid() {
            let dir = TestDir::new();
            let path = dir.path("embeddings.f32");
            write_f32le(&path, &[0.0_f32, 1.0]);
            let request = base_discovery_cluster_request(&path, 2, 2);

            let error = run_command_request(Command::DiscoveryCluster, &request_string(request))
                .unwrap_err();

            assert_eq!(error.reason(), "payload-invalid");
            assert_eq!(error.exit_code(), 69);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn discovery_cluster_happy_path_returns_labels_and_counts() {
            let dir = TestDir::new();
            let path = dir.path("embeddings.f32");
            write_f32le(
                &path,
                &unit_rows_2d(&[
                    (1.0, 0.0),
                    (1.0, 0.03),
                    (1.0, -0.03),
                    (-1.0, 0.0),
                    (-1.0, 0.03),
                    (-1.0, -0.03),
                ]),
            );
            let request = base_discovery_cluster_request(&path, 6, 2);

            let response = run_command_request(Command::DiscoveryCluster, &request_string(request))
                .expect("response");

            assert_eq!(response["schema"], DISCOVERY_CLUSTER_RESPONSE_SCHEMA);
            assert_eq!(response["labels"], json!([0, 0, 0, 1, 1, 1]));
            assert_eq!(response["cluster_count"], 2);
            assert_eq!(response["noise_count"], 0);
            assert_eq!(response["parameters"]["min_cluster_size"], 3);
            assert_eq!(response["parameters"]["min_samples"], 2);
            assert_eq!(response["algorithm"], DISCOVERY_CLUSTER_ALGORITHM);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn discovery_cluster_path_does_not_preflight_models() {
            let dir = TestDir::new();
            let path = dir.path("embeddings.f32");
            write_f32le(
                &path,
                &unit_rows_2d(&[
                    (1.0, 0.0),
                    (1.0, 0.03),
                    (1.0, -0.03),
                    (-1.0, 0.0),
                    (-1.0, 0.03),
                    (-1.0, -0.03),
                ]),
            );
            let mut request = base_discovery_cluster_request(&path, 6, 2);
            request["models"] = json!({
                "pyannote_segmentation_onnx_path": dir.path("absent-pyannote.onnx"),
                "wespeaker_onnx_path": dir.path("absent-wespeaker.onnx"),
            });

            // lib.rs:651-668 preflights model paths for the analyze path. These
            // intentionally absent model paths prove the cluster path does not route
            // through that preflight despite the ONNX crate import at module scope.
            let response = run_command_request(Command::DiscoveryCluster, &request_string(request))
                .expect("response");

            assert_eq!(response["schema"], DISCOVERY_CLUSTER_RESPONSE_SCHEMA);
            assert_eq!(response["labels"], json!([0, 0, 0, 1, 1, 1]));
        }
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    mod routine_response {
        use super::*;

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn payload_paths_may_not_collide_with_each_other() {
            let mut request = base_request();
            request["interval_embedding_payload_f32le_path"] = json!("/tmp/statements.f32");

            let error = parse_request(&request_string(request)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
            assert_eq!(error.exit_code(), 64);
            let detail = error.detail();
            assert!(detail.contains("output_payload_f32le_path"));
            assert!(detail.contains("interval_embedding_payload_f32le_path"));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn payload_path_may_not_collide_with_an_input_path() {
            let mut audio_collision = base_request();
            audio_collision["output_payload_f32le_path"] = json!("/tmp/full.f32");

            let error = parse_request(&request_string(audio_collision)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
            assert_eq!(error.exit_code(), 64);
            let detail = error.detail();
            assert!(detail.contains("output_payload_f32le_path"));
            assert!(detail.contains("full_audio_f32le_path"));

            let mut model_collision = base_request();
            model_collision["interval_embedding_payload_f32le_path"] =
                json!("/models/wespeaker.onnx");

            let error = parse_request(&request_string(model_collision)).unwrap_err();

            assert_eq!(error.reason(), "malformed-request");
            assert_eq!(error.exit_code(), 64);
            let detail = error.detail();
            assert!(detail.contains("interval_embedding_payload_f32le_path"));
            assert!(detail.contains("models.wespeaker_onnx_path"));
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn response_gate_declined_uses_null_diarization_fields() {
            let value = gate_declined_diarization_value();

            for field in [
                "intervals",
                "valid_intervals",
                "interval_embeddings",
                "cluster_labels",
                "statement_labels",
                "silhouette_k",
                "effective_k",
            ] {
                assert!(value[field].is_null(), "{field} should be null");
            }
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn interval_embedding_payload_omitted_when_path_null() {
            let (value, payload) = interval_embedding_payload_value(
                None,
                &[1.0_f32; WESPEAKER_EMBEDDING_SIZE],
                1,
                &[2],
            );

            assert!(value.is_null());
            assert!(payload.is_none());
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn interval_embedding_payload_written_with_interval_indices() {
            let path = "/tmp/intervals.f32";
            let (value, payload) = interval_embedding_payload_value(
                Some(path),
                &[1.0_f32; WESPEAKER_EMBEDDING_SIZE],
                1,
                &[2],
            );

            assert_eq!(value["payload_path"], path);
            assert_eq!(value["shape"], json!([1, WESPEAKER_EMBEDDING_SIZE]));
            assert_eq!(value["byte_count"], WESPEAKER_EMBEDDING_SIZE * 4);
            assert_eq!(value["interval_indices"], json!([2]));
            assert_eq!(
                payload.expect("payload").bytes.len(),
                WESPEAKER_EMBEDDING_SIZE * 4
            );
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn interval_embedding_payload_not_written_when_gate_declines() {
            let value = gate_declined_diarization_value();

            assert!(value["interval_embeddings"].is_null());
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn synthetic_frame_matrix_emits_diarization_fields() {
            let intervals = [
                SpeakerInterval {
                    start_s: 0.0,
                    end_s: 0.6,
                    local_class: 1,
                },
                SpeakerInterval {
                    start_s: 0.7,
                    end_s: 1.3,
                    local_class: 2,
                },
                SpeakerInterval {
                    start_s: 1.4,
                    end_s: 2.0,
                    local_class: 2,
                },
            ];
            let spans = [
                StatementSpan {
                    statement_id: 1,
                    start_s: Some(0.1),
                    end_s: Some(0.5),
                },
                StatementSpan {
                    statement_id: 2,
                    start_s: Some(0.8),
                    end_s: Some(1.8),
                },
            ];
            let mut embeddings = Vec::new();
            for row in 0..3 {
                for col in 0..WESPEAKER_EMBEDDING_SIZE {
                    embeddings.push(if col == row { 1.0 } else { 0.0 });
                }
            }

            let (value, payload) = diarization_with_interval_embeddings(
                &intervals,
                &intervals,
                &[0, 1, 2],
                &embeddings,
                &spans,
                Some("/tmp/intervals.f32"),
            )
            .expect("diarization");

            assert_eq!(value["intervals"].as_array().unwrap().len(), 3);
            assert_eq!(value["valid_intervals"].as_array().unwrap().len(), 3);
            assert_eq!(
                value["interval_embeddings"]["interval_indices"],
                json!([0, 1, 2])
            );
            assert_eq!(value["cluster_labels"].as_array().unwrap().len(), 3);
            assert_eq!(value["statement_labels"].as_array().unwrap().len(), 2);
            assert!(payload.is_some());
        }

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn all_statement_spans_skipped_writes_zero_row_payload() {
            let request = parse_request(&request_string(base_request())).expect("request");
            let value = statement_embeddings_value(
                &request,
                StatementAudioBuffer::Full,
                Vec::new(),
                Vec::new(),
                0,
                0,
                request.statement_spans.len(),
            );

            assert_eq!(value["shape"], json!([0, WESPEAKER_EMBEDDING_SIZE]));
            assert_eq!(value["byte_count"], 0);
            assert_eq!(value["admitted_count"], 0);
            assert_eq!(value["skipped_count"], 2);
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_filesystem {
        use super::*;

        #[cfg(feature = "full-tests")]
        #[test]
        fn missing_model_path_reports_model_unreadable() {
            let dir = TestDir::new();
            let audio_path = dir.path("audio.f32");
            write_f32le(&audio_path, &[0.0; 16]);
            let mut request = base_request();
            request["full_audio_f32le_path"] = json!(audio_path);
            request["models"]["pyannote_segmentation_onnx_path"] = json!(dir.path("missing.onnx"));

            let error = run_request(&request_string(request)).unwrap_err();

            assert_eq!(error.reason(), "model-unreadable");
            assert_eq!(error.exit_code(), 69);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn unreadable_audio_path_reports_audio_unreadable() {
            let dir = TestDir::new();
            let mut request = base_request();
            request["full_audio_f32le_path"] = json!(dir.path("missing.f32"));

            let error = run_request(&request_string(request)).unwrap_err();

            assert_eq!(error.reason(), "audio-unreadable");
            assert_eq!(error.exit_code(), 69);
        }
    }

    #[cfg(all(test, not(feature = "full-tests")))]
    mod routine_validation {
        use super::*;

        #[cfg(not(feature = "full-tests"))]
        #[test]
        fn unsupported_sample_rate_reports_unsupported_sample_rate() {
            let error = validate_sample_rate(8000).unwrap_err();

            assert_eq!(error.reason(), "unsupported-sample-rate");
            assert_eq!(error.exit_code(), 69);
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_runtime {
        use super::*;

        #[cfg(feature = "full-tests")]
        #[test]
        fn unwritable_output_path_reports_output_unwritable() {
            let dir = TestDir::new();
            let payload = PayloadWrite {
                path: dir.root.to_string_lossy().into_owned(),
                bytes: vec![1, 2, 3],
            };

            let error = write_payloads(&[payload]).unwrap_err();

            assert_eq!(error.reason(), "output-unwritable");
            assert_eq!(error.exit_code(), 75);
        }

        #[cfg(feature = "full-tests")]
        #[test]
        fn real_models_short_synthetic_declined_e2e() {
            let dir = TestDir::new();
            let audio_path = dir.path("audio.f32");
            let statement_payload_path = dir.path("statements.f32");
            let interval_payload_path = dir.path("intervals.f32");
            write_f32le(&audio_path, &vec![0.0_f32; 8_000]);
            let root = repo_root();
            let pyannote_model = root
                .join("core/models/assets/pyannote-segmentation-3.0.onnx")
                .to_string_lossy()
                .into_owned();
            let wespeaker_model = root
                .join("core/models/assets/wespeaker-resnet34-256.onnx")
                .to_string_lossy()
                .into_owned();
            let request = json!({
                "schema": REQUEST_SCHEMA,
                "sample_rate_hz": 16000,
                "full_audio_f32le_path": audio_path,
                "reduced_audio_f32le_path": Value::Null,
                "models": {
                    "pyannote_segmentation_onnx_path": pyannote_model,
                    "wespeaker_onnx_path": wespeaker_model,
                },
                "output_payload_f32le_path": statement_payload_path,
                "interval_embedding_payload_f32le_path": interval_payload_path.clone(),
                "statement_embedding": {
                    "spans": [
                        {"statement_id": 1, "start_s": 0.0, "end_s": 0.5}
                    ],
                },
                "diarization": {
                    "spans": [
                        {"statement_id": 1, "start_s": 0.0, "end_s": 0.5}
                    ],
                },
            });

            let response = run_request(&request_string(request)).expect("response");

            assert_eq!(response["schema"], RESPONSE_SCHEMA);
            let shape = response["statement_embeddings"]["shape"]
                .as_array()
                .expect("shape");
            let rows = shape[0].as_u64().expect("rows") as usize;
            let cols = shape[1].as_u64().expect("cols") as usize;
            let reported_bytes = response["statement_embeddings"]["byte_count"]
                .as_u64()
                .expect("byte count") as usize;
            assert_eq!(reported_bytes, rows * cols * 4);
            assert_eq!(
                fs::metadata(
                    response["statement_embeddings"]["payload_path"]
                        .as_str()
                        .unwrap()
                )
                .expect("statement payload")
                .len() as usize,
                reported_bytes
            );
            assert!(response["diarization"]["intervals"].is_null());
            assert!(!Path::new(&interval_payload_path).exists());
        }
    }

    #[cfg(feature = "full-tests")]
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
    }

    #[cfg(feature = "full-tests")]
    struct TestDir {
        root: std::path::PathBuf,
    }

    #[cfg(feature = "full-tests")]
    impl TestDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "solstone-speakers-analyze-test-{}-{nonce}",
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
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(path, bytes).expect("write audio");
    }

    #[cfg(feature = "full-tests")]
    fn unit_rows_2d(points: &[(f32, f32)]) -> Vec<f32> {
        let mut out = Vec::with_capacity(points.len() * 2);
        for (x, y) in points {
            let norm = (x * x + y * y).sqrt();
            out.push(*x / norm);
            out.push(*y / norm);
        }
        out
    }
}
