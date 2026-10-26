// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable JSONL transcript and NPZ embedding-sidecar publication.

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, atomic_replace, install_file, write_bytes_exclusive,
    write_reader_exclusive,
};
use solstone_core_speakers::SpeakerTurnEvidence;

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
    /// Publication stopped before the staged pair was admitted.
    Cancelled,
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
            Self::Cancelled => "cancelled",
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
            Self::Cancelled => "transcript publication was cancelled before admission".to_owned(),
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
    timing: Option<Value>,
    words: Option<Value>,
    speaker_turn_evidence: Option<SpeakerTurnEvidence>,
    turn_boundary_suspect: bool,
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
            // ASCII whitespace only: Unicode trim would refuse U+0085/U+2028/U+2029,
            // which hazard_bytes_are_not_present_raw pins as publishable (escaped).
            if text
                .trim_matches(|character: char| character.is_ascii_whitespace())
                .is_empty()
            {
                return Err(invalid_statement(
                    statement_index,
                    SpeakerTranscriptWriteError::MalformedRequest {
                        detail: "text must be a nonempty string".to_owned(),
                    },
                ));
            }
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
            let speaker_turn_evidence = match object.get("speaker_turn_evidence") {
                None => None,
                Some(value) => Some(SpeakerTurnEvidence::from_value(value).map_err(|error| {
                    SpeakerTranscriptWriteError::InvalidStatement {
                        statement_index,
                        detail: format!("invalid speaker_turn_evidence: {error}"),
                    }
                })?),
            };
            let timing = object
                .get("timing")
                .map(|value| {
                    validate_timing(value, "timing")?;
                    Ok::<_, SpeakerTranscriptWriteError>(value.clone())
                })
                .transpose()
                .map_err(|error| invalid_statement(statement_index, error))?;
            let words = object
                .get("words")
                .map(|value| {
                    validate_words(value)?;
                    Ok::<_, SpeakerTranscriptWriteError>(value.clone())
                })
                .transpose()
                .map_err(|error| invalid_statement(statement_index, error))?;
            let turn_boundary_suspect = match object.get("turn_boundary_suspect") {
                None => false,
                Some(Value::Bool(true)) => true,
                Some(_) => {
                    return Err(SpeakerTranscriptWriteError::InvalidStatement {
                        statement_index,
                        detail: "turn_boundary_suspect must be true if present".to_owned(),
                    });
                }
            };
            Ok(Statement {
                id: id as i32,
                start_offset_us,
                text,
                speaker,
                timing,
                words,
                speaker_turn_evidence,
                turn_boundary_suspect,
            })
        })
        .collect()
}

fn validate_timing(value: &Value, field: &str) -> Result<(), SpeakerTranscriptWriteError> {
    let fields = object(value, field)?;
    for key in ["start", "end"] {
        number(required(fields, key)?, &format!("{field}.{key}"))?;
    }
    Ok(())
}

fn validate_words(value: &Value) -> Result<(), SpeakerTranscriptWriteError> {
    for word in array(value, "words")? {
        let fields = object(word, "word")?;
        string(required(fields, "word")?, "word.word")?;
        validate_timing(word, "word")?;
        let probability = required(fields, "probability")?;
        if !probability.is_null() {
            number(probability, "word.probability")?;
        }
    }
    Ok(())
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
    for key in ["_solstone_processing", "sound_tags", "sound_tag_status"] {
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
        lines.push(statement_line(statement, source, base_time_us_of_day)?);
    }
    Ok(format!("{}\n", lines.join("\n")).into_bytes())
}

fn statement_line(
    statement: &Statement,
    source: Option<&str>,
    base_time_us_of_day: u64,
) -> Result<String, SpeakerTranscriptWriteError> {
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
    if let Some(timing) = &statement.timing {
        entry.insert("timing".to_owned(), timing.clone());
    }
    if let Some(words) = &statement.words {
        entry.insert("words".to_owned(), words.clone());
    }
    if let Some(evidence) = &statement.speaker_turn_evidence {
        entry.insert("speaker_turn_evidence".to_owned(), evidence.to_value());
    }
    if statement.turn_boundary_suspect {
        entry.insert("turn_boundary_suspect".to_owned(), Value::Bool(true));
    }
    Ok(ascii_json::to_string(&Value::Object(entry)))
}

