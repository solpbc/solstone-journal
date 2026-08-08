// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable JSONL transcript and NPZ embedding-sidecar publication.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, atomic_replace, write_bytes_exclusive,
};

use crate::ascii_json;
use crate::npz::NpzMembers;

const REQUEST_SCHEMA: &str = "solstone-speaker-transcript-write-request-v1";
pub const RESPONSE_SCHEMA: &str = "solstone-speaker-transcript-write-response-v1";
const MAX_BASE_TIME_US: u64 = 86_399_999_999;
const EMBEDDING_WIDTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeakerTranscriptWriteError {
    MalformedRequest {
        detail: String,
    },
    UnknownSchema {
        schema: String,
    },
    MissingStatementId {
        statement_index: usize,
    },
    InvalidStatementId {
        statement_index: usize,
        detail: String,
    },
    DuplicateStatementId {
        statement_index: usize,
        id: i64,
    },
    InvalidStatement {
        statement_index: usize,
        detail: String,
    },
    InvalidHeader {
        detail: String,
    },
    InvalidOutputPath {
        detail: String,
    },
    DestinationExists {
        path: String,
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
        row: usize,
        col: usize,
    },
    OutputUnwritable {
        path: String,
        detail: String,
    },
    NpzVerificationFailed {
        path: String,
        detail: String,
    },
    Internal {
        detail: String,
    },
}

impl SpeakerTranscriptWriteError {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::MalformedRequest { .. } => "malformed-request",
            Self::UnknownSchema { .. } => "unknown-schema",
            Self::MissingStatementId { .. } => "missing-statement-id",
            Self::InvalidStatementId { .. } => "invalid-statement-id",
            Self::DuplicateStatementId { .. } => "duplicate-statement-id",
            Self::InvalidStatement { .. } => "invalid-statement",
            Self::InvalidHeader { .. } => "invalid-header",
            Self::InvalidOutputPath { .. } => "invalid-output-path",
            Self::DestinationExists { .. } => "destination-exists",
            Self::PayloadUnreadable { .. } => "payload-unreadable",
            Self::PayloadInvalid { .. } => "payload-invalid",
            Self::PayloadNonFinite { .. } => "payload-non-finite",
            Self::OutputUnwritable { .. } => "output-unwritable",
            Self::NpzVerificationFailed { .. } => "npz-verification-failed",
            Self::Internal { .. } => "internal-error",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::MalformedRequest { detail }
            | Self::InvalidHeader { detail }
            | Self::InvalidOutputPath { detail }
            | Self::PayloadInvalid { detail, .. }
            | Self::Internal { detail } => detail.clone(),
            Self::UnknownSchema { schema } => format!("unknown schema: {schema}"),
            Self::MissingStatementId { statement_index } => {
                format!("statement {statement_index} is missing id")
            }
            Self::InvalidStatementId {
                statement_index,
                detail,
            }
            | Self::InvalidStatement {
                statement_index,
                detail,
            } => format!("statement {statement_index}: {detail}"),
            Self::DuplicateStatementId {
                statement_index,
                id,
            } => format!("statement {statement_index} duplicates id {id}"),
            Self::DestinationExists { path } => format!("destination already exists: {path}"),
            Self::PayloadUnreadable { path, detail }
            | Self::OutputUnwritable { path, detail }
            | Self::NpzVerificationFailed { path, detail } => format!("{path}: {detail}"),
            Self::PayloadNonFinite { row, col } => {
                format!("embedding payload is non-finite at row {row}, column {col}")
            }
        }
    }
}

impl fmt::Display for SpeakerTranscriptWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail())
    }
}

impl std::error::Error for SpeakerTranscriptWriteError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResponse {
    pub jsonl_path: String,
    pub npz_path: String,
    pub statement_count: usize,
    pub embedding_row_count: usize,
}

/// Parse, validate, and durably publish a transcript writer request.
pub fn write_request(bytes: &[u8]) -> Result<WriteResponse, SpeakerTranscriptWriteError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        SpeakerTranscriptWriteError::MalformedRequest {
            detail: error.to_string(),
        }
    })?;
    let request = Request::parse(value)?;
    request.publish()
}

struct Request {
    output: Output,
    jsonl: Vec<u8>,
    npz: Option<NpzMembers>,
    statement_count: usize,
    embedding_row_count: usize,
}

