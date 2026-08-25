// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use serde_json::{Map, Value};
use solstone_core_journal_io::{
    DirEntry, DirEntryKind, PathOrDay, SegmentIdentityError, day_dirs, iter_segments,
    list_dir_entries,
};
use solstone_core_processing_record::analysis_row_key;
use solstone_core_processing_record::media::expected_handler;
use solstone_core_processing_record::vocab::{
    HANDLER_DESCRIBE, HANDLER_TRANSCRIBE, MAX_FIRST_ROW_BYTES, REASON_NO_DECODABLE_AUDIO,
    REASON_NO_DECODABLE_FRAMES,
};

use crate::record::processing_record;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    StampEmpty,
    SkipHasRecord,
    SkipChunkBearing,
    SkipMarker,
    SkipIneligible,
    SkipUnreadable,
    SkipOversize,
    WriteFailed,
}

impl Outcome {
    const ALL: [Self; 8] = [
        Self::StampEmpty,
        Self::SkipHasRecord,
        Self::SkipChunkBearing,
        Self::SkipMarker,
        Self::SkipIneligible,
        Self::SkipUnreadable,
        Self::SkipOversize,
        Self::WriteFailed,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::StampEmpty => "stamp_empty",
            Self::SkipHasRecord => "skip_has_record",
            Self::SkipChunkBearing => "skip_chunk_bearing",
            Self::SkipMarker => "skip_marker",
            Self::SkipIneligible => "skip_ineligible",
            Self::SkipUnreadable => "skip_unreadable",
            Self::SkipOversize => "skip_oversize",
            Self::WriteFailed => "write_failed",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Counts {
    stamp_empty: u64,
    skip_has_record: u64,
    skip_chunk_bearing: u64,
    skip_marker: u64,
    skip_ineligible: u64,
    skip_unreadable: u64,
    skip_oversize: u64,
    pub(crate) write_failed: u64,
}

impl Counts {
    fn add(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::StampEmpty => self.stamp_empty += 1,
            Outcome::SkipHasRecord => self.skip_has_record += 1,
            Outcome::SkipChunkBearing => self.skip_chunk_bearing += 1,
            Outcome::SkipMarker => self.skip_marker += 1,
            Outcome::SkipIneligible => self.skip_ineligible += 1,
            Outcome::SkipUnreadable => self.skip_unreadable += 1,
            Outcome::SkipOversize => self.skip_oversize += 1,
            Outcome::WriteFailed => self.write_failed += 1,
        }
    }

    pub(crate) fn move_stamp_to_write_failed(&mut self) {
        self.stamp_empty = self.stamp_empty.saturating_sub(1);
        self.write_failed += 1;
    }

    fn value(&self, outcome: Outcome) -> u64 {
        match outcome {
            Outcome::StampEmpty => self.stamp_empty,
            Outcome::SkipHasRecord => self.skip_has_record,
            Outcome::SkipChunkBearing => self.skip_chunk_bearing,
            Outcome::SkipMarker => self.skip_marker,
            Outcome::SkipIneligible => self.skip_ineligible,
            Outcome::SkipUnreadable => self.skip_unreadable,
            Outcome::SkipOversize => self.skip_oversize,
            Outcome::WriteFailed => self.write_failed,
        }
    }

    pub(crate) fn write_to(&self, stdout: &mut dyn Write) {
        let mut total = 0;
        for outcome in Outcome::ALL {
            let value = self.value(outcome);
            total += value;
            let _ = writeln!(stdout, "{}: {value}", outcome.name());
        }
        let _ = writeln!(stdout, "total: {total}");
    }
}

#[derive(Debug, Clone)]
struct Modality {
    name: &'static str,
    handler: &'static str,
    reason: &'static str,
}

const SCREEN: Modality = Modality {
    name: "screen",
    handler: HANDLER_DESCRIBE,
    reason: REASON_NO_DECODABLE_FRAMES,
};
const AUDIO: Modality = Modality {
    name: "audio",
    handler: HANDLER_TRANSCRIBE,
    reason: REASON_NO_DECODABLE_AUDIO,
};

#[derive(Debug, Clone)]
pub(crate) struct Eligible {
    pub(crate) day: String,
    pub(crate) path: PathBuf,
    pub(crate) original: Vec<u8>,
    pub(crate) replacement: Vec<u8>,
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) counts: Counts,
    pub(crate) eligible: Vec<Eligible>,
}

