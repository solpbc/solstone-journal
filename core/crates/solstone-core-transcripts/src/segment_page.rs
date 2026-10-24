// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded transcript-only reads for one chronicle segment.

//! This module intentionally does not share the broad day-clustering surface:
//! it has no percept, browser, or talent selection flags.

use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_format::content::{
    ContentResolution, Family, RawPerceptFamily, classify, produce_chunks,
    produce_raw_percept_chunks,
};
use solstone_core_journal_io::paths::{Segment, segment_origin};
use solstone_core_processing_record::is_transcribe_output;

use crate::{VoiceNames, next_statement_ordinal, speaker_label_source};

/// Who an agent's transcript line names: only a speaker the journal's voice
/// gate names (the same gate an activity's Story reads), with the owner's own
/// recognized lines named by `owner`, so an agent can tell them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpeakers {
    pub voices: VoiceNames,
    pub owner: String,
}

/// Largest source-byte window parsed for one transcript page.
pub const MAX_TRANSCRIPT_PAGE_BYTES: usize = 64 * 1024;
/// Largest number of transcript entries returned for one transcript page.
pub const MAX_TRANSCRIPT_PAGE_ITEMS: usize = 100;
pub const POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE: &str =
    "possible speaker change within this sentence";

/// Parsed wall-clock times and formatted display range for a chronicle segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentClock {
    pub label: String,
    pub start_seconds: u32,
    pub exclusive_end_seconds: u32,
}

pub fn segment_clock(directory_name: &str) -> Option<SegmentClock> {
    let key = solstone_core_format::segment::segment_key(directory_name)?;
    let (time_part, length_part) = key.split_once('_')?;
    if time_part.len() != 6 || !time_part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour = time_part[0..2].parse::<u32>().ok()?;
    let minute = time_part[2..4].parse::<u32>().ok()?;
    let second = time_part[4..6].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let duration = length_part.parse::<u32>().ok()?;
    let start_seconds = hour * 3600 + minute * 60 + second;
    let exclusive_end_seconds = start_seconds.saturating_add(duration).min(86400);
    let display_end = start_seconds.saturating_add(duration).min(86399);
    let end_h = display_end / 3600;
    let end_m = (display_end % 3600) / 60;
    let label = format!("{hour:02}:{minute:02}-{end_h:02}:{end_m:02}");
    Some(SegmentClock {
        label,
        start_seconds,
        exclusive_end_seconds,
    })
}

/// Opaque-to-callers source version for one segment's approved transcript files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptVersion(u64);

impl SegmentTranscriptVersion {
    /// Stable source-version value for binding an opaque continuation.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        self.0
    }

    /// Reconstruct a cursor version previously obtained from this API.
    #[must_use]
    pub fn from_fingerprint(value: u64) -> Self {
        Self(value)
    }
}

/// Private continuation point within the approved transcript file list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptCursor {
    pub source_index: usize,
    pub byte_offset: u64,
    pub statement_ordinal: u64,
    pub version: SegmentTranscriptVersion,
}

/// One projected transcript entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptEntry {
    pub text: String,
    /// Channel heading understood by source units. Speech stays `Transcript`.
    /// Chat, markdown, and other imported text use a non-speech channel.
    pub channel: &'static str,
}

/// One bounded page from a single segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptPage {
    pub entries: Vec<SegmentTranscriptEntry>,
    pub next: Option<SegmentTranscriptCursor>,
    pub version: SegmentTranscriptVersion,
    pub untimed: bool,
}

/// Failure while reading a bounded transcript page.
#[derive(Debug)]
pub enum SegmentTranscriptReadError {
    Changed,
    InvalidCursor,
    Io,
    RecordTooLarge,
}

fn peek_untimed(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(file.take(MAX_TRANSCRIPT_PAGE_BYTES as u64));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return false;
    }
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(line.trim()) {
        map.get("untimed") == Some(&Value::Bool(true))
    } else {
        false
    }
}

