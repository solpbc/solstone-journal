// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveDate, NaiveDateTime};
use serde_json::{Map, Value};
use solstone_core_format::agent_memory::{Coordinate, Origin, SourceKey};
use solstone_core_format::content::{
    Family, RawPerceptFamily, iter_talent_text_projections, produce_chunks,
    produce_raw_percept_chunks, produce_screen_talent_raw_screen_chunks,
};
use solstone_core_format::segment::segment_parse;
use solstone_core_journal_io::paths::{PathOrDay, StreamLocation, iter_segments};
use solstone_core_memory_original::{OriginalRead, read_original};

mod segment_page;

pub use segment_page::{
    MAX_TRANSCRIPT_PAGE_BYTES, MAX_TRANSCRIPT_PAGE_ITEMS, SegmentTranscriptCursor,
    SegmentTranscriptEntry, SegmentTranscriptPage, SegmentTranscriptReadError,
    SegmentTranscriptVersion, read_segment_transcript_page,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TalentSource {
    Disabled,
    All,
    Only(BTreeSet<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    pub transcripts: bool,
    pub percepts: bool,
    pub talents: TalentSource,
    /// Speakers named by voice in rendered transcripts. `None` keeps every
    /// speaker as the anonymous diarization number.
    pub voices: Option<VoiceNames>,
}

/// The speaker heard on a transcript line when the journal owner's voice is recognized.
pub const OWNER_VOICE: &str = "You";

/// Who a voice-identified transcript line is attributed to. Only acoustic
/// evidence or the owner's own assignment names a speaker; a name the
/// journal inferred from what was said leaves the line anonymous.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceNames {
    /// The principal entity id.
    pub principal: String,
    /// The owner's voiceprint is confirmed, so an `owner_centroid` match is trusted.
    pub owner_voice_confirmed: bool,
    /// Entity id to display name for every other admitted person.
    pub people: BTreeMap<String, String>,
}

impl VoiceNames {
    /// The display name for one `speaker_labels.json` row, if its evidence names a speaker.
    pub fn name_for(&self, row: &Map<String, Value>) -> Option<String> {
        let speaker = row.get("speaker").and_then(Value::as_str)?;
        let method = row
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let by_owner = method.starts_with("user_");
        if speaker == self.principal {
            let heard = method == "owner_centroid"
                && self.owner_voice_confirmed
                && !row
                    .get("owner_margin_declined")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            (by_owner || heard).then(|| OWNER_VOICE.to_owned())
        } else if by_owner || matches!(method, "acoustic" | "acoustic_cluster") {
            self.people.get(speaker).cloned()
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCounts {
    pub transcripts: usize,
    pub percepts: usize,
    pub talents: usize,
}

/// Trusted source metadata consumed while building one cluster.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemoryContext {
    pub sources: Vec<solstone_core_format::content::ConsumedOriginal>,
    pub incomplete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenCut {
    pub byte_offset: usize,
    pub observation_byte_offset: usize,
    pub reset_carry: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenTranscript {
    pub text: String,
    pub cuts: Vec<ScreenCut>,
}

impl ScreenTranscript {
    pub fn plain(text: String) -> Self {
        Self {
            text,
            cuts: Vec::new(),
        }
    }
}

impl std::ops::Deref for ScreenTranscript {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl SourceCounts {
    pub fn total(&self) -> usize {
        self.transcripts + self.percepts + self.talents
    }

    fn from_entries(entries: &[Entry]) -> Self {
        let mut counts = Self::default();
        for entry in entries {
            match entry.prefix {
                "transcript" | "memory_original" => counts.transcripts += 1,
                "percept" | "browser" => counts.percepts += 1,
                "agent_output" => counts.talents += 1,
                _ => {}
            }
        }
        counts
    }
}

impl From<SourceCounts> for Value {
    fn from(counts: SourceCounts) -> Self {
        Value::Object(Map::from_iter([
            ("transcripts".to_owned(), Value::from(counts.transcripts)),
            ("percepts".to_owned(), Value::from(counts.percepts)),
            ("talents".to_owned(), Value::from(counts.talents)),
        ]))
    }
}

pub const MIN_INPUT_CHARS: usize = 50;

pub fn is_no_input(text: &str, counts: &SourceCounts) -> bool {
    counts.total() == 0 || text.trim().len() < MIN_INPUT_CHARS
}

struct Entry {
    timestamp: NaiveDateTime,
    segment_key: String,
    segment_start: NaiveDateTime,
    segment_end: NaiveDateTime,
    prefix: &'static str,
    content: String,
    stream: Option<String>,
    output_name: Option<String>,
    screen_cuts: Vec<ScreenCut>,
    memory_origin: Option<Origin>,
}

#[derive(Debug, PartialEq, Eq)]
struct RawContent {
    text: String,
    screen_cuts: Vec<ScreenCut>,
}

impl std::ops::Deref for RawContent {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.text
    }
}

impl fmt::Display for RawContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.text.fmt(formatter)
    }
}

struct Segment {
    path: PathBuf,
    stream: StreamLocation,
    key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PerceptProjection {
    Generic,
    ScreenTalent,
}

#[derive(Debug)]
pub struct RangeError(String);

impl fmt::Display for RangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for RangeError {}

pub fn cluster(root: &Path, day: &str, sources: &Sources) -> (String, SourceCounts) {
    let (transcript, counts) =
        cluster_with_projection(root, day, sources, PerceptProjection::Generic);
    (transcript.text, counts)
}

pub fn cluster_with_memory(
    root: &Path,
    day: &str,
    sources: &Sources,
) -> (ScreenTranscript, SourceCounts, MemoryContext) {
    cluster_with_projection_and_memory(root, day, sources, PerceptProjection::Generic)
}

pub fn cluster_for_screen_talent(
    root: &Path,
    day: &str,
    sources: &Sources,
) -> (ScreenTranscript, SourceCounts) {
    cluster_with_projection(root, day, sources, PerceptProjection::ScreenTalent)
}

pub fn cluster_for_screen_talent_with_memory(
    root: &Path,
    day: &str,
    sources: &Sources,
) -> (ScreenTranscript, SourceCounts, MemoryContext) {
    cluster_with_projection_and_memory(root, day, sources, PerceptProjection::ScreenTalent)
}

fn cluster_with_projection(
    root: &Path,
    day: &str,
    sources: &Sources,
    projection: PerceptProjection,
) -> (ScreenTranscript, SourceCounts) {
    let (transcript, counts, memory) =
        cluster_with_projection_and_memory(root, day, sources, projection);
    (disclose_memory_incomplete(transcript, &memory), counts)
}

fn cluster_with_projection_and_memory(
    root: &Path,
    day: &str,
    sources: &Sources,
    projection: PerceptProjection,
) -> (ScreenTranscript, SourceCounts, MemoryContext) {
    let mut memory = MemoryContext::default();
    let day_dir = day_dir(root, day);
    // Python's day_path at solstone/think/utils.py:289 creates this directory before
    // cluster.py:794-797 checks it. Native reads stay non-creating: native think creates
    // day directories before dispatch, while an owner's read must not create chronicle state.
    if !day_dir.is_dir() {
        return (
            ScreenTranscript::plain(format!("Day folder not found: {}", day_dir.display())),
            SourceCounts::default(),
            memory,
        );
    }
    let entries = load_day(root, day, sources, projection, &mut memory);
    let counts = SourceCounts::from_entries(&entries);
    if entries.is_empty() {
        (
            ScreenTranscript::plain(format!(
                "No transcript, screen, or browser files found for date {day} in {}.",
                day_dir.display()
            )),
            counts,
            memory,
        )
    } else {
        (groups_to_markdown(entries), counts, memory)
    }
}

pub fn cluster_period(
    root: &Path,
    day: &str,
    key: &str,
    sources: &Sources,
    stream: Option<&str>,
) -> (String, SourceCounts) {
    let (transcript, counts) =
        cluster_period_with_projection(root, day, key, sources, stream, PerceptProjection::Generic);
    (transcript.text, counts)
}

pub fn cluster_period_with_memory(
    root: &Path,
    day: &str,
    key: &str,
    sources: &Sources,
    stream: Option<&str>,
) -> (ScreenTranscript, SourceCounts, MemoryContext) {
    cluster_period_with_projection_and_memory(
        root,
        day,
        key,
        sources,
        stream,
        PerceptProjection::Generic,
    )
}

pub fn cluster_period_for_screen_talent(
    root: &Path,
    day: &str,
    key: &str,
    sources: &Sources,
    stream: Option<&str>,
) -> (ScreenTranscript, SourceCounts) {
    cluster_period_with_projection(
        root,
        day,
        key,
        sources,
        stream,
        PerceptProjection::ScreenTalent,
    )
}

pub fn cluster_period_for_screen_talent_with_memory(
    root: &Path,
    day: &str,
    key: &str,
    sources: &Sources,
    stream: Option<&str>,
) -> (ScreenTranscript, SourceCounts, MemoryContext) {
    cluster_period_with_projection_and_memory(
        root,
        day,
        key,
        sources,
        stream,
        PerceptProjection::ScreenTalent,
    )
}

fn cluster_period_with_projection(
    root: &Path,
    day: &str,
    key: &str,
    sources: &Sources,
    stream: Option<&str>,
    projection: PerceptProjection,
) -> (ScreenTranscript, SourceCounts) {
    let (transcript, counts, memory) =
        cluster_period_with_projection_and_memory(root, day, key, sources, stream, projection);
    (disclose_memory_incomplete(transcript, &memory), counts)
}

fn cluster_period_with_projection_and_memory(
    root: &Path,
    day: &str,
    key: &str,
    sources: &Sources,
    stream: Option<&str>,
    projection: PerceptProjection,
) -> (ScreenTranscript, SourceCounts, MemoryContext) {
    let mut memory = MemoryContext::default();
    let Some(segment) = find_segment(root, day, key, stream) else {
        return (
            ScreenTranscript::plain(format!("Segment folder not found: {day}/{key}")),
            SourceCounts::default(),
            memory,
        );
    };
    let entries = process_segment(root, &segment, day, sources, projection, &mut memory);
    let counts = SourceCounts::from_entries(&entries);
    if entries.is_empty() {
        (
            ScreenTranscript::plain(format!(
                "No transcript, screen, or browser files found for segment {key}"
            )),
            counts,
            memory,
        )
    } else {
        (groups_to_markdown(entries), counts, memory)
    }
}

pub fn cluster_span(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: &Sources,
    stream: Option<&str>,
) -> Result<(String, SourceCounts), String> {
    cluster_span_with_projection(root, day, span, sources, stream, PerceptProjection::Generic)
        .map(|(transcript, counts)| (transcript.text, counts))
}

pub fn cluster_span_with_memory(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: &Sources,
    stream: Option<&str>,
) -> Result<(ScreenTranscript, SourceCounts, MemoryContext), String> {
    cluster_span_with_projection_and_memory(
        root,
        day,
        span,
        sources,
        stream,
        PerceptProjection::Generic,
    )
}

pub fn cluster_span_for_screen_talent(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: &Sources,
    stream: Option<&str>,
) -> Result<(ScreenTranscript, SourceCounts), String> {
    cluster_span_with_projection(
        root,
        day,
        span,
        sources,
        stream,
        PerceptProjection::ScreenTalent,
    )
}

pub fn cluster_span_for_screen_talent_with_memory(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: &Sources,
    stream: Option<&str>,
) -> Result<(ScreenTranscript, SourceCounts, MemoryContext), String> {
    cluster_span_with_projection_and_memory(
        root,
        day,
        span,
        sources,
        stream,
        PerceptProjection::ScreenTalent,
    )
}

fn cluster_span_with_projection(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: &Sources,
    stream: Option<&str>,
    projection: PerceptProjection,
) -> Result<(ScreenTranscript, SourceCounts), String> {
    cluster_span_with_projection_and_memory(root, day, span, sources, stream, projection).map(
        |(transcript, counts, memory)| (disclose_memory_incomplete(transcript, &memory), counts),
    )
}

fn cluster_span_with_projection_and_memory(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: &Sources,
    stream: Option<&str>,
    projection: PerceptProjection,
) -> Result<(ScreenTranscript, SourceCounts, MemoryContext), String> {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for key in span {
        match find_segment(root, day, key, stream) {
            Some(segment) => found.push(segment),
            None => missing.push(*key),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "Segment directories not found: {}",
            missing.join(", ")
        ));
    }
    let mut memory = MemoryContext::default();
    let mut entries = found
        .iter()
        .flat_map(|segment| process_segment(root, segment, day, sources, projection, &mut memory))
        .collect::<Vec<_>>();
    let counts = SourceCounts::from_entries(&entries);
    if entries.is_empty() {
        return Ok((
            ScreenTranscript::plain(format!(
                "No transcript, screen, or browser files found in span: {}",
                span.join(", ")
            )),
            counts,
            memory,
        ));
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok((groups_to_markdown(entries), counts, memory))
}

pub fn cluster_range(
    root: &Path,
    day: &str,
    start: &str,
    end: &str,
    sources: &Sources,
) -> Result<String, RangeError> {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(range_error)?;
    let start =
        NaiveDateTime::parse_from_str(&format!("{}{start}", date.format("%Y%m%d")), "%Y%m%d%H%M%S")
            .map_err(range_error)?;
    let end =
        NaiveDateTime::parse_from_str(&format!("{}{end}", date.format("%Y%m%d")), "%Y%m%d%H%M%S")
            .map_err(range_error)?;
    let mut memory = MemoryContext::default();
    let entries = load_day(root, day, sources, PerceptProjection::Generic, &mut memory)
        .into_iter()
        .filter(|entry| entry.segment_start < end && entry.segment_end > start)
        .collect();
    Ok(disclose_memory_incomplete(groups_to_markdown(entries), &memory).text)
}

fn range_error(error: impl fmt::Display) -> RangeError {
    RangeError(error.to_string())
}

fn load_day(
    root: &Path,
    day: &str,
    sources: &Sources,
    projection: PerceptProjection,
    memory: &mut MemoryContext,
) -> Vec<Entry> {
    let mut entries = all_segments(root, day)
        .into_iter()
        .flat_map(|segment| process_segment(root, &segment, day, sources, projection, memory))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    entries
}

fn process_segment(
    root: &Path,
    segment: &Segment,
    day: &str,
    sources: &Sources,
    projection: PerceptProjection,
    memory: &mut MemoryContext,
) -> Vec<Entry> {
    let Some((start, end)) = segment_times(day, &segment.key) else {
        return Vec::new();
    };
    let stream = segment
        .record_stream()
        .map(str::to_owned)
        .or_else(|| stream_marker(&segment.path));
    let mut entries = Vec::new();
    let files = sorted_files(&segment.path);
    if sources.transcripts {
        if let Some(stream_name) = stream
            .as_deref()
            .filter(|name| name.starts_with("agent-memory-"))
        {
            let source_key = stream_name
                .strip_prefix("agent-memory-")
                .and_then(|component| SourceKey::parse(format!("sha256:{component}")).ok());
            if let Some(source_key) = source_key {
                let coordinate = Coordinate {
                    day: day.to_owned(),
                    stream: stream_name.to_owned(),
                    segment: segment.key.clone(),
                };
                match read_original(root, &source_key, &coordinate) {
                    OriginalRead::Ready { bytes, origin, .. } => {
                        let note = String::from_utf8(bytes)
                            .expect("the shared original reader validates UTF-8");
                        remember_memory_source(
                            memory,
                            solstone_core_format::content::ConsumedOriginal {
                                coordinate,
                                origin: origin.clone(),
                            },
                        );
                        let mut memory_entry = entry(
                            start,
                            end,
                            segment,
                            "memory_original",
                            note,
                            stream.clone(),
                            None,
                        );
                        memory_entry.memory_origin = Some(origin);
                        entries.push(memory_entry);
                    }
                    OriginalRead::Corrupt | OriginalRead::Unavailable { .. } => {
                        memory.incomplete = true;
                    }
                    OriginalRead::Absent
                    | OriginalRead::Unready
                    | OriginalRead::Deleted
                    | OriginalRead::Staged => {}
                }
            } else {
                memory.incomplete = true;
            }
        }
        let mut transcript = files
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.ends_with("audio.jsonl") || name.ends_with("_transcript.jsonl")
                    })
            })
            .collect::<Vec<_>>();
        transcript.sort();
        transcript.dedup();
        let voiced = sources
            .voices
            .as_ref()
            .and_then(|voices| segment_voices(&segment.path, voices));
        for path in transcript {
            let speakers = voiced.as_ref().and_then(|(source, names)| {
                (path.file_stem().and_then(|stem| stem.to_str()) == Some(source.as_str()))
                    .then_some(names)
            });
            if let Some(content) = raw_content_with_speakers(
                path,
                segment,
                day,
                RawPerceptFamily::Audio,
                PerceptProjection::Generic,
                speakers,
            ) {
                entries.push(entry(
                    start,
                    end,
                    segment,
                    "transcript",
                    content.text,
                    stream.clone(),
                    None,
                ));
            }
        }
        let mut markdown = files
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == "imported.md" || name.ends_with("_transcript.md"))
            })
            .collect::<Vec<_>>();
        markdown.sort();
        markdown.dedup();
        for path in markdown {
            match fs::read_to_string(path) {
                Ok(content) if !content.trim().is_empty() => {
                    entries.push(entry(
                        start,
                        end,
                        segment,
                        "transcript",
                        content,
                        stream.clone(),
                        None,
                    ));
                }
                Ok(_) => {}
                Err(error) => {
                    log::warn!(
                        "unable to read transcript input {}: {error}",
                        path.display()
                    );
                }
            }
        }
        // A Strava workout piece: only the workout's first piece renders, so a
        // workout reads once, on the day it started.
        for path in files
            .iter()
            .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("workout.json"))
        {
            let rel = format!(
                "{day}/{}/{}/workout.json",
                stream.as_deref().unwrap_or_default(),
                segment.key
            );
            match fs::read_to_string(path) {
                Ok(text) => {
                    let content = solstone_core_format::content::produce_chunks(
                        solstone_core_format::content::Family::Workout,
                        &rel,
                        &text,
                    )
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.content)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                    if !content.is_empty() {
                        entries.push(entry(
                            start,
                            end,
                            segment,
                            "transcript",
                            content,
                            stream.clone(),
                            None,
                        ));
                    }
                }
                Err(error) => {
                    log::warn!("unable to read workout input {}: {error}", path.display());
                }
            }
        }
        for path in &files {
            if first_kind(path).as_deref() == Some("image")
                && let Some(content) = raw_content(
                    path,
                    segment,
                    day,
                    RawPerceptFamily::Audio,
                    PerceptProjection::Generic,
                )
            {
                entries.push(entry(
                    start,
                    end,
                    segment,
                    "transcript",
                    content.text,
                    stream.clone(),
                    None,
                ));
            }
        }
    }
    if sources.percepts {
        for path in files.iter().filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("screen.jsonl"))
        }) {
            if let Some(content) =
                raw_content(path, segment, day, RawPerceptFamily::RawScreen, projection)
                && !content.text.is_empty()
            {
                let mut projected = entry(
                    start,
                    end,
                    segment,
                    "percept",
                    content.text,
                    stream.clone(),
                    None,
                );
                projected.screen_cuts = content.screen_cuts;
                entries.push(projected);
            }
        }
        let mut browser = files
            .iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("browser_") && name.ends_with(".jsonl"))
            })
            .collect::<Vec<_>>();
        browser.sort();
        for path in browser {
            match fs::read_to_string(path) {
                Ok(text) => {
                    let content = produce_chunks(
                        Family::Browser,
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or_default(),
                        &text,
                    )
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.content)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                    if !content.is_empty() {
                        entries.push(entry(
                            start,
                            end,
                            segment,
                            "browser",
                            content,
                            stream.clone(),
                            None,
                        ));
                    }
                }
                Err(error) => {
                    log::warn!("unable to read JSONL input {}: {error}", path.display());
                }
            }
        }
    }
    if !matches!(sources.talents, TalentSource::Disabled) {
        let talents = segment.path.join("talents");
        let stem_filter = |stem: &str| match &sources.talents {
            TalentSource::Disabled => false,
            TalentSource::All => true,
            TalentSource::Only(stems) => stems.contains(stem),
        };
        match iter_talent_text_projections(&talents, "", Some(&stem_filter)) {
            Ok(projections) => {
                for projection in projections {
                    if !projection.text.trim().is_empty() {
                        for source in projection.sources {
                            remember_memory_source(memory, source);
                        }
                        entries.push(Entry {
                            timestamp: start,
                            segment_key: segment.key.clone(),
                            segment_start: start,
                            segment_end: end,
                            prefix: "agent_output",
                            content: projection.text,
                            stream: stream.clone(),
                            output_name: Some(projection.stem),
                            screen_cuts: Vec::new(),
                            memory_origin: None,
                        });
                    }
                }
            }
            Err(_) => memory.incomplete = true,
        }
    }
    entries
}

