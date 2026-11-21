// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generic text import. The deterministic kind selects the stored representation.
//!
//! Times, speakers and words come from the file, never from a model. A transcript in a
//! recognized layout keeps every turn at the import's start plus its heading offset, on
//! the ~300-second segments the conversation spans, with the speaker its label names and
//! its words exactly as written. The tile clock is the owner-zone local time of the anchor
//! plus real elapsed seconds, and one inline `23:xx` → `00:xx` crossing is the next local day.
//! Any other file is untimed: it lands whole in the one ~300-second segment at its start, with
//! no duration estimated and no turn time invented. A model may add each segment's topics and
//! setting, and nothing else; when it refuses or fails, the segment is written without them.
//!
//! Recognized layouts (v1), each after an optional preamble such as a date or title:
//! - relative `## HH:MM:SS` headings counted from the start of the conversation, each
//!   followed by one or more `**Full Name:** text` lines;
//! - `[HH:MM:SS] Name: text` lines;
//! - `Name (HH:MM:SS)` lines, the words following on that line or the lines below.
//!
//! Hours may run past `01`. In the two single-line layouts a first time under an hour is
//! counted from the import's start and any other is a time of day on the import's day; a
//! file that is neither is untimed, never guessed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp};
use serde_json::{Map, Value};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, GenerateSessionAdapter,
};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, HealthMarkerKind, SegmentDeconflictError,
    bump_stream_marker, health_marker_path, write_jsonl, write_text,
};
use solstone_core_time::{day_key, from_local};

use solstone_core_segment::{ImportSource, Kind, StreamHints};

use crate::CreatedSegment;
use crate::triage::{self, Kind as ContentKind, Verdict};

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
    /// No turn times were read from the file, so it landed whole at its start. Each such
    /// segment's header carries `"untimed": true`.
    pub untimed: bool,
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
    LocalTimeMissing {
        day: String,
        time: String,
    },
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
        // Boxed so the error stays small on Windows, where `PathBuf` is wider (clippy
        // `result_large_err`).
        source: Box<AtomicWriteError>,
    },
    RawCopy {
        path: PathBuf,
        source: std::io::Error,
    },
    Stopped,
}

impl fmt::Display for TextImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalTimeMissing { day, time } => {
                write!(formatter, "no such local time {time} on {day}")
            }
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
            Self::Stopped => formatter.write_str("import stopped"),
        }
    }
}

