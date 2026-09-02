// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generic `.txt` and `.md` transcript import.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient,
};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, HealthMarkerKind, SegmentDeconflictError,
    bump_stream_marker, find_available_segment_with_occupied, health_marker_path, write_jsonl,
};

use crate::ModelDetectionError;

const PRIVATE_IMPORT_FILE_MODE: u32 = 0o600;
const SEGMENT_PROMPT: &str = include_str!("text_assets/detect_transcript_segment.md");
const SEGMENT_SCHEMA: &str = include_str!("text_assets/detect_transcript_segment.schema.json");
const JSON_PROMPT: &str = include_str!("text_assets/detect_transcript_json.md");
const JSON_SCHEMA: &str = include_str!("text_assets/detect_transcript_json.schema.json");

/// Which model-boundary call failed to communicate with the sibling process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextWirePhase {
    SegmentBoundary,
    SegmentJson,
}

/// Failure while importing a generic transcript.
#[derive(Debug)]
pub enum TextImportError {
    UnsupportedFormat {
        path: PathBuf,
    },
    SourceRead {
        path: PathBuf,
        source: std::io::Error,
    },
    RawFilename {
        path: PathBuf,
    },
    InvalidTime {
        value: String,
    },
    Wire {
        phase: TextWirePhase,
        source: ClientError,
    },
    NegativeDuration {
        duration: i128,
        time_part: String,
    },
    SegmentDeconflict(SegmentDeconflictError),
    SegmentKeyUnavailable {
        candidate: String,
    },
    /// The segmentation adapter is absent, so no segment boundary could be decided.
    ///
    /// This is deliberately an error rather than an empty success. Returning zero segments
    /// here would print a completion banner over an import that wrote nothing, which an owner
    /// cannot tell apart from a transcript that genuinely had no segments.
    SegmentationUnavailable,
    Write {
        path: PathBuf,
        source: AtomicWriteError,
    },
    StreamMarker {
        path: PathBuf,
        day: String,
        source: AtomicWriteError,
    },
    RawCopy {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for TextImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { .. } => formatter.write_str("unsupported transcript format"),
            Self::SourceRead { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::RawFilename { path } => {
                write!(
                    formatter,
                    "transcript path has no UTF-8 basename: {}",
                    path.display()
                )
            }
            Self::InvalidTime { value } => write!(formatter, "invalid transcript time: {value}"),
            Self::Wire { phase, source } => {
                write!(formatter, "{phase:?} generate wire failed: {source}")
            }
            Self::NegativeDuration {
                duration,
                time_part,
            } => write!(
                formatter,
                "Invalid segment duration: {duration}s for segment at {time_part}. Timestamps may be out of order or audio_duration is incorrect."
            ),
            Self::SegmentDeconflict(source) => source.fmt(formatter),
            Self::SegmentKeyUnavailable { candidate } => {
                write!(formatter, "no available segment key for {candidate}")
            }
            Self::SegmentationUnavailable => formatter.write_str(
                "generic text import requires a native segmentation adapter; nothing was imported",
            ),
            Self::Write { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::StreamMarker { path, day, source } => write!(
                formatter,
                "{}: generic text content for {day} remains written, but could not advance stream marker: {source}",
                path.display()
            ),
            Self::RawCopy { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for TextImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceRead { source, .. } => Some(source),
            Self::SegmentDeconflict(source) => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::StreamMarker { source, .. } => Some(source),
            Self::RawCopy { source, .. } => Some(source),
            Self::UnsupportedFormat { .. }
            | Self::RawFilename { .. }
            | Self::InvalidTime { .. }
            | Self::Wire { .. }
            | Self::NegativeDuration { .. }
            | Self::SegmentKeyUnavailable { .. }
            | Self::SegmentationUnavailable => None,
        }
    }
}

/// Model-boundary seam shared by transcript segmentation and normalization.
pub trait WireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError>;
}

/// Production client for the sibling `solstone-core generate` process.
pub struct SystemWireClient;

impl WireClient for SystemWireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        OneShotClient::sibling()?.execute(request)
    }
}

/// Process a generic transcript using the production generate boundary.
///
/// `start_time` remains a raw `HH:MM:SS` value to preserve the Python import
/// contract exactly. `audio_duration` is currently supplied by no native
/// dispatcher, but remains public so the final-segment duration contract is
/// available to its future owner. A refused or unparseable boundary-detection
/// response returns an empty result without writing files; a refused or
/// unparseable per-segment response skips only that segment and continues.
#[allow(clippy::too_many_arguments)]
pub fn process_transcript(
    path: &Path,
    day_dir: &Path,
    start_time: &str,
    import_id: &str,
    stream: &str,
    facet: Option<&str>,
    setting: Option<&str>,
    audio_duration: Option<u64>,
) -> Result<Vec<PathBuf>, TextImportError> {
    process_transcript_with_wire(
        path,
        day_dir,
        start_time,
        import_id,
        stream,
        facet,
        setting,
        audio_duration,
        &SystemWireClient,
    )
}