fn raw_content(
    path: &Path,
    segment: &Segment,
    day: &str,
    family: RawPerceptFamily,
    projection: PerceptProjection,
) -> Option<RawContent> {
    raw_content_with_speakers(path, segment, day, family, projection, None)
}

fn raw_content_with_speakers(
    path: &Path,
    segment: &Segment,
    day: &str,
    family: RawPerceptFamily,
    projection: PerceptProjection,
    speakers: Option<&BTreeMap<i64, String>>,
) -> Option<RawContent> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            log::warn!(
                "unable to read transcript input {}: {error}",
                path.display()
            );
            return None;
        }
    };
    let text = match speakers {
        Some(speakers) => name_speakers(&text, speakers),
        None => text,
    };
    let name = path.file_name()?.to_str()?;
    let rel = match segment.record_stream() {
        Some(stream) => format!("{day}/{stream}/{}/{}", segment.key, name),
        // Diagnostic chunk header only; path remains the authority.
        None => format!("{}/{}", segment.path.display(), name),
    };
    // Unlike solstone/think/cluster.py:173, the formatter skips malformed JSONL lines
    // individually (content/mod.rs:500-513), preserving valid body rows rather than
    // reducing what native reads from the file.
    let (header, chunks, error, tmux_chunk_indices) = match (family, projection) {
        (RawPerceptFamily::RawScreen, PerceptProjection::ScreenTalent) => {
            let produced = produce_screen_talent_raw_screen_chunks(&rel, &text);
            (
                produced.header,
                produced.chunks,
                produced.error,
                produced.tmux_chunk_indices,
            )
        }
        _ => {
            let produced = produce_raw_percept_chunks(family, &rel, &text);
            (produced.header, produced.chunks, produced.error, Vec::new())
        }
    };
    if let Some(error) = &error {
        log::warn!("{error}");
    }
    let tmux_chunk_indices = tmux_chunk_indices.into_iter().collect::<BTreeSet<_>>();
    let mut rendered = String::new();
    if let Some(header) = header {
        rendered.push_str(&header);
    }
    let mut tmux_chunk_offsets = Vec::new();
    for (index, chunk) in chunks.into_iter().enumerate() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        let chunk_start = rendered.len();
        rendered.push_str(&chunk.content);
        if tmux_chunk_indices.contains(&index) {
            tmux_chunk_offsets.push(chunk_start);
        }
    }
    let screen_cuts = tmux_chunk_offsets
        .into_iter()
        .enumerate()
        .map(|(index, observation_byte_offset)| ScreenCut {
            byte_offset: if index == 0 {
                0
            } else {
                observation_byte_offset
            },
            observation_byte_offset,
            reset_carry: index == 0,
        })
        .collect();
    Some(RawContent {
        text: rendered,
        screen_cuts,
    })
}