/// Zip staged NPY payload files into one transcript NPZ.
///
/// Each payload chunk polls `cancel`. A true callback is
/// [`SpeakerTranscriptWriteError::Cancelled`] and leaves no admitted NPZ.
pub fn stage_npz_from_files(
    destination: &Path,
    embeddings_f32le: &Path,
    rows: usize,
    statement_ids_i32le: &Path,
    durations_f32le: &Path,
    encoder: &str,
    cancel: &dyn Fn() -> bool,
) -> Result<(), SpeakerTranscriptWriteError> {
    crate::npz::write_npz_from_files(
        destination,
        embeddings_f32le,
        rows,
        statement_ids_i32le,
        durations_f32le,
        encoder,
        cancel,
    )
    .map_err(|detail| {
        if detail == "cancelled" {
            SpeakerTranscriptWriteError::Cancelled
        } else {
            SpeakerTranscriptWriteError::NpzVerificationFailed {
                path: destination.display().to_string(),
                detail,
            }
        }
    })
}

/// One JSONL header line, after the writer's header normalization.
pub fn stage_header_line(header: &Value) -> Result<String, SpeakerTranscriptWriteError> {
    Ok(ascii_json::to_string(&build_header(header)?))
}

/// One JSONL statement line. `statement` is one native request statement object.
pub fn stage_statement_line(
    statement: &Value,
    source: Option<&str>,
    base_time_us_of_day: u64,
) -> Result<String, SpeakerTranscriptWriteError> {
    let statements = parse_statements(&Value::Array(vec![statement.clone()]))?;
    statement_line(
        statements.first().expect("one statement"),
        source,
        base_time_us_of_day,
    )
}

const COPY_CHUNK: usize = 64 * 1024;

static STAGED_COPY_SEQ: AtomicU64 = AtomicU64::new(0);

/// Copy staged JSONL and optional NPZ into their destinations.
///
/// This copy does not observe cancellation. A stop that is already true still
/// publishes both files. Embedded publication uses [`prepare_private_pair`]
/// and [`admit_private_pair`] so the recording-sized copy finishes, and can
/// still be cancelled, before the destination names exist. `redo` replaces;
/// otherwise the destination names must be absent. Bytes are copied in 64 KiB
/// chunks.
pub fn publish_staged_pair(
    staged_jsonl: &Path,
    staged_npz: Option<&Path>,
    jsonl_path: &Path,
    npz_path: &Path,
    redo: bool,
    statement_count: usize,
    embedding_row_count: usize,
) -> Result<WriteResponse, SpeakerTranscriptWriteError> {
    validate_output_paths(jsonl_path, npz_path)?;
    if !redo {
        ensure_absent(jsonl_path)?;
        if staged_npz.is_some() {
            ensure_absent(npz_path)?;
        }
    }
    if let Some(staged_npz) = staged_npz {
        copy_published(staged_npz, npz_path, redo)?;
        crate::npz::verify_npz_file(npz_path).map_err(|detail| {
            SpeakerTranscriptWriteError::NpzVerificationFailed {
                path: npz_path.display().to_string(),
                detail,
            }
        })?;
    }
    copy_published(staged_jsonl, jsonl_path, redo)?;
    Ok(WriteResponse {
        jsonl_path: jsonl_path.display().to_string(),
        npz_path: npz_path.display().to_string(),
        statement_count,
        embedding_row_count,
    })
}

fn copy_published(
    source: &Path,
    destination: &Path,
    redo: bool,
) -> Result<(), SpeakerTranscriptWriteError> {
    if redo {
        let parent =
            destination
                .parent()
                .ok_or_else(|| SpeakerTranscriptWriteError::InvalidOutputPath {
                    detail: "output path has no parent".to_owned(),
                })?;
        fs::create_dir_all(parent).map_err(|error| {
            SpeakerTranscriptWriteError::OutputUnwritable {
                path: destination.display().to_string(),
                detail: error.to_string(),
            }
        })?;
        let temporary = parent.join(format!(
            ".solstone-stage-{}-{}",
            std::process::id(),
            STAGED_COPY_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let copy = (|| -> Result<(), SpeakerTranscriptWriteError> {
            stream_copy(source, &temporary)?;
            install_file(&temporary, destination, AtomicWriteOptions::default())
                .map_err(|error| atomic_error(destination, error))
        })();
        if copy.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return copy;
    }
    let mut reader =
        File::open(source).map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
            path: source.display().to_string(),
            detail: error.to_string(),
        })?;
    write_reader_exclusive(destination, &mut reader, AtomicWriteOptions::default())
        .map(|_| ())
        .map_err(|error| atomic_error(destination, error))
}

