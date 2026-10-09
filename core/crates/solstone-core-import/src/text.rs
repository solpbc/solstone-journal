// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generic `.txt` and `.md` transcript import.
//!
//! Times, speakers and words come from the file, never from a model. A transcript in a
//! recognized layout keeps every turn at the import's start plus its heading offset, on
//! the ~300-second segments the conversation spans, with the speaker its label names and
//! its words exactly as written. Any other file is untimed: it lands whole in the one
//! ~300-second segment at its start, with no duration estimated and no turn time invented.
//! A model may add each segment's topics and setting, and nothing else; when it refuses or
//! fails, the segment is written without them.
//!
//! Recognized layout (v1): an optional preamble (a date or title heading, for example),
//! then relative `## HH:MM:SS` headings counted from the start of the conversation, each
//! followed by one or more `**Full Name:** text` lines. Hours may run past `01`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Days, NaiveDate};
use serde_json::{Map, Value};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, GenerateSessionAdapter,
};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, HealthMarkerKind, SegmentDeconflictError,
    bump_stream_marker, find_available_segment_with_occupied, health_marker_path, write_jsonl,
};

use solstone_core_segment::{ImportSource, Kind, StreamHints};

use crate::CreatedSegment;

/// One segment created and written to disk during text import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextCreated {
    pub day: String,
    pub segment: String,
    pub stream: String,
    pub hints: StreamHints,
    pub path: PathBuf,
}

impl TextCreated {
    /// Convert to the publication segment descriptor.
    pub fn created_segment(&self) -> CreatedSegment {
        CreatedSegment {
            day: self.day.clone(),
            segment: self.segment.clone(),
            stream: self.stream.clone(),
            hints: self.hints.clone(),
        }
    }
}

/// Work completed so far by a text import.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextImportWork {
    pub created: Vec<TextCreated>,
}

/// Outcome of processing a generic transcript. Always carries identities written to disk.
#[derive(Debug)]
pub enum TextImportOutcome {
    Success(TextImportWork),
    Failed {
        created: TextImportWork,
        error: TextImportError,
    },
}

impl TextImportOutcome {
    /// Return the slice of segments written to disk.
    pub fn created(&self) -> &[TextCreated] {
        match self {
            Self::Success(work) => &work.created,
            Self::Failed { created, .. } => &created.created,
        }
    }
}

const PRIVATE_IMPORT_FILE_MODE: u32 = 0o600;
/// The journal's ordinary segment length. A timed transcript tiles it from its start.
const TILE_SECONDS: u64 = 300;
const DAY_SECONDS: u64 = 86_400;
/// The first heading of a relative layout is counted from the start. One an hour or more
/// in reads as a time of day, which this layout does not carry.
const FIRST_HEADING_LIMIT_SECONDS: u64 = 3_600;
/// Only the start of a very long segment is shown to the model for its topics.
const TOPICS_INPUT_LIMIT_BYTES: usize = 32 * 1024;
const TOPICS_PROMPT: &str = include_str!("text_assets/detect_transcript_topics.md");
const TOPICS_SCHEMA: &str = include_str!("text_assets/detect_transcript_topics.schema.json");

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
    /// A timed transcript runs past midnight, but its day directory is not a `YYYYMMDD` day
    /// the next one can be named from.
    NextDay {
        day: String,
    },
    SegmentDeconflict(SegmentDeconflictError),
    SegmentKeyUnavailable {
        candidate: String,
    },
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
            Self::NextDay { day } => write!(
                formatter,
                "the transcript runs past midnight, but {day} is not a day the next one can follow"
            ),
            Self::SegmentDeconflict(source) => source.fmt(formatter),
            Self::SegmentKeyUnavailable { candidate } => {
                write!(formatter, "no available segment key for {candidate}")
            }
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
            | Self::NextDay { .. }
            | Self::SegmentKeyUnavailable { .. } => None,
        }
    }
}

/// Model-boundary seam for the topics and setting of each imported segment.
pub trait WireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError>;
}

/// Production client for the sibling `solstone-core generate` process.
pub struct SystemWireClient {
    adapter: GenerateSessionAdapter,
}