struct Output {
    jsonl_path: PathBuf,
    npz_path: PathBuf,
    redo: bool,
}

struct Statement {
    id: i32,
    start_offset_us: i64,
    text: String,
    speaker: Option<Value>,
}

struct Embeddings {
    values: Vec<f32>,
    statement_ids: Vec<i32>,
    durations_s: Vec<f32>,
    encoder: String,
}

impl Request {
    fn parse(value: Value) -> Result<Self, SpeakerTranscriptWriteError> {
        let object = object(&value, "request")?;
        let schema = string(required(object, "schema")?, "schema")?;
        if schema != REQUEST_SCHEMA {
            return Err(SpeakerTranscriptWriteError::UnknownSchema {
                schema: schema.to_owned(),
            });
        }
        let output = Output::parse(required(object, "output")?)?;
        let base_time_us_of_day = unsigned(
            required(object, "base_time_us_of_day")?,
            "base_time_us_of_day",
        )?;
        if base_time_us_of_day > MAX_BASE_TIME_US {
            return Err(SpeakerTranscriptWriteError::MalformedRequest {
                detail: "base_time_us_of_day is outside a day".to_owned(),
            });
        }
        let source = optional_string(object.get("source"), "source")?;
        let statements = parse_statements(required(object, "statements")?)?;
        let header = build_header(required(object, "header")?)?;
        let embeddings = Embeddings::parse(required(object, "embeddings")?, &statements)?;
        let jsonl = build_jsonl(&header, &statements, source, base_time_us_of_day)?;
        let row_count = embeddings.statement_ids.len();
        let npz = if row_count == 0 {
            None
        } else {
            Some(NpzMembers::build(
                &embeddings.values,
                row_count,
                &embeddings.statement_ids,
                &embeddings.durations_s,
                &embeddings.encoder,
            ))
        };
        Ok(Self {
            output,
            jsonl,
            npz,
            statement_count: statements.len(),
            embedding_row_count: row_count,
        })
    }

    fn publish(self) -> Result<WriteResponse, SpeakerTranscriptWriteError> {
        self.preflight_destinations()?;
        if let Some(npz) = &self.npz {
            let bytes = npz
                .archive()
                .map_err(|detail| SpeakerTranscriptWriteError::Internal { detail })?;
            publish_file(&self.output.npz_path, &bytes, self.output.redo)?;
            npz.verify_at(&self.output.npz_path).map_err(|detail| {
                SpeakerTranscriptWriteError::NpzVerificationFailed {
                    path: self.output.npz_path.display().to_string(),
                    detail,
                }
            })?;
        }
        publish_file(&self.output.jsonl_path, &self.jsonl, self.output.redo)?;
        Ok(WriteResponse {
            jsonl_path: self.output.jsonl_path.display().to_string(),
            npz_path: self.output.npz_path.display().to_string(),
            statement_count: self.statement_count,
            embedding_row_count: self.embedding_row_count,
        })
    }

    fn preflight_destinations(&self) -> Result<(), SpeakerTranscriptWriteError> {
        if self.output.redo {
            return Ok(());
        }
        ensure_absent(&self.output.jsonl_path)?;
        if self.npz.is_some() {
            ensure_absent(&self.output.npz_path)?;
        }
        Ok(())
    }
}

impl Output {
    fn parse(value: &Value) -> Result<Self, SpeakerTranscriptWriteError> {
        let object = object(value, "output")?;
        let jsonl_path = PathBuf::from(string(
            required(object, "jsonl_path")?,
            "output.jsonl_path",
        )?);
        let npz_path = PathBuf::from(string(required(object, "npz_path")?, "output.npz_path")?);
        let redo = object
            .get("redo")
            .map_or(Ok(false), |value| boolean(value, "output.redo"))?;
        validate_output_paths(&jsonl_path, &npz_path)?;
        Ok(Self {
            jsonl_path,
            npz_path,
            redo,
        })
    }
}

impl Embeddings {
    fn parse(value: &Value, statements: &[Statement]) -> Result<Self, SpeakerTranscriptWriteError> {
        Self::parse_inner(value, statements).map_err(|error| match error {
            SpeakerTranscriptWriteError::MalformedRequest { detail } => payload_invalid(&detail),
            error => error,
        })
    }

