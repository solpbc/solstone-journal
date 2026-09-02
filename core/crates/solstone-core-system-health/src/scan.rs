// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use solstone_core_format::segment::segment_start_and_end_seconds;
use solstone_core_journal_io::SegmentIdentityError;
use solstone_core_processing_record::{MediaKind, media_kind, vocab};

use crate::{
    BODY_CARD_STREAMS, DataState, DataStateMap, HealthError, SegmentInput, SegmentSource,
    derive_modality_state,
};

const PDF_EXTENSIONS: &[&str] = &["pdf"];

pub type TimeRange = (String, String);
pub type ScanResult = Result<(Vec<TimeRange>, Vec<TimeRange>, Vec<DaySegment>), HealthError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaySegment {
    pub key: String,
    pub stream: String,
    pub start: String,
    pub end: String,
    pub types: Vec<String>,
    pub data_state: DataStateMap,
    pub modality_input_mtime_ms: BTreeMap<String, Option<i64>>,
}

impl From<DaySegment> for SegmentInput {
    fn from(segment: DaySegment) -> Self {
        Self {
            key: segment.key,
            stream: segment.stream,
            data_state: segment.data_state,
        }
    }
}

pub fn scan_day<S: SegmentSource>(
    source: &S,
    journal: &Path,
    day: &str,
    now: DateTime<Utc>,
) -> ScanResult {
    let day_path = solstone_core_journal_io::day_path(journal, Some(day), false)?;
    if !day_path.is_dir() {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }
    let _day_date = NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| HealthError::InvalidDay(day.to_owned()))?;
    let mut audio_slots = BTreeSet::new();
    let mut screen_slots = BTreeSet::new();
    let mut segments = Vec::new();

    for segment in source.segments(journal, day)? {
        let Some(raw_name) = segment.name().to_str() else {
            return Err(HealthError::UnrepresentableSegment {
                path: segment.path().to_path_buf(),
            });
        };
        let Some((times, end_seconds)) = segment_start_and_end_seconds(raw_name) else {
            continue;
        };
        let start_seconds =
            u64::from(times.hour) * 3_600 + u64::from(times.minute) * 60 + u64::from(times.second);
        let start = format_time(start_seconds);
        // This shares Python's end-of-day clamp with chronological consumers;
        // rendering remains the same `HH:MM` display format. A positive segment
        // that collapses into its start minute needs a visible end for range attachment.
        let natural_end = format_time(end_seconds);
        let end = if end_seconds > start_seconds && natural_end == start {
            let next_minute = start_seconds - (start_seconds % 60) + 60;
            format_time(next_minute.min(86_399))
        } else {
            natural_end
        };
        // The card check uses the path parent, which differs from Direct layout
        // (`_default` has no directory). Do not substitute a stream spelling here.
        let parent_name = segment
            .path()
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let (data_state, modality_input_mtime_ms) =
            detect_data_state(segment.path(), parent_name, now)?;
        let types = ["audio", "screen", "markdown", "browser"]
            .into_iter()
            .filter(|modality| data_state.0.contains_key(*modality))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if types.is_empty() {
            continue;
        }
        let slot = (start_seconds / 900) * 900;
        if types.iter().any(|kind| kind == "audio") {
            audio_slots.insert(slot);
        }
        if types.iter().any(|kind| kind == "screen") {
            screen_slots.insert(slot);
        }
        segments.push(DaySegment {
            // The raw directory basename controls parse eligibility and is the
            // value the Python day scan reports, rather than Segment.key().
            key: raw_name.to_owned(),
            stream: match segment.record_identity() {
                Ok(identity) => identity.stream.to_owned(),
                Err(SegmentIdentityError::NotUtf8 { path }) => {
                    return Err(HealthError::UnrepresentableSegment { path });
                }
                Err(SegmentIdentityError::AmbiguousNamedDefault { path }) => {
                    return Err(HealthError::AmbiguousNamedDefault { path });
                }
                Err(error) => return Err(HealthError::Identity(error)),
            },
            start,
            end,
            types,
            data_state,
            modality_input_mtime_ms,
        });
    }

    segments.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then(left.stream.cmp(&right.stream))
            .then(left.key.cmp(&right.key))
    });
    Ok((
        slots_to_ranges(audio_slots.into_iter().collect()),
        slots_to_ranges(screen_slots.into_iter().collect()),
        segments,
    ))
}