impl Error for TextImportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::SourceRead { source, .. } => Some(source),
            Self::SegmentDeconflict(source) => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::StreamMarker { source, .. } => Some(&**source),
            Self::RawCopy { source, .. } => Some(source),
            Self::Stopped => None,
            Self::LocalTimeMissing { .. }
            | Self::UnsupportedFormat { .. }
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

    /// Release a sibling session. Injected clients keep the default.
    fn finish(&self) {}
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
    source_instant: Option<Timestamp>,
) -> TextImportOutcome {
    let wire = SystemWireClient::sibling();
    let outcome = process_transcript_with_wire(
        path,
        day_dir,
        start_time,
        import_id,
        stream,
        facet,
        setting,
        &wire,
        source_instant,
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
    source_instant: Option<Timestamp>,
) -> TextImportOutcome {
    match import(
        path,
        day_dir,
        start_time,
        import_id,
        stream,
        facet,
        setting,
        wire,
        source_instant,
    ) {
        Ok(work) => TextImportOutcome::Success(work),
        Err((created, error)) => TextImportOutcome::Failed {
            created: TextImportWork {
                created,
                untimed: false,
            },
            error,
        },
    }
}

type ImportResult = Result<TextImportWork, (Vec<TextCreated>, TextImportError)>;

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
    source_instant: Option<Timestamp>,
) -> ImportResult {
    let early = |error| (Vec::new(), error);
    if !crate::work_continues() {
        return Err(early(TextImportError::Stopped));
    }
    let (text, verdict, bytes) = read_source(path).map_err(early)?;
    let raw_filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            early(TextImportError::RawFilename {
                path: path.to_path_buf(),
            })
        })?;
    let (journal_root, _) = journal_marker_context(day_dir).map_err(early)?;
    stage_raw_source(day_dir, import_id, &bytes, raw_filename).map_err(early)?;
    drop(bytes);

    let hints = StreamHints {
        kind: Some(Kind::Imported(ImportSource::Named("text".to_owned()))),
        host: None,
        platform: None,
    };
    let mut model_available = true;
    let mut created = Vec::new();

    let (planned, untimed) = plan_tiles(
        reading_for(&text, &verdict),
        day_dir,
        start_time,
        stream,
        source_instant,
    )
    .map_err(early)?;
    let filename = match verdict.kind {
        ContentKind::Documents => "document_transcript.md",
        ContentKind::Notes => "note_transcript.md",
        _ => "conversation_transcript.jsonl",
    };
    for tile_plan in planned {
        if !crate::work_continues() {
            return Err((created, TextImportError::Stopped));
        }
        let tile_plan = tile_plan.map_err(|error| (created.clone(), error))?;
        let PlannedTile {
            start_offset,
            turns,
            day: tile_day,
            segment: segment_key,
            origin,
        } = tile_plan;
        let parent = day_dir
            .parent()
            .unwrap_or(day_dir)
            .join(&tile_day)
            .join(stream);
        let directory = parent.join(&segment_key);
        // Planning reserves nothing. Refuse a destination that became occupied before
        // this writer could claim it; do not overwrite a different import's recording.
        // A removed segment's tombstone gives way here; only the caller that
        // unlinked it gets `Reclaimed`, so its emptied folder is this tile's claim.
        let claimed = fs::create_dir_all(&parent).and_then(|()| {
            match solstone_core_segment::reclaim_for_arrival(&directory)
                .map_err(std::io::Error::other)?
            {
                solstone_core_segment::Arrival::Reclaimed => Ok(()),
                solstone_core_segment::Arrival::Free => fs::create_dir(&directory),
                solstone_core_segment::Arrival::Live | solstone_core_segment::Arrival::Removing => {
                    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
                }
            }
        });
        if let Err(source) = claimed {
            return Err((
                created,
                TextImportError::SourceRead {
                    path: directory,
                    source,
                },
            ));
        }
        if let Err(source) = solstone_core_segment::output_attempt::prepare_segment(&directory) {
            return Err((
                created,
                TextImportError::SourceRead {
                    path: directory,
                    source,
                },
            ));
        }
        let claims = match solstone_core_segment::output_attempt::OutputClaims::acquire(
            [directory.join(filename).with_extension("jsonl")],
            solstone_core_journal_io::LockOptions::default(),
        ) {
            Ok(claims) => claims,
            Err(source) => {
                return Err((
                    created,
                    TextImportError::SourceRead {
                        path: directory,
                        source,
                    },
                ));
            }
        };
        let context = if model_available {
            tile_context(wire, &turns, &mut model_available)
        } else {
            None
        };
        if !crate::work_continues() {
            return Err((created, TextImportError::Stopped));
        }
        let output = directory.join(filename);
        let commit = match claims.commit(&directory) {
            Ok(commit) => commit,
            Err(source) => {
                return Err((
                    created,
                    TextImportError::SourceRead {
                        path: directory,
                        source,
                    },
                ));
            }
        };
        let options = AtomicWriteOptions {
            mode: Some(PRIVATE_IMPORT_FILE_MODE),
        };
        let metadata = directory.join(triage::METADATA_FILENAME);
        #[derive(serde::Serialize)]
        struct TextMetadata<'a> {
            #[serde(flatten)]
            verdict: &'a Verdict,
            import_id: &'a str,
            source_ms: i64,
            #[serde(skip_serializing_if = "Option::is_none")]
            facet: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            setting: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            topics: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            suggested_setting: Option<&'a str>,
        }
        let metadata_record = TextMetadata {
            verdict: &verdict,
            import_id,
            source_ms: crate::timestamp::epoch_millis(after(origin, start_offset)),
            facet,
            setting,
            topics: context
                .as_ref()
                .and_then(|context| context.topics.as_deref()),
            suggested_setting: context
                .as_ref()
                .and_then(|context| context.setting.as_deref()),
        };
        triage::write_metadata(&metadata, &metadata_record).map_err(|source| {
            (
                created.clone(),
                TextImportError::Write {
                    path: metadata,
                    source,
                },
            )
        })?;
        let write = if verdict.kind == ContentKind::Transcripts {
            write_jsonl(
                &output,
                jsonl_rows(
                    &turns,
                    start_offset,
                    origin,
                    import_id,
                    raw_filename,
                    &verdict,
                    facet,
                    setting,
                    context.as_ref(),
                    untimed,
                ),
                options,
            )
        } else {
            write_text(&output, &text, options)
        };
        if let Err(source) = write {
            return Err((
                created,
                TextImportError::Write {
                    path: output,
                    source,
                },
            ));
        }

        drop(commit);
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
                    source: Box::new(source),
                },
            ));
        }
    }

    // An empty file wrote nothing, so there is no untimed segment to speak of.
    let untimed = untimed && !created.is_empty();
    Ok(TextImportWork { created, untimed })
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

/// Plan the same tiles and destination collisions as the writer, without creating files.
pub fn preview(
    path: &Path,
    day_dir: &Path,
    start_time: &str,
    stream: &str,
    source_instant: Option<Timestamp>,
) -> Result<crate::size::ImportSize, TextImportError> {
    let (text, verdict, _) = read_source(path)?;
    let (planned, _) = plan_tiles(
        reading_for(&text, &verdict),
        day_dir,
        start_time,
        stream,
        source_instant,
    )?;
    Ok(crate::size::ImportSize {
        recordings: planned
            .into_iter()
            .map(|tile| {
                tile.map(|tile| crate::size::Recording {
                    day: tile.day,
                    stream: stream.to_owned(),
                    segment: tile.segment,
                })
            })
            .collect::<Result<_, _>>()?,
        ..Default::default()
    })
}

struct PlannedTile {
    start_offset: u64,
    turns: Vec<Turn>,
    day: String,
    segment: String,
    origin: Timestamp,
}