pub(crate) fn is_day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn refuse_ambiguous_named_default(
    journal: &Path,
    requested_day: Option<&str>,
) -> Result<(), Vec<SegmentIdentityError>> {
    let days = match day_dirs(journal) {
        Ok(days) => days,
        Err(_) => return Ok(()),
    };
    let selected = select_days(days, requested_day, &mut std::io::sink());
    let mut found = Vec::new();
    for (_day, day_path) in selected {
        let Ok(segments) = iter_segments(journal, PathOrDay::Directory(&day_path)) else {
            continue;
        };
        for segment in segments {
            if let Err(error @ SegmentIdentityError::AmbiguousNamedDefault { .. }) =
                segment.record_identity()
            {
                found.push(error);
            }
        }
    }
    if found.is_empty() { Ok(()) } else { Err(found) }
}

pub(crate) fn plan(
    journal: &Path,
    requested_day: Option<&str>,
    instant: DateTime<Utc>,
    stderr: &mut dyn Write,
) -> Result<Report, SegmentIdentityError> {
    let days = match day_dirs(journal) {
        Ok(days) => days,
        Err(error) => {
            let _ = writeln!(stderr, "Could not list chronicle days: {error}");
            HashMap::new()
        }
    };
    let selected = select_days(days, requested_day, stderr);
    let current_day = instant.with_timezone(&Local).format("%Y%m%d").to_string();
    let mut report = Report::default();

    for (day, day_path) in selected {
        report_non_directory_day_entries(&day_path, stderr);
        let segments = match iter_segments(journal, PathOrDay::Directory(&day_path)) {
            Ok(segments) => segments,
            Err(error) => {
                let _ = writeln!(stderr, "Could not list segments for day {day}: {error}");
                continue;
            }
        };
        for segment in segments {
            let spelling = match segment.record_identity() {
                Ok(identity) => identity.stream,
                Err(SegmentIdentityError::NotUtf8 { .. }) => {
                    let _ = writeln!(
                        stderr,
                        "segment stream is not UTF-8 representable: {}",
                        segment.path().display()
                    );
                    report.counts.add(Outcome::SkipUnreadable);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let entries = match list_dir_entries(segment.path()) {
                Ok(entries) => entries,
                Err(error) => {
                    let _ = writeln!(
                        stderr,
                        "Could not list segment directory {}: {error}",
                        segment.path().display()
                    );
                    continue;
                }
            };
            for entry in entries.iter().filter(|entry| is_jsonl_name(&entry.name)) {
                match classify(
                    &day,
                    &current_day,
                    segment.path(),
                    spelling,
                    entry,
                    &entries,
                    instant,
                ) {
                    Ok(item) => {
                        report.counts.add(Outcome::StampEmpty);
                        report.eligible.push(item);
                    }
                    Err(outcome) => report.counts.add(outcome),
                }
            }
        }
    }
    Ok(report)
}

fn report_non_directory_day_entries(day_path: &Path, stderr: &mut dyn Write) {
    let Ok(entries) = list_dir_entries(day_path) else {
        return;
    };
    for entry in entries
        .iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
    {
        let _ = writeln!(
            stderr,
            "Could not list stream directory {}: not a directory",
            entry.path.display()
        );
    }
}

fn select_days(
    days: HashMap<String, PathBuf>,
    requested_day: Option<&str>,
    stderr: &mut dyn Write,
) -> Vec<(String, PathBuf)> {
    if let Some(day) = requested_day {
        return match days.get(day) {
            Some(path) => vec![(day.to_owned(), path.clone())],
            None => {
                let _ = writeln!(stderr, "Day {day} was not found in the journal");
                Vec::new()
            }
        };
    }
    let mut days: Vec<_> = days.into_iter().collect();
    days.sort_by(|left, right| left.0.cmp(&right.0));
    days
}

pub(crate) fn classify(
    day: &str,
    current_day: &str,
    segment_path: &Path,
    segment_stream: &str,
    candidate: &DirEntry,
    entries: &[DirEntry],
    instant: DateTime<Utc>,
) -> Result<Eligible, Outcome> {
    let name = candidate.name.to_string_lossy();
    let Some(modality) = modality_for(&name) else {
        return Err(Outcome::SkipIneligible);
    };
    let stream = stream_for(segment_path, segment_stream);
    if stream.is_empty() || stream.starts_with("import.") || day == current_day {
        return Err(Outcome::SkipIneligible);
    }
    if candidate.kind != DirEntryKind::File {
        return Err(Outcome::SkipIneligible);
    }
    let sibling = matching_sibling(&name, entries, modality.handler)?;
    let input_size = regular_file_size(&sibling.path).ok_or(Outcome::SkipUnreadable)?;
    let original = fs::read(&candidate.path).map_err(|_| Outcome::SkipUnreadable)?;
    let (mut header, header_start, header_end, has_chunk) =
        parse_jsonl(&original, modality.handler)?;
    if header
        .get("_solstone_processing")
        .is_some_and(Value::is_object)
    {
        return Err(Outcome::SkipHasRecord);
    }
    if has_chunk {
        return Err(Outcome::SkipChunkBearing);
    }
    if (segment_path.join(format!(".analyzing_{}", modality.name))).exists()
        || (segment_path.join(format!(".analyze_failed_{}", modality.name))).exists()
    {
        return Err(Outcome::SkipMarker);
    }

    header.insert(
        "_solstone_processing".to_owned(),
        processing_record(modality.handler, modality.reason, input_size, instant),
    );
    let header_bytes =
        serde_json::to_vec(&Value::Object(header)).map_err(|_| Outcome::SkipUnreadable)?;
    if header_bytes.len().saturating_add(1) > MAX_FIRST_ROW_BYTES {
        return Err(Outcome::SkipOversize);
    }
    let mut replacement = Vec::with_capacity(original.len().saturating_add(header_bytes.len()));
    replacement.extend_from_slice(&original[..header_start]);
    replacement.extend_from_slice(&header_bytes);
    replacement.push(b'\n');
    replacement.extend_from_slice(&original[header_end..]);
    Ok(Eligible {
        day: day.to_owned(),
        path: candidate.path.clone(),
        original,
        replacement,
    })
}

fn modality_for(name: &str) -> Option<&'static Modality> {
    if name == "screen.jsonl" || name.ends_with("_screen.jsonl") {
        Some(&SCREEN)
    } else if name == "audio.jsonl" || name.ends_with("_audio.jsonl") {
        Some(&AUDIO)
    } else {
        None
    }
}

fn stream_for(segment_path: &Path, fallback: &str) -> String {
    let marker = segment_path.join("stream.json");
    if let Ok(bytes) = fs::read(marker)
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && let Some(stream) = value.get("stream").and_then(Value::as_str)
    {
        return stream.to_owned();
    }
    // Direct layout spells `_default`; a named stream uses its UTF-8 directory.
    fallback.to_owned()
}

fn matching_sibling<'a>(
    candidate_name: &str,
    entries: &'a [DirEntry],
    handler: &str,
) -> Result<&'a DirEntry, Outcome> {
    let Some(stem) = candidate_name.strip_suffix(".jsonl") else {
        return Err(Outcome::SkipIneligible);
    };
    let matches: Vec<_> = entries
        .iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .filter(|entry| {
            let name = entry.name.to_string_lossy();
            let Some((entry_stem, extension)) = name.rsplit_once('.') else {
                return false;
            };
            entry_stem == stem && expected_handler(&extension.to_ascii_lowercase()) == Some(handler)
        })
        .collect();
    match matches.as_slice() {
        [only] => Ok(*only),
        _ => Err(Outcome::SkipIneligible),
    }
}