/// Sentence id to speaker name for the segment's labelled audio source, and
/// that source's file stem. `None` when no line has a voice-identified speaker.
fn segment_voices(dir: &Path, voices: &VoiceNames) -> Option<(String, BTreeMap<i64, String>)> {
    let text = fs::read_to_string(dir.join("talents").join("speaker_labels.json")).ok()?;
    let payload = serde_json::from_str::<Value>(&text).ok()?;
    let names = payload
        .get("labels")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|row| Some((row.get("sentence_id")?.as_i64()?, voices.name_for(row)?)))
        .collect::<BTreeMap<_, _>>();
    if names.is_empty() {
        return None;
    }
    Some((labelled_audio_source(dir)?, names))
}

/// What the owner said in a segment, by recognized voice: the text of every
/// labelled sentence `VoiceNames` names as the owner, in transcript order.
pub fn owner_voice_lines(dir: &Path, voices: &VoiceNames) -> Vec<String> {
    let Some((source, names)) = segment_voices(dir, voices) else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(dir.join(format!("{source}.jsonl"))) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| {
            record
                .get("sentence_id")
                .and_then(Value::as_i64)
                .and_then(|id| names.get(&id))
                .is_some_and(|name| name == OWNER_VOICE)
        })
        .filter_map(|record| {
            ["corrected", "text"]
                .iter()
                .filter_map(|key| record.get(*key).and_then(Value::as_str))
                .find(|text| !text.trim().is_empty())
                .map(str::to_owned)
        })
        .collect()
}