/// Read one bounded page of approved transcript text from exactly one segment.
///
/// The caller owns authorization. This function only projects transcript input;
/// it never walks a day, reads unrelated percept/browser/talent content, or applies scope.
/// Segment-local speaker labels supply names only, and only as `speakers` allows;
/// the entity profile is never projected. Without `speakers`, lines keep the
/// stored diarization label.
pub fn read_segment_transcript_page(
    segment: &Segment,
    cursor: Option<&SegmentTranscriptCursor>,
    speakers: Option<&AgentSpeakers>,
) -> Result<SegmentTranscriptPage, SegmentTranscriptReadError> {
    let files = transcript_files(segment.path())?;
    let labels = speakers.and_then(|speakers| local_speaker_names(segment.path(), speakers));
    let version = source_version(&files, &labels)?;
    let untimed = files
        .iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .any(|path| peek_untimed(path));
    let (mut source_index, mut byte_offset, mut statement_ordinal) = match cursor {
        Some(cursor) if cursor.version != version => {
            return Err(SegmentTranscriptReadError::Changed);
        }
        Some(cursor) if cursor.source_index > files.len() => {
            return Err(SegmentTranscriptReadError::InvalidCursor);
        }
        Some(cursor) => (
            cursor.source_index,
            cursor.byte_offset,
            cursor.statement_ordinal,
        ),
        None => (0, 0, 0),
    };

    let segment_origin = segment_origin(segment.path());
    let mut entries = Vec::new();
    while source_index < files.len() && entries.len() < MAX_TRANSCRIPT_PAGE_ITEMS {
        let path = &files[source_index];
        let (window, advanced, exhausted) = read_window(path, byte_offset)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let speech = is_speech_transcript_name(name) || is_transcribe_output(path);
        let names = labels
            .as_ref()
            .filter(|(source, _)| {
                name.strip_suffix(".jsonl") == Some(source.as_str())
                    && !matches!(
                        classify(&format!("{segment_origin}/{name}")),
                        ContentResolution::Indexed(Family::AiChat)
                    )
            })
            .map(|(_, names)| names);
        let mut consumed = 0;
        let mut stopped_in_window = false;
        for (line, line_end) in complete_lines(&window)? {
            let sentence_id = next_statement_ordinal(line, &mut statement_ordinal);
            let named_line = names.and_then(|names| {
                let speaker = names.get(&sentence_id?)?;
                let mut value: Value = serde_json::from_str(line).ok()?;
                value
                    .as_object_mut()?
                    .insert("speaker".into(), Value::String(speaker.clone()));
                Some(value.to_string())
            });
            let text = project_line(
                &segment_origin,
                name,
                speech,
                named_line.as_deref().unwrap_or(line),
            );
            if !text.trim().is_empty() {
                entries.push(SegmentTranscriptEntry {
                    channel: source_channel(&segment_origin, name, speech),
                    text,
                });
            }
            consumed = line_end;
            if entries.len() == MAX_TRANSCRIPT_PAGE_ITEMS {
                stopped_in_window = consumed < advanced;
                break;
            }
        }
        byte_offset = byte_offset.saturating_add(
            (if stopped_in_window {
                consumed
            } else {
                advanced
            }) as u64,
        );
        if exhausted && !stopped_in_window {
            source_index += 1;
            byte_offset = 0;
            statement_ordinal = 0;
        }
        if advanced == 0 && !exhausted {
            return Err(SegmentTranscriptReadError::RecordTooLarge);
        }
    }

    let next = (source_index < files.len()).then(|| SegmentTranscriptCursor {
        source_index,
        byte_offset,
        statement_ordinal,
        version: version.clone(),
    });
    Ok(SegmentTranscriptPage {
        entries,
        next,
        version,
        untimed,
    })
}

fn transcript_files(segment: &Path) -> Result<Vec<PathBuf>, SegmentTranscriptReadError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(segment).map_err(|_| SegmentTranscriptReadError::Io)? {
        let entry = entry.map_err(|_| SegmentTranscriptReadError::Io)?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_transcript_name(name) || is_transcribe_output(&path) {
            let meta = fs::symlink_metadata(&path).map_err(|_| SegmentTranscriptReadError::Io)?;
            if !meta.file_type().is_file() {
                return Err(SegmentTranscriptReadError::Io);
            }
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn is_transcript_name(name: &str) -> bool {
    name == "audio.jsonl"
        || name.ends_with("_audio.jsonl")
        || (name.ends_with("_transcript.jsonl")
            || solstone_core_format::audio_archive::is_audio_archive_transcript_name(name))
        || name == "imported.md"
        || name.ends_with("_transcript.md")
        || name == "workout.json"
}

fn source_version(
    files: &[PathBuf],
    labels: &Option<(String, BTreeMap<i64, String>)>,
) -> Result<SegmentTranscriptVersion, SegmentTranscriptReadError> {
    let mut hasher = DefaultHasher::new();
    labels.hash(&mut hasher);
    for path in files {
        let metadata = fs::metadata(path).map_err(|_| SegmentTranscriptReadError::Io)?;
        path.file_name().hash(&mut hasher);
        metadata.len().hash(&mut hasher);
        let modified = metadata
            .modified()
            .map_err(|_| SegmentTranscriptReadError::Io)?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| SegmentTranscriptReadError::Io)?;
        modified.as_secs().hash(&mut hasher);
        modified.subsec_nanos().hash(&mut hasher);
    }
    Ok(SegmentTranscriptVersion(hasher.finish()))
}

// Resolve only names used by this segment, through the voice gate: a name the
// journal inferred from what was said, or a declined owner match, names no one.
fn local_speaker_names(
    segment: &Path,
    speakers: &AgentSpeakers,
) -> Option<(String, BTreeMap<i64, String>)> {
    let path = segment.join("talents/speaker_labels.json");
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_TRANSCRIPT_PAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_TRANSCRIPT_PAGE_BYTES {
        return None;
    }
    let payload: Value = serde_json::from_slice(&bytes).ok()?;
    let rows = payload.get("labels")?.as_array()?;
    let (source, _) = speaker_label_source(segment);
    let source = source?;
    let labels = rows
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|row| {
            let id = row.get("sentence_id")?.as_i64()?;
            let name = speakers.voices.name_for(row)?;
            let owner = row.get("speaker").and_then(Value::as_str)
                == Some(speakers.voices.principal.as_str());
            Some((id, if owner { speakers.owner.clone() } else { name }))
        })
        .collect::<BTreeMap<_, _>>();
    (!labels.is_empty()).then_some((source, labels))
}