fn regular_file_size(path: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata.file_type().is_file().then_some(metadata.len())
}

fn is_jsonl_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| name.ends_with(".jsonl"))
}

fn parse_jsonl(
    bytes: &[u8],
    handler: &str,
) -> Result<(Map<String, Value>, usize, usize, bool), Outcome> {
    let key = analysis_row_key(handler).expect("closed processing-record handler");
    let mut first = None;
    let mut has_chunk = false;
    let mut start = 0;
    while start < bytes.len() {
        let end = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| start + offset + 1);
        let content_end = if end > start && bytes[end - 1] == b'\n' {
            end - 1
        } else {
            end
        };
        let line = &bytes[start..content_end];
        if !line.iter().all(u8::is_ascii_whitespace) {
            // Unlike Python's `_has_chunk_row`, which silently swallows `json.JSONDecodeError`
            // per line while scanning for the chunk key (so a valid analysis row plus one torn
            // line reaches `STAMP_EMPTY`), failed non-blank JSON parsing is unreadable here
            // before that probe. This verb repairs torn transcripts, so authorizing release of
            // the raw media torn out of one would be wrong.
            let value: Value = serde_json::from_slice(line).map_err(|_| Outcome::SkipUnreadable)?;
            if first.is_none() {
                let Some(header) = value.as_object() else {
                    return Err(Outcome::SkipUnreadable);
                };
                first = Some((header.clone(), start, end));
            }
            if value
                .as_object()
                .is_some_and(|object| object.contains_key(key))
            {
                has_chunk = true;
            }
        }
        start = end;
    }
    first
        .map(|(header, start, end)| (header, start, end, has_chunk))
        .ok_or(Outcome::SkipUnreadable)
}
