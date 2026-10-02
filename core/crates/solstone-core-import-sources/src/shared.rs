// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared read-only import planning types and local-day windowing.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
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
    /// Title of each conversation, indexed by [`PlannedEntry::thread`].
    pub threads: Vec<String>,
}

/// One five-minute segment on the owner's local day.
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
    /// The conversation this entry came from.
    pub thread: usize,
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
    pub thread: usize,
}

#[derive(Eq, PartialEq)]
pub(crate) enum SourcePathKind {
    File,
    Directory,
    Other,
}

pub(crate) fn plan_entries(
    entries: Vec<ParsedEntry>,
    skipped: Vec<SkippedEntry>,
    threads: Vec<String>,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> ImportPlan {
    let item_count = u64::try_from(entries.len()).unwrap_or(u64::MAX);
    let windows = window(
        entries
            .into_iter()
            .map(|entry| (entry.timestamp, entry))
            .collect(),
        zone,
    );
    let date_range = match (windows.first(), windows.last()) {
        (Some(first), Some(last)) => (first.day.clone(), last.day.clone()),
        _ => (String::new(), String::new()),
    };
    let segments = windows
        .into_iter()
        .map(|window| {
            let model_slug = window
                .items
                .iter()
                .find_map(|(_, entry)| entry.model_slug.clone());
            PlannedSegment {
                entries: window
                    .items
                    .into_iter()
                    .map(|(timestamp, entry)| PlannedEntry {
                        start: clock_offset(timestamp - window.start),
                        speaker: entry.speaker,
                        text: entry.text,
                        thread: entry.thread,
                    })
                    .collect(),
                day: window.day,
                segment_key: window.segment_key,
                model_slug,
            }
        })
        .collect::<Vec<_>>();
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
        threads,
    }
}

/// Items that share one five-minute segment of one local day.
pub(crate) struct Window<T> {
    pub day: String,
    pub segment_key: String,
    pub start: DateTime<Utc>,
    pub items: Vec<(DateTime<Utc>, T)>,
}

pub(crate) const WINDOW_SECONDS: i64 = 300;

/// Group timestamped items into five-minute segments of the owner's local days.
///
/// A window opens at its first item and closes when an item falls on another
/// local day or five minutes or more after that first item.
pub(crate) fn window<T>(
    mut items: Vec<(DateTime<Utc>, T)>,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> Vec<Window<T>> {
    items.sort_by_key(|(timestamp, _)| *timestamp);
    let mut windows: Vec<Window<T>> = Vec::new();
    for (timestamp, item) in items {
        let day = day_key(timestamp, zone);
        let open = windows.last_mut().filter(|window| {
            window.day == day && (timestamp - window.start).num_seconds() < WINDOW_SECONDS
        });
        match open {
            Some(window) => window.items.push((timestamp, item)),
            None => windows.push(Window {
                segment_key: format!(
                    "{}_{WINDOW_SECONDS}",
                    timestamp.with_timezone(zone).format("%H%M%S")
                ),
                day,
                start: timestamp,
                items: vec![(timestamp, item)],
            }),
        }
    }
    windows
}

fn clock_offset(offset: chrono::TimeDelta) -> String {
    let offset = offset.num_seconds().max(0);
    format!(
        "{:02}:{:02}:{:02}",
        offset / 3600,
        (offset % 3600) / 60,
        offset % 60
    )
}

pub(crate) fn day_key(
    timestamp: DateTime<Utc>,
    zone: &impl TimeZone<Offset: fmt::Display>,
) -> String {
    timestamp.with_timezone(zone).format("%Y%m%d").to_string()
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

pub(crate) fn source_path_kind(path: &Path) -> Result<SourcePathKind, SourceError> {
    let metadata = path
        .metadata()
        .map_err(|error| source_io(path, "inspect source", error))?;
    if metadata.is_file() {
        Ok(SourcePathKind::File)
    } else if metadata.is_dir() {
        Ok(SourcePathKind::Directory)
    } else {
        Ok(SourcePathKind::Other)
    }
}

pub(crate) fn skip_conversation(
    skipped: &mut Vec<SkippedEntry>,
    conversation_index: usize,
    reason: SkipReason,
) {
    skipped.push(SkippedEntry {
        locator: SkipLocator::Conversation {
            conversation_index,
            message_index: None,
        },
        reason,
    });
}

pub(crate) fn skip_message(
    skipped: &mut Vec<SkippedEntry>,
    conversation_index: usize,
    message_index: usize,
    reason: SkipReason,
) {
    skipped.push(SkippedEntry {
        locator: SkipLocator::Conversation {
            conversation_index,
            message_index: Some(message_index),
        },
        reason,
    });
}

/// Open an installed file only to set its modified time.
///
/// Windows refuses `SetFileTime` on a read-only handle (access denied), so a
/// plain `File::open` there fails every install that keeps the source's time.
/// Ask for `FILE_WRITE_ATTRIBUTES` alone: it changes no content and opens no
/// write stream. Elsewhere a read-only handle is enough.
pub(crate) fn open_to_set_times(path: &Path) -> std::io::Result<File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
        std::fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .open(path)
    }
    #[cfg(not(windows))]
    {
        File::open(path)
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

pub(crate) fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

#[cfg(all(test, windows))]
mod windows_set_times_tests {
    use std::fs::{self, File};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn an_installed_file_keeps_the_source_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.pdf");
        fs::write(&path, b"%PDF-1.4").unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        // The control: a read-only handle cannot set the time on Windows.
        assert!(
            File::open(&path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .is_err()
        );

        super::open_to_set_times(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_eq!(fs::read(&path).unwrap(), b"%PDF-1.4");
    }
}