/// The audio file the segment's labels refer to: the embedded source, else
/// the only audio transcript. Ambiguous segments name no one.
fn labelled_audio_source(dir: &Path) -> Option<String> {
    let mut embedded = Vec::new();
    let mut transcripts = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if let Some(stem) = name.strip_suffix(".npz")
            && (stem == "audio" || stem.ends_with("_audio"))
        {
            embedded.push(stem.to_owned());
        } else if let Some(stem) = name.strip_suffix(".jsonl")
            && stem.ends_with("audio")
        {
            transcripts.push(stem.to_owned());
        }
    }
    embedded.sort();
    match (embedded.into_iter().next(), transcripts.len()) {
        (Some(stem), _) => Some(stem),
        (None, 1) => transcripts.pop(),
        _ => None,
    }
}

/// Rewrite each identified sentence's `speaker` to the name heard on it.
fn name_speakers(text: &str, speakers: &BTreeMap<i64, String>) -> String {
    let mut output = String::with_capacity(text.len());
    for line in text.lines() {
        let named = serde_json::from_str::<Value>(line)
            .ok()
            .and_then(|value| match value {
                Value::Object(mut record) => {
                    let name = speakers.get(&record.get("sentence_id")?.as_i64()?)?;
                    record.insert("speaker".to_owned(), Value::String(name.clone()));
                    Some(Value::Object(record).to_string())
                }
                _ => None,
            });
        output.push_str(named.as_deref().unwrap_or(line));
        output.push('\n');
    }
    output
}