fn read_window(
    path: &Path,
    offset: u64,
) -> Result<(Vec<u8>, usize, bool), SegmentTranscriptReadError> {
    let meta = fs::symlink_metadata(path).map_err(|_| SegmentTranscriptReadError::Io)?;
    if !meta.file_type().is_file() {
        return Err(SegmentTranscriptReadError::Io);
    }
    let mut file = File::open(path).map_err(|_| SegmentTranscriptReadError::Io)?;
    let length = meta.len();
    if offset > length {
        return Err(SegmentTranscriptReadError::Changed);
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| SegmentTranscriptReadError::Io)?;
    let mut bytes = Vec::with_capacity(MAX_TRANSCRIPT_PAGE_BYTES);
    file.take(MAX_TRANSCRIPT_PAGE_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| SegmentTranscriptReadError::Io)?;
    let exhausted = offset + bytes.len() as u64 == length;
    let advanced = if exhausted {
        bytes.len()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0)
    };
    bytes.truncate(advanced);
    Ok((bytes, advanced, exhausted))
}

fn complete_lines(bytes: &[u8]) -> Result<Vec<(&str, usize)>, SegmentTranscriptReadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| SegmentTranscriptReadError::Io)?;
    let mut end = 0;
    let lines = text
        .split_inclusive('\n')
        .map(|raw| {
            end += raw.len();
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            let line = line.strip_suffix('\r').unwrap_or(line);
            (line, end)
        })
        .collect::<Vec<_>>();
    Ok(lines)
}

fn append_speaker_change_warning(text: &str, line: &str) -> String {
    let suspect = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("turn_boundary_suspect").and_then(Value::as_bool))
        == Some(true);
    if suspect && !text.is_empty() {
        format!("{text} [{POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE}]")
    } else {
        text.to_owned()
    }
}

fn stream_name(segment_origin: &str) -> Option<&str> {
    let mut parts = segment_origin.split('/');
    let _day = parts.next()?;
    let stream = parts.next()?;
    let _segment = parts.next()?;
    parts.next().is_none().then_some(stream)
}

fn is_speech_transcript_name(name: &str) -> bool {
    name == "audio.jsonl"
        || name.ends_with("_audio.jsonl")
        || name.ends_with("_transcript.jsonl")
        || solstone_core_format::audio_archive::is_audio_archive_transcript_name(name)
}

/// The source-unit channel for one approved transcript file. Classification is
/// the same path `project_line` already resolves; this does not open another file.
fn source_channel(segment_origin: &str, name: &str, speech: bool) -> &'static str {
    let rel = format!("{segment_origin}/{name}");
    let stream = stream_name(segment_origin);
    match classify(&rel) {
        ContentResolution::Indexed(Family::AiChat) => match stream {
            Some("import.chatgpt") => "ChatGPT Conversation",
            Some("import.claude") => "Claude Conversation",
            Some("import.gemini") => "Gemini Conversation",
            _ => "Imported Text",
        },
        ContentResolution::Unindexed(RawPerceptFamily::Audio) => "Transcript",
        _ if speech => "Transcript",
        _ => match stream {
            Some("import.obsidian") => "Note",
            Some("import.kindle") => "Highlights",
            Some("import.ics") => "Calendar Event",
            _ => "Imported Text",
        },
    }
}

