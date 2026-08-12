// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared read-only import planning types and UTC windowing.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use zip::ZipArchive;

/// A complete read-only import plan.
#[derive(Debug, Eq, PartialEq)]
pub struct ImportPlan {
    pub segments: Vec<PlannedSegment>,
    pub affected_days: Vec<String>,
    pub item_count: u64,
    pub date_range: (String, String),
    pub skipped: Vec<SkippedEntry>,
}

/// One UTC five-minute segment ready for a later staging wave.
#[derive(Debug, Eq, PartialEq)]
pub struct PlannedSegment {
    pub day: String,
    pub segment_key: String,
    pub model_slug: Option<String>,
    pub entries: Vec<PlannedEntry>,
}

/// One rendered entry in a planned segment.
#[derive(Debug, Eq, PartialEq)]
pub struct PlannedEntry {
    pub start: String,
    pub speaker: String,
    pub text: String,
}

/// A source-local locator for an entry that was not planned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkipLocator {
    Conversation {
        conversation_index: usize,
        message_index: Option<usize>,
    },
    Activity {
        activity_index: usize,
    },
    ClippingBlock {
        clipping_block_index: usize,
    },
}

/// A non-fatal reason an otherwise valid source entry could not be imported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkipReason {
    EmptyConversation,
    EmptyMessageText,
    NoUsableTimestamp,
    NoImportableConversationContent,
    MissingConversationMapping,
    InvalidConversationPath,
    UnsupportedMessageRole,
    EmptyMessageContent,
    InvalidMessageTimestamp,
    NoActivityContent,
    MissingActivityTimestamp,
    InvalidActivityTimestamp,
    InsufficientClippingLines,
    EmptyClippingTitle,
    InvalidClippingDate,
    EmptyBookmark,
}

/// A non-fatal source entry that was skipped by the parser.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedEntry {
    pub locator: SkipLocator,
    pub reason: SkipReason,
}