fn stream_copy(source: &Path, destination: &Path) -> Result<(), SpeakerTranscriptWriteError> {
    let mut input =
        File::open(source).map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
            path: source.display().to_string(),
            detail: error.to_string(),
        })?;
    let mut output = File::create(destination).map_err(|error| {
        SpeakerTranscriptWriteError::OutputUnwritable {
            path: destination.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    let mut buffer = [0_u8; COPY_CHUNK];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            SpeakerTranscriptWriteError::OutputUnwritable {
                path: source.display().to_string(),
                detail: error.to_string(),
            }
        })?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            SpeakerTranscriptWriteError::OutputUnwritable {
                path: destination.display().to_string(),
                detail: error.to_string(),
            }
        })?;
    }
    output
        .sync_all()
        .map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
            path: destination.display().to_string(),
            detail: error.to_string(),
        })
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

/// One word of a streamed statement line. Only this word is resident.
#[derive(Clone)]
pub struct StagedWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub probability: Option<f64>,
}

/// Fields around the streamed text of one statement line.
pub struct StagedStatementLine<'a> {
    pub start_offset_us: i64,
    pub base_time_us_of_day: u64,
    pub source: Option<&'a str>,
    pub speaker: Option<i64>,
    pub sentence_id: i64,
    pub timing: Option<(f64, f64)>,
    pub turn_evidence: Option<&'a SpeakerTurnEvidence>,
    pub turn_boundary_suspect: bool,
}

/// Write one JSONL statement object.
///
/// `text` is the already-trimmed statement text. It is escaped in chunks and
/// is not wrapped in one `String`. `next_word` yields one word at a time.
pub fn write_staged_statement_line(
    output: &mut impl Write,
    fields: &StagedStatementLine<'_>,
    text: &mut impl Read,
    text_len: u64,
    mut next_word: impl FnMut() -> Result<Option<StagedWord>, SpeakerTranscriptWriteError>,
) -> Result<(), SpeakerTranscriptWriteError> {
    let start = format_start(fields.base_time_us_of_day, fields.start_offset_us)?;
    write_staged(output, b"{")?;
    write_json_key(output, "start")?;
    write_json_string(output, &start)?;
    write_staged(output, b",")?;
    write_json_key(output, "text")?;
    write_quoted_reader(output, text, text_len)?;
    if let Some(source) = fields.source.filter(|source| !source.is_empty()) {
        write_staged(output, b",")?;
        write_json_key(output, "source")?;
        write_json_string(output, source)?;
    }
    if let Some(speaker) = fields.speaker {
        write_staged(output, b",")?;
        write_json_key(output, "speaker")?;
        write!(output, "{speaker}").map_err(staged_io)?;
    }
    write_staged(output, b",")?;
    write_json_key(output, "sentence_id")?;
    write!(output, "{}", fields.sentence_id).map_err(staged_io)?;
    if let Some((start_s, end_s)) = fields.timing {
        write_staged(output, b",")?;
        write_json_key(output, "timing")?;
        let timing = serde_json::json!({"start": start_s, "end": end_s});
        write_staged(output, ascii_json::to_string(&timing).as_bytes())?;
    }
    write_staged(output, b",\"words\":[")?;
    let mut any_word = false;
    while let Some(word) = next_word()? {
        if any_word {
            write_staged(output, b",")?;
        }
        any_word = true;
        let value = serde_json::json!({
            "word": word.word,
            "start": word.start,
            "end": word.end,
            "probability": word.probability,
        });
        write_staged(output, ascii_json::to_string(&value).as_bytes())?;
    }
    write_staged(output, b"]")?;
    if let Some(evidence) = fields.turn_evidence {
        write_staged(output, b",")?;
        write_json_key(output, "speaker_turn_evidence")?;
        write_staged(
            output,
            ascii_json::to_string(&evidence.to_value()).as_bytes(),
        )?;
    }
    if fields.turn_boundary_suspect {
        write_staged(output, b",\"turn_boundary_suspect\":true")?;
    }
    write_staged(output, b"}")
}

