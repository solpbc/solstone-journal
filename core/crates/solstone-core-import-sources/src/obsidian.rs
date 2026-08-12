// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Obsidian vault source parsing.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Local, NaiveDate};
use regex::Regex;
use solstone_core_import::ImportPreview;

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
    pub day: String,
    pub inferred_entity_type: Option<String>,
}

/// A later-writer-ready Obsidian entity projection.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ObsidianEntity {
    pub name: String,
    pub entity_type: String,
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

    let mut notes = Vec::new();
    for file in files {
        let metadata = fs::metadata(&file).map_err(|source| ObsidianError::Metadata {
            path: file.clone(),
            source,
        })?;
        let modified = metadata
            .modified()
            .map_err(|source| ObsidianError::Metadata {
                path: file.clone(),
                source,
            })?;
        let day = DateTime::<Local>::from(modified)
            .format("%Y%m%d")
            .to_string();
        let title = file
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let content = fs::read_to_string(&file).unwrap_or_default();
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);

        notes.push(NoteEntry {
            source_path: file.strip_prefix(path).unwrap_or(&file).to_path_buf(),
            tags: extract_tags(content),
            wikilinks: wikilink_re()
                .captures_iter(content)
                .map(|captures| captures[1].trim().to_owned())
                .collect(),
            is_daily: parse_daily_note_date(&title).is_some(),
            daily_note_day: parse_daily_note_date(&title),
            inferred_entity_type: infer_entity_type_from_path(&file, path),
            title,
            content: content.to_owned(),
            day,
        });
    }
    Ok(notes)
}

/// Aggregate an Obsidian vault into the fixed import preview contract.
pub fn preview(path: &Path) -> Result<ImportPreview, ObsidianError> {
    let notes = collect_notes(path)?;
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
        .map(|note| note.day.as_str())
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
        date_range: (days[0].to_owned(), days[days.len() - 1].to_owned()),
        item_count,
        entity_count,
        summary: format!(
            "{}; date range reflects file modification time",
            parts.join(", ")
        ),
    })
}

/// Project wikilinks and `@` note names to deterministic entity facts.
#[must_use]
pub fn wikilink_entities(notes: &[NoteEntry]) -> Vec<ObsidianEntity> {
    let entity_types = notes
        .iter()
        .filter_map(|note| {
            note.inferred_entity_type
                .as_ref()
                .map(|entity_type| (note.title.as_str(), entity_type.as_str()))
        })
        .collect::<BTreeMap<_, _>>();
    let mut entities = BTreeMap::new();

    for note in notes {
        for link in &note.wikilinks {
            let (name, is_person) = clean_at_prefix(link);
            if name.is_empty() {
                continue;
            }
            let entity_type = if is_person {
                "Person"
            } else {
                entity_types.get(name).copied().unwrap_or("Topic")
            };
            let should_replace = entities
                .get(name)
                .is_none_or(|existing: &&str| is_person && *existing != "Person");
            if should_replace {
                entities.insert(name.to_owned(), entity_type);
            }
        }
    }
    for note in notes {
        let (name, is_person) = clean_at_prefix(&note.title);
        if is_person && !name.is_empty() {
            entities.entry(name.to_owned()).or_insert("Person");
        }
    }

    entities
        .into_iter()
        .map(|(name, entity_type)| ObsidianEntity {
            name,
            entity_type: entity_type.to_owned(),
        })
        .collect()
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

fn infer_entity_type_from_path(path: &Path, root: &Path) -> Option<String> {
    let parent = path.strip_prefix(root).ok()?.parent()?;
    for component in parent.components() {
        let folder = numeric_prefix_re()
            .replace(component.as_os_str().to_string_lossy().as_ref(), "")
            .to_ascii_lowercase();
        let entity_type = match folder.as_str() {
            "people" | "contacts" => "Person",
            "projects" => "Project",
            "organizations" | "companies" => "Organization",
            "places" | "locations" => "Place",
            _ => continue,
        };
        return Some(entity_type.to_owned());
    }
    None
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

fn clean_at_prefix(name: &str) -> (&str, bool) {
    name.strip_prefix('@')
        .map_or((name, false), |name| (name.trim_start(), true))
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

fn numeric_prefix_re() -> &'static Regex {
    static NUMERIC_PREFIX_RE: OnceLock<Regex> = OnceLock::new();
    NUMERIC_PREFIX_RE.get_or_init(|| Regex::new(r"^\d+\s+").expect("valid numeric prefix regex"))
}