pub(crate) fn detect_data_state(
    segment_path: &Path,
    stream_parent_name: &str,
    now: DateTime<Utc>,
) -> Result<(DataStateMap, BTreeMap<String, Option<i64>>), HealthError> {
    let files = segment_files(segment_path)?;
    if is_markdown_only_body_card_segment(stream_parent_name, &files) {
        return Ok((
            DataStateMap(BTreeMap::from([(
                "markdown".to_owned(),
                DataState::Analyzed.as_str().to_owned(),
            )])),
            BTreeMap::new(),
        ));
    }

    let audio_jsonl = files
        .iter()
        .filter(|path| is_audio_jsonl(path))
        .cloned()
        .collect::<Vec<_>>();
    let audio_raw_paths: Vec<&PathBuf> = files
        .iter()
        .filter(|path| media_kind_for(path) == Some(MediaKind::Audio))
        .collect();
    let markdown = markdown_transcript_files(&files);
    let audio_analyzed = audio_jsonl
        .iter()
        .any(|path| jsonl_has_row_with_key(path, vocab::AUDIO_TRANSCRIPT_ROW_KEY))
        || markdown.iter().any(|path| has_nonempty_text(path));
    let audio = derive_modality_state(
        segment_path,
        "audio",
        audio_analyzed,
        !audio_jsonl.is_empty(),
        has_raw_media(&files, MediaKind::Audio),
        read_processing_record(&audio_jsonl).as_ref(),
        now,
    );

    let screen_jsonl = files
        .iter()
        .filter(|path| is_screen_jsonl(path))
        .cloned()
        .collect::<Vec<_>>();
    let screen_raw_paths: Vec<&PathBuf> = files
        .iter()
        .filter(|path| media_kind_for(path) == Some(MediaKind::Video))
        .collect();
    let screen_analyzed = screen_jsonl
        .iter()
        .any(|path| jsonl_has_row_with_key(path, vocab::SCREEN_ANALYSIS_ROW_KEY));
    let screen = derive_modality_state(
        segment_path,
        "screen",
        screen_analyzed,
        !screen_jsonl.is_empty(),
        has_raw_media(&files, MediaKind::Video),
        read_processing_record(&screen_jsonl).as_ref(),
        now,
    );

    let browser_analyzed = files
        .iter()
        .filter(|path| is_browser_jsonl(path))
        .any(|path| has_nonempty_text(path));
    let mut states = BTreeMap::new();
    let mut modality_input_mtime_ms = BTreeMap::new();
    if audio != DataState::Absent {
        states.insert("audio".to_owned(), audio.as_str().to_owned());
        let mtime = if !audio_raw_paths.is_empty() {
            newest_input_mtime_ms(&audio_raw_paths)
        } else if !audio_jsonl.is_empty() {
            newest_input_mtime_ms(&audio_jsonl.iter().collect::<Vec<_>>())
        } else {
            None
        };
        modality_input_mtime_ms.insert("audio".to_owned(), mtime);
    }
    if screen != DataState::Absent {
        states.insert("screen".to_owned(), screen.as_str().to_owned());
        let mtime = if !screen_raw_paths.is_empty() {
            newest_input_mtime_ms(&screen_raw_paths)
        } else if !screen_jsonl.is_empty() {
            newest_input_mtime_ms(&screen_jsonl.iter().collect::<Vec<_>>())
        } else {
            None
        };
        modality_input_mtime_ms.insert("screen".to_owned(), mtime);
    }
    if browser_analyzed {
        states.insert(
            "browser".to_owned(),
            DataState::Analyzed.as_str().to_owned(),
        );
    }
    Ok((DataStateMap(states), modality_input_mtime_ms))
}

fn is_markdown_only_body_card_segment(stream_parent_name: &str, files: &[PathBuf]) -> bool {
    if !BODY_CARD_STREAMS.contains(&stream_parent_name)
        || !markdown_transcript_files(files)
            .iter()
            .any(|path| has_nonempty_text(path))
    {
        return false;
    }
    if files
        .iter()
        .any(|path| is_audio_jsonl(path) || is_screen_jsonl(path))
    {
        return false;
    }
    !files.iter().any(|path| is_body_content_file(path))
}

