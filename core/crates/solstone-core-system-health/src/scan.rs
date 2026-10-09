// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use solstone_core_format::segment::segment_start_and_end_seconds;
use solstone_core_journal_io::{SegmentIdentityError, SegmentLayout, StreamLocation};
use solstone_core_processing_record::{
    MediaKind, analysis_row_key, jsonl_has_row_with_key, media_kind, vocab,
};

use crate::{
    DataState, DataStateMap, HealthError, SegmentInput, SegmentSource, derive_modality_state,
};

const PDF_EXTENSIONS: &[&str] = &["pdf"];

pub type TimeRange = (String, String);
pub type ScanResult = Result<(Vec<TimeRange>, Vec<TimeRange>, Vec<DaySegment>), HealthError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnclaimedImageState {
    pub state: DataState,
    pub raw_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationSegmentRow {
    pub key: String,
    pub stream: String,
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaySegment {
    pub key: String,
    pub stream: String,
    /// On-disk layout; `stream` alone cannot tell Direct from a Named stream.
    pub stream_layout: SegmentLayout,
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

fn segment_start_and_end(
    times: solstone_core_format::segment::SegmentTimes,
    end_seconds: u64,
) -> (u64, String, String) {
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
    (start_seconds, start, end)
}

fn timeline_types(data_state: &DataStateMap) -> Vec<String> {
    ["audio", "screen", "image", "markdown", "browser"]
        .into_iter()
        .filter(|modality| data_state.0.contains_key(*modality))
        .map(str::to_owned)
        .collect()
}

pub fn holds_location_file(dir: &Path) -> bool {
    dir.join("location.jsonl").is_file()
}

pub fn is_location_only(
    dir: &Path,
    stream_parent_name: &str,
    now: DateTime<Utc>,
) -> Result<bool, HealthError> {
    if !holds_location_file(dir) {
        return Ok(false);
    }
    let (data_state, _) = detect_data_state(dir, stream_parent_name, now)?;
    Ok(timeline_types(&data_state).is_empty())
}

pub fn list_location_only_segments<S: SegmentSource>(
    source: &S,
    journal: &Path,
    day: &str,
    now: DateTime<Utc>,
) -> Result<Vec<LocationSegmentRow>, HealthError> {
    let day_path = solstone_core_journal_io::day_path(journal, day, false)?;
    if !day_path.is_dir() {
        return Ok(Vec::new());
    }
    let _day_date = NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| HealthError::InvalidDay(day.to_owned()))?;
    let mut rows = Vec::new();

    for segment in source.segments(journal, day)? {
        let Some(raw_name) = segment.name().to_str() else {
            return Err(HealthError::UnrepresentableSegment {
                path: segment.path().to_path_buf(),
            });
        };
        let Some((times, end_seconds)) = segment_start_and_end_seconds(raw_name) else {
            continue;
        };
        let (_start_seconds, start, end) = segment_start_and_end(times, end_seconds);
        let parent_name = segment
            .path()
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        if !is_location_only(segment.path(), parent_name, now)? {
            continue;
        }

        let Ok(identity) = segment.record_identity() else {
            continue;
        };

        rows.push(LocationSegmentRow {
            key: raw_name.to_owned(),
            stream: identity.stream.to_owned(),
            start,
            end,
        });
    }

    rows.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then(left.stream.cmp(&right.stream))
            .then(left.key.cmp(&right.key))
    });
    Ok(rows)
}

