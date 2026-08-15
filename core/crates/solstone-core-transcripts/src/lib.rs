// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Duration, NaiveDate, NaiveDateTime};
use serde_json::Value;
use solstone_core_format::content::{
    ChatLabels, Family, RawPerceptFamily, iter_talent_text_projections, produce_chunks,
    produce_raw_percept_chunks,
};
use solstone_core_format::segment::segment_parse;
use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

#[derive(Clone, Copy)]
pub struct Sources {
    pub transcripts: bool,
    pub percepts: bool,
    pub agents: bool,
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
}

struct Segment {
    path: PathBuf,
    stream_dir: String,
    key: String,
}

#[derive(Debug)]
pub struct RangeError(String);

impl fmt::Display for RangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for RangeError {}

pub fn cluster(root: &Path, day: &str, sources: Sources) -> String {
    let day_dir = day_dir(root, day);
    if !day_dir.is_dir() {
        return format!("Day folder not found: {}", day_dir.display());
    }
    let entries = load_day(root, day, sources);
    if entries.is_empty() {
        format!(
            "No transcript, screen, or browser files found for date {day} in {}.",
            day_dir.display()
        )
    } else {
        groups_to_markdown(entries)
    }
}

pub fn cluster_period(
    root: &Path,
    day: &str,
    key: &str,
    sources: Sources,
    stream: Option<&str>,
) -> String {
    let Some(segment) = find_segment(root, day, key, stream) else {
        return format!("Segment folder not found: {day}/{key}");
    };
    let entries = process_segment(&segment, day, sources);
    if entries.is_empty() {
        format!("No transcript, screen, or browser files found for segment {key}")
    } else {
        groups_to_markdown(entries)
    }
}

pub fn cluster_span(
    root: &Path,
    day: &str,
    span: &[&str],
    sources: Sources,
    stream: Option<&str>,
) -> Result<String, String> {
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
        .flat_map(|segment| process_segment(segment, day, sources))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(format!(
            "No transcript, screen, or browser files found in span: {}",
            span.join(", ")
        ));
    }
    entries.sort_by_key(|entry| entry.timestamp);
    Ok(groups_to_markdown(entries))
}

pub fn cluster_range(
    root: &Path,
    day: &str,
    start: &str,
    end: &str,
    sources: Sources,
) -> Result<String, RangeError> {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(range_error)?;
    let start =
        NaiveDateTime::parse_from_str(&format!("{}{start}", date.format("%Y%m%d")), "%Y%m%d%H%M%S")
            .map_err(range_error)?;
    let end =
        NaiveDateTime::parse_from_str(&format!("{}{end}", date.format("%Y%m%d")), "%Y%m%d%H%M%S")
            .map_err(range_error)?;
    let entries = load_day(root, day, sources)
        .into_iter()
        .filter(|entry| entry.segment_start < end && entry.segment_end > start)
        .collect();
    Ok(groups_to_markdown(entries))
}

fn range_error(error: impl fmt::Display) -> RangeError {
    RangeError(error.to_string())
}

fn load_day(root: &Path, day: &str, sources: Sources) -> Vec<Entry> {
    let mut entries = all_segments(root, day)
        .into_iter()
        .flat_map(|segment| process_segment(&segment, day, sources))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.timestamp);
    entries
}

fn process_segment(segment: &Segment, day: &str, sources: Sources) -> Vec<Entry> {
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
            if let Some(content) = raw_content(path, segment, day, RawPerceptFamily::Audio) {
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
            if let Ok(content) = fs::read_to_string(path)
                && !content.trim().is_empty()
            {
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
        for path in &files {
            if first_kind(path).as_deref() == Some("image")
                && let Some(content) = raw_content(path, segment, day, RawPerceptFamily::Audio)
            {
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
    }
    if sources.percepts {
        for path in files.iter().filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("screen.jsonl"))
        }) {
            if let Some(content) = raw_content(path, segment, day, RawPerceptFamily::RawScreen)
                && !content.is_empty()
            {
                entries.push(entry(
                    start,
                    end,
                    segment,
                    "percept",
                    content,
                    stream.clone(),
                    None,
                ));
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
            if let Ok(text) = fs::read_to_string(path) {
                let content = produce_chunks(
                    Family::Browser,
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                    &text,
                    &ChatLabels::default(),
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
        }
    }
    if sources.agents {
        let talents = segment.path.join("talents");
        if let Ok(projections) = iter_talent_text_projections(&talents, "", None) {
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
) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let name = path.file_name()?.to_str()?;
    let rel = format!("{day}/{}/{}/{}", segment.stream_dir, segment.key, name);
    let produced = produce_raw_percept_chunks(family, &rel, &text);
    let mut parts = Vec::new();
    if let Some(header) = produced.header {
        parts.push(header);
    }
    parts.extend(produced.chunks.into_iter().map(|chunk| chunk.content));
    Some(parts.join("\n"))
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
    }
}

fn groups_to_markdown(mut entries: Vec<Entry>) -> String {
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
    for group in groups {
        let first = &group[0];
        lines.push(format!(
            "## {} - {}",
            first.segment_start.format("%Y-%m-%d %H:%M:%S"),
            first.segment_end.format("%H:%M:%S")
        ));
        lines.push(String::new());
        for entry in group {
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
            lines.push(entry.content.trim().into());
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

fn day_dir(root: &Path, day: &str) -> PathBuf {
    root.join("chronicle").join(day)
}

fn all_segments(root: &Path, day: &str) -> Vec<Segment> {
    iter_segments(root, PathOrDay::Day(day))
        .unwrap_or_default()
        .into_iter()
        .map(|segment| Segment {
            path: segment.path,
            stream_dir: segment.stream,
            key: segment.key,
        })
        .collect()
}

fn find_segment(root: &Path, day: &str, key: &str, stream: Option<&str>) -> Option<Segment> {
    all_segments(root, day).into_iter().find(|segment| {
        segment.key == key && stream.is_none_or(|stream| segment.stream_dir == stream)
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

    use tempfile::TempDir;

    use super::all_segments;

    #[test]
    fn all_segments_includes_legacy_default_stream_segments() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260731/090000_60")).unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260731/field/100000_60")).unwrap();

        let found = all_segments(root.path(), "20260731")
            .into_iter()
            .map(|segment| (segment.stream_dir, segment.key))
            .collect::<Vec<_>>();

        assert_eq!(
            found,
            vec![
                ("_default".to_owned(), "090000_60".to_owned()),
                ("field".to_owned(), "100000_60".to_owned()),
            ]
        );
    }
}