impl SystemWireClient {
    #[must_use]
    pub fn new(client: solstone_core_generate::OneShotClient) -> Self {
        Self {
            adapter: client.into_session_adapter(),
        }
    }

    #[must_use]
    pub fn sibling() -> Self {
        Self {
            adapter: solstone_core_generate::GenerateSessionAdapter::sibling(),
        }
    }

    pub fn finish(&self) {
        self.adapter.finish();
    }
}

impl WireClient for SystemWireClient {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.adapter.execute(request)
    }
}

/// Process a generic transcript using the production generate boundary.
///
/// `start_time` is the import's start as a `HH:MM:SS` clock on `day_dir`'s day. Turn
/// times, speakers and words are read from the file alone, so the import is the same
/// with any model or none. The model is asked only for each segment's topics and setting:
/// a refusal, an unreadable reply or a failed call leaves those out and changes nothing
/// else, and once the model is unavailable it is not asked again in this import.
#[allow(clippy::too_many_arguments)]
pub fn process_transcript(
    path: &Path,
    day_dir: &Path,
    start_time: &str,
    import_id: &str,
    stream: &str,
    facet: Option<&str>,
    setting: Option<&str>,
) -> TextImportOutcome {
    let wire = SystemWireClient::sibling();
    let outcome = process_transcript_with_wire(
        path, day_dir, start_time, import_id, stream, facet, setting, &wire,
    );
    wire.finish();
    outcome
}

/// Process a generic transcript with an injected generate-boundary client.
///
/// This behaves exactly as [`process_transcript`].
#[allow(clippy::too_many_arguments)]
pub fn process_transcript_with_wire(
    path: &Path,
    day_dir: &Path,
    start_time: &str,
    import_id: &str,
    stream: &str,
    facet: Option<&str>,
    setting: Option<&str>,
    wire: &dyn WireClient,
) -> TextImportOutcome {
    match import(
        path, day_dir, start_time, import_id, stream, facet, setting, wire,
    ) {
        Ok(created) => TextImportOutcome::Success(TextImportWork { created }),
        Err((created, error)) => TextImportOutcome::Failed {
            created: TextImportWork { created },
            error,
        },
    }
}

type ImportResult = Result<Vec<TextCreated>, (Vec<TextCreated>, TextImportError)>;

#[allow(clippy::too_many_arguments)]
fn import(
    path: &Path,
    day_dir: &Path,
    start_time: &str,
    import_id: &str,
    stream: &str,
    facet: Option<&str>,
    setting: Option<&str>,
    wire: &dyn WireClient,
) -> ImportResult {
    let early = |error| (Vec::new(), error);
    let text = read_transcript(path).map_err(early)?;
    let raw_filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            early(TextImportError::RawFilename {
                path: path.to_path_buf(),
            })
        })?;
    let (journal_root, day) = journal_marker_context(day_dir).map_err(early)?;
    let start_seconds = clock_seconds(start_time).ok_or_else(|| {
        early(TextImportError::InvalidTime {
            value: start_time.to_owned(),
        })
    })?;
    stage_raw_source(day_dir, import_id, path, raw_filename).map_err(early)?;

    let chronicle = day_dir.parent().unwrap_or(day_dir);
    let hints = StreamHints {
        kind: Some(Kind::Imported(ImportSource::Named("text".to_owned()))),
        host: None,
        platform: None,
    };
    let mut occupied: HashMap<String, HashSet<String>> = HashMap::new();
    let mut model_available = true;
    let mut created = Vec::new();

    for (tile, turns) in tiles(read_turns(&text)) {
        let tile_start = start_seconds + tile * TILE_SECONDS;
        let tile_day = match tile_start / DAY_SECONDS {
            0 => day.to_owned(),
            later => match next_day(day, later) {
                Some(next) => next,
                None => {
                    return Err((
                        created,
                        TextImportError::NextDay {
                            day: day.to_owned(),
                        },
                    ));
                }
            },
        };
        let tile_day_dir = if tile_day == day {
            day_dir.to_path_buf()
        } else {
            chronicle.join(&tile_day)
        };
        let parent = tile_day_dir.join(stream);
        let candidate = format!("{}_{TILE_SECONDS}", clock_key(tile_start % DAY_SECONDS));
        let day_occupied = occupied.entry(tile_day.clone()).or_default();
        let segment_key =
            match find_available_segment_with_occupied(&parent, &candidate, 100, day_occupied) {
                Ok(Some(key)) => key,
                Ok(None) => {
                    return Err((
                        created,
                        TextImportError::SegmentKeyUnavailable { candidate },
                    ));
                }
                Err(error) => return Err((created, TextImportError::SegmentDeconflict(error))),
            };

        let context = if model_available {
            tile_context(wire, &turns, &mut model_available)
        } else {
            None
        };
        let output = parent
            .join(&segment_key)
            .join("conversation_transcript.jsonl");
        let rows = jsonl_rows(
            &turns,
            tile * TILE_SECONDS,
            import_id,
            raw_filename,
            facet,
            setting,
            context.as_ref(),
        );
        if let Err(source) = write_jsonl(
            &output,
            rows,
            AtomicWriteOptions {
                mode: Some(PRIVATE_IMPORT_FILE_MODE),
            },
        ) {
            return Err((
                created,
                TextImportError::Write {
                    path: output,
                    source,
                },
            ));
        }

        created.push(TextCreated {
            day: tile_day.clone(),
            segment: segment_key.clone(),
            stream: stream.to_owned(),
            hints: hints.clone(),
            path: output,
        });
        if let Err(source) = bump_stream_marker(journal_root, &tile_day) {
            return Err((
                created,
                TextImportError::StreamMarker {
                    path: health_marker_path(journal_root, &tile_day, HealthMarkerKind::Stream),
                    day: tile_day,
                    source,
                },
            ));
        }
        day_occupied.insert(segment_key);
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

/// One entry of the transcript as the file gives it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Turn {
    /// Seconds from the start of the import.
    offset: u64,
    speaker: Option<String>,
    text: String,
}