/// Private copies of a staged transcript. Dropping this removes them.
pub struct PrivatePair {
    jsonl: Option<PathBuf>,
    npz: Option<PathBuf>,
}

impl Drop for PrivatePair {
    fn drop(&mut self) {
        if let Some(path) = self.jsonl.take() {
            let _ = fs::remove_file(path);
        }
        if let Some(path) = self.npz.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Copy and verify the staged pair in the destination directory.
///
/// Every 64 KiB read polls `cancel`. The public JSONL and NPZ names are not
/// created. Cancellation removes the private files.
pub fn prepare_private_pair(
    staged_jsonl: &Path,
    staged_npz: Option<&Path>,
    destination_dir: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<PrivatePair, SpeakerTranscriptWriteError> {
    publication_cancelled(cancel)?;
    fs::create_dir_all(destination_dir).map_err(|error| {
        SpeakerTranscriptWriteError::OutputUnwritable {
            path: destination_dir.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    let seq = STAGED_COPY_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut pair = PrivatePair {
        jsonl: None,
        npz: None,
    };
    let jsonl = destination_dir.join(format!(
        ".solstone-private-{}-{seq}.jsonl",
        std::process::id()
    ));
    pair.jsonl = Some(jsonl.clone());
    copy_cancellable(staged_jsonl, &jsonl, cancel)?;
    compare_private(staged_jsonl, &jsonl, cancel)?;
    if let Some(staged_npz) = staged_npz {
        let npz = destination_dir.join(format!(
            ".solstone-private-{}-{seq}.npz",
            std::process::id()
        ));
        pair.npz = Some(npz.clone());
        copy_cancellable(staged_npz, &npz, cancel)?;
        compare_private(staged_npz, &npz, cancel)?;
        publication_cancelled(cancel)?;
        crate::npz::verify_npz_file(&npz).map_err(|detail| {
            SpeakerTranscriptWriteError::NpzVerificationFailed {
                path: npz.display().to_string(),
                detail,
            }
        })?;
    }
    Ok(pair)
}

/// Install an already verified private pair.
///
/// The caller has taken the last cancellation check. This renames the private
/// files into the public names and does not read the recording again. NPZ is
/// installed first. A failed JSONL install removes that public NPZ.
pub fn admit_private_pair(
    mut pair: PrivatePair,
    jsonl_path: &Path,
    npz_path: &Path,
    redo: bool,
    statement_count: usize,
    embedding_row_count: usize,
) -> Result<WriteResponse, SpeakerTranscriptWriteError> {
    validate_output_paths(jsonl_path, npz_path)?;
    let mut installed_npz = false;
    if let Some(source) = pair.npz.take() {
        if !redo {
            if let Err(error) = ensure_absent(npz_path) {
                let _ = fs::remove_file(&source);
                return Err(error);
            }
        }
        if let Err(error) = install_copied(&source, npz_path) {
            let _ = fs::remove_file(&source);
            return Err(error);
        }
        installed_npz = true;
    }
    let Some(source) = pair.jsonl.take() else {
        if installed_npz {
            let _ = fs::remove_file(npz_path);
        }
        return Err(SpeakerTranscriptWriteError::Internal {
            detail: "private transcript jsonl is missing".to_owned(),
        });
    };
    if !redo {
        if let Err(error) = ensure_absent(jsonl_path) {
            let _ = fs::remove_file(&source);
            if installed_npz {
                let _ = fs::remove_file(npz_path);
            }
            return Err(error);
        }
    }
    if let Err(error) = install_copied(&source, jsonl_path) {
        let _ = fs::remove_file(&source);
        if installed_npz {
            let _ = fs::remove_file(npz_path);
        }
        return Err(error);
    }
    Ok(WriteResponse {
        jsonl_path: jsonl_path.display().to_string(),
        npz_path: npz_path.display().to_string(),
        statement_count,
        embedding_row_count,
    })
}

fn install_copied(source: &Path, destination: &Path) -> Result<(), SpeakerTranscriptWriteError> {
    let parent =
        destination
            .parent()
            .ok_or_else(|| SpeakerTranscriptWriteError::InvalidOutputPath {
                detail: "output path has no parent".to_owned(),
            })?;
    fs::create_dir_all(parent).map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
        path: destination.display().to_string(),
        detail: error.to_string(),
    })?;
    install_file(source, destination, AtomicWriteOptions::default())
        .map_err(|error| atomic_error(destination, error))
}

fn copy_cancellable(
    source: &Path,
    destination: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<(), SpeakerTranscriptWriteError> {
    publication_cancelled(cancel)?;
    let mut input =
        File::open(source).map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
            path: source.display().to_string(),
            detail: error.to_string(),
        })?;
    let mut output = File::create(destination).map_err(|error| {
        SpeakerTranscriptWriteError::OutputUnwritable {
            path: destination.display().to_string(),
            detail: error.to_string(),
        }
    })?;
    let mut buffer = [0_u8; COPY_CHUNK];
    loop {
        publication_cancelled(cancel)?;
        let read = input.read(&mut buffer).map_err(|error| {
            SpeakerTranscriptWriteError::OutputUnwritable {
                path: source.display().to_string(),
                detail: error.to_string(),
            }
        })?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            SpeakerTranscriptWriteError::OutputUnwritable {
                path: destination.display().to_string(),
                detail: error.to_string(),
            }
        })?;
    }
    output
        .sync_all()
        .map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
            path: destination.display().to_string(),
            detail: error.to_string(),
        })
}