fn entry(
    start: NaiveDateTime,
    end: NaiveDateTime,
    segment: &Segment,
    prefix: &'static str,
    content: String,
    stream: Option<String>,
    output_name: Option<String>,
) -> Entry {
    Entry {
        timestamp: start,
        segment_key: segment.key.clone(),
        segment_start: start,
        segment_end: end,
        prefix,
        content,
        stream,
        output_name,
        screen_cuts: Vec::new(),
        memory_origin: None,
    }
}

fn remember_memory_source(
    memory: &mut MemoryContext,
    source: solstone_core_format::content::ConsumedOriginal,
) {
    if !memory.sources.contains(&source) {
        memory.sources.push(source);
    }
}

const INCOMPLETE_MEMORY_NOTICE: &str = "agent memory context is incomplete.";

fn disclose_memory_incomplete(
    mut transcript: ScreenTranscript,
    memory: &MemoryContext,
) -> ScreenTranscript {
    if memory.incomplete && transcript.text.lines().last() != Some(INCOMPLETE_MEMORY_NOTICE) {
        if !transcript.text.is_empty() && !transcript.text.ends_with('\n') {
            transcript.text.push('\n');
        }
        transcript.text.push_str(INCOMPLETE_MEMORY_NOTICE);
    }
    transcript
}

fn groups_to_markdown(mut entries: Vec<Entry>) -> ScreenTranscript {
    entries.sort_by_key(|entry| entry.timestamp);
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    for entry in entries {
        if let Some(group) = groups.iter_mut().find(|group| {
            group
                .first()
                .is_some_and(|first| first.segment_key == entry.segment_key)
        }) {
            group.push(entry);
        } else {
            groups.push(vec![entry]);
        }
    }
    groups.sort_by_key(|group| group[0].segment_start);
    let mut lines = Vec::new();
    let mut pending_cuts = Vec::new();
    for group in groups {
        let first = &group[0];
        lines.push(format!(
            "## {} - {}",
            first.segment_start.format("%Y-%m-%d %H:%M:%S"),
            first.segment_end.format("%H:%M:%S")
        ));
        lines.push(String::new());
        for entry in group {
            let entry_heading_line = lines.len();
            match entry.prefix {
                "transcript" => lines.push(format!(
                    "### {}",
                    transcript_header(entry.stream.as_deref())
                )),
                "memory_original" => {
                    lines.push("### agent memory original".into());
                    let origin = entry
                        .memory_origin
                        .as_ref()
                        .expect("memory original carries its trusted origin");
                    lines.push(format!(
                        "Origin: {}",
                        serde_json::to_string(origin).expect("origin serializes")
                    ));
                }
                "percept" => lines.push("### Screen Activity".into()),
                "browser" => lines.push("### Browser Content".into()),
                "agent_output" => lines.push(format!(
                    "### {} summary",
                    entry.output_name.as_deref().unwrap_or("output")
                )),
                _ => continue,
            }
            let trimmed = if entry.prefix == "memory_original" {
                entry.content.as_str()
            } else {
                entry.content.trim()
            };
            let trimmed_start = entry.content.len() - entry.content.trim_start().len();
            let content_line = lines.len();
            lines.push(trimmed.into());
            for cut in entry.screen_cuts {
                if cut.byte_offset < trimmed_start || cut.observation_byte_offset < trimmed_start {
                    continue;
                }
                let (cut_line, cut_relative) = if cut.byte_offset == 0 {
                    (entry_heading_line, 0)
                } else {
                    (content_line, cut.byte_offset - trimmed_start)
                };
                pending_cuts.push((
                    cut_line,
                    cut_relative,
                    content_line,
                    cut.observation_byte_offset - trimmed_start,
                    cut.reset_carry,
                ));
            }
            lines.push(String::new());
        }
    }
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut byte_offset = 0usize;
    for line in &lines {
        line_offsets.push(byte_offset);
        byte_offset = byte_offset.saturating_add(line.len()).saturating_add(1);
    }
    let text = lines.join("\n");
    let mut cuts = pending_cuts
        .into_iter()
        .filter_map(
            |(line, relative, observation_line, observation_relative, reset_carry)| {
                let byte_offset = line_offsets.get(line)?.checked_add(relative)?;
                let observation_byte_offset = line_offsets
                    .get(observation_line)?
                    .checked_add(observation_relative)?;
                (byte_offset <= observation_byte_offset
                    && observation_byte_offset <= text.len()
                    && text.is_char_boundary(byte_offset)
                    && text.is_char_boundary(observation_byte_offset))
                .then_some(ScreenCut {
                    byte_offset,
                    observation_byte_offset,
                    reset_carry,
                })
            },
        )
        .collect::<Vec<_>>();
    cuts.sort_by_key(|cut| cut.byte_offset);
    cuts.dedup_by_key(|cut| cut.byte_offset);
    ScreenTranscript { text, cuts }
}

fn day_dir(root: &Path, day: &str) -> PathBuf {
    root.join("chronicle").join(day)
}

impl Segment {
    fn record_stream(&self) -> Option<&str> {
        match &self.stream {
            StreamLocation::Direct => Some(solstone_core_journal_io::DEFAULT_STREAM),
            StreamLocation::Named(name)
                if name.to_str() == Some(solstone_core_journal_io::DEFAULT_STREAM) =>
            {
                None
            }
            StreamLocation::Named(name) => name.to_str(),
        }
    }
}

fn all_segments(root: &Path, day: &str) -> Vec<Segment> {
    let mut segments = iter_segments(root, PathOrDay::Day(day))
        .unwrap_or_default()
        .into_iter()
        .map(|segment| Segment {
            path: segment.path().to_path_buf(),
            stream: segment.stream().clone(),
            key: segment.key().to_owned(),
        })
        .collect::<Vec<_>>();
    segments.sort_by(|left, right| {
        left.key.cmp(&right.key).then_with(|| {
            match (left.stream.directory(), right.stream.directory()) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(left), Some(right)) => left.cmp(right),
            }
        })
    });
    segments
}

fn find_segment(root: &Path, day: &str, key: &str, stream: Option<&str>) -> Option<Segment> {
    let stream = stream.filter(|stream| !stream.is_empty());
    all_segments(root, day).into_iter().find(|segment| {
        segment.key == key && stream.is_none_or(|stream| segment.stream.matches(stream))
    })
}

fn sorted_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn segment_times(day: &str, key: &str) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let start = segment_parse(key)?;
    let length = key.split_once('_')?.1.parse::<i64>().ok()?;
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let start = date.and_hms_opt(start.hour.into(), start.minute.into(), start.second.into())?;
    Some((start, start + Duration::seconds(length)))
}