pub fn scan_day<S: SegmentSource>(
    source: &S,
    journal: &Path,
    day: &str,
    now: DateTime<Utc>,
) -> ScanResult {
    let day_path = solstone_core_journal_io::day_path(journal, day, false)?;
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
        let (start_seconds, start, end) = segment_start_and_end(times, end_seconds);
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
        let types = timeline_types(&data_state);
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
            stream_layout: match segment.stream() {
                StreamLocation::Direct => SegmentLayout::Direct,
                StreamLocation::Named(_) => SegmentLayout::Named,
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

    let image_state = unclaimed_image_state(segment_path, stream_parent_name, now)?;
    let image = image_state.state;
    let image_raw_paths = image_state.raw_paths;

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
    if image != DataState::Absent {
        states.insert("image".to_owned(), image.as_str().to_owned());
        modality_input_mtime_ms.insert(
            "image".to_owned(),
            newest_input_mtime_ms(&image_raw_paths.iter().collect::<Vec<_>>()),
        );
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
    if !solstone_core_body_source::health_card_streams().any(|stream| stream == stream_parent_name)
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

pub fn unclaimed_image_state(
    segment_path: &Path,
    stream_parent_name: &str,
    now: DateTime<Utc>,
) -> Result<UnclaimedImageState, HealthError> {
    if stream_parent_name.starts_with("import.") {
        return Ok(UnclaimedImageState {
            state: DataState::Absent,
            raw_paths: Vec::new(),
        });
    }
    let files = segment_files(segment_path)?;
    let raw_paths = files
        .into_iter()
        .filter(|path| {
            media_kind_for(path) == Some(MediaKind::Image) && !claimed_by_non_depict_handler(path)
        })
        .collect::<Vec<_>>();
    let state = aggregate_image_state(segment_path, &raw_paths, now);
    Ok(UnclaimedImageState { state, raw_paths })
}

fn aggregate_image_state(
    segment_path: &Path,
    image_raw_paths: &[PathBuf],
    now: DateTime<Utc>,
) -> DataState {
    if image_raw_paths.is_empty() {
        return DataState::Absent;
    }
    let states = image_raw_paths
        .iter()
        .map(|raw| {
            let output = raw.with_extension("jsonl");
            derive_modality_state(
                segment_path,
                "image",
                jsonl_has_row_with_key(&output, vocab::IMAGE_ANALYSIS_ROW_KEY),
                output.is_file(),
                true,
                read_processing_record(std::slice::from_ref(&output)).as_ref(),
                now,
            )
        })
        .collect::<Vec<_>>();
    if states.contains(&DataState::Failed) {
        return DataState::Failed;
    }
    if states.contains(&DataState::Analyzing) {
        return DataState::Analyzing;
    }
    if states.contains(&DataState::Pending) {
        return DataState::Pending;
    }
    if states.contains(&DataState::FailedFinal) {
        return DataState::FailedFinal;
    }
    if states.iter().all(|state| *state == DataState::Empty) {
        return DataState::Empty;
    }
    if states.iter().all(|state| *state == DataState::Purged) {
        return DataState::Purged;
    }
    DataState::Analyzed
}

fn claimed_by_non_depict_handler(raw: &Path) -> bool {
    let output = raw.with_extension("jsonl");
    let Some(record) = read_processing_record(std::slice::from_ref(&output)) else {
        return false;
    };
    let Some(handler) = record.get("handler").and_then(Value::as_str) else {
        return false;
    };
    handler != vocab::HANDLER_DEPICT
        && analysis_row_key(handler).is_some_and(|key| jsonl_has_row_with_key(&output, key))
}

fn has_nonempty_text(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)
}

fn has_raw_media(files: &[PathBuf], kind: MediaKind) -> bool {
    files.iter().any(|path| media_kind_for(path) == Some(kind))
}

/// The newest modification time, in epoch milliseconds, among a segment's input files.
///
/// Input is every regular file directly in the segment directory except what
/// thinking itself writes there (`events.jsonl`) and transient lock sidecars
/// and dot-named temporaries.  Talent output lives under `talents/` and is not
/// read.  `None` when the segment holds no input file.
pub fn newest_segment_input_ms(segment_path: &Path) -> Result<Option<i64>, HealthError> {
    let files = segment_files(segment_path)?;
    let inputs = files
        .iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name != "events.jsonl" && !name.ends_with(".lock") && !name.starts_with('.')
                })
        })
        .collect::<Vec<_>>();
    Ok(newest_input_mtime_ms(&inputs))
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::SystemTime;

    use chrono::{DateTime, Utc};
    use tempfile::TempDir;

    use super::*;
    use crate::FilesystemSegmentSource;

    fn write_file(path: &Path, name: &str, contents: &str) {
        fs::write(path.join(name), contents).unwrap();
    }

    #[test]
    fn segment_input_is_what_thinking_reads_not_what_it_writes() {
        let segment = TempDir::new().unwrap();
        let at = |seconds: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        let place = |name: &str, seconds: u64| {
            let path = segment.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "x").unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(at(seconds))
                .unwrap();
        };
        assert_eq!(newest_segment_input_ms(segment.path()).unwrap(), None);
        place("audio.m4a", 100);
        place("audio.jsonl", 200);
        place("events.jsonl", 900);
        place("audio.jsonl.lock", 900);
        place(".describe-1-0.jsonl.tmp", 900);
        place("talents/sense.json", 900);
        assert_eq!(
            newest_segment_input_ms(segment.path()).unwrap(),
            Some(200_000)
        );
        place("screen.jsonl", 300);
        assert_eq!(
            newest_segment_input_ms(segment.path()).unwrap(),
            Some(300_000)
        );
    }

    #[test]
    fn health_card_streams_registry_scan_exemption() {
        let streams = solstone_core_body_source::health_card_streams().collect::<Vec<_>>();
        assert!(!streams.is_empty());
        assert!(streams.contains(&"import.apple_health"));
        assert!(streams.contains(&"import.oura"));

        let day = "20990202";
        let now = DateTime::<Utc>::from(std::time::SystemTime::now());

        for stream in &streams {
            let temporary = TempDir::new().unwrap();
            let root = temporary.path();
            let seg_dir = root
                .join("chronicle")
                .join(day)
                .join(stream)
                .join("090000_300");
            fs::create_dir_all(&seg_dir).unwrap();
            write_file(&seg_dir, "imported.md", "card\n");

            let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();
            assert_eq!(segments.len(), 1, "stream {stream} should yield 1 segment");
            assert_eq!(
                segments[0].types,
                vec!["markdown".to_owned()],
                "stream {stream} should be markdown-only"
            );
            assert_eq!(segments[0].data_state.0.len(), 1);
            assert_eq!(
                segments[0].data_state.0.get("markdown").map(|s| s.as_str()),
                Some("analyzed")
            );
        }

        // import.notes with only nonempty imported.md has types == ["audio"]
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let notes_dir = root
            .join("chronicle")
            .join(day)
            .join("import.notes")
            .join("090000_300");
        fs::create_dir_all(&notes_dir).unwrap();
        write_file(&notes_dir, "imported.md", "notes\n");
        let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();
        assert_eq!(segments[0].types, vec!["audio".to_owned()]);

        // card stream with imported.md plus attachment.PDF is not exempted (types == ["audio"])
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let pdf_dir = root
            .join("chronicle")
            .join(day)
            .join("import.apple_health")
            .join("090000_300");
        fs::create_dir_all(&pdf_dir).unwrap();
        write_file(&pdf_dir, "imported.md", "card\n");
        write_file(&pdf_dir, "attachment.PDF", "pdf-data\n");
        let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();
        assert_eq!(segments[0].types, vec!["audio".to_owned()]);

        // card stream with imported.md plus audio.jsonl or screen.jsonl is not exempted
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let audio_dir = root
            .join("chronicle")
            .join(day)
            .join("import.oura")
            .join("090000_300");
        fs::create_dir_all(&audio_dir).unwrap();
        write_file(&audio_dir, "imported.md", "card\n");
        write_file(&audio_dir, "audio.jsonl", "{}\n{\"start\":0}\n");
        let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();
        assert_ne!(segments[0].types, vec!["markdown".to_owned()]);

        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let screen_dir = root
            .join("chronicle")
            .join(day)
            .join("import.apple_health")
            .join("090000_300");
        fs::create_dir_all(&screen_dir).unwrap();
        write_file(&screen_dir, "imported.md", "card\n");
        write_file(&screen_dir, "screen.jsonl", "{\"start\":0}\n");
        let (_, _, segments) = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();
        assert_ne!(segments[0].types, vec!["markdown".to_owned()]);
    }

    const STILL_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    struct RefusingWire;

    impl solstone_core_depict::WireClient for RefusingWire {
        fn execute(
            &self,
            _: &solstone_core_generate::GenerateRequest,
        ) -> Result<solstone_core_generate::GenerateResponse, solstone_core_generate::ClientError>
        {
            Ok(solstone_core_generate::GenerateResponse::Refused(
                solstone_core_generate::RefusedResponse {
                    id: None,
                    reason: solstone_core_generate::RefusalReason::IncompleteText,
                    reason_code: Some(solstone_core_generate::ReasonCodeValue::Known(
                        solstone_core_generate::ReasonCode::new("incomplete_text_length")
                            .expect("known reason"),
                    )),
                    retryable: false,
                    blocking: false,
                    reset_at_ms: None,
                    provider: None,
                    detail: "wire detail".to_owned(),
                },
            ))
        }
    }

    struct SilentDetector;

    impl solstone_core_depict::Detector for SilentDetector {
        fn detect(&self, _: &[u8]) -> Result<Option<serde_json::Value>, String> {
            Ok(None)
        }
    }

    #[test]
    fn writer_failed_depict_record_is_failed_not_pending() {
        let temporary = TempDir::new().unwrap();
        let segment_dir = temporary.path().join("chronicle/20260101/field/120000_60");
        fs::create_dir_all(&segment_dir).unwrap();
        let image = segment_dir.join("photo.png");
        fs::write(&image, STILL_PNG).unwrap();
        assert!(
            solstone_core_depict::run_with_clients(&image, false, &RefusingWire, &SilentDetector)
                .is_err()
        );
        let now = DateTime::<Utc>::from(std::time::SystemTime::UNIX_EPOCH);
        let (states, _) = detect_data_state(&segment_dir, "field", now).unwrap();
        assert_eq!(states.0.get("image").map(String::as_str), Some("failed"));
    }

    fn device_ingest_event_line(stream: &str, day: &str, segment: &str) -> String {
        serde_json::json!({
            "record_type": "device_ingest",
            "record_version": 1,
            "outcome": "accepted",
            "protocol_version": 3,
            "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source": "",
            "stream": stream,
            "day": day,
            "segment": segment,
            "files": [],
            "meta": {},
        })
        .to_string()
            + "\n"
    }

    #[test]
    fn producer_shaped_day_scan_day_equality_with_and_without_location() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let day = "20260101";
        let now = Utc::now();

        let loc_dir = root
            .join("chronicle")
            .join(day)
            .join("field")
            .join("120000_60");
        fs::create_dir_all(&loc_dir).unwrap();
        write_file(&loc_dir, "location.jsonl", "{\"lat\":0.0}\n");
        write_file(&loc_dir, "stream.json", "{\"stream\":\"field\"}\n");
        write_file(
            &loc_dir,
            "events.jsonl",
            &device_ingest_event_line("field", day, "120000_60"),
        );

        let mix_dir = root
            .join("chronicle")
            .join(day)
            .join("field")
            .join("120500_60");
        fs::create_dir_all(&mix_dir).unwrap();
        write_file(
            &mix_dir,
            "audio.jsonl",
            "{\"_solstone_processing\":{\"handler\":\"transcribe\"}}\n{\"row_type\":\"audio_transcript\"}\n",
        );
        write_file(&mix_dir, "location.jsonl", "{\"lat\":0.0}\n");
        write_file(&mix_dir, "stream.json", "{\"stream\":\"field\"}\n");
        write_file(
            &mix_dir,
            "events.jsonl",
            &device_ingest_event_line("field", day, "120500_60"),
        );

        let with_location = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();

        fs::remove_file(loc_dir.join("location.jsonl")).unwrap();
        fs::remove_file(mix_dir.join("location.jsonl")).unwrap();

        let without_location = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();
        assert_eq!(with_location, without_location);
    }

    #[test]
    fn location_only_partition_against_scan_day() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let day = "20260101";
        let now = Utc::now();
        let stream = "phone";

        // 1. location.jsonl alone -> location-only
        let d1 = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join("090000_60");
        fs::create_dir_all(&d1).unwrap();
        write_file(&d1, "location.jsonl", "{\"lat\":0.0}\n");

        // 2. beside audio.jsonl -> scan_day
        let d2 = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join("090100_60");
        fs::create_dir_all(&d2).unwrap();
        write_file(&d2, "location.jsonl", "{\"lat\":0.0}\n");
        write_file(&d2, "audio.jsonl", "{\"row_type\":\"audio_transcript\"}\n");

        // 3. beside fresh .analyzing_audio -> scan_day
        let d3 = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join("090200_60");
        fs::create_dir_all(&d3).unwrap();
        write_file(&d3, "location.jsonl", "{\"lat\":0.0}\n");
        let marker = d3.join(".analyzing_audio");
        write_file(&d3, ".analyzing_audio", "{}\n");
        fs::File::open(&marker)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(SystemTime::from(now)))
            .unwrap();

        // 4. beside .analyze_failed_audio -> scan_day
        let d4 = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join("090300_60");
        fs::create_dir_all(&d4).unwrap();
        write_file(&d4, "location.jsonl", "{\"lat\":0.0}\n");
        write_file(&d4, ".analyze_failed_audio", "{}\n");

        // 5. beside raw audio.m4a -> scan_day
        let d5 = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join("090400_60");
        fs::create_dir_all(&d5).unwrap();
        write_file(&d5, "location.jsonl", "{\"lat\":0.0}\n");
        write_file(&d5, "audio.m4a", "fake-audio");

        // 6. beside nonempty *_transcript.md on a normal stream -> scan_day
        let d6 = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join("090500_60");
        fs::create_dir_all(&d6).unwrap();
        write_file(&d6, "location.jsonl", "{\"lat\":0.0}\n");
        write_file(&d6, "mic_transcript.md", "transcript contents");

        // 7. beside *_transcript.jsonl on a normal stream -> scan_day
        let d7 = root
            .join("chronicle")
            .join(day)
            .join(stream)
            .join("090600_60");
        fs::create_dir_all(&d7).unwrap();
        write_file(&d7, "location.jsonl", "{\"lat\":0.0}\n");
        write_file(&d7, "mic_transcript.jsonl", "{}\n");

        let (_, _, timeline) = scan_day(&FilesystemSegmentSource, root, day, now).unwrap();
        let location_only =
            list_location_only_segments(&FilesystemSegmentSource, root, day, now).unwrap();

        let timeline_keys: BTreeSet<String> = timeline.into_iter().map(|seg| seg.key).collect();
        let location_keys: BTreeSet<String> =
            location_only.into_iter().map(|row| row.key).collect();

        let all_keys = [
            "090000_60",
            "090100_60",
            "090200_60",
            "090300_60",
            "090400_60",
            "090500_60",
            "090600_60",
        ];

        for key in all_keys {
            let in_timeline = timeline_keys.contains(key);
            let in_location = location_keys.contains(key);
            assert!(
                in_timeline ^ in_location,
                "key {key} must be in exactly one set: timeline={in_timeline}, location={in_location}"
            );
        }
        assert!(location_keys.contains("090000_60"));
    }

    #[test]
    fn ambiguous_named_default_dropped_from_location_list_and_sibling_returns() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let day = "20260101";
        let now = Utc::now();

        let default_named = root
            .join("chronicle")
            .join(day)
            .join("_default")
            .join("120000_60");
        fs::create_dir_all(&default_named).unwrap();
        write_file(&default_named, "location.jsonl", "{\"lat\":0.0}\n");

        let phone_named = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("120000_60");
        fs::create_dir_all(&phone_named).unwrap();
        write_file(&phone_named, "location.jsonl", "{\"lat\":0.0}\n");

        let location_only =
            list_location_only_segments(&FilesystemSegmentSource, root, day, now).unwrap();
        assert_eq!(location_only.len(), 1);
        assert_eq!(location_only[0].stream, "phone");
        assert_eq!(location_only[0].key, "120000_60");

        // scan_day skips both because neither has timeline modality, returning Ok(empty)
        assert!(scan_day(&FilesystemSegmentSource, root, day, now).is_ok());
    }

    #[test]
    fn direct_layout_location_only_listed_with_default_stream() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let day = "20260101";
        let now = Utc::now();

        let direct = root.join("chronicle").join(day).join("120000_60");
        fs::create_dir_all(&direct).unwrap();
        write_file(&direct, "location.jsonl", "{\"lat\":0.0}\n");

        let location_only =
            list_location_only_segments(&FilesystemSegmentSource, root, day, now).unwrap();
        assert_eq!(location_only.len(), 1);
        assert_eq!(
            location_only[0].stream,
            solstone_core_journal_io::DEFAULT_STREAM
        );
        assert_eq!(location_only[0].key, "120000_60");
    }

    #[test]
    fn unparseable_directory_skipped_and_missing_day_is_empty() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let day = "20260101";
        let now = Utc::now();

        let invalid = root
            .join("chronicle")
            .join(day)
            .join("phone")
            .join("invalid_segment_name");
        fs::create_dir_all(&invalid).unwrap();
        write_file(&invalid, "location.jsonl", "{\"lat\":0.0}\n");

        let rows = list_location_only_segments(&FilesystemSegmentSource, root, day, now).unwrap();
        assert!(rows.is_empty());

        let missing =
            list_location_only_segments(&FilesystemSegmentSource, root, "20260102", now).unwrap();
        assert!(missing.is_empty());
    }
}