fn segment_files(segment_path: &Path) -> Result<Vec<PathBuf>, HealthError> {
    let entries = fs::read_dir(segment_path).map_err(|error| HealthError::Directory {
        path: segment_path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut files = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| HealthError::Directory {
            path: segment_path.to_path_buf(),
            message: error.to_string(),
        })?
        .into_iter()
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn markdown_transcript_files(files: &[PathBuf]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|path| {
            file_name(path)
                .is_some_and(|name| name == "imported.md" || name.ends_with("_transcript.md"))
        })
        .cloned()
        .collect()
}

fn is_audio_jsonl(path: &Path) -> bool {
    file_name(path).is_some_and(|name| {
        name == "audio.jsonl"
            || name.ends_with("_audio.jsonl")
            || name.ends_with("_transcript.jsonl")
    })
}

fn is_screen_jsonl(path: &Path) -> bool {
    file_name(path).is_some_and(|name| name == "screen.jsonl" || name.ends_with("_screen.jsonl"))
}

fn is_browser_jsonl(path: &Path) -> bool {
    file_name(path).is_some_and(|name| name.starts_with("browser_") && name.ends_with(".jsonl"))
}

fn has_nonempty_text(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)
}

fn has_raw_media(files: &[PathBuf], kind: MediaKind) -> bool {
    files.iter().any(|path| media_kind_for(path) == Some(kind))
}

fn newest_input_mtime_ms(paths: &[&PathBuf]) -> Option<i64> {
    paths
        .iter()
        .filter_map(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok())
        .map(|modified| DateTime::<Utc>::from(modified).timestamp_millis())
        .max()
}

fn is_body_content_file(path: &Path) -> bool {
    media_kind_for(path).is_some() || has_pdf_extension(path)
}

fn has_pdf_extension(path: &Path) -> bool {
    extension(path)
        .map(|value| value.to_ascii_lowercase())
        .is_some_and(|value| PDF_EXTENSIONS.contains(&value.as_str()))
}

fn media_kind_for(path: &Path) -> Option<MediaKind> {
    media_kind(extension(path)?.to_ascii_lowercase().as_str())
}

fn extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|extension| extension.to_str())
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

fn read_processing_record(paths: &[PathBuf]) -> Option<Value> {
    for path in paths {
        let Ok(mut file) = fs::File::open(path) else {
            continue;
        };
        let mut window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES);
        if file
            .by_ref()
            .take(vocab::MAX_FIRST_ROW_BYTES as u64)
            .read_to_end(&mut window)
            .is_err()
        {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&window) else {
            continue;
        };
        let Some(line) = text.split('\n').find(|line| !line.trim().is_empty()) else {
            continue;
        };
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(record @ Value::Object(_)) = object.get("_solstone_processing") {
            return Some(record.clone());
        }
    }
    None
}

fn jsonl_has_row_with_key(path: &Path, row_key: &str) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut window = Vec::with_capacity(vocab::MAX_FIRST_ROW_BYTES);
    if file
        .by_ref()
        .take(vocab::MAX_FIRST_ROW_BYTES as u64)
        .read_to_end(&mut window)
        .is_err()
    {
        return false;
    }
    let Ok(text) = std::str::from_utf8(&window) else {
        return false;
    };
    text.split('\n')
        .filter(|line| !line.trim().is_empty())
        .take(2)
        .any(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|row| row.as_object().map(|row| row.contains_key(row_key)))
                .unwrap_or(false)
        })
}

fn slots_to_ranges(slots: Vec<u64>) -> Vec<TimeRange> {
    let Some((&first, rest)) = slots.split_first() else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    let mut start = first;
    let mut previous = first;
    for &current in rest {
        if current - previous == 900 {
            previous = current;
            continue;
        }
        ranges.push((format_time(start), format_time((previous + 900) % 86_400)));
        start = current;
        previous = current;
    }
    ranges.push((format_time(start), format_time((previous + 900) % 86_400)));
    ranges
}

fn format_time(seconds: u64) -> String {
    format!("{:02}:{:02}", seconds / 3_600, (seconds % 3_600) / 60)
}