    fn parse_inner(
        value: &Value,
        statements: &[Statement],
    ) -> Result<Self, SpeakerTranscriptWriteError> {
        let object = object(value, "embeddings")?;
        let payload_path = string(required(object, "payload_path")?, "embeddings.payload_path")?;
        let payload_format = string(
            required(object, "payload_format")?,
            "embeddings.payload_format",
        )?;
        if payload_format != "raw-f32le-row-major-v1" {
            return Err(payload_invalid(
                "payload_format must be raw-f32le-row-major-v1",
            ));
        }
        let dtype = string(required(object, "dtype")?, "embeddings.dtype")?;
        if dtype != "float32-le" {
            return Err(payload_invalid("dtype must be float32-le"));
        }
        let shape = array(required(object, "shape")?, "embeddings.shape")?;
        if shape.len() != 2 || unsigned(&shape[1], "embeddings.shape[1]")? != EMBEDDING_WIDTH as u64
        {
            return Err(payload_invalid("shape must be [row_count, 256]"));
        }
        let rows = usize::try_from(unsigned(&shape[0], "embeddings.shape[0]")?)
            .map_err(|_| payload_invalid("shape row count is too large"))?;
        let byte_count = usize::try_from(unsigned(
            required(object, "byte_count")?,
            "embeddings.byte_count",
        )?)
        .map_err(|_| payload_invalid("byte_count is too large"))?;
        let expected_byte_count = rows
            .checked_mul(EMBEDDING_WIDTH)
            .and_then(|count| count.checked_mul(std::mem::size_of::<f32>()))
            .ok_or_else(|| payload_invalid("shape byte count overflows"))?;
        if byte_count != expected_byte_count {
            return Err(payload_invalid("byte_count does not match shape"));
        }
        let statement_ids =
            parse_embedding_ids(required(object, "statement_ids")?, statements, rows)?;
        let durations_s = parse_durations(required(object, "durations_s")?, rows)?;
        let encoder = string(required(object, "encoder")?, "embeddings.encoder")?.to_owned();
        if encoder.is_empty() {
            return Err(payload_invalid("encoder must be nonempty"));
        }
        let bytes = fs::read(payload_path).map_err(|error| {
            SpeakerTranscriptWriteError::PayloadUnreadable {
                path: payload_path.to_owned(),
                detail: error.to_string(),
            }
        })?;
        if bytes.len() != byte_count {
            return Err(payload_invalid("payload size does not match byte_count"));
        }
        let mut values = Vec::with_capacity(rows * EMBEDDING_WIDTH);
        for (index, chunk) in bytes.chunks_exact(std::mem::size_of::<f32>()).enumerate() {
            let value = f32::from_le_bytes(chunk.try_into().map_err(|_| {
                SpeakerTranscriptWriteError::Internal {
                    detail: "invalid f32 payload chunk".to_owned(),
                }
            })?);
            if !value.is_finite() {
                return Err(SpeakerTranscriptWriteError::PayloadNonFinite {
                    row: index / EMBEDDING_WIDTH,
                    col: index % EMBEDDING_WIDTH,
                });
            }
            values.push(value);
        }
        Ok(Self {
            values,
            statement_ids,
            durations_s,
            encoder,
        })
    }
}