/// Process a generic transcript with an injected generate-boundary client.
///
/// This has the same refusal behavior as [`process_transcript`].
#[allow(clippy::too_many_arguments)]
pub fn process_transcript_with_wire(
    path: &Path,
    day_dir: &Path,
    start_time: &str,
    import_id: &str,
    stream: &str,
    facet: Option<&str>,
    setting: Option<&str>,
    audio_duration: Option<u64>,
    wire: &dyn WireClient,
) -> Result<Vec<PathBuf>, TextImportError> {
    let text = read_transcript(path)?;
    let raw_filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| TextImportError::RawFilename {
            path: path.to_path_buf(),
        })?;
    let (journal_root, day) = journal_marker_context(day_dir)?;
    stage_raw_source(day_dir, import_id, path, raw_filename)?;
    let (segments, native_fallback) = match segment_transcript(wire, &text, start_time) {
        Ok(segments) => (segments, false),
        Err(ModelDetectionError::Unavailable) => (whole_file_segment(&text, start_time), true),
        Err(ModelDetectionError::Failed(ClientError::Resolve(_))) => {
            (whole_file_segment(&text, start_time), true)
        }
        Err(ModelDetectionError::Failed(source)) => {
            return Err(TextImportError::Wire {
                phase: TextWirePhase::SegmentBoundary,
                source,
            });
        }
    };

    let recording_start_seconds = time_to_seconds(start_time)?;
    let parent = day_dir.join(stream);
    let mut occupied = HashSet::new();
    let mut created = Vec::new();

    for (index, segment) in segments.iter().enumerate() {
        let wrapper = match normalize_segment(wire, &segment.text, &segment.start_at) {
            Ok(wrapper) => wrapper,
            Err(ModelDetectionError::Unavailable)
            | Err(ModelDetectionError::Failed(ClientError::Resolve(_)))
                if native_fallback =>
            {
                raw_text_wrapper(segment)
            }
            Err(ModelDetectionError::Unavailable) => continue,
            Err(ModelDetectionError::Failed(source)) => {
                return Err(TextImportError::Wire {
                    phase: TextWirePhase::SegmentJson,
                    source,
                });
            }
        };

        let segment_start_seconds = time_to_seconds(&segment.start_at)?;
        let mut entries = wrapper.entries;
        relativize_entries(&mut entries, segment_start_seconds);

        let duration = if let Some(next) = segments.get(index + 1) {
            i128::from(time_to_seconds(&next.start_at)?) - i128::from(segment_start_seconds)
        } else if let Some(audio_duration) = audio_duration.filter(|duration| *duration != 0) {
            i128::from(audio_duration)
                - (i128::from(segment_start_seconds) - i128::from(recording_start_seconds))
        } else {
            5
        };
        let time_part = segment.start_at.replace(':', "");
        if duration < 0 {
            return Err(TextImportError::NegativeDuration {
                duration,
                time_part,
            });
        }
        let candidate = format!("{time_part}_{}", duration.max(1));
        let Some(segment_key) =
            find_available_segment_with_occupied(&parent, &candidate, 100, &occupied)
                .map_err(TextImportError::SegmentDeconflict)?
        else {
            return Err(TextImportError::SegmentKeyUnavailable { candidate });
        };

        let output = parent
            .join(&segment_key)
            .join("conversation_transcript.jsonl");
        let rows = jsonl_rows(
            entries,
            import_id,
            raw_filename,
            facet,
            setting,
            wrapper.topics.as_deref(),
            wrapper.setting.as_deref(),
        );
        write_jsonl(
            &output,
            rows,
            AtomicWriteOptions {
                mode: Some(PRIVATE_IMPORT_FILE_MODE),
            },
        )
        .map_err(|source| TextImportError::Write {
            path: output.clone(),
            source,
        })?;
        bump_stream_marker(journal_root, day).map_err(|source| TextImportError::StreamMarker {
            path: health_marker_path(journal_root, day, HealthMarkerKind::Stream),
            day: day.to_owned(),
            source,
        })?;
        occupied.insert(segment_key);
        created.push(output);
    }

    Ok(created)
}