fn project_line(segment_origin: &str, name: &str, speech: bool, line: &str) -> String {
    let rel = format!("{segment_origin}/{name}");
    match classify(&rel) {
        ContentResolution::Unindexed(RawPerceptFamily::Audio) => {
            let text = produce_raw_percept_chunks(RawPerceptFamily::Audio, &rel, line)
                .chunks
                .into_iter()
                .map(|chunk| chunk.content)
                .collect::<Vec<_>>()
                .join("\n");
            append_speaker_change_warning(&text, line)
        }
        ContentResolution::Indexed(Family::AiChat) => {
            let produced = produce_chunks(Family::AiChat, &rel, line);
            let start = serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|v| v.get("start").and_then(Value::as_str).map(str::to_owned));
            produced
                .chunks
                .into_iter()
                .map(|chunk| {
                    if let Some(ref start) = start {
                        format!("[{start}] {}", chunk.content)
                    } else {
                        chunk.content
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => {
            if name == "workout.json" {
                produce_chunks(Family::Workout, name, line)
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.content)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else if speech {
                let text = produce_raw_percept_chunks(RawPerceptFamily::Audio, &rel, line)
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.content)
                    .collect::<Vec<_>>()
                    .join("\n");
                append_speaker_change_warning(&text, line)
            } else {
                line.to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        MAX_TRANSCRIPT_PAGE_BYTES, MAX_TRANSCRIPT_PAGE_ITEMS, read_segment_transcript_page,
        segment_clock,
    };
    use solstone_core_format::content::{
        Family, RawPerceptFamily, produce_chunks, produce_raw_percept_chunks,
    };
    use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

    #[test]
    fn routine_tests_for_segment_clock() {
        let clock = segment_clock("000000_86400").unwrap();
        assert_eq!(clock.label, "00:00-23:59");
        assert_eq!(clock.start_seconds, 0);
        assert_eq!(clock.exclusive_end_seconds, 86400);

        let clock = segment_clock("110000_60").unwrap();
        assert_eq!(clock.label, "11:00-11:01");
        assert_eq!(clock.start_seconds, 39600);
        assert_eq!(clock.exclusive_end_seconds, 39660);

        let clock = segment_clock("110000_30").unwrap();
        assert_eq!(clock.label, "11:00-11:00");
        assert_eq!(clock.start_seconds, 39600);
        assert_eq!(clock.exclusive_end_seconds, 39630);

        let clock = segment_clock("233000_3600").unwrap();
        assert_eq!(clock.label, "23:30-23:59");
        assert_eq!(clock.start_seconds, 84600);
        assert_eq!(clock.exclusive_end_seconds, 86400);

        let clock = segment_clock("x-120000_300").unwrap();
        assert_eq!(clock.label, "12:00-12:05");
        assert_eq!(clock.start_seconds, 43200);
        assert_eq!(clock.exclusive_end_seconds, 43500);
    }

    #[test]
    fn pages_one_large_transcript_without_reading_talent_or_percept_files() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("meeting_transcript.md"),
            "approved transcript line\n".repeat(MAX_TRANSCRIPT_PAGE_BYTES / 8),
        )
        .unwrap();
        fs::write(segment.join("screen.jsonl"), "secret percept").unwrap();
        fs::write(segment.join("talents/brief.md"), "secret talent").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let first = read_segment_transcript_page(&segment, None, None).unwrap();
        assert!(!first.entries.is_empty());
        assert!(
            first
                .entries
                .iter()
                .all(|entry| entry.text == "approved transcript line")
        );
        assert!(first.next.is_some());
        assert!(!first.untimed);
    }

    #[test]
    fn transcript_page_reads_speech_recorded_under_any_name() {
        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle/20261023/device_audio/140000_12");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("note-0.wav"), b"RIFF").unwrap();
        fs::write(
            segment.join("note-0.jsonl"),
            concat!(
                r#"{"raw":"note-0.wav","_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","handler":"transcribe","input_size":4}}"#,
                "\n",
                r#"{"start":"14:00:00","text":"Maya checked the orchard irrigation pump this morning."}"#,
                "\n",
            ),
        )
        .unwrap();
        fs::write(
            segment.join("events.jsonl"),
            "{\"start\":\"14:00:01\",\"text\":\"ingest bookkeeping\"}\n",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20261023"))
            .unwrap()
            .pop()
            .unwrap();

        let page = read_segment_transcript_page(&segment, None, None).unwrap();
        assert_eq!(
            page.entries,
            vec![super::SegmentTranscriptEntry {
                text: "[14:00:00] Maya checked the orchard irrigation pump this morning."
                    .to_owned(),
                channel: "Transcript",
            }]
        );
    }

    #[test]
    fn transcript_page_projects_recorded_speech_without_other_sources() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("meeting_transcript.md"),
            "approved transcript line\n",
        )
        .unwrap();
        fs::write(
            segment.join("audio.jsonl"),
            "{\"recording\":\"metadata only\"}\n{\"start\":\"00:00:01\",\"text\":\"what was said\",\"raw\":\"secret audio metadata\"}\n",
        )
        .unwrap();
        fs::write(segment.join("screen.jsonl"), "screen percept\n").unwrap();
        fs::write(segment.join("browser_history.jsonl"), "browser percept\n").unwrap();
        fs::write(segment.join("talents/brief.md"), "secret talent\n").unwrap();
        fs::write(segment.join("talents/sense.json"), "secret sense\n").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let page = read_segment_transcript_page(&segment, None, None).unwrap();
        assert_eq!(
            page.entries,
            vec![
                super::SegmentTranscriptEntry {
                    text: "[00:00:01] what was said".to_owned(),
                    channel: "Transcript",
                },
                super::SegmentTranscriptEntry {
                    text: "approved transcript line".to_owned(),
                    channel: "Imported Text",
                },
            ]
        );
        assert!(!page.untimed);
    }

    #[test]
    fn audio_and_aichat_projections_and_renderer_guard() {
        let root = tempfile::tempdir().unwrap();
        // 1. import.audio
        let seg_audio = root
            .path()
            .join("chronicle/20260914/import.audio/090000_300");
        fs::create_dir_all(&seg_audio).unwrap();
        let audio_bytes =
            "{\"recording\":\"metadata\"}\n{\"start\":\"00:00:02\",\"text\":\"spoken words\"}\n";
        fs::write(seg_audio.join("imported_audio.jsonl"), audio_bytes).unwrap();

        // 2. legacy import.chatgpt
        let seg_chatgpt = root
            .path()
            .join("chronicle/20260914/import.chatgpt/090000_300");
        fs::create_dir_all(&seg_chatgpt).unwrap();
        let chat_bytes = "{\"imported\":{}}\n{\"start\":\"00:00:00\",\"speaker\":\"Ana Lima\",\"text\":\"Hello all.\"}\n";
        fs::write(seg_chatgpt.join("imported_audio.jsonl"), chat_bytes).unwrap();

        // 3. source_audio.jsonl in default
        let seg_custom = root.path().join("chronicle/20260914/default/090500_300");
        fs::create_dir_all(&seg_custom).unwrap();
        fs::write(seg_custom.join("mic_audio.jsonl"), audio_bytes).unwrap();

        // 4. timed import.text
        let seg_text = root
            .path()
            .join("chronicle/20260914/import.text/091000_300");
        fs::create_dir_all(&seg_text).unwrap();
        fs::write(seg_text.join("conversation_transcript.jsonl"), chat_bytes).unwrap();

        let segments = iter_segments(root.path(), PathOrDay::Day("20260914")).unwrap();

        // Verify import.audio page
        let seg = segments.iter().find(|s| s.path() == seg_audio).unwrap();
        let page = read_segment_transcript_page(seg, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:02] spoken words");

        // Verify custom _audio.jsonl page
        let seg = segments.iter().find(|s| s.path() == seg_custom).unwrap();
        let page = read_segment_transcript_page(seg, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:02] spoken words");

        // Verify import.chatgpt page
        let seg = segments.iter().find(|s| s.path() == seg_chatgpt).unwrap();
        let page = read_segment_transcript_page(seg, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:00] **Ana Lima:** Hello all.");

        // Verify import.text page
        let seg = segments.iter().find(|s| s.path() == seg_text).unwrap();
        let page = read_segment_transcript_page(seg, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:00] **Ana Lima:** Hello all.");

        // Renderer guard: call produce_raw_percept_chunks and produce_chunks directly
        let raw = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            "20260914/import.audio/090000_300/imported_audio.jsonl",
            audio_bytes,
        );
        assert_eq!(raw.chunks.len(), 1);
        assert_eq!(raw.chunks[0].content, "[00:00:02] spoken words");

        let chat = produce_chunks(
            Family::AiChat,
            "20260914/import.chatgpt/090000_300/imported_audio.jsonl",
            chat_bytes,
        );
        assert_eq!(chat.chunks.len(), 1);
        assert_eq!(chat.chunks[0].content, "**Ana Lima:** Hello all.");
        assert_eq!(chat.header.as_deref(), Some("# ChatGPT conversation"));
    }

    #[test]
    fn a_flat_legacy_source_transcript_reads_projected_not_raw() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let segment = root.path().join("chronicle/20250301/093000_300");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("session_transcript.jsonl"),
            "{\"raw\": \"session.flac\"}\n{\"start\": \"00:00:04\", \"speaker\": 2, \"text\": \"send the deck on friday\"}\n",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20250301"))
            .unwrap()
            .pop()
            .unwrap();
        let page = read_segment_transcript_page(&segment, None, None).unwrap();
        assert_eq!(page.entries.len(), 1, "{:?}", page.entries);
        assert!(page.entries[0].text.contains("send the deck on friday"));
        assert!(
            !page.entries[0].text.contains('{'),
            "{}",
            page.entries[0].text
        );
    }

    #[test]
    fn untimed_flag_persists_across_continuation_pages() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let segment = root
            .path()
            .join("chronicle/20260914/import.text/090000_300");
        fs::create_dir_all(&segment).unwrap();

        let mut lines = String::from("{\"untimed\": true, \"imported\": {}}\n");
        for i in 1..=MAX_TRANSCRIPT_PAGE_ITEMS + 10 {
            lines.push_str(&format!(
                "{{\"start\": \"00:00:00\", \"speaker\": \"User\", \"text\": \"turn {i}\"}}\n"
            ));
        }
        fs::write(segment.join("conversation_transcript.jsonl"), lines).unwrap();

        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let first = read_segment_transcript_page(&segment, None, None).unwrap();
        assert!(first.untimed);
        assert_eq!(first.entries.len(), MAX_TRANSCRIPT_PAGE_ITEMS);
        assert_eq!(first.entries[0].channel, "Imported Text");
        assert_eq!(first.entries[0].text, "[00:00:00] **User:** turn 1");
        assert!(first.next.is_some());

        let second = read_segment_transcript_page(&segment, first.next.as_ref(), None).unwrap();
        assert!(second.untimed);
        assert_eq!(second.entries.len(), 10);
        assert_eq!(second.entries[0].text, "[00:00:00] **User:** turn 101");
        assert!(second.next.is_none());
    }

    #[test]
    fn a_strava_workout_reads_as_what_strava_recorded_on_its_first_piece_only() {
        let root = tempfile::tempdir().unwrap();
        let workout = serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "activity_id": 9,
            "tile": {"index": 0, "count": 2},
            "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
        });
        for (key, index) in [("070200_300", 0), ("070700_300", 1)] {
            let segment = root
                .path()
                .join("chronicle/20261004/import.strava")
                .join(key);
            fs::create_dir_all(&segment).unwrap();
            let mut tile = workout.clone();
            tile["tile"]["index"] = serde_json::json!(index);
            fs::write(segment.join("workout.json"), format!("{tile}\n")).unwrap();
        }
        let segments = iter_segments(root.path(), PathOrDay::Day("20261004")).unwrap();
        let pages = segments
            .iter()
            .map(|segment| {
                read_segment_transcript_page(segment, None, None)
                    .unwrap()
                    .entries
            })
            .collect::<Vec<_>>();
        assert_eq!(
            pages[0],
            vec![super::SegmentTranscriptEntry {
                text: "## Strava workout: Morning Run\nRun\n10.2 km".to_owned(),
                channel: "Imported Text",
            }]
        );
        assert!(pages[1].is_empty());
    }

    #[test]
    fn item_limited_page_resumes_at_the_first_unreturned_line() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let segment = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(&segment).unwrap();
        let lines = (1..=MAX_TRANSCRIPT_PAGE_ITEMS + 5)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(segment.join("meeting_transcript.md"), lines).unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let first = read_segment_transcript_page(&segment, None, None).unwrap();
        assert_eq!(first.entries.len(), MAX_TRANSCRIPT_PAGE_ITEMS);
        assert_eq!(first.entries[0].text, "line 1");
        assert_eq!(
            first.entries[MAX_TRANSCRIPT_PAGE_ITEMS - 1].text,
            "line 100"
        );

        let second = read_segment_transcript_page(&segment, first.next.as_ref(), None).unwrap();
        assert_eq!(
            second
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["line 101", "line 102", "line 103", "line 104", "line 105"]
        );
        assert!(second.next.is_none());
    }

    fn speakers() -> super::AgentSpeakers {
        super::AgentSpeakers {
            voices: crate::VoiceNames {
                principal: "person-b".into(),
                owner_voice_confirmed: true,
                people: [("person-a".to_owned(), "Ada".to_owned())].into(),
            },
            owner: "Bea (owner)".into(),
        }
    }

    fn named_segment(root: &std::path::Path) -> std::path::PathBuf {
        let segment = root.join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        for (id, entity) in [
            (
                "person-a",
                serde_json::json!({"name":"Ada", "type":"Person"}),
            ),
            (
                "person-b",
                serde_json::json!({"name":"Bea", "type":"Person", "is_principal":true}),
            ),
            (
                "tool",
                serde_json::json!({"name":"Secret tool", "type":"Tool"}),
            ),
            (
                "blocked",
                serde_json::json!({"name":"Blocked name", "type":"Person", "blocked":true}),
            ),
            ("nameless", serde_json::json!({"type":"Person"})),
        ] {
            let dir = root.join("entities").join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("entity.json"), entity.to_string()).unwrap();
        }
        segment
    }

    #[test]
    fn local_names_follow_statement_ordinals_across_pages() {
        let root = tempfile::tempdir().unwrap();
        let dir = named_segment(root.path());
        let mut text = String::from("{\"raw\":\"audio.flac\"}\n\ninvalid\n");
        for id in 1..=102 {
            // Explicit IDs deliberately disagree with the owner-view ordinals.
            text.push_str(
                &serde_json::json!({"start":"00:00:01", "sentence_id":9000+id,
                "source":"mic", "speaker":1, "text":format!("sentence {id}"),
                "ignored": "x".repeat(1024)})
                .to_string(),
            );
            text.push('\n');
        }
        fs::write(dir.join("audio.jsonl"), text).unwrap();
        fs::write(
            dir.join("talents/speaker_labels.json"),
            serde_json::json!({"labels":[
                {"sentence_id":1,"speaker":"person-b","method":"owner_centroid"},
                {"sentence_id":100,"speaker":"person-a","method":"acoustic"},
                {"sentence_id":101,"speaker":"person-b","method":"owner_centroid","owner_margin_declined":true},
                {"sentence_id":102,"speaker":"person-a","method":"contextual"}
            ]})
            .to_string(),
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let first = read_segment_transcript_page(&segment, None, Some(&speakers())).unwrap();
        assert_eq!(
            first.entries[0].text,
            "[00:00:01] (mic) Bea (owner): sentence 1"
        );
        assert_eq!(first.entries[99].text, "[00:00:01] (mic) Ada: sentence 100");
        let second =
            read_segment_transcript_page(&segment, first.next.as_ref(), Some(&speakers())).unwrap();
        assert_eq!(
            second
                .entries
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            // A declined owner match and a name inferred from what was said name no one.
            [
                "[00:00:01] (mic) Speaker 1: sentence 101",
                "[00:00:01] (mic) Speaker 1: sentence 102"
            ]
        );
        assert!(second.next.is_none());

        // The rendered identity is part of the cursor version.
        fs::write(
            dir.join("talents/speaker_labels.json"),
            serde_json::json!({"labels":[
                {"sentence_id":1,"speaker":"person-b","method":"owner_centroid"},
                {"sentence_id":100,"speaker":"person-a","method":"contextual"}
            ]})
            .to_string(),
        )
        .unwrap();
        assert!(matches!(
            read_segment_transcript_page(&segment, first.next.as_ref(), Some(&speakers())),
            Err(super::SegmentTranscriptReadError::Changed)
        ));
    }

    #[test]
    fn label_admission_and_source_selection_preserve_unnamed_text() {
        let root = tempfile::tempdir().unwrap();
        let dir = named_segment(root.path());
        let text = (1..=6)
            .map(|id| {
                format!("{{\"start\":\"00:00:01\",\"speaker\":1,\"text\":\"sentence {id}\"}}\n")
            })
            .collect::<String>();
        fs::write(dir.join("audio.jsonl"), &text).unwrap();
        let rows = ["person-a", "tool", "blocked", "missing", "nameless"]
            .iter()
            .enumerate()
            .map(|(i, id)| serde_json::json!({"sentence_id":i+1,"speaker":id,"method":"acoustic"}))
            .collect::<Vec<_>>();
        let labels = serde_json::json!({"labels":rows}).to_string();
        fs::write(dir.join("talents/speaker_labels.json"), &labels).unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let read = || read_segment_transcript_page(&segment, None, Some(&speakers())).unwrap();
        let named = read();
        assert_eq!(named.entries[0].text, "[00:00:01] Ada: sentence 1");
        for id in 2..=6 {
            assert_eq!(
                named.entries[id - 1].text,
                format!("[00:00:01] Speaker 1: sentence {id}")
            );
        }
        // Two sources and no embedding source are ambiguous, so neither is named.
        fs::write(dir.join("mic_audio.jsonl"), &text).unwrap();
        assert!(
            read()
                .entries
                .iter()
                .all(|row| row.text.contains("Speaker 1:"))
        );
        // The owner's source rule chooses the npz stem; no names bleed into audio.
        fs::write(dir.join("mic_audio.npz"), "").unwrap();
        let selected = read();
        assert_eq!(selected.entries[0].text, "[00:00:01] Speaker 1: sentence 1");
        assert_eq!(selected.entries[6].text, "[00:00:01] Ada: sentence 1");
        for bytes in ["{".to_owned(), "x".repeat(MAX_TRANSCRIPT_PAGE_BYTES + 1)] {
            fs::write(dir.join("talents/speaker_labels.json"), bytes).unwrap();
            assert!(
                read()
                    .entries
                    .iter()
                    .all(|row| row.text.contains("Speaker 1:"))
            );
        }
        fs::remove_file(dir.join("talents/speaker_labels.json")).unwrap();
        assert!(
            read()
                .entries
                .iter()
                .all(|row| row.text.contains("Speaker 1:"))
        );
        // Without agent speakers, every line keeps its stored label.
        fs::write(dir.join("talents/speaker_labels.json"), labels).unwrap();
        assert!(
            read_segment_transcript_page(&segment, None, None)
                .unwrap()
                .entries
                .iter()
                .all(|row| row.text.contains("Speaker 1:"))
        );
    }

    #[test]
    fn audio_named_chat_keeps_its_chat_speakers_even_with_local_labels() {
        let root = tempfile::tempdir().unwrap();
        let old_dir = named_segment(root.path());
        let dir = root
            .path()
            .join("chronicle/20260914/import.chatgpt/090000_300");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::rename(old_dir, &dir).unwrap();
        fs::write(
            dir.join("imported_audio.jsonl"),
            "{\"start\":\"00:00:00\",\"speaker\":\"User\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        fs::write(
            dir.join("talents/speaker_labels.json"),
            "{\"labels\":[{\"sentence_id\":1,\"speaker\":\"person-a\",\"method\":\"acoustic\"}]}",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let page = read_segment_transcript_page(&segment, None, Some(&speakers())).unwrap();
        assert_eq!(page.entries[0].text, "[00:00:00] **User:** hello");
    }

    #[test]
    fn audio_transcript_page_appends_warning_to_flagged_line_only() {
        let root = tempfile::tempdir().unwrap();
        let segment_dir = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(&segment_dir).unwrap();

        let flagged_line = serde_json::json!({
            "start": "00:00:10",
            "text": "Is the gate open? It is open.",
            "speaker": 1,
            "sentence_id": 1,
            "turn_boundary_suspect": true,
            "speaker_turn_evidence": {
                "version": "window-local-turns-v1",
                "audio_duration_s": 20.0,
                "ranges": [{"start_s": 11.0, "end_s": 12.0}],
            },
        });
        let neighbor_line = serde_json::json!({
            "start": "00:00:15",
            "text": "Neighbor speech without warning.",
            "speaker": 2,
            "sentence_id": 2,
        });

        let audio_content = format!(
            "{}\n{}\n",
            serde_json::to_string(&flagged_line).unwrap(),
            serde_json::to_string(&neighbor_line).unwrap()
        );
        fs::write(segment_dir.join("audio.jsonl"), audio_content).unwrap();

        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let page = read_segment_transcript_page(&segment, None, None).unwrap();
        assert_eq!(page.entries.len(), 2);

        let flagged_entry = &page.entries[0].text;
        assert!(flagged_entry.contains("Is the gate open? It is open."));
        assert!(flagged_entry.contains("Speaker 1:"));
        assert!(flagged_entry.contains(super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE));
        assert!(flagged_entry.ends_with(&format!(
            "[{}]",
            super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE
        )));

        let neighbor_entry = &page.entries[1].text;
        let neighbor_chunks = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            "chronicle/20260914/default/090000_300/audio.jsonl",
            &serde_json::to_string(&neighbor_line).unwrap(),
        );
        assert_eq!(neighbor_entry, &neighbor_chunks.chunks[0].content);
        assert!(!neighbor_entry.contains(super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE));
    }

    #[test]
    fn produce_raw_percept_chunks_ignores_turn_boundary_suspect_and_speaker_turn_evidence() {
        let flagged = serde_json::json!({
            "start": "00:00:10",
            "text": "Is the gate open? It is open.",
            "speaker": 1,
            "sentence_id": 1,
            "turn_boundary_suspect": true,
            "speaker_turn_evidence": {
                "version": "window-local-turns-v1",
                "audio_duration_s": 20.0,
                "ranges": [{"start_s": 11.0, "end_s": 12.0}],
            },
        });
        let unflagged = serde_json::json!({
            "start": "00:00:10",
            "text": "Is the gate open? It is open.",
            "speaker": 1,
            "sentence_id": 1,
        });

        let rel = "chronicle/20260914/default/090000_300/audio.jsonl";
        let chunks_flagged = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            rel,
            &serde_json::to_string(&flagged).unwrap(),
        );
        let chunks_unflagged = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            rel,
            &serde_json::to_string(&unflagged).unwrap(),
        );

        assert_eq!(chunks_flagged.chunks.len(), chunks_unflagged.chunks.len());
        assert_eq!(
            chunks_flagged.chunks[0].content,
            chunks_unflagged.chunks[0].content
        );
        assert_eq!(
            chunks_flagged.chunks[0].occurrence_time_ms,
            chunks_unflagged.chunks[0].occurrence_time_ms
        );
    }
}