fn stream_marker(path: &Path) -> Option<String> {
    fs::read(path.join("stream.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("stream")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

fn first_kind(path: &Path) -> Option<String> {
    let line = fs::read_to_string(path)
        .ok()?
        .lines()
        .next()?
        .trim()
        .to_owned();
    serde_json::from_str::<Value>(&line)
        .ok()?
        .get("kind")?
        .as_str()
        .map(str::to_owned)
}

fn transcript_header(stream: Option<&str>) -> &'static str {
    match stream {
        Some("import.chatgpt") => "ChatGPT Conversation",
        Some("import.claude") => "Claude Conversation",
        Some("import.gemini") => "Gemini Conversation",
        Some("import.ics") => "Calendar Event",
        Some("import.obsidian") => "Note",
        Some("import.kindle") => "Highlights",
        _ => "Transcript",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    const DAY: &str = "20260731";
    const SEGMENT: &str = "090000_60";

    fn sources(transcripts: bool, percepts: bool, talents: TalentSource) -> Sources {
        Sources {
            transcripts,
            percepts,
            talents,
            voices: None,
        }
    }

    fn segment(root: &TempDir) -> PathBuf {
        let path = root.path().join("chronicle").join(DAY).join(SEGMENT);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn only(stems: &[&str]) -> TalentSource {
        TalentSource::Only(stems.iter().map(|stem| (*stem).to_owned()).collect())
    }

    #[test]
    fn a_strava_workout_reaches_the_transcript_once_on_its_first_piece() {
        let root = TempDir::new().unwrap();
        for (key, index) in [("070200_300", 0), ("070700_300", 1)] {
            let path = root
                .path()
                .join("chronicle")
                .join(DAY)
                .join("import.strava")
                .join(key);
            fs::create_dir_all(&path).unwrap();
            fs::write(
                path.join("workout.json"),
                json!({
                    "schema": "solstone.import.strava.tile.v1",
                    "activity_id": 9,
                    "tile": {"index": index, "count": 2},
                    "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
                })
                .to_string(),
            )
            .unwrap();
        }
        let (markdown, counts) =
            cluster(root.path(), DAY, &sources(true, false, TalentSource::All));
        assert_eq!(
            markdown.matches("## Strava workout: Morning Run").count(),
            1
        );
        assert!(markdown.contains("10.2 km"));
        assert_eq!(counts.transcripts, 1);
    }

    #[test]
    fn disclose_memory_incomplete_appends_the_notice_once() {
        let incomplete = MemoryContext {
            sources: Vec::new(),
            incomplete: true,
        };
        let transcript = disclose_memory_incomplete(
            ScreenTranscript::plain("existing text".to_owned()),
            &incomplete,
        );
        assert_eq!(
            transcript.text,
            format!("existing text\n{INCOMPLETE_MEMORY_NOTICE}")
        );
        let already_disclosed = disclose_memory_incomplete(transcript, &incomplete);
        assert_eq!(
            already_disclosed
                .text
                .matches(INCOMPLETE_MEMORY_NOTICE)
                .count(),
            1
        );

        let complete = disclose_memory_incomplete(
            ScreenTranscript::plain("complete text".to_owned()),
            &MemoryContext::default(),
        );
        assert_eq!(complete.text, "complete text");
    }

    #[test]
    fn all_segments_includes_legacy_default_stream_segments() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260731/090000_60")).unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260731/field/100000_60")).unwrap();

        let found = all_segments(root.path(), "20260731");
        assert_eq!(found.len(), 2);
        assert!(found[0].stream.is_direct());
        assert_eq!(found[0].key, "090000_60");
        assert_eq!(
            found[1].stream.directory().and_then(|name| name.to_str()),
            Some("field")
        );
        assert_eq!(found[1].key, "100000_60");
    }

    #[test]
    fn named_default_keeps_exact_path_rel_and_still_reads_content() {
        let root = TempDir::new().unwrap();
        let day = root.path().join("chronicle").join(DAY);
        let direct = day.join("080000_60");
        let named = day.join("_default").join("090000_60");
        fs::create_dir_all(&direct).unwrap();
        fs::create_dir_all(&named).unwrap();
        fs::write(
            direct.join("audio.jsonl"),
            r#"{"start":"00:00:00","text":"direct-payload"}"#,
        )
        .unwrap();
        fs::write(
            named.join("audio.jsonl"),
            r#"{"start":"00:00:00","text":"named-payload"}"#,
        )
        .unwrap();

        let found = all_segments(root.path(), DAY);
        assert_eq!(found.len(), 2);
        let direct_segment = found
            .iter()
            .find(|segment| segment.stream.is_direct())
            .unwrap();
        let named_segment = found
            .iter()
            .find(|segment| {
                segment.stream.directory().and_then(|name| name.to_str()) == Some("_default")
            })
            .unwrap();

        assert_eq!(
            direct_segment.record_stream(),
            Some(solstone_core_journal_io::DEFAULT_STREAM)
        );
        assert_eq!(named_segment.record_stream(), None);

        let file = "audio.jsonl";
        let direct_rel = match direct_segment.record_stream() {
            Some(stream) => format!("{DAY}/{stream}/{}/{file}", direct_segment.key),
            None => format!("{}/{file}", direct_segment.path.display()),
        };
        let named_rel = match named_segment.record_stream() {
            Some(stream) => format!("{DAY}/{stream}/{}/{file}", named_segment.key),
            None => format!("{}/{file}", named_segment.path.display()),
        };
        assert_eq!(direct_rel, format!("{DAY}/_default/080000_60/{file}"));
        assert_ne!(named_rel, format!("{DAY}/_default/090000_60/{file}"));
        assert_eq!(
            named_rel,
            format!("{}/{file}", named_segment.path.display())
        );

        let direct_content = raw_content(
            &direct.join(file),
            direct_segment,
            DAY,
            RawPerceptFamily::Audio,
            PerceptProjection::Generic,
        )
        .unwrap();
        let named_content = raw_content(
            &named.join(file),
            named_segment,
            DAY,
            RawPerceptFamily::Audio,
            PerceptProjection::Generic,
        )
        .unwrap();
        assert!(
            direct_content.contains("direct-payload"),
            "{direct_content}"
        );
        assert!(named_content.contains("named-payload"), "{named_content}");
        assert_ne!(direct_content, named_content);

        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(true, false, TalentSource::Disabled),
        );
        assert_eq!(counts.transcripts, 2);
        assert!(markdown.contains("direct-payload"), "{markdown}");
        assert!(markdown.contains("named-payload"), "{markdown}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn two_non_utf8_streams_stay_distinct_through_cluster() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let root = TempDir::new().unwrap();
        let day = root.path().join("chronicle").join(DAY);
        let first = day.join(OsStr::from_bytes(b"s\xff")).join("080000_60");
        let second = day.join(OsStr::from_bytes(b"s\xfe")).join("080000_60");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(
            first.join("audio.jsonl"),
            r#"{"start":"00:00:00","text":"First stream"}"#,
        )
        .unwrap();
        fs::write(
            second.join("audio.jsonl"),
            r#"{"start":"00:00:00","text":"Second stream"}"#,
        )
        .unwrap();

        let found = all_segments(root.path(), DAY);
        assert_eq!(found.len(), 2);
        assert_ne!(found[0].path, found[1].path);
        assert_ne!(found[0].stream, found[1].stream);
        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(true, false, TalentSource::Disabled),
        );
        assert_eq!(counts.transcripts, 2);
        assert!(markdown.contains("First stream"));
        assert!(markdown.contains("Second stream"));
    }

    #[test]
    fn criterion_2_percepts_only_renders_sections_and_counts() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        fs::write(
            segment.join("screen.jsonl"),
            r#"{"timestamp":0,"content":{"window":"Planning notes"}}"#,
        )
        .unwrap();
        fs::write(
            segment.join("browser_events.jsonl"),
            r#"{"t":"segment_start","ts":1,"title":"Inbox"}"#,
        )
        .unwrap();

        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(false, true, TalentSource::Disabled),
        );

        assert!(markdown.contains("### Screen Activity"));
        assert!(markdown.contains("### Browser Content"));
        assert_eq!(counts.transcripts, 0);
        assert_eq!(counts.percepts, 2);
        assert_eq!(counts.talents, 0);
    }

    #[test]
    fn screen_talent_projection_is_private_to_its_explicit_reader() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        let fixture = include_str!(
            "../../solstone-core-format/tests/data/golden/tmux-observer-envelope-main.jsonl"
        );
        fs::write(segment.join("tmux_0_screen.jsonl"), fixture).unwrap();
        let sources = sources(false, true, TalentSource::Disabled);

        let generic = cluster_period(root.path(), DAY, SEGMENT, &sources, None);
        let screen = cluster_period_for_screen_talent(root.path(), DAY, SEGMENT, &sources, None);

        assert_eq!(generic.1, screen.1);
        assert!(generic.0.contains("Terminal session 'main'"));
        assert!(generic.0.contains("@8"));
        assert!(generic.0.contains("\\u001b[31m"));
        assert!(!generic.0.contains("**Tmux window:**"));
        assert!(screen.0.contains("**Tmux window:**"));
        assert!(screen.0.contains("RED café"));
        assert!(!screen.0.contains("Terminal session 'main'"));
        assert!(!screen.0.contains("@8"));
        assert!(!screen.0.contains("\\u001b[31m"));
    }

    #[test]
    fn criterion_2_talents_only_renders_summary_and_counts() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/planning.md"), "Plan the next step.").unwrap();

        let (markdown, counts) =
            cluster(root.path(), DAY, &sources(false, false, TalentSource::All));

        assert!(markdown.contains("### planning summary"));
        assert_eq!(
            counts,
            SourceCounts {
                transcripts: 0,
                percepts: 0,
                talents: 1,
            }
        );
    }

    #[test]
    fn criterion_4_talent_filter_keeps_only_named_output_stem() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/selected.md"), "Selected output.").unwrap();
        fs::write(segment.join("talents/other.md"), "Other output.").unwrap();

        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(false, false, only(&["selected"])),
        );

        assert!(markdown.contains("### selected summary"));
        assert!(markdown.contains("Selected output."));
        assert!(!markdown.contains("other summary"));
        assert!(!markdown.contains("Other output."));
        assert_eq!(counts.talents, 1);
    }

    #[test]
    fn criterion_6_counts_are_zero_filled_and_browser_folds_into_percepts() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        fs::write(
            segment.join("browser_events.jsonl"),
            r#"{"t":"segment_start","ts":1,"title":"Inbox"}"#,
        )
        .unwrap();
        let (_, browser_counts) = cluster(
            root.path(),
            DAY,
            &sources(false, true, TalentSource::Disabled),
        );

        assert_eq!(browser_counts.transcripts, 0);
        assert_eq!(browser_counts.percepts, 1);
        assert_eq!(browser_counts.talents, 0);
        let counts = SourceCounts {
            transcripts: 1,
            percepts: 2,
            talents: 3,
        };

        assert_eq!(counts.total(), 6);
        assert_eq!(
            Value::from(counts),
            json!({"transcripts": 1, "percepts": 2, "talents": 3})
        );
        assert_eq!(
            Value::from(SourceCounts::default()),
            json!({"transcripts": 0, "percepts": 0, "talents": 0})
        );
    }

    #[test]
    fn criterion_8_start_key_record_is_rendered_as_a_chunk_not_metadata() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        fs::write(
            segment.join("capture_audio.jsonl"),
            r#"{"start":"00:00:01","text":"First row","title":"metadata-looking"}"#,
        )
        .unwrap();

        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(true, false, TalentSource::Disabled),
        );

        assert!(markdown.contains("[00:00:01] First row"));
        assert!(markdown.contains("## 2026-07-31 09:00:00 - 09:01:00"));
        assert!(markdown.contains("### Transcript"));
        assert!(markdown.contains("Start: 2026-07-31 09:00am"));
        assert!(!markdown.contains("Title: metadata-looking"));
        assert_eq!(counts.transcripts, 1);
    }

    #[test]
    fn voice_identified_lines_name_the_owner_and_people_by_voice_only() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        fs::write(
            segment.join("audio.jsonl"),
            [
                r#"{"raw":"audio.flac"}"#,
                r#"{"start":"00:00:01","speaker":1,"sentence_id":1,"text":"I will send it"}"#,
                r#"{"start":"00:00:02","speaker":2,"sentence_id":2,"text":"Thanks"}"#,
                r#"{"start":"00:00:03","speaker":2,"sentence_id":3,"text":"Guessed name"}"#,
                r#"{"start":"00:00:04","speaker":1,"sentence_id":4,"text":"Too close to call"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("talents/speaker_labels.json"),
            json!({"labels": [
                {"sentence_id": 1, "speaker": "owner", "method": "owner_centroid", "confidence": "high"},
                {"sentence_id": 2, "speaker": "mina", "method": "acoustic", "confidence": "high"},
                {"sentence_id": 3, "speaker": "mina", "method": "contextual", "confidence": "medium"},
                {"sentence_id": 4, "speaker": "owner", "method": "owner_centroid", "owner_margin_declined": true}
            ]})
            .to_string(),
        )
        .unwrap();
        let voices = VoiceNames {
            principal: "owner".into(),
            owner_voice_confirmed: true,
            people: [("mina".to_owned(), "Mina".to_owned())].into(),
        };
        let mut heard = sources(true, false, TalentSource::Disabled);
        heard.voices = Some(voices.clone());

        let (markdown, _) = cluster(root.path(), DAY, &heard);
        assert!(markdown.contains("[00:00:01] You: I will send it"));
        assert!(markdown.contains("[00:00:02] Mina: Thanks"));
        // A name inferred from what was said is not a voice.
        assert!(markdown.contains("[00:00:03] Speaker 2: Guessed name"));
        assert!(markdown.contains("[00:00:04] Speaker 1: Too close to call"));

        // Without a confirmed voiceprint, the owner is never named.
        heard.voices = Some(VoiceNames {
            owner_voice_confirmed: false,
            ..voices.clone()
        });
        let (markdown, _) = cluster(root.path(), DAY, &heard);
        assert!(markdown.contains("[00:00:01] Speaker 1: I will send it"));
        assert!(markdown.contains("[00:00:02] Mina: Thanks"));

        let (markdown, _) = cluster(
            root.path(),
            DAY,
            &sources(true, false, TalentSource::Disabled),
        );
        assert!(markdown.contains("[00:00:01] Speaker 1: I will send it"));
        assert!(!markdown.contains("Mina"));

        // Only the owner's recognized lines are what the owner said.
        assert_eq!(owner_voice_lines(&segment, &voices), vec!["I will send it"]);
    }

    #[test]
    fn criterion_10_day_not_found_keeps_the_day_directory_absent() {
        let root = TempDir::new().unwrap();
        let day_dir = root.path().join("chronicle").join(DAY);
        let (markdown, counts) = cluster(root.path(), DAY, &sources(true, true, TalentSource::All));

        // Unlike solstone/think/utils.py:289, the native day read does not create a directory.
        assert_eq!(
            markdown,
            format!("Day folder not found: {}", day_dir.display())
        );
        assert_eq!(counts, SourceCounts::default());
        assert!(!day_dir.exists());
    }

    #[test]
    fn criterion_10_segment_not_found_has_its_own_message_and_zero_counts() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("chronicle").join(DAY)).unwrap();

        let (markdown, counts) = cluster_period(
            root.path(),
            DAY,
            SEGMENT,
            &sources(true, true, TalentSource::All),
            None,
        );

        assert_eq!(markdown, "Segment folder not found: 20260731/090000_60");
        assert_eq!(counts, SourceCounts::default());
    }

    #[test]
    fn empty_stream_searches_all_segments_while_named_stream_filters() {
        let root = TempDir::new().unwrap();
        let default_segment = segment(&root);
        let field_segment = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("field")
            .join(SEGMENT);
        fs::create_dir_all(&field_segment).unwrap();
        fs::write(
            default_segment.join("capture_audio.jsonl"),
            r#"{"start":"00:00:00","text":"Default stream input"}"#,
        )
        .unwrap();
        fs::write(
            field_segment.join("capture_audio.jsonl"),
            r#"{"start":"00:00:00","text":"Field stream input"}"#,
        )
        .unwrap();
        let sources = sources(true, false, TalentSource::Disabled);

        let unspecified = cluster_period(root.path(), DAY, SEGMENT, &sources, None);
        let empty = cluster_period(root.path(), DAY, SEGMENT, &sources, Some(""));
        let field = cluster_period(root.path(), DAY, SEGMENT, &sources, Some("field"));

        assert_eq!(empty, unspecified);
        assert!(field.0.contains("Field stream input"));
        assert!(!field.0.contains("Default stream input"));
        assert_eq!(field.1.transcripts, 1);
    }

    #[test]
    fn criterion_10_span_fails_when_any_member_is_missing() {
        let root = TempDir::new().unwrap();
        segment(&root);

        let error = cluster_span(
            root.path(),
            DAY,
            &[SEGMENT, "100000_60"],
            &sources(true, true, TalentSource::All),
            None,
        )
        .unwrap_err();

        assert_eq!(error, "Segment directories not found: 100000_60");
    }

    #[test]
    fn criterion_11_emptiness_uses_counts_and_the_shared_threshold() {
        let present = SourceCounts {
            transcripts: 1,
            ..SourceCounts::default()
        };

        assert!(is_no_input("enough text", &SourceCounts::default()));
        assert!(is_no_input(
            "x".repeat(MIN_INPUT_CHARS - 1).as_str(),
            &present
        ));
        assert!(!is_no_input("x".repeat(MIN_INPUT_CHARS).as_str(), &present));
    }

    #[test]
    fn criterion_7_malformed_jsonl_line_keeps_the_remaining_audio_rows() {
        let root = TempDir::new().unwrap();
        let segment = segment(&root);
        fs::write(
            segment.join("capture_audio.jsonl"),
            "{\"start\":\"00:00:00\",\"text\":\"Before\"}\nnot json\n{\"start\":\"00:00:01\",\"text\":\"After\"}\n",
        )
        .unwrap();

        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(true, false, TalentSource::Disabled),
        );

        // Native keeps valid rows after a malformed body line; cluster.py:173 drops the file.
        assert!(markdown.contains("Before"));
        assert!(markdown.contains("After"));
        assert_eq!(counts.transcripts, 1);
    }

    #[test]
    fn criterion_17_dropped_input_diagnostics_do_not_enter_markdown() {
        let root = TempDir::new().unwrap();
        let path = segment(&root);
        fs::write(
            path.join("capture_audio.jsonl"),
            "{\"start\":\"00:00:00\",\"text\":\"Kept row\"}\n{\"text\":\"Dropped row\"}\n",
        )
        .unwrap();

        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(true, false, TalentSource::Disabled),
        );

        assert!(markdown.contains("Kept row"));
        assert!(!markdown.contains("Dropped row"));
        assert!(!markdown.contains("Skipped 1 entries missing 'start'"));
        assert_eq!(counts.transcripts, 1);
    }

    #[test]
    fn criterion_17_unreadable_input_drops_only_that_entry() {
        let root = TempDir::new().unwrap();
        let path = segment(&root);
        fs::write(
            path.join("good_audio.jsonl"),
            r#"{"start":"00:00:00","text":"Good row"}"#,
        )
        .unwrap();
        let missing = path.join("missing_audio.jsonl");
        let missing_segment = Segment {
            path: path.clone(),
            stream: StreamLocation::Direct,
            key: SEGMENT.to_owned(),
        };

        assert_eq!(
            raw_content(
                &missing,
                &missing_segment,
                DAY,
                RawPerceptFamily::Audio,
                PerceptProjection::Generic,
            ),
            None
        );
        let (markdown, counts) = cluster(
            root.path(),
            DAY,
            &sources(true, false, TalentSource::Disabled),
        );
        assert!(markdown.contains("Good row"));
        assert_eq!(counts.transcripts, 1);
    }
}