fn plan_tiles(
    reading: Reading,
    day_dir: &Path,
    start_time: &str,
    stream: &str,
    source_instant: Option<Timestamp>,
) -> Result<(Vec<Result<PlannedTile, TextImportError>>, bool), TextImportError> {
    let (journal_root, day) = journal_marker_context(day_dir)?;
    let zone = solstone_core_journal_config::owner_zone(journal_root);
    let start_seconds = clock_seconds(start_time).ok_or_else(|| TextImportError::InvalidTime {
        value: start_time.to_owned(),
    })?;
    let parsed_date = solstone_core_time::parse_day(day);
    let is_literal_day = parsed_date.is_none();
    let day_date = parsed_date.unwrap_or_else(|| jiff::civil::date(2026, 1, 1));
    let chronicle = day_dir.parent().unwrap_or(day_dir);
    let untimed = reading.untimed;

    let (anchor, turns) = if let Some(first) = reading.clock_start {
        let naive = day_date.to_datetime(time_of_day(first));
        let anchor = from_local(&zone, naive)
            .earliest()
            .ok_or_else(|| TextImportError::LocalTimeMissing {
                day: day.to_owned(),
                time: clock_text(first),
            })?
            .timestamp();
        let mut resolved_turns = Vec::with_capacity(reading.turns.len());
        for mut turn in reading.turns {
            if let Some(stated) = turn.stated {
                let turn_date = day_date
                    .checked_add(Span::new().days(i64::from(stated.day_shift)))
                    .map_err(|_| TextImportError::NextDay {
                        day: day.to_owned(),
                    })?;
                let turn_naive = turn_date.to_datetime(time_of_day(stated.seconds));
                let turn_utc = from_local(&zone, turn_naive)
                    .earliest()
                    .ok_or_else(|| TextImportError::LocalTimeMissing {
                        day: if is_literal_day {
                            day.to_owned()
                        } else {
                            day_key(turn_date)
                        },
                        time: clock_text(stated.seconds),
                    })?
                    .timestamp();
                turn.offset = turn_utc.duration_since(anchor).as_secs().max(0) as u64;
            }
            resolved_turns.push(turn);
        }
        (anchor, resolved_turns)
    } else {
        let naive = day_date.to_datetime(time_of_day(start_seconds));
        let anchor = source_instant
            .or_else(|| {
                from_local(&zone, naive)
                    .earliest()
                    .map(|instant| instant.timestamp())
            })
            .ok_or_else(|| TextImportError::LocalTimeMissing {
                day: day.to_owned(),
                time: start_time.to_owned(),
            })?;
        (anchor, reading.turns)
    };
    // Past the last day the calendar holds there is no next day to file on.
    let last = turns.iter().map(|turn| turn.offset).max().unwrap_or(0);
    if anchor
        .checked_add(SignedDuration::from_secs(last as i64))
        .is_err()
    {
        return Err(TextImportError::NextDay {
            day: day.to_owned(),
        });
    }

    let mut occupied: HashMap<String, HashSet<String>> = HashMap::new();
    let mut planned = Vec::new();
    for ((tile, local_day), turns) in tiles(turns, anchor, &zone) {
        let result = (|| {
            let nominal_offset = tile * TILE_SECONDS;
            let nominal_instant = after(anchor, nominal_offset);
            // One elapsed tile may straddle midnight. Keep each turn on its
            // own local day, starting the second piece at its first turn.
            let start_offset =
                if nominal_instant.to_zoned_datetime(zone.clone()).date() == local_day {
                    nominal_offset
                } else {
                    turns[0].offset
                };
            let tile_instant = after(anchor, start_offset);
            let tile_local = tile_instant.to_zoned_datetime(zone.clone()).datetime();
            let tile_day = if is_literal_day && tile_local.date() == day_date {
                day.to_owned()
            } else {
                day_key(tile_local.date())
            };
            let tile_seconds = tile_local.time().duration_since(Time::midnight()).as_secs() as u64;
            let parent = chronicle.join(&tile_day).join(stream);
            let candidate = format!("{}_{TILE_SECONDS}", clock_key(tile_seconds));
            let day_occupied = occupied.entry(tile_day.clone()).or_default();
            let segment = text_segment_key(&parent, tile_seconds, day_occupied)?
                .ok_or(TextImportError::SegmentKeyUnavailable { candidate })?;
            day_occupied.insert(segment.clone());
            Ok(PlannedTile {
                start_offset,
                turns,
                day: tile_day,
                segment,
                origin: anchor,
            })
        })();
        let failed = result.is_err();
        planned.push(result);
        if failed {
            break;
        }
    }
    Ok((planned, untimed))
}

