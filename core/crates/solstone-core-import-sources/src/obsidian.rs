// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Obsidian vault source parsing.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use regex::Regex;
use serde_json::{Map, Value, json};
use solstone_core_import::{ImportPreview, RegistrySource};

use crate::save::{RenderedImport, SegmentFile};
use crate::shared::{day_key, window};

/// A note's read-only source facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoteEntry {
    pub title: String,
    pub source_path: PathBuf,
    pub content: String,
    pub tags: Vec<String>,
    pub wikilinks: Vec<String>,
    pub is_daily: bool,
    pub daily_note_day: Option<String>,
    /// When the note file last changed: the moment it lands in the journal.
    pub modified: DateTime<Utc>,
}

/// Failure while reading an Obsidian source tree.
#[derive(Debug)]
pub enum ObsidianError {
    ReadDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    Metadata {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ObsidianError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadDirectory { path, source } | Self::Metadata { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
        }
    }
}

impl Error for ObsidianError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadDirectory { source, .. } | Self::Metadata { source, .. } => Some(source),
        }
    }
}

/// Return whether a path has the reference Obsidian source shape.
#[must_use]
pub fn detect(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    if path.join(".obsidian").is_dir() || path.join("logseq").is_dir() {
        return true;
    }
    visible_markdown_count(path) >= 3
}

/// Read a vault into in-memory note facts without mutating the source.
pub fn collect_notes(path: &Path) -> Result<Vec<NoteEntry>, ObsidianError> {
    let mut files = Vec::new();
    walk_md_files(path, &mut files)?;
    files.sort_unstable();
    files.iter().map(|file| read_note(path, file)).collect()
}

/// Read one note of a vault.
pub fn read_note(vault: &Path, file: &Path) -> Result<NoteEntry, ObsidianError> {
    let metadata = fs::metadata(file).map_err(|source| ObsidianError::Metadata {
        path: file.to_path_buf(),
        source,
    })?;
    let modified = metadata
        .modified()
        .map_err(|source| ObsidianError::Metadata {
            path: file.to_path_buf(),
            source,
        })?;
    let title = file
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let content = fs::read_to_string(file).unwrap_or_default();
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    Ok(NoteEntry {
        source_path: file.strip_prefix(vault).unwrap_or(file).to_path_buf(),
        tags: extract_tags(content),
        wikilinks: wikilink_re()
            .captures_iter(content)
            .map(|captures| captures[1].trim().to_owned())
            .collect(),
        is_daily: parse_daily_note_date(&title).is_some(),
        daily_note_day: parse_daily_note_date(&title),
        title,
        content: content.to_owned(),
        modified: DateTime::<Utc>::from(modified),
    })
}

/// The transcript file a note segment carries.
pub const TRANSCRIPT_FILE: &str = "note_transcript.md";