/// Named failures at the source boundary.
#[derive(Debug)]
pub enum SourceError {
    Io {
        path: PathBuf,
        operation: &'static str,
        source: std::io::Error,
    },
    UnsupportedPathKind {
        path: PathBuf,
    },
    UnsupportedExtension {
        path: PathBuf,
    },
    ArchiveOpen {
        path: PathBuf,
        message: String,
    },
    ArchiveMemberMissing {
        path: PathBuf,
        member: &'static str,
    },
    ArchiveMemberRead {
        path: PathBuf,
        member: &'static str,
        message: String,
    },
    InvalidJson {
        path: PathBuf,
        context: &'static str,
        message: String,
    },
    InvalidJsonShape {
        path: PathBuf,
        context: &'static str,
    },
    TextDecode {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                path, operation, ..
            } => write!(formatter, "could not {operation}: {}", path.display()),
            Self::UnsupportedPathKind { path } => {
                write!(
                    formatter,
                    "unsupported source path kind: {}",
                    path.display()
                )
            }
            Self::UnsupportedExtension { path } => {
                write!(
                    formatter,
                    "unsupported source extension: {}",
                    path.display()
                )
            }
            Self::ArchiveOpen { path, message } => {
                write!(
                    formatter,
                    "could not open archive {}: {message}",
                    path.display()
                )
            }
            Self::ArchiveMemberMissing { path, member } => write!(
                formatter,
                "archive member {member} is missing from {}",
                path.display()
            ),
            Self::ArchiveMemberRead {
                path,
                member,
                message,
            } => write!(
                formatter,
                "could not read archive member {member} from {}: {message}",
                path.display()
            ),
            Self::InvalidJson {
                path,
                context,
                message,
            } => write!(
                formatter,
                "invalid JSON for {context} in {}: {message}",
                path.display()
            ),
            Self::InvalidJsonShape { path, context, .. } => write!(
                formatter,
                "invalid JSON shape for {context} in {}",
                path.display()
            ),
            Self::TextDecode { path, message } => {
                write!(
                    formatter,
                    "could not decode text source {}: {message}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) struct ParsedEntry {
    pub timestamp: DateTime<Utc>,
    pub speaker: String,
    pub text: String,
    pub model_slug: Option<String>,
}

pub(crate) fn plan_entries(
    mut entries: Vec<ParsedEntry>,
    skipped: Vec<SkippedEntry>,
) -> ImportPlan {
    entries.sort_by_key(|entry| entry.timestamp);
    let date_range = match (entries.first(), entries.last()) {
        (Some(first), Some(last)) => (day_key(first.timestamp), day_key(last.timestamp)),
        _ => (String::new(), String::new()),
    };
    let item_count = u64::try_from(entries.len()).unwrap_or(u64::MAX);
    let segments = window_entries(entries);
    let mut affected_days = segments
        .iter()
        .map(|segment| segment.day.clone())
        .collect::<Vec<_>>();
    affected_days.sort();
    affected_days.dedup();
    ImportPlan {
        segments,
        affected_days,
        item_count,
        date_range,
        skipped,
    }
}

fn window_entries(entries: Vec<ParsedEntry>) -> Vec<PlannedSegment> {
    let mut segments = Vec::new();
    let mut current: Option<Window> = None;
    for entry in entries {
        let day = day_key(entry.timestamp);
        let starts_new_window = current.as_ref().is_none_or(|window| {
            window.day != day || (entry.timestamp - window.start).num_seconds() >= 300
        });
        if starts_new_window {
            if let Some(window) = current.take() {
                segments.push(window.into_segment());
            }
            current = Some(Window::new(day, entry.timestamp));
        }
        if let Some(window) = current.as_mut() {
            window.push(entry);
        }
    }
    if let Some(window) = current {
        segments.push(window.into_segment());
    }
    segments
}

struct Window {
    day: String,
    start: DateTime<Utc>,
    model_slug: Option<String>,
    entries: Vec<PlannedEntry>,
}

impl Window {
    fn new(day: String, start: DateTime<Utc>) -> Self {
        Self {
            day,
            start,
            model_slug: None,
            entries: Vec::new(),
        }
    }

    fn push(&mut self, entry: ParsedEntry) {
        let offset = (entry.timestamp - self.start).num_seconds().max(0);
        let hours = offset / 3600;
        let minutes = (offset % 3600) / 60;
        let seconds = offset % 60;
        if self.model_slug.is_none() {
            self.model_slug = entry.model_slug;
        }
        self.entries.push(PlannedEntry {
            start: format!("{hours:02}:{minutes:02}:{seconds:02}"),
            speaker: entry.speaker,
            text: entry.text,
        });
    }

    fn into_segment(self) -> PlannedSegment {
        PlannedSegment {
            day: self.day,
            segment_key: format!("{}_300", self.start.format("%H%M%S")),
            model_slug: self.model_slug,
            entries: self.entries,
        }
    }
}

pub(crate) fn day_key(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y%m%d").to_string()
}

pub(crate) fn source_io(
    path: &Path,
    operation: &'static str,
    source: std::io::Error,
) -> SourceError {
    SourceError::Io {
        path: path.to_owned(),
        operation,
        source,
    }
}

pub(crate) fn read_json_file(path: &Path, context: &'static str) -> Result<Value, SourceError> {
    let bytes = std::fs::read(path).map_err(|error| source_io(path, "read source", error))?;
    serde_json::from_slice(&bytes).map_err(|error| SourceError::InvalidJson {
        path: path.to_owned(),
        context,
        message: error.to_string(),
    })
}

pub(crate) fn read_zip_json(
    path: &Path,
    member: &'static str,
    context: &'static str,
) -> Result<Value, SourceError> {
    let bytes =
        read_zip_member(path, member)?.ok_or_else(|| SourceError::ArchiveMemberMissing {
            path: path.to_owned(),
            member,
        })?;
    serde_json::from_slice(&bytes).map_err(|error| SourceError::InvalidJson {
        path: path.to_owned(),
        context,
        message: error.to_string(),
    })
}

pub(crate) fn read_zip_member(
    path: &Path,
    member: &'static str,
) -> Result<Option<Vec<u8>>, SourceError> {
    let file = File::open(path).map_err(|error| source_io(path, "open source", error))?;
    let mut archive = ZipArchive::new(file).map_err(|error| SourceError::ArchiveOpen {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    let mut entry = match archive.by_name(member) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => {
            return Err(SourceError::ArchiveMemberRead {
                path: path.to_owned(),
                member,
                message: error.to_string(),
            });
        }
    };
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|error| SourceError::ArchiveMemberRead {
            path: path.to_owned(),
            member,
            message: error.to_string(),
        })?;
    Ok(Some(bytes))
}

pub(crate) fn parse_iso_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f"))
                .ok()
                .map(|timestamp| timestamp.and_utc())
        })
}

pub(crate) fn is_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|metadata| metadata.is_file())
}

pub(crate) fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}