fn text_segment_key(
    parent: &Path,
    seconds: u64,
    occupied: &HashSet<String>,
) -> Result<Option<String>, TextImportError> {
    for attempt in 0_i64..100 {
        let offset = if attempt % 2 == 0 {
            -(attempt / 2)
        } else {
            (attempt + 1) / 2
        };
        let candidate = seconds as i64 + offset;
        if !(0..DAY_SECONDS as i64).contains(&candidate) {
            continue;
        }
        let key = format!("{}_{TILE_SECONDS}", clock_key(candidate as u64));
        let path = parent.join(&key);
        if occupied.contains(&key) {
            continue;
        }
        let present = solstone_core_journal_io::path_lexists(&path).map_err(|error| {
            TextImportError::SegmentDeconflict(SegmentDeconflictError::Path(error))
        })?;
        // A key the owner removed is free again: what arrives is new data. Only
        // live content or a removal still in progress steps on. Planning does
        // not touch the tombstone; the writer lets it give way.
        let free = match solstone_core_segment::owner_deletion_state(&path).map_err(|source| {
            TextImportError::SourceRead {
                path,
                source: std::io::Error::other(source),
            }
        })? {
            solstone_core_segment::OwnerDeletionState::Live => !present,
            solstone_core_segment::OwnerDeletionState::Deleted => true,
            solstone_core_segment::OwnerDeletionState::Staged
            | solstone_core_segment::OwnerDeletionState::Occupied => false,
        };
        if free {
            return Ok(Some(key));
        }
    }
    Ok(None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StatedClock {
    day_shift: u32,
    seconds: u64,
}

/// One entry of the transcript as the file gives it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Turn {
    /// Seconds from the start of the import.
    offset: u64,
    speaker: Option<String>,
    recipient: Option<String>,
    text: String,
    stated: Option<StatedClock>,
}

/// Only select legacy timing when the known headers carry the file. The unchanged
/// parser can also accept a timed excerpt after unrelated prose.
pub(crate) fn has_existing_timed_layout(text: &str, lines: &[&str]) -> bool {
    let Some(first) = lines
        .iter()
        .position(|line| heading_offset(line).is_some() || inline_turn(line).is_some())
    else {
        return false;
    };
    let mut plain_titles = 0;
    for line in lines[..first]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("# ") {
            continue;
        }
        plain_titles += 1;
        if plain_titles > 1
            || line.split_whitespace().count() > 10
            || line.ends_with(['.', '!', '?', ':'])
        {
            return false;
        }
    }
    let Some(reading) = existing_timed_reading(text) else {
        return false;
    };
    // The older parser deliberately keeps summary headings and prose. Large
    // trailing sections outside the dialogue are evidence of a pasted excerpt.
    let from_first = &lines[first..];
    if let Some(heading) = from_first.iter().rposition(|line| {
        let line = line.trim_start();
        line.starts_with('#') && heading_offset(line).is_none()
    }) {
        let tail_words = from_first[heading + 1..]
            .iter()
            .filter(|line| inline_turn(line).is_none() && heading_offset(line).is_none())
            .flat_map(|line| line.split_whitespace())
            .count();
        let speech_words = reading
            .turns
            .iter()
            .filter(|turn| turn.speaker.is_some())
            .flat_map(|turn| turn.text.split_whitespace())
            .count();
        if tail_words >= 16 && tail_words > speech_words {
            return false;
        }
    }
    true
}

fn existing_timed_reading(text: &str) -> Option<Reading> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if let Some(turns) = timed_turns(text) {
        return Some(Reading {
            clock_start: None,
            turns,
            untimed: false,
        });
    }
    inline_turns(text)
}

fn zoom_chat_turns(text: &str) -> Option<Vec<Turn>> {
    use std::sync::LazyLock;
    static HEADER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(solstone_core_format::content::conversation::ZOOM_CHAT_LINE)
            .expect("Zoom saved chat header")
    });
    let mut turns: Vec<Turn> = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let header_line = crate::triage::strip_direction_marks(line);
        // A standalone CR is a line boundary in the classifier. Preserve such
        // ambiguous input through the earlier reader instead of merging senders.
        if line.contains('\r') {
            return None;
        }
        if let Some(parts) = HEADER.captures(header_line) {
            let body = header_line.get(parts.get(0)?.end()..)?;
            let speaker = parts.name("speaker")?.as_str().trim();
            if speaker.is_empty() {
                return None;
            }
            turns.push(Turn {
                offset: 0,
                speaker: Some(speaker.to_owned()),
                recipient: parts
                    .name("recipient")
                    .map(|name| name.as_str().trim().to_owned()),
                text: body.to_owned(),
                stated: None,
            });
        } else if let Some(turn) = turns.last_mut() {
            if !turn.text.is_empty() {
                turn.text.push('\n')
            }
            turn.text.push_str(line);
        } else if !line.trim().is_empty() {
            return None;
        }
    }
    // A partial parse leaves the original import representation intact.
    (!turns.is_empty()
        && turns
            .iter()
            .all(|t| !t.text.trim().is_empty() && t.text.len() <= 8 * 1024))
    .then_some(turns)
}

fn reading_for(text: &str, verdict: &Verdict) -> Reading {
    if verdict.kind == ContentKind::Transcripts
        && verdict.reasons.contains(&"zoom_chat")
        && let Some(turns) = zoom_chat_turns(text)
    {
        // Zoom's saved chat carries a local clock but no date or time zone.
        // Keep the filing time separate from a message's own send instant.
        return Reading {
            clock_start: None,
            turns,
            untimed: true,
        };
    }
    if verdict.kind == ContentKind::Transcripts
        && verdict.reasons.contains(&"existing_timed_layout")
        && let Some(reading) = existing_timed_reading(text)
    {
        return reading;
    }

    // Whole-file layouts inherit the untimed transcript's line representation.
    // Bound a long line so a later segment read can read every JSON record.
    Reading {
        clock_start: None,
        turns: untimed_turns(text),
        untimed: verdict.kind == ContentKind::Transcripts,
    }
}