fn parse_statements(value: &Value) -> Result<Vec<Statement>, SpeakerTranscriptWriteError> {
    let values = array(value, "statements")?;
    let mut ids = HashSet::with_capacity(values.len());
    values
        .iter()
        .enumerate()
        .map(|(statement_index, value)| {
            let object = object(value, "statement").map_err(|error| match error {
                SpeakerTranscriptWriteError::MalformedRequest { detail } => {
                    SpeakerTranscriptWriteError::InvalidStatement {
                        statement_index,
                        detail,
                    }
                }
                error => error,
            })?;
            let id = match object.get("id") {
                None | Some(Value::Null) => {
                    return Err(SpeakerTranscriptWriteError::MissingStatementId {
                        statement_index,
                    });
                }
                Some(value) => integer(value, "id").map_err(|detail| {
                    SpeakerTranscriptWriteError::InvalidStatementId {
                        statement_index,
                        detail,
                    }
                })?,
            };
            if id <= 0 || id > i64::from(i32::MAX) {
                return Err(SpeakerTranscriptWriteError::InvalidStatementId {
                    statement_index,
                    detail: "id must be in 1..=i32::MAX".to_owned(),
                });
            }
            if !ids.insert(id) {
                return Err(SpeakerTranscriptWriteError::DuplicateStatementId {
                    statement_index,
                    id,
                });
            }
            let start_offset_us = match object.get("start_offset_us") {
                None | Some(Value::Null) => 0,
                Some(value) => integer(value, "start_offset_us").map_err(|detail| {
                    SpeakerTranscriptWriteError::InvalidStatement {
                        statement_index,
                        detail,
                    }
                })?,
            };
            let text = string(required_statement(object, "text", statement_index)?, "text")
                .map_err(|error| invalid_statement(statement_index, error))?
                .to_owned();
            let speaker = match object.get("speaker") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) => Some(Value::String(value.clone())),
                Some(Value::Number(value)) if value.is_i64() || value.is_u64() => {
                    Some(Value::Number(value.clone()))
                }
                Some(_) => {
                    return Err(SpeakerTranscriptWriteError::InvalidStatement {
                        statement_index,
                        detail: "speaker must be a string, integer, null, or absent".to_owned(),
                    });
                }
            };
            Ok(Statement {
                id: id as i32,
                start_offset_us,
                text,
                speaker,
            })
        })
        .collect()
}

fn parse_embedding_ids(
    value: &Value,
    statements: &[Statement],
    rows: usize,
) -> Result<Vec<i32>, SpeakerTranscriptWriteError> {
    let values = array(value, "embeddings.statement_ids")?;
    if values.len() != rows {
        return Err(payload_invalid("statement_ids count does not match shape"));
    }
    let statement_ids: HashSet<_> = statements.iter().map(|statement| statement.id).collect();
    let mut seen = HashSet::with_capacity(values.len());
    values
        .iter()
        .map(|value| {
            let id = integer(value, "embeddings.statement_ids")
                .map_err(|detail| payload_invalid(&detail))?;
            let id =
                i32::try_from(id).map_err(|_| payload_invalid("statement ID is outside i32"))?;
            if id <= 0 || !statement_ids.contains(&id) {
                return Err(payload_invalid(
                    "statement ID is not a transcript statement ID",
                ));
            }
            if !seen.insert(id) {
                return Err(payload_invalid("statement IDs must be unique"));
            }
            Ok(id)
        })
        .collect()
}

fn parse_durations(value: &Value, rows: usize) -> Result<Vec<f32>, SpeakerTranscriptWriteError> {
    let values = array(value, "embeddings.durations_s")?;
    if values.len() != rows {
        return Err(payload_invalid("durations_s count does not match shape"));
    }
    values
        .iter()
        .map(|value| {
            let value = number(value, "embeddings.durations_s")
                .map_err(|error| payload_invalid(&error.detail()))?;
            let value = value as f32;
            if value.is_finite() {
                Ok(value)
            } else {
                Err(payload_invalid("durations_s must be finite f32 values"))
            }
        })
        .collect()
}

fn build_header(value: &Value) -> Result<Value, SpeakerTranscriptWriteError> {
    let input = object(value, "header")?;
    let mut header = Map::new();
    header.insert(
        "raw".to_owned(),
        Value::String(string(required(input, "raw")?, "header.raw")?.to_owned()),
    );
    header.insert(
        "backend".to_owned(),
        Value::String(
            optional_string(input.get("backend"), "header.backend")?
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown")
                .to_owned(),
        ),
    );
    for key in ["model", "device", "compute_type"] {
        header.insert(
            key.to_owned(),
            Value::String(
                optional_string(input.get(key), key)?
                    .unwrap_or("unknown")
                    .to_owned(),
            ),
        );
    }
    if let Some(observer) = optional_string(input.get("observer"), "header.observer")?
        && !observer.is_empty()
    {
        header.insert("observer".to_owned(), Value::String(observer.to_owned()));
    }
    add_vad(input, &mut header)?;
    add_overlap(input, &mut header)?;
    add_speaker_evidence(input, &mut header)?;
    if let Some(value) = input
        .get("speaker_analysis_producer")
        .filter(|value| !value.is_null())
    {
        header.insert(
            "speaker_analysis_producer".to_owned(),
            Value::String(string(value, "header.speaker_analysis_producer")?.to_owned()),
        );
    }
    if let Some(Value::Object(segment_meta)) = input.get("segment_meta") {
        if !segment_meta.is_empty() {
            for (key, value) in segment_meta {
                header.insert(key.clone(), value.clone());
            }
        }
    } else if input
        .get("segment_meta")
        .is_some_and(|value| !value.is_null())
    {
        return Err(SpeakerTranscriptWriteError::InvalidHeader {
            detail: "segment_meta must be an object".to_owned(),
        });
    }
    for key in ["_solstone_processing", "sound_tags"] {
        if let Some(value) = input.get(key).filter(|value| !value.is_null()) {
            header.insert(key.to_owned(), value.clone());
        }
    }
    Ok(Value::Object(header))
}