fn compare_private(
    left: &Path,
    right: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<(), SpeakerTranscriptWriteError> {
    let mut left_file =
        File::open(left).map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
            path: left.display().to_string(),
            detail: error.to_string(),
        })?;
    let mut right_file =
        File::open(right).map_err(|error| SpeakerTranscriptWriteError::OutputUnwritable {
            path: right.display().to_string(),
            detail: error.to_string(),
        })?;
    let mut left_buffer = [0_u8; COPY_CHUNK];
    let mut right_buffer = [0_u8; COPY_CHUNK];
    loop {
        publication_cancelled(cancel)?;
        let left_read = left_file.read(&mut left_buffer).map_err(|error| {
            SpeakerTranscriptWriteError::OutputUnwritable {
                path: left.display().to_string(),
                detail: error.to_string(),
            }
        })?;
        let right_read = right_file.read(&mut right_buffer).map_err(|error| {
            SpeakerTranscriptWriteError::OutputUnwritable {
                path: right.display().to_string(),
                detail: error.to_string(),
            }
        })?;
        if left_read == 0 && right_read == 0 {
            return Ok(());
        }
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Err(SpeakerTranscriptWriteError::Internal {
                detail: "private transcript copy did not match the staged file".to_owned(),
            });
        }
    }
}

fn publication_cancelled(cancel: &dyn Fn() -> bool) -> Result<(), SpeakerTranscriptWriteError> {
    if cancel() {
        Err(SpeakerTranscriptWriteError::Cancelled)
    } else {
        Ok(())
    }
}

fn write_staged(output: &mut impl Write, bytes: &[u8]) -> Result<(), SpeakerTranscriptWriteError> {
    output.write_all(bytes).map_err(staged_io)
}

fn write_json_key(output: &mut impl Write, key: &str) -> Result<(), SpeakerTranscriptWriteError> {
    write!(output, "\"{key}\":").map_err(staged_io)
}

fn write_json_string(
    output: &mut impl Write,
    value: &str,
) -> Result<(), SpeakerTranscriptWriteError> {
    output.write_all(b"\"").map_err(staged_io)?;
    ascii_json::write_escaped_to(output, value).map_err(staged_io)?;
    output.write_all(b"\"").map_err(staged_io)
}

