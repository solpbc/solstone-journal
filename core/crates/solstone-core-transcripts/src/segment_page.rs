// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded transcript-only reads for one chronicle segment.

//! This module intentionally does not share the broad day-clustering surface:
//! it has no percept, browser, or talent selection flags.

use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use solstone_core_journal_io::paths::Segment;

/// Largest source-byte window parsed for one transcript page.
pub const MAX_TRANSCRIPT_PAGE_BYTES: usize = 64 * 1024;
/// Largest number of transcript entries returned for one transcript page.
pub const MAX_TRANSCRIPT_PAGE_ITEMS: usize = 100;

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
    pub version: SegmentTranscriptVersion,
}

/// One projected transcript entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptEntry {
    pub text: String,
}

/// One bounded page from a single segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptPage {
    pub entries: Vec<SegmentTranscriptEntry>,
    pub next: Option<SegmentTranscriptCursor>,
    pub version: SegmentTranscriptVersion,
}

/// Failure while reading a bounded transcript page.
#[derive(Debug)]
pub enum SegmentTranscriptReadError {
    Changed,
    InvalidCursor,
    Io,
    RecordTooLarge,
}

/// Read one bounded page of approved transcript text from exactly one segment.
///
/// The caller owns authorization. This function only projects transcript input;
/// it never walks a day, reads percept/browser/talent sources, or applies scope.
pub fn read_segment_transcript_page(
    segment: &Segment,
    cursor: Option<&SegmentTranscriptCursor>,
) -> Result<SegmentTranscriptPage, SegmentTranscriptReadError> {
    let files = transcript_files(segment.path())?;
    let version = source_version(&files)?;
    let (mut source_index, mut byte_offset) = match cursor {
        Some(cursor) if cursor.version != version => {
            return Err(SegmentTranscriptReadError::Changed);
        }
        Some(cursor) if cursor.source_index > files.len() => {
            return Err(SegmentTranscriptReadError::InvalidCursor);
        }
        Some(cursor) => (cursor.source_index, cursor.byte_offset),
        None => (0, 0),
    };

    // Read the sidecar through the same bound. Labels are deliberately local to
    // this projection; no speaker-label storage API is widened for connection reads.
    let _labels = bounded_labels(segment.path());

    let mut entries = Vec::new();
    while source_index < files.len() && entries.len() < MAX_TRANSCRIPT_PAGE_ITEMS {
        let path = &files[source_index];
        let (window, advanced, exhausted) = read_window(path, byte_offset)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let mut consumed = 0;
        let mut stopped_in_window = false;
        for (line, line_end) in complete_lines(&window)? {
            let text = project_line(name, line);
            if !text.trim().is_empty() {
                entries.push(SegmentTranscriptEntry { text });
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
        }
        if advanced == 0 && !exhausted {
            return Err(SegmentTranscriptReadError::RecordTooLarge);
        }
    }

    let next = (source_index < files.len()).then(|| SegmentTranscriptCursor {
        source_index,
        byte_offset,
        version: version.clone(),
    });
    Ok(SegmentTranscriptPage {
        entries,
        next,
        version,
    })
}

fn transcript_files(segment: &Path) -> Result<Vec<PathBuf>, SegmentTranscriptReadError> {
    let mut files = fs::read_dir(segment)
        .map_err(|_| SegmentTranscriptReadError::Io)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            (path.is_file() && is_transcript_name(name)).then_some(path)
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn is_transcript_name(name: &str) -> bool {
    name.ends_with("_transcript.jsonl") || name == "imported.md" || name.ends_with("_transcript.md")
}

fn source_version(
    files: &[PathBuf],
) -> Result<SegmentTranscriptVersion, SegmentTranscriptReadError> {
    let mut hasher = DefaultHasher::new();
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

fn bounded_labels(segment: &Path) -> Option<Vec<u8>> {
    let path = segment.join("talents/speaker_labels.json");
    let metadata = fs::metadata(&path).ok()?;
    if metadata.len() > MAX_TRANSCRIPT_PAGE_BYTES as u64 {
        return None;
    }
    fs::read(path).ok()
}

fn read_window(
    path: &Path,
    offset: u64,
) -> Result<(Vec<u8>, usize, bool), SegmentTranscriptReadError> {
    let mut file = File::open(path).map_err(|_| SegmentTranscriptReadError::Io)?;
    let length = file
        .metadata()
        .map_err(|_| SegmentTranscriptReadError::Io)?
        .len();
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

fn project_line(_name: &str, line: &str) -> String {
    line.to_owned()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        MAX_TRANSCRIPT_PAGE_BYTES, MAX_TRANSCRIPT_PAGE_ITEMS, read_segment_transcript_page,
    };
    use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

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

        let first = read_segment_transcript_page(&segment, None).unwrap();
        assert!(!first.entries.is_empty());
        assert!(
            first
                .entries
                .iter()
                .all(|entry| entry.text == "approved transcript line")
        );
        assert!(first.next.is_some());
    }

    #[test]
    fn transcript_page_excludes_percept_media_browser_and_talent_sources() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("meeting_transcript.md"),
            "approved transcript line\n",
        )
        .unwrap();
        fs::write(segment.join("audio.jsonl"), "audio percept\n").unwrap();
        fs::write(segment.join("screen.jsonl"), "screen percept\n").unwrap();
        fs::write(segment.join("browser_history.jsonl"), "browser percept\n").unwrap();
        fs::write(segment.join("talents/brief.md"), "secret talent\n").unwrap();
        fs::write(segment.join("talents/sense.json"), "secret sense\n").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let page = read_segment_transcript_page(&segment, None).unwrap();
        assert_eq!(
            page.entries,
            vec![super::SegmentTranscriptEntry {
                text: "approved transcript line".to_owned(),
            }]
        );
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

        let first = read_segment_transcript_page(&segment, None).unwrap();
        assert_eq!(first.entries.len(), MAX_TRANSCRIPT_PAGE_ITEMS);
        assert_eq!(first.entries[0].text, "line 1");
        assert_eq!(
            first.entries[MAX_TRANSCRIPT_PAGE_ITEMS - 1].text,
            "line 100"
        );

        let second = read_segment_transcript_page(&segment, first.next.as_ref()).unwrap();
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
}