fn add_vad(
    input: &Map<String, Value>,
    header: &mut Map<String, Value>,
) -> Result<(), SpeakerTranscriptWriteError> {
    let duration = input.get("duration").filter(|value| !value.is_null());
    let noisy = input.get("noisy").filter(|value| !value.is_null());
    match (duration, noisy) {
        (None, None) => return Ok(()),
        (Some(duration), Some(noisy)) => {
            header.insert(
                "duration".to_owned(),
                rounded_value(number(duration, "header.duration")?, 2)?,
            );
            header.insert(
                "noisy".to_owned(),
                Value::Bool(boolean(noisy, "header.noisy")?),
            );
        }
        _ => {
            return Err(SpeakerTranscriptWriteError::InvalidHeader {
                detail: "duration and noisy must be provided together".to_owned(),
            });
        }
    }
    if let Some(noisy_rms) = input.get("noisy_rms").filter(|value| !value.is_null()) {
        header.insert(
            "noisy_rms".to_owned(),
            rounded_value(number(noisy_rms, "header.noisy_rms")?, 4)?,
        );
        let noisy_s = input
            .get("noisy_s")
            .filter(|value| !value.is_null())
            .ok_or_else(|| SpeakerTranscriptWriteError::InvalidHeader {
                detail: "noisy_s is required when noisy_rms is present".to_owned(),
            })?;
        header.insert(
            "noisy_s".to_owned(),
            rounded_value(number(noisy_s, "header.noisy_s")?, 1)?,
        );
    }
    let loud_windows = input.get("loud_windows").filter(|value| !value.is_null());
    if let Some(loud_windows) = loud_windows {
        let loud_windows = unsigned(loud_windows, "header.loud_windows")?;
        if loud_windows > 0 {
            header.insert("loud_windows".to_owned(), Value::from(loud_windows));
            let speech = input
                .get("speech_loud_windows")
                .filter(|value| !value.is_null())
                .ok_or_else(|| SpeakerTranscriptWriteError::InvalidHeader {
                    detail: "speech_loud_windows is required when loud_windows is positive"
                        .to_owned(),
                })?;
            header.insert(
                "speech_loud_windows".to_owned(),
                Value::from(unsigned(speech, "header.speech_loud_windows")?),
            );
            if let Some(ratio) = input
                .get("loud_speech_ratio")
                .filter(|value| !value.is_null())
            {
                header.insert(
                    "loud_speech_ratio".to_owned(),
                    rounded_value(number(ratio, "header.loud_speech_ratio")?, 2)?,
                );
            }
        }
    }
    Ok(())
}

fn add_overlap(
    input: &Map<String, Value>,
    header: &mut Map<String, Value>,
) -> Result<(), SpeakerTranscriptWriteError> {
    let fraction = input
        .get("overlap_fraction")
        .filter(|value| !value.is_null());
    let detector = input
        .get("overlap_detector")
        .filter(|value| !value.is_null());
    if let (Some(fraction), Some(detector)) = (fraction, detector) {
        header.insert(
            "overlap_fraction".to_owned(),
            rounded_value(number(fraction, "header.overlap_fraction")?, 4)?,
        );
        header.insert(
            "overlap_detector".to_owned(),
            Value::String(string(detector, "header.overlap_detector")?.to_owned()),
        );
    }
    Ok(())
}