/// Read the file's turns: timed when the layout is recognized, otherwise every line at
/// the start, untimed.
fn read_turns(text: &str) -> Vec<Turn> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    timed_turns(text).unwrap_or_else(|| untimed_turns(text))
}

/// Each non-blank line, at the start, as written. Nothing here carries a time.
fn untimed_turns(text: &str) -> Vec<Turn> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Turn {
            offset: 0,
            speaker: None,
            text: line.to_owned(),
        })
        .collect()
}

/// The v1 layout, or `None` when the file is not in it.
///
/// Recognized when it has at least one relative time heading, the first under an hour,
/// none running backwards, and at least one speaker line under a heading. Lines before the
/// first heading are kept as one entry at the start. A line under a heading that is not a
/// speaker line continues the turn above it; one with no turn above it, or a markdown
/// heading (a closing summary, say), starts an entry with no speaker. No text is dropped.
fn timed_turns(text: &str) -> Option<Vec<Turn>> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut offset: Option<u64> = None;
    let mut continues = false;
    let mut spoken = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(next) = heading_offset(line) {
            match offset {
                None if next >= FIRST_HEADING_LIMIT_SECONDS => return None,
                Some(current) if next < current => return None,
                _ => {}
            }
            offset = Some(next);
            continues = false;
            continue;
        }
        let at = offset.unwrap_or(0);
        if offset.is_some() {
            if let Some((speaker, said)) = speaker_line(line) {
                turns.push(Turn {
                    offset: at,
                    speaker: Some(speaker.to_owned()),
                    text: said.to_owned(),
                });
                spoken = true;
                continues = true;
                continue;
            }
            if line.trim_start().starts_with('#') {
                continues = false;
            }
        }
        match turns.last_mut() {
            Some(turn) if continues => {
                if !turn.text.is_empty() {
                    turn.text.push('\n');
                }
                turn.text.push_str(line);
            }
            _ => {
                turns.push(Turn {
                    offset: at,
                    speaker: None,
                    text: line.to_owned(),
                });
                continues = true;
            }
        }
    }
    (offset.is_some() && spoken).then_some(turns)
}