/// Read the file's turns: timed when the layout is recognized, otherwise every line at
/// the start, untimed.
#[cfg(test)]
fn read_turns(text: &str) -> Reading {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    existing_timed_reading(text).unwrap_or_else(|| Reading {
        clock_start: None,
        turns: untimed_turns(text),
        untimed: true,
    })
}

/// A transcript's turns, and where they are counted from.
struct Reading {
    /// The time of day the file's first turn names, when its times are clock times; the
    /// turns are then counted from it. `None` counts them from the import's start.
    clock_start: Option<u64>,
    turns: Vec<Turn>,
    /// No layout was recognized: every turn sits at the start and none carries a time.
    untimed: bool,
}

/// The two single-line turn layouts: `[HH:MM:SS] Name: text` and `Name (HH:MM:SS)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InlineLayout {
    Bracketed,
    Parenthesized,
}

/// A file whose turns each start with a time on the speaker's line, or `None` when it is
/// not one.
///
/// Recognized when at least two lines start a turn, all in the same layout, and no time
/// runs backwards. A first time under an hour counts every time from the import's start;
/// otherwise the times are clock times on the import's day, and each must be one, or the
/// file is not recognized. Lines before the first turn are kept as one entry at the start
/// of the conversation; other lines continue the turn above them, except a markdown
/// heading, which starts an entry with no speaker. No text is dropped.
fn inline_turns(text: &str) -> Option<Reading> {
    let mut layout = None;
    let mut turns: Vec<(Option<StatedClock>, Turn)> = Vec::new();
    let mut last_time: Option<u64> = None;
    let mut spoken = 0_usize;
    let mut continues = false;
    let mut first_time: Option<u64> = None;
    let mut crossed = false;
    let mut day_shift: u32 = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((kind, time, speaker, said)) = inline_turn(line) {
            if layout.is_some_and(|known| known != kind) {
                return None;
            }
            if let Some(last) = last_time {
                if time < last {
                    let first = first_time.unwrap();
                    if first >= FIRST_HEADING_LIMIT_SECONDS
                        && !crossed
                        && last >= 23 * 3600
                        && time < 3600
                        && time < DAY_SECONDS
                    {
                        crossed = true;
                        day_shift = 1;
                    } else {
                        return None;
                    }
                } else if first_time.unwrap() >= FIRST_HEADING_LIMIT_SECONDS && time >= DAY_SECONDS
                {
                    return None;
                }
            } else {
                first_time = Some(time);
            }
            layout = Some(kind);
            last_time = Some(time);
            turns.push((
                Some(StatedClock {
                    day_shift,
                    seconds: time,
                }),
                Turn {
                    offset: 0,
                    speaker: Some(speaker.to_owned()),
                    recipient: None,
                    text: said.to_owned(),
                    stated: None,
                },
            ));
            spoken += 1;
            continues = true;
            continue;
        }
        if layout.is_some() && line.trim_start().starts_with('#') {
            continues = false;
        }
        match turns.last_mut() {
            Some((_, turn)) if continues => {
                if !turn.text.is_empty() {
                    turn.text.push('\n');
                }
                turn.text.push_str(line);
            }
            _ => {
                turns.push((
                    last_time.map(|time| StatedClock {
                        day_shift,
                        seconds: time,
                    }),
                    Turn {
                        offset: 0,
                        speaker: None,
                        recipient: None,
                        text: line.to_owned(),
                        stated: None,
                    },
                ));
                continues = true;
            }
        }
    }
    if spoken < 2 {
        return None;
    }
    let first = first_time?;
    let clock_start = if first < FIRST_HEADING_LIMIT_SECONDS {
        None
    } else if first < DAY_SECONDS && last_time? < DAY_SECONDS {
        Some(first)
    } else {
        return None;
    };
    let base = clock_start.unwrap_or(0);
    Some(Reading {
        clock_start,
        untimed: false,
        turns: turns
            .into_iter()
            .map(|(clock, turn)| {
                let (offset, stated) = if clock_start.is_some() {
                    if let Some(sc) = clock {
                        let off = (sc.day_shift as u64 * DAY_SECONDS + sc.seconds) - base;
                        (off, Some(sc))
                    } else {
                        (0, None)
                    }
                } else {
                    let off = clock.map(|sc| sc.seconds).unwrap_or(0);
                    (off, None)
                };
                Turn {
                    offset,
                    stated,
                    ..turn
                }
            })
            .collect(),
    })
}