fn add_speaker_evidence(
    input: &Map<String, Value>,
    header: &mut Map<String, Value>,
) -> Result<(), SpeakerTranscriptWriteError> {
    let Some(evidence) = input
        .get("speaker_evidence")
        .filter(|value| !value.is_null())
    else {
        return Ok(());
    };
    header.insert("speaker_evidence".to_owned(), evidence.clone());
    let fraction = input
        .get("speaker_evidence_multi_fraction")
        .filter(|value| !value.is_null())
        .ok_or_else(|| SpeakerTranscriptWriteError::InvalidHeader {
            detail: "speaker_evidence_multi_fraction is required with speaker_evidence".to_owned(),
        })?;
    header.insert(
        "speaker_evidence_multi_fraction".to_owned(),
        rounded_value(
            number(fraction, "header.speaker_evidence_multi_fraction")?,
            4,
        )?,
    );
    let version = input
        .get("speaker_evidence_version")
        .filter(|value| !value.is_null())
        .ok_or_else(|| SpeakerTranscriptWriteError::InvalidHeader {
            detail: "speaker_evidence_version is required with speaker_evidence".to_owned(),
        })?;
    header.insert(
        "speaker_evidence_version".to_owned(),
        Value::String(string(version, "header.speaker_evidence_version")?.to_owned()),
    );
    Ok(())
}

fn build_jsonl(
    header: &Value,
    statements: &[Statement],
    source: Option<&str>,
    base_time_us_of_day: u64,
) -> Result<Vec<u8>, SpeakerTranscriptWriteError> {
    let mut lines = Vec::with_capacity(statements.len() + 1);
    lines.push(ascii_json::to_string(header));
    for statement in statements {
        let mut entry = Map::new();
        entry.insert(
            "start".to_owned(),
            Value::String(format_start(
                base_time_us_of_day,
                statement.start_offset_us,
            )?),
        );
        entry.insert("text".to_owned(), Value::String(statement.text.clone()));
        if let Some(source) = source.filter(|source| !source.is_empty()) {
            entry.insert("source".to_owned(), Value::String(source.to_owned()));
        }
        if let Some(speaker) = &statement.speaker {
            entry.insert("speaker".to_owned(), speaker.clone());
        }
        entry.insert(
            "sentence_id".to_owned(),
            Value::from(i64::from(statement.id)),
        );
        lines.push(ascii_json::to_string(&Value::Object(entry)));
    }
    Ok(format!("{}\n", lines.join("\n")).into_bytes())
}

fn format_start(
    base_time_us_of_day: u64,
    start_offset_us: i64,
) -> Result<String, SpeakerTranscriptWriteError> {
    let total_us = i128::from(base_time_us_of_day) + i128::from(start_offset_us);
    let display_second = total_us.div_euclid(1_000_000).rem_euclid(86_400);
    let hour = display_second / 3_600;
    let minute = (display_second % 3_600) / 60;
    let second = display_second % 60;
    Ok(format!("{hour:02}:{minute:02}:{second:02}"))
}

fn validate_output_paths(
    jsonl_path: &Path,
    npz_path: &Path,
) -> Result<(), SpeakerTranscriptWriteError> {
    if jsonl_path.as_os_str().is_empty() || npz_path.as_os_str().is_empty() {
        return Err(SpeakerTranscriptWriteError::InvalidOutputPath {
            detail: "output paths must be nonempty".to_owned(),
        });
    }
    let valid = jsonl_path
        .extension()
        .is_some_and(|extension| extension == "jsonl")
        && npz_path
            .extension()
            .is_some_and(|extension| extension == "npz")
        && jsonl_path.parent() == npz_path.parent()
        && jsonl_path.file_stem() == npz_path.file_stem();
    if valid {
        Ok(())
    } else {
        Err(SpeakerTranscriptWriteError::InvalidOutputPath {
            detail: "jsonl_path and npz_path must be sibling files sharing a stem".to_owned(),
        })
    }
}

fn publish_file(path: &Path, bytes: &[u8], redo: bool) -> Result<(), SpeakerTranscriptWriteError> {
    let result = if redo {
        atomic_replace(path, bytes, AtomicWriteOptions::default())
    } else {
        write_bytes_exclusive(path, bytes, AtomicWriteOptions::default())
    };
    result.map_err(|error| atomic_error(path, error))
}

