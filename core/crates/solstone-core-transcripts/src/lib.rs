// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveDate, NaiveDateTime};
use serde_json::{Map, Value};
use solstone_core_format::content::{
    Family, RawPerceptFamily, iter_talent_text_projections, produce_chunks,
    produce_raw_percept_chunks, produce_screen_talent_raw_screen_chunks,
};
use solstone_core_format::segment::segment_parse;
use solstone_core_journal_io::paths::{PathOrDay, StreamLocation, iter_segments};

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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCounts {
    pub transcripts: usize,
    pub percepts: usize,
    pub talents: usize,
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
                "transcript" => counts.transcripts += 1,
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

pub fn cluster_for_screen_talent(
    root: &Path,
    day: &str,
    sources: &Sources,
) -> (ScreenTranscript, SourceCounts) {
    cluster_with_projection(root, day, sources, PerceptProjection::ScreenTalent)
}

fn cluster_with_projection(
    root: &Path,
    day: &str,
    sources: &Sources,
    projection: PerceptProjection,
) -> (ScreenTranscript, SourceCounts) {
    let day_dir = day_dir(root, day);
    // Python's day_path at solstone/think/utils.py:289 creates this directory before
    // cluster.py:794-797 checks it. Native reads stay non-creating: native think creates
    // day directories before dispatch, while an owner's read must not create chronicle state.
    if !day_dir.is_dir() {
        return (
            ScreenTranscript::plain(format!("Day folder not found: {}", day_dir.display())),
            SourceCounts::default(),
        );
    }
    let entries = load_day(root, day, sources, projection);
    let counts = SourceCounts::from_entries(&entries);
    if entries.is_empty() {
        (
            ScreenTranscript::plain(format!(
                "No transcript, screen, or browser files found for date {day} in {}.",
                day_dir.display()
            )),
            counts,
        )
    } else {
        (groups_to_markdown(entries), counts)
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

fn cluster_period_with_projection(
    root: &Path,
    day: &str,
    key: &str,
    sources: &Sources,
    stream: Option<&str>,
    projection: PerceptProjection,
) -> (ScreenTranscript, SourceCounts) {
    let Some(segment) = find_segment(root, day, key, stream) else {
        return (
            ScreenTranscript::plain(format!("Segment folder not found: {day}/{key}")),
            SourceCounts::default(),
        );
    };
    let entries = process_segment(&segment, day, sources, projection);
    let counts = SourceCounts::from_entries(&entries);
    if entries.is_empty() {
        (
            ScreenTranscript::plain(format!(
                "No transcript, screen, or browser files found for segment {key}"
            )),
            counts,
        )
    } else {
        (groups_to_markdown(entries), counts)
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

fn cluster_span_with_projection(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: &Sources,
    stream: Option<&str>,
    projection: PerceptProjection,
) -> Result<(ScreenTranscript, SourceCounts), String> {
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
    let mut entries = found
        .iter()
        .flat_map(|segment| process_segment(segment, day, sources, projection))
        .collect::<Vec<_>>();
    let counts = SourceCounts::from_entries(&entries);
    if entries.is_empty() {
        return Ok((
            ScreenTranscript::plain(format!(
                "No transcript, screen, or browser files found in span: {}",
                span.join(", ")
            )),
            counts,
        ));
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok((groups_to_markdown(entries), counts))
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
    let entries = load_day(root, day, sources, PerceptProjection::Generic)
        .into_iter()
        .filter(|entry| entry.segment_start < end && entry.segment_end > start)
        .collect();
    Ok(groups_to_markdown(entries).text)
}

fn range_error(error: impl fmt::Display) -> RangeError {
    RangeError(error.to_string())
}

fn load_day(
    root: &Path,
    day: &str,
    sources: &Sources,
    projection: PerceptProjection,
) -> Vec<Entry> {
    let mut entries = all_segments(root, day)
        .into_iter()
        .flat_map(|segment| process_segment(&segment, day, sources, projection))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    entries
}

fn process_segment(
    segment: &Segment,
    day: &str,
    sources: &Sources,
    projection: PerceptProjection,
) -> Vec<Entry> {
    let Some((start, end)) = segment_times(day, &segment.key) else {
        return Vec::new();
    };
    let stream = stream_marker(&segment.path);
    let mut entries = Vec::new();
    let files = sorted_files(&segment.path);
    if sources.transcripts {
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
        for path in transcript {
            if let Some(content) = raw_content(
                path,
                segment,
                day,
                RawPerceptFamily::Audio,
                PerceptProjection::Generic,
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
        if let Ok(projections) = iter_talent_text_projections(&talents, "", Some(&stem_filter)) {
            for projection in projections {
                if !projection.text.trim().is_empty() {
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
                    });
                }
            }
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
    }
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
                "percept" => lines.push("### Screen Activity".into()),
                "browser" => lines.push("### Browser Content".into()),
                "agent_output" => lines.push(format!(
                    "### {} summary",
                    entry.output_name.as_deref().unwrap_or("output")
                )),
                _ => continue,
            }
            let trimmed = entry.content.trim();
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
        assert!(!generic.0.contains("**Tmux observation:**"));
        assert!(screen.0.contains("**Tmux observation:**"));
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