/// A line that starts a turn in one of the inline layouts: its layout, time, speaker and
/// the words after them.
fn inline_turn(line: &str) -> Option<(InlineLayout, u64, &str, &str)> {
    let speaker_ok = |speaker: &str| {
        !speaker.is_empty()
            && speaker.chars().count() <= 80
            && !speaker.contains(['[', ']', '(', ')', '*'])
            && !speaker.starts_with(['#', '-', '>', '|'])
    };
    if let Some(rest) = line.strip_prefix('[') {
        let (time, rest) = rest.split_once(']')?;
        let (speaker, said) = rest.split_once(':')?;
        let speaker = speaker.trim();
        return speaker_ok(speaker).then_some((
            InlineLayout::Bracketed,
            duration_seconds(time)?,
            speaker,
            said.trim_start_matches([' ', '\t']),
        ));
    }
    if line.starts_with([' ', '\t']) {
        return None;
    }
    let (speaker, rest) = line.split_once('(')?;
    let (time, rest) = rest.split_once(')')?;
    let speaker = speaker.trim();
    let said = rest.strip_prefix(':').unwrap_or(rest);
    speaker_ok(speaker).then_some((
        InlineLayout::Parenthesized,
        duration_seconds(time)?,
        speaker,
        said.trim_start_matches([' ', '\t']),
    ))
}

/// Untimed entries sit at the import start. The original retains all line breaks.
fn untimed_turns(text: &str) -> Vec<Turn> {
    const MAX_TEXT_BYTES: usize = 8 * 1024;
    let mut turns = Vec::new();
    for mut line in text.lines().filter(|line| !line.trim().is_empty()) {
        while !line.is_empty() {
            let mut end = line.len().min(MAX_TEXT_BYTES);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            let (text, rest) = line.split_at(end);
            turns.push(Turn {
                offset: 0,
                speaker: None,
                recipient: None,
                text: text.to_owned(),
                stated: None,
            });
            line = rest;
        }
    }
    turns
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
                    recipient: None,
                    text: said.to_owned(),
                    stated: None,
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
                    recipient: None,
                    text: line.to_owned(),
                    stated: None,
                });
                continues = true;
            }
        }
    }
    (offset.is_some() && spoken).then_some(turns)
}

/// Seconds counted by a `## H:MM:SS` heading.
fn heading_offset(line: &str) -> Option<u64> {
    let rest = line.trim_start().strip_prefix("##")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    duration_seconds(rest.trim())
}