/// Seconds counted by a `## H:MM:SS` heading, hours one to three digits.
fn heading_offset(line: &str) -> Option<u64> {
    let rest = line.trim_start().strip_prefix("##")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut parts = rest.trim().split(':');
    let hours = parts
        .next()
        .filter(|part| (1..=3).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))?
        .parse::<u64>()
        .ok()?;
    let mut sexagesimal = || {
        parts
            .next()
            .filter(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|part| part.parse::<u64>().ok())
            .filter(|value| *value < 60)
    };
    let (minutes, seconds) = (sexagesimal()?, sexagesimal()?);
    parts
        .next()
        .is_none()
        .then_some(hours * 3600 + minutes * 60 + seconds)
}

/// `**Full Name:** text` (or `**Full Name**: text`): the label, and the words after it.
fn speaker_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("**")?;
    let end = match (rest.find(":**"), rest.find("**:")) {
        (Some(inside), Some(outside)) => inside.min(outside),
        (Some(at), None) | (None, Some(at)) => at,
        (None, None) => return None,
    };
    let speaker = rest[..end].trim();
    if speaker.is_empty() || speaker.contains('*') {
        return None;
    }
    Some((speaker, rest[end + 3..].trim_start_matches([' ', '\t'])))
}

/// Group turns by the ~300-second tile of the import they fall in, in order.
fn tiles(turns: Vec<Turn>) -> BTreeMap<u64, Vec<Turn>> {
    let mut tiles: BTreeMap<u64, Vec<Turn>> = BTreeMap::new();
    for turn in turns {
        tiles
            .entry(turn.offset / TILE_SECONDS)
            .or_default()
            .push(turn);
    }
    tiles
}

fn next_day(day: &str, days: u64) -> Option<String> {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()?
        .checked_add_days(Days::new(days))
        .map(|next| next.format("%Y%m%d").to_string())
}