fn ensure_absent(path: &Path) -> Result<(), SpeakerTranscriptWriteError> {
    match path.try_exists() {
        Ok(true) => Err(SpeakerTranscriptWriteError::DestinationExists {
            path: path.display().to_string(),
        }),
        Ok(false) => Ok(()),
        Err(error) => Err(SpeakerTranscriptWriteError::OutputUnwritable {
            path: path.display().to_string(),
            detail: error.to_string(),
        }),
    }
}

fn atomic_error(path: &Path, error: AtomicWriteError) -> SpeakerTranscriptWriteError {
    match error {
        AtomicWriteError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists => {
            SpeakerTranscriptWriteError::DestinationExists {
                path: path.display().to_string(),
            }
        }
        error => SpeakerTranscriptWriteError::OutputUnwritable {
            path: path.display().to_string(),
            detail: error.to_string(),
        },
    }
}

fn rounded_value(value: f64, places: u32) -> Result<Value, SpeakerTranscriptWriteError> {
    if !value.is_finite() {
        return Err(SpeakerTranscriptWriteError::InvalidHeader {
            detail: "header numbers must be finite".to_owned(),
        });
    }
    let scale = 10_f64.powi(places as i32);
    serde_json::Number::from_f64((value * scale).round_ties_even() / scale)
        .map(Value::Number)
        .ok_or_else(|| SpeakerTranscriptWriteError::InvalidHeader {
            detail: "rounded header number is not JSON-representable".to_owned(),
        })
}

fn required<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Value, SpeakerTranscriptWriteError> {
    object
        .get(key)
        .ok_or_else(|| SpeakerTranscriptWriteError::MalformedRequest {
            detail: format!("missing required field {key}"),
        })
}

fn required_statement<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    statement_index: usize,
) -> Result<&'a Value, SpeakerTranscriptWriteError> {
    object
        .get(key)
        .ok_or_else(|| SpeakerTranscriptWriteError::InvalidStatement {
            statement_index,
            detail: format!("missing required field {key}"),
        })
}

fn object<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a Map<String, Value>, SpeakerTranscriptWriteError> {
    value
        .as_object()
        .ok_or_else(|| SpeakerTranscriptWriteError::MalformedRequest {
            detail: format!("{field} must be an object"),
        })
}

fn array<'a>(value: &'a Value, field: &str) -> Result<&'a [Value], SpeakerTranscriptWriteError> {
    value.as_array().map(Vec::as_slice).ok_or_else(|| {
        SpeakerTranscriptWriteError::MalformedRequest {
            detail: format!("{field} must be an array"),
        }
    })
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, SpeakerTranscriptWriteError> {
    value
        .as_str()
        .ok_or_else(|| SpeakerTranscriptWriteError::MalformedRequest {
            detail: format!("{field} must be a string"),
        })
}

fn optional_string<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<Option<&'a str>, SpeakerTranscriptWriteError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => string(value, field).map(Some),
    }
}

fn number(value: &Value, field: &str) -> Result<f64, SpeakerTranscriptWriteError> {
    value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| SpeakerTranscriptWriteError::MalformedRequest {
            detail: format!("{field} must be a finite number"),
        })
}

fn integer(value: &Value, field: &str) -> Result<i64, String> {
    value
        .as_i64()
        .ok_or_else(|| format!("{field} must be an integer"))
}

fn unsigned(value: &Value, field: &str) -> Result<u64, SpeakerTranscriptWriteError> {
    value
        .as_u64()
        .ok_or_else(|| SpeakerTranscriptWriteError::MalformedRequest {
            detail: format!("{field} must be an unsigned integer"),
        })
}

fn boolean(value: &Value, field: &str) -> Result<bool, SpeakerTranscriptWriteError> {
    value
        .as_bool()
        .ok_or_else(|| SpeakerTranscriptWriteError::MalformedRequest {
            detail: format!("{field} must be a boolean"),
        })
}

fn invalid_statement(
    statement_index: usize,
    error: SpeakerTranscriptWriteError,
) -> SpeakerTranscriptWriteError {
    SpeakerTranscriptWriteError::InvalidStatement {
        statement_index,
        detail: error.detail(),
    }
}

fn payload_invalid(detail: &str) -> SpeakerTranscriptWriteError {
    SpeakerTranscriptWriteError::PayloadInvalid {
        path: "embeddings.payload".to_owned(),
        detail: detail.to_owned(),
    }
}