fn journal_marker_context(day_dir: &Path) -> Result<(&Path, &str), TextImportError> {
    let Some(journal_root) = day_dir.parent().and_then(Path::parent) else {
        return Err(TextImportError::RawCopy {
            path: day_dir.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "day directory has no journal root",
            ),
        });
    };
    let Some(day) = day_dir.file_name().and_then(|name| name.to_str()) else {
        return Err(TextImportError::RawCopy {
            path: day_dir.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "day directory has no UTF-8 day name",
            ),
        });
    };
    Ok((journal_root, day))
}

struct Segment {
    start_at: String,
    text: String,
}

fn whole_file_segment(text: &str, start_time: &str) -> Vec<Segment> {
    vec![Segment {
        start_at: start_time.to_owned(),
        text: text.to_owned(),
    }]
}

fn raw_text_wrapper(segment: &Segment) -> TranscriptWrapper {
    TranscriptWrapper {
        entries: vec![json!({
            "start": segment.start_at,
            "text": segment.text,
        })],
        topics: None,
        setting: None,
    }
}

struct TranscriptWrapper {
    entries: Vec<Value>,
    topics: Option<String>,
    setting: Option<String>,
}

fn read_transcript(path: &Path) -> Result<String, TextImportError> {
    let supported = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "txt" | "md"));
    if !supported {
        return Err(TextImportError::UnsupportedFormat {
            path: path.to_path_buf(),
        });
    }
    fs::read_to_string(path).map_err(|source| TextImportError::SourceRead {
        path: path.to_path_buf(),
        source,
    })
}

fn segment_transcript(
    wire: &dyn WireClient,
    text: &str,
    start_time: &str,
) -> Result<Vec<Segment>, ModelDetectionError<ClientError>> {
    let lines: Vec<&str> = text.lines().collect();
    let numbered = lines
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{}: {line}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    let request = generate_request(
        "observe.detect.segment",
        format!("START_TIME: {start_time}\n{numbered}"),
        SEGMENT_PROMPT,
        SEGMENT_SCHEMA,
        4096,
    );
    let response = generated_text(wire.execute(&request))?;
    parse_segments(&response, &lines).ok_or(ModelDetectionError::Unavailable)
}

fn normalize_segment(
    wire: &dyn WireClient,
    text: &str,
    segment_start: &str,
) -> Result<TranscriptWrapper, ModelDetectionError<ClientError>> {
    let request = generate_request(
        "observe.detect.json",
        format!("SEGMENT_START: {segment_start}\n{text}"),
        JSON_PROMPT,
        JSON_SCHEMA,
        8192,
    );
    let response = generated_text(wire.execute(&request))?;
    parse_wrapper(&response).ok_or(ModelDetectionError::Unavailable)
}

fn generated_text(
    response: Result<GenerateResponse, ClientError>,
) -> Result<String, ModelDetectionError<ClientError>> {
    match response {
        Ok(GenerateResponse::Generated(response)) => Ok(response.text),
        Ok(GenerateResponse::Refused(_)) => Err(ModelDetectionError::Unavailable),
        Err(source) => Err(ModelDetectionError::Failed(source)),
    }
}

fn generate_request(
    context: &str,
    contents: String,
    prompt: &str,
    schema: &str,
    max_output_tokens: u64,
) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: context.to_owned(),
        contents: vec![ContentPart::Text { text: contents }],
        system_instruction: Some(prompt.to_owned()),
        temperature: 0.3,
        max_output_tokens,
        thinking_budget: Some(8192),
        timeout_s: None,
        json_output: true,
        json_schema: serde_json::from_str(schema)
            .expect("vendored transcript schema is valid JSON"),
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn parse_segments(response: &str, lines: &[&str]) -> Option<Vec<Segment>> {
    let value: Value = serde_json::from_str(response).ok()?;
    let boundaries = match value {
        Value::Object(mut object) => object.remove("segments")?,
        Value::Array(_) => value,
        _ => return None,
    };
    let boundaries = boundaries.as_array()?;
    if boundaries.is_empty() {
        return None;
    }
    let mut parsed = Vec::with_capacity(boundaries.len());
    let mut last_line = 0_usize;
    for boundary in boundaries {
        let object = boundary.as_object()?;
        let start_at = object.get("start_at")?.as_str()?.to_owned();
        let line = usize::try_from(object.get("line")?.as_u64()?).ok()?;
        if line < 1 || line > lines.len() || line <= last_line {
            return None;
        }
        parsed.push((start_at, line));
        last_line = line;
    }
    Some(
        parsed
            .iter()
            .enumerate()
            .map(|(index, (start_at, start_line))| {
                let end_line = parsed
                    .get(index + 1)
                    .map_or(lines.len(), |(_, next_line)| next_line - 1);
                Segment {
                    start_at: start_at.clone(),
                    text: lines[start_line - 1..end_line].join("\n").trim().to_owned(),
                }
            })
            .collect(),
    )
}