/// Render a vault for saving. A note with no text has nothing to save and is left out.
///
/// Each note lands at the moment its file last changed, on the owner's local day.
pub fn render(
    path: &Path,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> Result<RenderedImport, ObsidianError> {
    Ok(render_notes(collect_notes(path)?, zone))
}

/// Render the given notes for saving, as a vault import or one sync pass renders them.
#[must_use]
pub fn render_notes(
    notes: Vec<NoteEntry>,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> RenderedImport {
    let notes = notes
        .into_iter()
        .filter(|note| !note.content.trim().is_empty())
        .collect::<Vec<_>>();
    let entry_count = u64::try_from(notes.len()).expect("note count fits u64");
    let stream = format!("import.{}", RegistrySource::Obsidian.name());
    let windows = window(
        notes
            .into_iter()
            .map(|note| (note.modified, note))
            .collect(),
        zone,
    );
    let mut files = Vec::with_capacity(windows.len());
    let mut items = Vec::new();
    for window in &windows {
        let mut contents = window
            .items
            .iter()
            .map(|(_, note)| note_markdown(note))
            .collect::<Vec<_>>()
            .join("\n\n");
        contents.push('\n');
        files.push(SegmentFile {
            day: window.day.clone(),
            segment: window.segment_key.clone(),
            name: TRANSCRIPT_FILE,
            contents,
        });
        for (_, note) in &window.items {
            let mut meta = Map::new();
            if !note.tags.is_empty() {
                meta.insert("tags".to_owned(), json!(note.tags));
            }
            if note.is_daily {
                meta.insert("is_daily".to_owned(), Value::Bool(true));
            }
            items.push(json!({
                "id": format!("note-{}", items.len()),
                "title": note.title,
                "date": window.day,
                "type": "note",
                "preview": note_preview(&note.content),
                "meta": meta,
                "segments": [{ "day": window.day, "key": window.segment_key, "stream": stream }],
            }));
        }
    }
    let days = windows
        .iter()
        .map(|window| window.day.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    RenderedImport {
        source: RegistrySource::Obsidian,
        files,
        items,
        entries: entry_count,
        summary: format!("imported {entry_count} notes across {days} days"),
    }
}

fn note_markdown(note: &NoteEntry) -> String {
    let mut lines = vec![format!("## {}", note.title)];
    lines.push(format!("Source: {}", note.source_path.display()));
    if !note.tags.is_empty() {
        lines.push(format!("Tags: {}", note.tags.join(", ")));
    }
    if !note.wikilinks.is_empty() {
        lines.push(format!(
            "Links: {}",
            note.wikilinks
                .iter()
                .map(|link| format!("[[{link}]]"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let body = strip_frontmatter(&note.content);
    let body = body.trim();
    if !body.is_empty() {
        lines.push(String::new());
        lines.push(body.to_owned());
    }
    lines.join("\n")
}

/// A plain-text preview of a note: markdown syntax removed, at most 200 characters.
fn note_preview(content: &str) -> String {
    let mut text = strip_frontmatter(content)
        .trim()
        .chars()
        .take(300)
        .collect::<String>();
    for (pattern, replacement) in preview_patterns() {
        text = pattern.replace_all(&text, *replacement).into_owned();
    }
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

fn strip_frontmatter(content: &str) -> std::borrow::Cow<'_, str> {
    frontmatter_re().replace(content, "")
}

fn preview_patterns() -> &'static [(Regex, &'static str)] {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            (r"(?m)^#{1,6}\s+", ""),
            (r"\*\*([^*]+)\*\*", "$1"),
            (r"\*([^*]+)\*", "$1"),
            (r"(?m)^[-*+]\s+", ""),
            (r"\[\[[^\]|]+\|([^\]]+)\]\]", "$1"),
            (r"\[\[([^\]|]+)\]\]", "$1"),
            (r"\[([^\]]+)\]\([^)]+\)", "$1"),
            (r"`([^`]+)`", "$1"),
            (r"(?m)^>\s+", ""),
        ]
        .into_iter()
        .map(|(pattern, replacement)| {
            (
                Regex::new(pattern).expect("valid note preview pattern"),
                replacement,
            )
        })
        .collect()
    })
}

/// Aggregate an Obsidian vault into the fixed import preview contract, on the zone's days.
pub fn preview(
    path: &Path,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> Result<ImportPreview, ObsidianError> {
    // Count what a save writes: a note with no text has nothing to save.
    let notes = collect_notes(path)?
        .into_iter()
        .filter(|note| !note.content.trim().is_empty())
        .collect::<Vec<_>>();
    if notes.is_empty() {
        return Ok(ImportPreview {
            date_range: (String::new(), String::new()),
            item_count: 0,
            entity_count: 0,
            summary: "Empty vault".to_owned(),
        });
    }

    let daily_count = notes.iter().filter(|note| note.is_daily).count();
    let knowledge_count = notes.len() - daily_count;
    let entity_count = notes
        .iter()
        .flat_map(|note| note.wikilinks.iter().map(String::as_str))
        .collect::<BTreeSet<_>>()
        .len();
    let mut days = notes
        .iter()
        .map(|note| day_key(note.modified, zone))
        .collect::<Vec<_>>();
    days.sort_unstable();
    let item_count = u64::try_from(notes.len()).expect("note count fits u64");
    let entity_count = u64::try_from(entity_count).expect("entity count fits u64");

    let mut parts = Vec::new();
    if daily_count > 0 {
        parts.push(format!("{daily_count} daily notes"));
    }
    if knowledge_count > 0 {
        parts.push(format!("{knowledge_count} knowledge notes"));
    }
    if entity_count > 0 {
        parts.push(format!("{entity_count} unique wikilinks"));
    }

    Ok(ImportPreview {
        date_range: (days[0].clone(), days[days.len() - 1].clone()),
        item_count,
        entity_count,
        summary: format!(
            "{}; date range reflects file modification time",
            parts.join(", ")
        ),
    })
}

fn visible_markdown_count(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !name.starts_with('.'))
        })
        .map(|path| {
            if path.is_dir() {
                visible_markdown_count(&path)
            } else {
                usize::from(has_lowercase_markdown_extension(&path))
            }
        })
        .sum()
}

fn walk_md_files(current: &Path, files: &mut Vec<PathBuf>) -> Result<(), ObsidianError> {
    let entries = fs::read_dir(current).map_err(|source| ObsidianError::ReadDirectory {
        path: current.to_path_buf(),
        source,
    })?;
    for entry in entries.filter_map(Result::ok) {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if should_skip_directory(current, name) {
                continue;
            }
            walk_md_files(&path, files)?;
        } else if !name.starts_with('.') && has_markdown_extension(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn should_skip_directory(parent: &Path, name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("templates")
        || name.eq_ignore_ascii_case("_templates")
        || (name == ".recycle" && parent.file_name().is_some_and(|parent| parent == "logseq"))
}

fn has_markdown_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn has_lowercase_markdown_extension(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("md")
}

fn parse_daily_note_date(title: &str) -> Option<String> {
    ["%Y-%m-%d", "%Y_%m_%d", "%Y%m%d"]
        .into_iter()
        .find_map(|format| NaiveDate::parse_from_str(title, format).ok())
        .map(|date| date.format("%Y%m%d").to_string())
}

fn extract_tags(content: &str) -> Vec<String> {
    let frontmatter = frontmatter_re().captures(content).map_or("", |captures| {
        captures.get(1).map_or("", |capture| capture.as_str())
    });
    if let Some(captures) = inline_tags_re().captures(frontmatter) {
        return captures[1]
            .split(',')
            .map(str::trim)
            .map(|tag| tag.trim_matches(['"', '\'']))
            .filter(|tag| !tag.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    let Some(tags_start) = frontmatter.find("tags:") else {
        return Vec::new();
    };
    list_tags_re()
        .captures_iter(&frontmatter[tags_start..])
        .map(|captures| captures[1].to_owned())
        .collect()
}

fn wikilink_re() -> &'static Regex {
    static WIKILINK_RE: OnceLock<Regex> = OnceLock::new();
    WIKILINK_RE.get_or_init(|| {
        Regex::new(r"\[\[([^\]|]+)(?:\|[^\]]+)?\]\]").expect("valid wikilink regex")
    })
}

fn frontmatter_re() -> &'static Regex {
    static FRONTMATTER_RE: OnceLock<Regex> = OnceLock::new();
    FRONTMATTER_RE.get_or_init(|| {
        Regex::new(r"(?s)\A---\s*\n(.*?)\n---\s*\n").expect("valid frontmatter regex")
    })
}

fn inline_tags_re() -> &'static Regex {
    static INLINE_TAGS_RE: OnceLock<Regex> = OnceLock::new();
    INLINE_TAGS_RE
        .get_or_init(|| Regex::new(r"(?m)^tags:\s*\[([^\]]*)\]").expect("valid tags regex"))
}

fn list_tags_re() -> &'static Regex {
    static LIST_TAGS_RE: OnceLock<Regex> = OnceLock::new();
    LIST_TAGS_RE.get_or_init(|| Regex::new(r"(?m)^  ?- (.+)$").expect("valid tags list regex"))
}
