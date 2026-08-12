// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Kindle clippings detection, preview, and planning.

use std::path::Path;

use chrono::{DateTime, NaiveDateTime, Utc};
use solstone_core_import::ImportPreview;

use crate::shared::{ParsedEntry, SourcePathKind, has_extension, plan_entries, source_path_kind};
use crate::{ImportPlan, SkipLocator, SkipReason, SkippedEntry, SourceError};

const DELIMITER: &str = "==========";
const DATE_FORMATS: [&str; 4] = [
    "%A, %B %d, %Y %I:%M:%S %p",
    "%A, %d %B %Y %H:%M:%S",
    "%A %d %B %Y %H:%M:%S",
    "%A, %B %d, %Y %I:%M %p",
];

/// Detect a Kindle clippings file from its text structure.
pub fn detect(path: &Path) -> Result<bool, SourceError> {
    if source_path_kind(path)? != SourcePathKind::File || !has_extension(path, "txt") {
        return Ok(false);
    }
    let text = read_text(path)?;
    if !text.contains(DELIMITER) {
        return Ok(false);
    }
    Ok(text
        .split(DELIMITER)
        .take(5)
        .any(|block| clip_type(block).is_some()))
}

/// Preview the atomic clipping count, Kindle entity count, and UTC date range.
pub fn preview(path: &Path) -> Result<ImportPreview, SourceError> {
    let parsed = parse_clippings(path)?;
    let plan = plan_from_parsed(&parsed);
    let mut books = parsed
        .records
        .iter()
        .map(|record| record.book_title.as_str())
        .collect::<Vec<_>>();
    books.sort_unstable();
    books.dedup();
    let mut authors = parsed
        .records
        .iter()
        .map(|record| record.author.as_str())
        .filter(|author| !author.is_empty())
        .collect::<Vec<_>>();
    authors.sort_unstable();
    authors.dedup();
    let mut type_counts = Vec::<(String, u64)>::new();
    for record in &parsed.records {
        if let Some((_, count)) = type_counts
            .iter_mut()
            .find(|(kind, _)| kind == &record.clip_type)
        {
            *count += 1;
        } else {
            type_counts.push((record.clip_type.clone(), 1));
        }
    }
    type_counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let kinds = type_counts
        .iter()
        .map(|(kind, count)| format!("{count} {kind}s"))
        .collect::<Vec<_>>()
        .join(", ");
    let summary = if parsed.records.is_empty() {
        "0 highlights from 0 books".to_owned()
    } else {
        format!("{kinds} from {} books", books.len())
    };
    Ok(ImportPreview {
        date_range: plan.date_range,
        item_count: plan.item_count,
        entity_count: u64::try_from(books.len() + authors.len()).unwrap_or(u64::MAX),
        summary,
    })
}

/// Parse Kindle clippings into a write-free UTC segment plan.
pub fn plan(path: &Path) -> Result<ImportPlan, SourceError> {
    let parsed = parse_clippings(path)?;
    Ok(plan_from_parsed(&parsed))
}

struct ParsedClippings {
    records: Vec<ClippingRecord>,
    skipped: Vec<SkippedEntry>,
}

struct ClippingRecord {
    timestamp: DateTime<Utc>,
    book_title: String,
    author: String,
    content: String,
    clip_type: String,
}

fn parse_clippings(path: &Path) -> Result<ParsedClippings, SourceError> {
    if source_path_kind(path)? != SourcePathKind::File {
        return Err(SourceError::UnsupportedPathKind {
            path: path.to_owned(),
        });
    }
    if !has_extension(path, "txt") {
        return Err(SourceError::UnsupportedExtension {
            path: path.to_owned(),
        });
    }
    let text = read_text(path)?;
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    for (zero_based_index, block) in text.split(DELIMITER).enumerate() {
        if block.trim().is_empty() {
            continue;
        }
        match parse_block(block) {
            Ok(record) => records.push(record),
            Err(reason) => skipped.push(SkippedEntry {
                locator: SkipLocator::ClippingBlock {
                    clipping_block_index: zero_based_index + 1,
                },
                reason,
            }),
        }
    }
    Ok(ParsedClippings { records, skipped })
}

fn plan_from_parsed(parsed: &ParsedClippings) -> ImportPlan {
    let entries = parsed
        .records
        .iter()
        .map(|record| ParsedEntry {
            timestamp: record.timestamp,
            speaker: "Kindle".to_owned(),
            text: record.content.clone(),
            model_slug: None,
        })
        .collect();
    plan_entries(entries, parsed.skipped.clone())
}

fn read_text(path: &Path) -> Result<String, SourceError> {
    let bytes = std::fs::read(path).map_err(|error| SourceError::Io {
        path: path.to_owned(),
        operation: "read source",
        source: error,
    })?;
    let text = String::from_utf8(bytes).map_err(|error| SourceError::TextDecode {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
}

fn parse_block(block: &str) -> Result<ClippingRecord, SkipReason> {
    let lines = block.trim().split('\n').collect::<Vec<_>>();
    if lines.len() < 2 {
        return Err(SkipReason::InsufficientClippingLines);
    }
    let title_line = lines[0].trim().trim_start_matches('\u{feff}');
    if title_line.is_empty() {
        return Err(SkipReason::EmptyClippingTitle);
    }
    let (book_title, author) = title_author(title_line);
    let metadata = lines[1].trim();
    let clip_type = clip_type(metadata).unwrap_or("highlight").to_lowercase();
    let Some(date) = metadata
        .split_once("Added on")
        .and_then(|(_, date)| parse_date(date.trim()))
    else {
        return Err(SkipReason::InvalidClippingDate);
    };
    let content_lines = if lines.get(2).is_some_and(|line| line.trim().is_empty()) {
        &lines[3..]
    } else {
        &lines[2..]
    };
    let content = content_lines.join("\n").trim().to_owned();
    if clip_type == "bookmark" && content.is_empty() {
        return Err(SkipReason::EmptyBookmark);
    }
    Ok(ClippingRecord {
        timestamp: date.and_utc(),
        book_title,
        author,
        content,
        clip_type,
    })
}

fn title_author(title: &str) -> (String, String) {
    let Some(close) = title.strip_suffix(')') else {
        return (title.trim().to_owned(), String::new());
    };
    let Some(open) = close.rfind('(') else {
        return (title.trim().to_owned(), String::new());
    };
    let book = close[..open].trim();
    let author = close[open + 1..].trim();
    if book.is_empty() || author.is_empty() {
        (title.trim().to_owned(), String::new())
    } else {
        (book.to_owned(), author.to_owned())
    }
}

fn clip_type(value: &str) -> Option<&str> {
    let lower = value.to_lowercase();
    let words = lower.split_whitespace().collect::<Vec<_>>();
    words.windows(2).find_map(|words| match words {
        ["your", "highlight"] => Some("highlight"),
        ["your", "note"] => Some("note"),
        ["your", "bookmark"] => Some("bookmark"),
        _ => None,
    })
}

fn parse_date(value: &str) -> Option<NaiveDateTime> {
    DATE_FORMATS
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
}