/// Seconds in an `H:MM:SS` time, hours one to three digits.
fn duration_seconds(value: &str) -> Option<u64> {
    let mut parts = value.split(':');
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

/// Group turns by elapsed tile and owner day, preserving source order.
fn tiles(turns: Vec<Turn>, anchor: Timestamp, zone: &TimeZone) -> BTreeMap<(u64, Date), Vec<Turn>> {
    let mut tiles: BTreeMap<(u64, Date), Vec<Turn>> = BTreeMap::new();
    for turn in turns {
        let day = after(anchor, turn.offset)
            .to_zoned_datetime(zone.clone())
            .date();
        tiles
            .entry((turn.offset / TILE_SECONDS, day))
            .or_default()
            .push(turn);
    }
    tiles
}

/// `offset` seconds after `anchor`. The plan has checked that its last turn
/// fits the calendar.
fn after(anchor: Timestamp, offset: u64) -> Timestamp {
    anchor + SignedDuration::from_secs(offset as i64)
}

/// The time of day `seconds` after midnight.
fn time_of_day(seconds: u64) -> Time {
    let seconds = i64::from(seconds as u32);
    Time::midnight()
        .checked_add(SignedDuration::from_secs(seconds))
        .ok()
        .filter(|_| seconds < 86_400)
        .expect("a clock within one day")
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

fn read_source(path: &Path) -> Result<(String, Verdict, Vec<u8>), TextImportError> {
    let supported = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(crate::detect::is_generic_text_extension);
    if !supported {
        return Err(TextImportError::UnsupportedFormat {
            path: path.to_path_buf(),
        });
    }
    let source_error = |source| TextImportError::SourceRead {
        path: path.to_path_buf(),
        source,
    };
    let bytes = fs::read(path).map_err(source_error)?;
    let text = triage::decode_text(&bytes)
        .ok_or_else(|| {
            source_error(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            ))
        })?
        .into_owned();
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let verdict = triage::classify(
        &bytes,
        &name,
        path.parent().unwrap_or(Path::new("")),
        None,
        None,
    );
    if !matches!(
        verdict.kind,
        ContentKind::Notes | ContentKind::Documents | ContentKind::Transcripts
    ) {
        return Err(TextImportError::UnsupportedFormat {
            path: path.to_path_buf(),
        });
    }
    Ok((text, verdict, bytes))
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
        attempt_index: 0,
        exclusive_admission: false,
        overflow_text: None,
        transport_retries: None,
    };
    if !crate::work_continues() {
        return None;
    }
    match wire.execute(&request) {
        Ok(GenerateResponse::Generated(response)) => parse_context(&response.text),
        Ok(GenerateResponse::Refused(refused)) => {
            if refused.blocking {
                *model_available = false;
            }
            None
        }
        Ok(GenerateResponse::Cancelled) => None,
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
    bytes: &[u8],
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
    let dest_dir =
        crate::staging::ensure_import_private_chain(journal_root, import_id).map_err(|source| {
            TextImportError::RawCopy {
                path: journal_root.join("imports").join(import_id),
                source: std::io::Error::other(source),
            }
        })?;
    let dest = dest_dir.join(raw_filename);
    let options = AtomicWriteOptions {
        mode: Some(PRIVATE_IMPORT_FILE_MODE),
    };
    match fs::read(&dest) {
        Ok(saved) if saved == bytes => Ok(()),
        Ok(_) => Err(TextImportError::RawCopy {
            path: dest,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the file no longer matches the copy your journal prepared, so choose the file again",
            ),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match solstone_core_journal_io::write_bytes_exclusive(&dest, bytes, options) {
                Err(AtomicWriteError::Io { ref source, .. })
                    if source.kind() == std::io::ErrorKind::AlreadyExists
                        && fs::read(&dest).is_ok_and(|saved| saved == bytes) =>
                {
                    Ok(())
                }
                result => result.map_err(|source| TextImportError::Write { path: dest, source }),
            }
        }
        Err(source) => Err(TextImportError::RawCopy { path: dest, source }),
    }
}

/// The segment file: a header, then one entry per turn, its `start` counted from the
/// segment's own start.
#[allow(clippy::too_many_arguments)]
fn jsonl_rows(
    turns: &[Turn],
    tile_offset: u64,
    origin: Timestamp,
    import_id: &str,
    raw_filename: &str,
    verdict: &Verdict,
    facet: Option<&str>,
    caller_setting: Option<&str>,
    context: Option<&TileContext>,
    untimed: bool,
) -> Vec<Value> {
    let mut imported = Map::new();
    imported.insert("id".to_owned(), Value::String(import_id.to_owned()));
    // Keep the source-format evidence with this import's rows. A neighboring
    // note, attachment or earlier sidecar may have a different origin.
    imported.insert("triage".to_owned(), serde_json::json!(verdict));
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
    // Every entry still needs a `start` to render and be searched, so an untimed file's
    // entries sit at `00:00:00`; this tells any reader that none of them is a real time.
    if untimed {
        header.insert("untimed".to_owned(), Value::Bool(true));
    }
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
        let source_ms = crate::timestamp::epoch_millis(after(origin, turn.offset));
        entry.insert("source_ms".to_owned(), Value::Number(source_ms.into()));
        if let Some(speaker) = &turn.speaker {
            entry.insert("speaker".to_owned(), Value::String(speaker.clone()));
        }
        if let Some(to) = &turn.recipient {
            entry.insert("to".into(), serde_json::json!([to]));
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

    #[test]
    fn zoom_keeps_each_sender_recipient_and_multiline_body_without_inventing_a_date() {
        let source = "10:07:10 From Ivo to Everyone : Hold it.\n\n> quoted café\n\u{200e}10:07:12\u{a0}From\u{202f}Pia : Holding.";
        let verdict = triage::classify(source.as_bytes(), "meeting.txt", Path::new(""), None, None);
        assert!(verdict.reasons.contains(&"zoom_chat"));
        let reading = reading_for(source, &verdict);
        assert!(reading.untimed);
        assert_eq!(reading.turns.len(), 2);
        assert_eq!(reading.turns[0].speaker.as_deref(), Some("Ivo"));
        assert_eq!(reading.turns[0].recipient.as_deref(), Some("Everyone"));
        assert_eq!(reading.turns[0].text, "Hold it.\n\n> quoted café");
        assert_eq!(reading.turns[1].recipient, None);
        assert_eq!(reading.turns[1].speaker.as_deref(), Some("Pia"));
        let long = format!("10:07:12 From Pia : {}", "é".repeat(6000));
        assert!(zoom_chat_turns(&long).is_none());
        assert!(
            reading_for(&long, &verdict)
                .turns
                .iter()
                .all(|turn| turn.text.len() <= 8 * 1024)
        );
    }

    #[test]
    fn document_evidence_wins_before_tiling_a_timed_excerpt() {
        let text = "*** START OF THE PROJECT GUTENBERG EBOOK CLAY ***\n\n[00:00:00] Val: Ready.\n[00:10:00] Lio: Yes.\n";
        let verdict = triage::classify(text.as_bytes(), "document.txt", Path::new(""), None, None);
        assert_eq!(verdict.kind, ContentKind::Documents);
        assert!(verdict.reasons.contains(&"existing_timed_layout"));
        let reading = reading_for(text, &verdict);
        assert!(reading.turns.iter().all(|turn| turn.offset == 0));
    }

    fn turn(offset: u64, speaker: Option<&str>, text: &str) -> Turn {
        Turn {
            offset,
            speaker: speaker.map(str::to_owned),
            recipient: None,
            text: text.to_owned(),
            stated: None,
        }
    }

    fn turn_stated(
        offset: u64,
        speaker: Option<&str>,
        text: &str,
        day_shift: u32,
        seconds: u64,
    ) -> Turn {
        Turn {
            offset,
            speaker: speaker.map(str::to_owned),
            recipient: None,
            text: text.to_owned(),
            stated: Some(StatedClock { day_shift, seconds }),
        }
    }

    #[test]
    fn the_v1_layout_reads_times_speakers_and_words_from_the_file() {
        let file = "# 2026-03-11\n# Weekly sync\n\n## 00:00:00\n**Ana Lima:** Hello all.\n**Ben Ode:**   Morning.\nsecond line\n\n## 01:02:03\n**Ana Lima**: Bye.\n\n## Summary\nWe met.\n";
        assert!(!read_turns(file).untimed);
        assert_eq!(
            read_turns(file).turns,
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
            let reading = read_turns(file);
            assert_eq!(reading.clock_start, None);
            assert!(reading.untimed, "{file}");
            let turns = reading.turns;
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
    fn bracketed_lines_read_relative_times_speakers_and_words() {
        let file = "Standup\n[00:00:00] Ana Lima: Morning.\n[00:00:04] Ben Okafor:  Hi,\nsecond line\n[01:00:10] Ana Lima: Bye.\n";
        let reading = read_turns(file);
        assert_eq!(reading.clock_start, None);
        assert!(!reading.untimed);
        assert_eq!(
            reading.turns,
            vec![
                turn(0, None, "Standup"),
                turn(0, Some("Ana Lima"), "Morning."),
                turn(4, Some("Ben Okafor"), "Hi,\nsecond line"),
                turn(3610, Some("Ana Lima"), "Bye."),
            ]
        );
    }

    #[test]
    fn parenthesized_clock_times_count_from_the_first_turn() {
        let file = "# Granola notes\nAna Lima (14:30:05)\nWe should start.\nBen Okafor (14:31:00): Agreed.\n\n## Summary\nStarted.\n";
        let reading = read_turns(file);
        assert_eq!(reading.clock_start, Some(14 * 3600 + 30 * 60 + 5));
        assert!(!reading.untimed);
        assert_eq!(
            reading.turns,
            vec![
                turn(0, None, "# Granola notes"),
                turn_stated(
                    0,
                    Some("Ana Lima"),
                    "We should start.",
                    0,
                    14 * 3600 + 30 * 60 + 5,
                ),
                turn_stated(55, Some("Ben Okafor"), "Agreed.", 0, 14 * 3600 + 31 * 60),
                turn_stated(55, None, "## Summary\nStarted.", 0, 14 * 3600 + 31 * 60),
            ]
        );
    }

    #[test]
    fn inline_midnight_crossing_is_recognized_in_bracketed_and_parenthesized() {
        let bracketed = "[23:50:00] Alice: night\n[00:10:00] Bob: morning\n## Note\n";
        let reading = read_turns(bracketed);
        assert_eq!(reading.clock_start, Some(23 * 3600 + 50 * 60));
        assert!(!reading.untimed);
        assert_eq!(
            reading.turns,
            vec![
                turn_stated(0, Some("Alice"), "night", 0, 85800),
                turn_stated(1200, Some("Bob"), "morning", 1, 600),
                turn_stated(1200, None, "## Note", 1, 600),
            ]
        );

        let parenthesized = "Alice (23:50:00)\nnight\nBob (00:10:00): morning\n## Note\n";
        let reading = read_turns(parenthesized);
        assert_eq!(reading.clock_start, Some(23 * 3600 + 50 * 60));
        assert!(!reading.untimed);
        assert_eq!(
            reading.turns,
            vec![
                turn_stated(0, Some("Alice"), "night", 0, 85800),
                turn_stated(1200, Some("Bob"), "morning", 1, 600),
                turn_stated(1200, None, "## Note", 1, 600),
            ]
        );
    }

    #[test]
    fn invalid_midnight_steps_stay_untimed() {
        for file in [
            "[22:59:00] Alice: too early to cross\n[00:01:00] Bob: morning\n",
            "[23:50:00] Alice: night\n[01:00:00] Bob: skipped 00:xx\n",
            "[23:50:00] Alice: night\n[00:05:00] Bob: morning\n[00:04:00] Carol: backwards again\n",
            "[00:10:00] Alice: relative layout\n[00:05:00] Bob: backwards in relative\n",
            "[15:00:00] Alice: afternoon\n[14:00:00] Bob: earlier\n",
            "[10:00:00] Alice: morning\n[09:00:00] Bob: earlier\n",
        ] {
            let reading = read_turns(file);
            assert_eq!(reading.clock_start, None, "{file}");
            assert!(reading.untimed, "{file}");
        }
    }

    #[test]
    fn an_inline_file_the_rules_cannot_settle_is_untimed() {
        for file in [
            "[00:00:01] Ana: only one turn\n",
            "[00:00:01] Ana: one\nBen (00:00:02) mixed layouts\n",
            "[00:05:00] Ana: later\n[00:01:00] Ben: earlier\n",
            "[23:59:00] Ana: before midnight\n[24:00:30] Ben: not a clock time\n",
            "Lunch (at noon) Ana\nDinner (later) Ben\n",
        ] {
            let reading = read_turns(file);
            assert_eq!(reading.clock_start, None, "{file}");
            assert!(reading.untimed, "{file}");
            assert!(
                reading.turns.iter().all(|turn| turn.speaker.is_none()),
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