fn parse_wrapper(response: &str) -> Option<TranscriptWrapper> {
    let mut object = serde_json::from_str::<Value>(response)
        .ok()?
        .as_object()?
        .clone();
    let entries = object.remove("entries")?.as_array()?.clone();
    if entries.iter().any(|entry| !entry.is_object()) {
        return None;
    }
    let mut text_field = |field: &str| {
        object
            .remove(field)
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|value| !value.is_empty())
    };
    Some(TranscriptWrapper {
        entries,
        topics: text_field("topics"),
        setting: text_field("setting"),
    })
}

fn time_to_seconds(value: &str) -> Result<i64, TextImportError> {
    let mut parts = value.split(':');
    let parse = |part: Option<&str>| part.and_then(|part| part.parse::<i64>().ok());
    let (Some(hours), Some(minutes), Some(seconds), None) = (
        parse(parts.next()),
        parse(parts.next()),
        parse(parts.next()),
        parts.next(),
    ) else {
        return Err(TextImportError::InvalidTime {
            value: value.to_owned(),
        });
    };
    Ok(hours * 3600 + minutes * 60 + seconds)
}

fn relativize_entries(entries: &mut [Value], segment_start_seconds: i64) {
    for entry in entries {
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        let Some(start) = object.get("start").and_then(Value::as_str) else {
            continue;
        };
        let Ok(entry_seconds) = time_to_seconds(start) else {
            continue;
        };
        let offset = (entry_seconds - segment_start_seconds).max(0);
        object.insert(
            "start".to_owned(),
            Value::String(format!(
                "{:02}:{:02}:{:02}",
                offset / 3600,
                offset % 3600 / 60,
                offset % 60
            )),
        );
    }
}

/// Copy the owner's source next to the destination-independent `raw` pointer.
///
/// The transcript header always records `../../../imports/{id}/{filename}`.
/// From a segment file that resolves to `{journal}/imports/{id}/{filename}`.
/// Recording the pointer without this copy is a dangling provenance link.
fn stage_raw_source(
    day_dir: &Path,
    import_id: &str,
    source: &Path,
    raw_filename: &str,
) -> Result<(), TextImportError> {
    let Some(journal_root) = day_dir.parent().and_then(Path::parent) else {
        return Err(TextImportError::RawCopy {
            path: day_dir.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "day directory has no journal root",
            ),
        });
    };
    let dest_dir = journal_root.join("imports").join(import_id);
    fs::create_dir_all(&dest_dir).map_err(|source| TextImportError::RawCopy {
        path: dest_dir.clone(),
        source,
    })?;
    let dest = dest_dir.join(raw_filename);
    if dest.exists() {
        return Ok(());
    }
    fs::copy(source, &dest).map_err(|source| TextImportError::RawCopy {
        path: dest.clone(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(PRIVATE_IMPORT_FILE_MODE));
    }
    Ok(())
}

fn jsonl_rows(
    mut entries: Vec<Value>,
    import_id: &str,
    raw_filename: &str,
    facet: Option<&str>,
    caller_setting: Option<&str>,
    topics: Option<&str>,
    detected_setting: Option<&str>,
) -> Vec<Value> {
    let mut imported = Map::new();
    imported.insert("id".to_owned(), Value::String(import_id.to_owned()));
    if let Some(facet) = facet.filter(|value| !value.is_empty()) {
        imported.insert("facet".to_owned(), Value::String(facet.to_owned()));
    }
    if let Some(setting) = caller_setting.filter(|value| !value.is_empty()) {
        imported.insert("setting".to_owned(), Value::String(setting.to_owned()));
    }
    let mut header = Map::new();
    header.insert("imported".to_owned(), Value::Object(imported));
    // This is intentionally destination-independent: Python always emits this literal relative path.
    header.insert(
        "raw".to_owned(),
        Value::String(format!("../../../imports/{import_id}/{raw_filename}")),
    );
    if let Some(topics) = topics.filter(|value| !value.is_empty()) {
        header.insert("topics".to_owned(), Value::String(topics.to_owned()));
    }
    if let Some(setting) = detected_setting.filter(|value| !value.is_empty()) {
        header.insert("setting".to_owned(), Value::String(setting.to_owned()));
    }

    let mut rows = Vec::with_capacity(entries.len() + 1);
    rows.push(Value::Object(header));
    for entry in &mut entries {
        if let Some(object) = entry.as_object_mut()
            && object.contains_key("text")
            && !object.contains_key("source")
        {
            object.insert("source".to_owned(), Value::String("import".to_owned()));
        }
    }
    rows.extend(entries);
    rows
}