fn write_quoted_reader(
    output: &mut impl Write,
    input: &mut impl Read,
    len: u64,
) -> Result<(), SpeakerTranscriptWriteError> {
    output.write_all(b"\"").map_err(staged_io)?;
    let mut left = len;
    let mut pending = [0_u8; 3];
    let mut pending_len = 0_usize;
    let mut raw = [0_u8; 64 * 1024 + 3];
    while left > 0 {
        raw[..pending_len].copy_from_slice(&pending[..pending_len]);
        let want = std::cmp::min(64 * 1024, left) as usize;
        let read = input
            .read(&mut raw[pending_len..pending_len + want])
            .map_err(staged_io)?;
        if read == 0 {
            return Err(SpeakerTranscriptWriteError::InvalidStatement {
                statement_index: 0,
                detail: "statement text ended before its trimmed length".to_owned(),
            });
        }
        left -= read as u64;
        let filled = pending_len + read;
        let split = if left == 0 {
            filled
        } else {
            utf8_floor(&raw[..filled])
        };
        let text = std::str::from_utf8(&raw[..split]).map_err(|_| {
            SpeakerTranscriptWriteError::InvalidStatement {
                statement_index: 0,
                detail: "statement text is not valid utf-8".to_owned(),
            }
        })?;
        ascii_json::write_escaped_to(output, text).map_err(staged_io)?;
        pending_len = filled - split;
        pending[..pending_len].copy_from_slice(&raw[split..filled]);
    }
    if pending_len != 0 {
        return Err(SpeakerTranscriptWriteError::InvalidStatement {
            statement_index: 0,
            detail: "statement text is not valid utf-8".to_owned(),
        });
    }
    output.write_all(b"\"").map_err(staged_io)
}

fn utf8_floor(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let mut index = bytes.len() - 1;
    let mut continuations = 0_usize;
    while bytes[index] & 0b1100_0000 == 0b1000_0000 {
        continuations += 1;
        if index == 0 || continuations == 3 {
            break;
        }
        index -= 1;
    }
    let width = match bytes[index] {
        byte if byte & 0b1000_0000 == 0 => 1,
        byte if byte & 0b1110_0000 == 0b1100_0000 => 2,
        byte if byte & 0b1111_0000 == 0b1110_0000 => 3,
        byte if byte & 0b1111_1000 == 0b1111_0000 => 4,
        _ => return index,
    };
    if width == continuations + 1 {
        bytes.len()
    } else {
        index
    }
}

fn staged_io(error: io::Error) -> SpeakerTranscriptWriteError {
    SpeakerTranscriptWriteError::OutputUnwritable {
        path: "staged-statement".to_owned(),
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod staged_line_tests {
    use super::*;

    #[test]
    fn streamed_statement_matches_the_parsed_line() {
        let native = serde_json::json!({
            "id": 1,
            "start_offset_us": 1_500_000,
            "text": "hello \"there\".",
            "speaker": 2,
            "timing": {"start": 1.5, "end": 2.25},
            "words": [
                {"word": " hello", "start": 1.5, "end": 1.8, "probability": null},
                {"word": " \"there\".", "start": 1.8, "end": 2.25, "probability": 0.5}
            ]
        });
        let expected = stage_statement_line(&native, Some("mic"), 0).unwrap();
        let text = "hello \"there\".";
        let words = [
            StagedWord {
                word: " hello".to_owned(),
                start: 1.5,
                end: 1.8,
                probability: None,
            },
            StagedWord {
                word: " \"there\".".to_owned(),
                start: 1.8,
                end: 2.25,
                probability: Some(0.5),
            },
        ];
        let mut index = 0_usize;
        let mut actual = Vec::new();
        write_staged_statement_line(
            &mut actual,
            &StagedStatementLine {
                start_offset_us: 1_500_000,
                base_time_us_of_day: 0,
                source: Some("mic"),
                speaker: Some(2),
                sentence_id: 1,
                timing: Some((1.5, 2.25)),
                turn_evidence: None,
                turn_boundary_suspect: false,
            },
            &mut io::Cursor::new(text.as_bytes()),
            text.len() as u64,
            &mut || {
                if index >= words.len() {
                    return Ok(None);
                }
                let word = words[index].clone();
                index += 1;
                Ok(Some(word))
            },
        )
        .unwrap();
        assert_eq!(String::from_utf8(actual).unwrap(), expected);
    }
}