fn clock_key(seconds: u64) -> String {
    format!(
        "{:02}{:02}{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

fn clock_text(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// Seconds since midnight for a strict `HH:MM:SS` time of day.
fn clock_seconds(value: &str) -> Option<u64> {
    let mut parts = value.split(':');
    let mut field = |limit: u64| {
        parts
            .next()
            .filter(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|part| part.parse::<u64>().ok())
            .filter(|number| *number <= limit)
    };
    let (hours, minutes, seconds) = (field(23)?, field(59)?, field(59)?);
    parts
        .next()
        .is_none()
        .then_some(hours * 3600 + minutes * 60 + seconds)
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

/// What a model may add to a segment.
struct TileContext {
    topics: Option<String>,
    setting: Option<String>,
}

/// Ask the model for one segment's topics and setting. A failed call or a blocking
/// refusal marks the model unavailable for the rest of the import.
fn tile_context(
    wire: &dyn WireClient,
    turns: &[Turn],
    model_available: &mut bool,
) -> Option<TileContext> {
    let mut contents = String::new();
    for turn in turns {
        if let Some(speaker) = &turn.speaker {
            contents.push_str(speaker);
            contents.push_str(": ");
        }
        contents.push_str(&turn.text);
        contents.push('\n');
    }
    if contents.len() > TOPICS_INPUT_LIMIT_BYTES {
        let mut end = TOPICS_INPUT_LIMIT_BYTES;
        while !contents.is_char_boundary(end) {
            end -= 1;
        }
        contents.truncate(end);
    }
    let request = GenerateRequest {
        id: None,
        context: "observe.detect.topics".to_owned(),
        contents: vec![ContentPart::Text { text: contents }],
        system_instruction: Some(TOPICS_PROMPT.to_owned()),
        temperature: 0.3,
        max_output_tokens: 256,
        timeout_s: None,
        json_output: true,
        json_schema: serde_json::from_str(TOPICS_SCHEMA)
            .expect("vendored transcript schema is valid JSON"),
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    };
    match wire.execute(&request) {
        Ok(GenerateResponse::Generated(response)) => parse_context(&response.text),
        Ok(GenerateResponse::Refused(refused)) => {
            if refused.blocking {
                *model_available = false;
            }
            None
        }
        Err(_) => {
            *model_available = false;
            None
        }
    }
}

fn parse_context(response: &str) -> Option<TileContext> {
    let object = serde_json::from_str::<Value>(response).ok()?;
    let object = object.as_object()?;
    let field = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Some(TileContext {
        topics: field("topics"),
        setting: field("setting"),
    })
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

/// The segment file: a header, then one entry per turn, its `start` counted from the
/// segment's own start.
fn jsonl_rows(
    turns: &[Turn],
    tile_offset: u64,
    import_id: &str,
    raw_filename: &str,
    facet: Option<&str>,
    caller_setting: Option<&str>,
    context: Option<&TileContext>,
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
    if let Some(context) = context {
        if let Some(topics) = &context.topics {
            header.insert("topics".to_owned(), Value::String(topics.clone()));
        }
        if let Some(setting) = &context.setting {
            header.insert("setting".to_owned(), Value::String(setting.clone()));
        }
    }

    let mut rows = Vec::with_capacity(turns.len() + 1);
    rows.push(Value::Object(header));
    for turn in turns {
        let mut entry = Map::new();
        entry.insert(
            "start".to_owned(),
            Value::String(clock_text(turn.offset - tile_offset)),
        );
        if let Some(speaker) = &turn.speaker {
            entry.insert("speaker".to_owned(), Value::String(speaker.clone()));
        }
        entry.insert("text".to_owned(), Value::String(turn.text.clone()));
        entry.insert("source".to_owned(), Value::String("import".to_owned()));
        rows.push(Value::Object(entry));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(offset: u64, speaker: Option<&str>, text: &str) -> Turn {
        Turn {
            offset,
            speaker: speaker.map(str::to_owned),
            text: text.to_owned(),
        }
    }

    #[test]
    fn the_v1_layout_reads_times_speakers_and_words_from_the_file() {
        let file = "# 2026-03-11\n# Weekly sync\n\n## 00:00:00\n**Ana Lima:** Hello all.\n**Ben Ode:**   Morning.\nsecond line\n\n## 01:02:03\n**Ana Lima**: Bye.\n\n## Summary\nWe met.\n";
        assert_eq!(
            read_turns(file),
            vec![
                turn(0, None, "# 2026-03-11\n# Weekly sync"),
                turn(0, Some("Ana Lima"), "Hello all."),
                turn(0, Some("Ben Ode"), "Morning.\nsecond line"),
                turn(3723, Some("Ana Lima"), "Bye."),
                turn(3723, None, "## Summary\nWe met."),
            ]
        );
    }

    #[test]
    fn a_file_outside_the_v1_layout_is_untimed_line_by_line() {
        for file in [
            "Ana: hello\n\nBen: hi\n",
            "## 00:00:00\njust notes, nobody speaks\n",
            "## 00:05:00\n**Ana:** later\n## 00:01:00\n**Ben:** earlier\n",
            "## 14:30:00\n**Ana:** a time of day, not an offset\n",
            "[00:00:05] **Ana:** another layout\n",
        ] {
            let turns = read_turns(file);
            assert!(
                turns
                    .iter()
                    .all(|turn| turn.offset == 0 && turn.speaker.is_none())
            );
            let lines = file.lines().filter(|line| !line.trim().is_empty());
            assert!(
                turns.iter().map(|turn| turn.text.as_str()).eq(lines),
                "{file}"
            );
        }
    }

    #[test]
    fn heading_and_speaker_shapes() {
        assert_eq!(heading_offset("## 00:00:00"), Some(0));
        assert_eq!(heading_offset("##\t1:00:01 "), Some(3601));
        assert_eq!(heading_offset("## 101:00:00"), Some(363_600));
        assert_eq!(heading_offset("### 00:00:01"), None);
        assert_eq!(heading_offset("##00:00:01"), None);
        assert_eq!(heading_offset("## 00:60:00"), None);
        assert_eq!(heading_offset("## 00:00"), None);
        assert_eq!(heading_offset("## 00:00:00 intro"), None);
        assert_eq!(speaker_line("**A B:** x"), Some(("A B", "x")));
        assert_eq!(speaker_line("**A B**: x:** y"), Some(("A B", "x:** y")));
        assert_eq!(speaker_line("**:** x"), None);
        assert_eq!(speaker_line("**bold** text"), None);
        assert_eq!(speaker_line(" **A:** x"), None);
    }
}
