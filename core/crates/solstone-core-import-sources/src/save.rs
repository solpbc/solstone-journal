// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Writing a rendered import into the journal.
//!
//! A source renders itself into segment files plus the rows an owner browses on
//! the Import page. This module writes both. Publication (stream binding and
//! indexing) and the attempt record are the caller's.

use std::fmt;
use std::path::{Path, PathBuf};

use chrono_tz::Tz;
use serde_json::Value;
use solstone_core_import::RegistrySource;
use solstone_core_import::text::TextCreated;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use solstone_core_segment::{AiChatSource, ImportSource, Kind, StreamHints};

/// Private mode for everything an import writes about the owner.
const PRIVATE_FILE_MODE: u32 = 0o600;

pub(crate) fn conversations_summary(messages: u64, conversations: usize, days: usize) -> String {
    format!("imported {messages} messages from {conversations} conversations across {days} days")
}

pub(crate) fn ics_summary(entry_count: u64, days: usize) -> String {
    format!("imported {entry_count} calendar events across {days} days")
}

pub(crate) fn obsidian_summary(entry_count: u64, days: usize) -> String {
    format!("imported {entry_count} notes across {days} days")
}

/// One file written into one segment of the import's stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentFile {
    pub day: String,
    pub segment: String,
    pub name: &'static str,
    pub contents: String,
    pub units: u64,
}

/// An import rendered from its source and ready to write.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedImport {
    pub source: RegistrySource,
    pub files: Vec<SegmentFile>,
    /// Rows of `content_manifest.jsonl`: what the owner browses on the Import page.
    pub items: Vec<Value>,
    /// The source's own units: messages, events or notes.
    pub entries: u64,
    pub summary: String,
}

impl RenderedImport {
    #[must_use]
    pub fn stream(&self) -> String {
        format!("import.{}", self.source.name())
    }
}

/// A file the save could not write.
#[derive(Debug)]
pub struct SaveError {
    pub path: PathBuf,
    pub detail: String,
}

impl fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not write {}: {}",
            self.path.display(),
            self.detail
        )
    }
}

impl std::error::Error for SaveError {}

/// What a save wrote, and the error that stopped it, if one did.
#[derive(Debug)]
pub struct WriteOutcome {
    pub created: Vec<TextCreated>,
    pub error: Option<SaveError>,
    pub skipped_deleted: Vec<(String, String)>,
    pub entries: u64,
    pub summary: String,
}

/// Render a source for saving, placed on the owner's local days in `zone`.
/// A time the source states with its own zone keeps it; a floating time is
/// read in `zone`.
///
/// `None` means the source has no text save path here.
pub fn render(
    source: RegistrySource,
    path: &Path,
    import_id: &str,
    zone: Tz,
) -> Option<Result<RenderedImport, String>> {
    let conversations = |plan: Result<crate::ImportPlan, crate::SourceError>| {
        plan.map(|plan| crate::conversations::render(source, plan, import_id))
            .map_err(|error| error.to_string())
    };
    Some(match source {
        RegistrySource::Chatgpt => conversations(crate::chatgpt::plan(path, &zone)),
        RegistrySource::Claude => conversations(crate::claude::plan(path, &zone)),
        RegistrySource::Gemini => conversations(crate::gemini::plan(path, &zone)),
        RegistrySource::Ics => crate::ics::render(path, &zone).map_err(|error| error.to_string()),
        RegistrySource::Obsidian => {
            crate::obsidian::render(path, &zone).map_err(|error| error.to_string())
        }
        _ => return None,
    })
}

/// Write every rendered segment file and, for an import with an id, its content manifest.
///
/// Segment keys are derived from the source's own timestamps, so saving the same source
/// again rewrites the same files rather than adding copies beside them.
#[must_use]
pub fn write_rendered(
    journal: &Path,
    import_id: Option<&str>,
    rendered: &RenderedImport,
) -> WriteOutcome {
    let stream = rendered.stream();
    let mut created = Vec::with_capacity(rendered.files.len());
    let mut skipped_deleted = Vec::new();
    let mut written_files = Vec::with_capacity(rendered.files.len());

    for file in &rendered.files {
        let segment_dir = journal
            .join("chronicle")
            .join(&file.day)
            .join(&stream)
            .join(&file.segment);

        match solstone_core_segment::owner_deleted(&segment_dir) {
            Ok(true) => {
                skipped_deleted.push((file.day.clone(), file.segment.clone()));
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                let path = segment_dir.join(file.name);
                return WriteOutcome {
                    created,
                    error: Some(SaveError {
                        path,
                        detail: error.to_string(),
                    }),
                    skipped_deleted,
                    entries: 0,
                    summary: String::new(),
                };
            }
        }

        let path = segment_dir.join(file.name);
        if let Err(error) = write_private(&path, file.contents.as_bytes()) {
            return WriteOutcome {
                created,
                error: Some(error),
                skipped_deleted,
                entries: 0,
                summary: String::new(),
            };
        }
        created.push(TextCreated {
            day: file.day.clone(),
            segment: file.segment.clone(),
            stream: stream.clone(),
            hints: hints(rendered.source),
            path,
        });
        written_files.push(file);
    }

    let filtered_items: Vec<&Value> = rendered
        .items
        .iter()
        .filter(|item| {
            if skipped_deleted.is_empty() {
                return true;
            }
            let Some(segments) = item.get("segments").and_then(Value::as_array) else {
                return false;
            };
            if segments.is_empty() {
                return false;
            }
            let mut valid_any = false;
            for seg in segments {
                let day = seg.get("day").and_then(Value::as_str);
                let key = seg.get("key").and_then(Value::as_str);
                match (day, key) {
                    (Some(d), Some(k)) => {
                        valid_any = true;
                        if skipped_deleted.iter().any(|(sd, sk)| sd == d && sk == k) {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
            valid_any
        })
        .collect();

    let (entries, summary) = if skipped_deleted.is_empty() {
        (rendered.entries, rendered.summary.clone())
    } else {
        let entries: u64 = written_files.iter().map(|f| f.units).sum();
        let distinct_days: std::collections::BTreeSet<&str> =
            written_files.iter().map(|f| f.day.as_str()).collect();
        let days_count = distinct_days.len();

        let summary = match rendered.source {
            RegistrySource::Chatgpt | RegistrySource::Claude | RegistrySource::Gemini => {
                let mut conversations_count = 0;
                for item in &rendered.items {
                    if let Some(segments) = item.get("segments").and_then(Value::as_array) {
                        let has_written_segment = segments.iter().any(|seg| {
                            let day = seg.get("day").and_then(Value::as_str);
                            let key = seg.get("key").and_then(Value::as_str);
                            if let (Some(d), Some(k)) = (day, key) {
                                written_files
                                    .iter()
                                    .any(|wf| wf.day == d && wf.segment == k)
                            } else {
                                false
                            }
                        });
                        if has_written_segment {
                            conversations_count += 1;
                        }
                    }
                }
                conversations_summary(entries, conversations_count, days_count)
            }
            RegistrySource::Ics => ics_summary(entries, days_count),
            RegistrySource::Obsidian => obsidian_summary(entries, days_count),
            _ => String::new(),
        };
        (entries, summary)
    };

    let error = import_id.and_then(|import_id| {
        let path = journal
            .join("imports")
            .join(import_id)
            .join("content_manifest.jsonl");
        let mut rows = String::new();
        for item in &filtered_items {
            rows.push_str(&item.to_string());
            rows.push('\n');
        }
        write_private(&path, rows.as_bytes()).err()
    });
    WriteOutcome {
        created,
        error,
        skipped_deleted,
        entries,
        summary,
    }
}

fn write_private(path: &Path, contents: &[u8]) -> Result<(), SaveError> {
    atomic_replace(
        path,
        contents,
        AtomicWriteOptions {
            mode: Some(PRIVATE_FILE_MODE),
        },
    )
    .map_err(|error| SaveError {
        path: path.to_owned(),
        detail: error.to_string(),
    })
}

/// Stream hints for an imported source stream.
pub fn stream_hints(source: RegistrySource) -> StreamHints {
    hints(source)
}

fn hints(source: RegistrySource) -> StreamHints {
    let source = match source {
        RegistrySource::Chatgpt => ImportSource::AiChat(AiChatSource::ChatGpt),
        RegistrySource::Claude => ImportSource::AiChat(AiChatSource::Claude),
        RegistrySource::Gemini => ImportSource::AiChat(AiChatSource::Gemini),
        other => ImportSource::Named(other.name().to_owned()),
    };
    StreamHints {
        kind: Some(Kind::Imported(source)),
        host: None,
        platform: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    fn dir_listing(dir: &Path) -> Vec<String> {
        if !dir.exists() {
            return vec![];
        }
        let mut entries: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        entries
    }

    #[test]
    fn write_rendered_respects_owner_deleted_and_filters_manifest() {
        let temporary = tempdir().unwrap();
        let root = temporary.path();

        let stream_dir = root.join("chronicle/20260804/import.ics");
        fs::create_dir_all(&stream_dir).unwrap();

        // A holds only tombstone.json
        let a_dir = stream_dir.join("seg_a");
        fs::create_dir_all(&a_dir).unwrap();
        fs::write(a_dir.join("tombstone.json"), b"{\"tombstone\":true}").unwrap();
        let a_bytes = fs::read(a_dir.join("tombstone.json")).unwrap();
        let a_listing = dir_listing(&a_dir);

        // B is absent and parent holds .removing_seg_b
        let b_dir = stream_dir.join("seg_b");
        let removing_b = stream_dir.join(".removing_seg_b");
        fs::create_dir_all(&removing_b).unwrap();
        fs::write(removing_b.join("tombstone.json"), b"{\"staged\":true}").unwrap();
        let removing_b_listing = dir_listing(&removing_b);

        // C is free
        let c_dir = stream_dir.join("seg_c");

        let rendered = RenderedImport {
            source: RegistrySource::Ics,
            files: vec![
                SegmentFile {
                    day: "20260804".to_owned(),
                    segment: "seg_a".to_owned(),
                    name: "calendar.ics",
                    contents: "event a\n".to_owned(),
                    units: 1,
                },
                SegmentFile {
                    day: "20260804".to_owned(),
                    segment: "seg_b".to_owned(),
                    name: "calendar.ics",
                    contents: "event b\n".to_owned(),
                    units: 2,
                },
                SegmentFile {
                    day: "20260804".to_owned(),
                    segment: "seg_c".to_owned(),
                    name: "calendar.ics",
                    contents: "event c\n".to_owned(),
                    units: 3,
                },
            ],
            items: vec![
                json!({
                    "id": "item-a",
                    "title": "Item A",
                    "segments": [{ "day": "20260804", "key": "seg_a" }],
                }),
                json!({
                    "id": "item-c",
                    "title": "Item C",
                    "segments": [{ "day": "20260804", "key": "seg_c" }],
                }),
                json!({
                    "id": "item-no-segments",
                    "title": "No Segments",
                }),
            ],
            entries: 999,
            summary: "sentinel_summary_value".to_owned(),
        };

        // Execution with deletions
        let outcome = write_rendered(root, Some("imp-1"), &rendered);
        assert_eq!(outcome.error.as_ref().map(|e| e.to_string()), None);
        assert_eq!(dir_listing(&a_dir), a_listing);
        assert_eq!(fs::read(a_dir.join("tombstone.json")).unwrap(), a_bytes);
        assert!(!b_dir.exists());
        assert_eq!(dir_listing(&removing_b), removing_b_listing);
        assert!(c_dir.join("calendar.ics").exists());
        assert_eq!(
            outcome.skipped_deleted,
            vec![
                ("20260804".to_owned(), "seg_a".to_owned()),
                ("20260804".to_owned(), "seg_b".to_owned()),
            ]
        );
        assert_eq!(outcome.created.len(), 1);
        assert_eq!(outcome.created[0].segment, "seg_c");
        assert_eq!(outcome.entries, 3);
        assert_eq!(outcome.summary, "imported 3 calendar events across 1 days");

        // Content manifest should contain only item-c; item-no-segments is dropped when anything was skipped
        let manifest_path = root.join("imports/imp-1/content_manifest.jsonl");
        let manifest_content = fs::read_to_string(&manifest_path).unwrap();
        assert!(!manifest_content.contains("item-a"));
        assert!(!manifest_content.contains("item-no-segments"));
        assert!(manifest_content.contains("item-c"));

        // Control: nothing deleted
        let temp_control = tempdir().unwrap();
        let control_root = temp_control.path();
        let control_outcome = write_rendered(control_root, Some("imp-ctrl"), &rendered);
        assert!(control_outcome.skipped_deleted.is_empty());
        assert_eq!(control_outcome.created.len(), 3);
        assert_eq!(control_outcome.entries, 999);
        assert_eq!(control_outcome.summary, "sentinel_summary_value");

        let ctrl_manifest =
            fs::read_to_string(control_root.join("imports/imp-ctrl/content_manifest.jsonl"))
                .unwrap();
        assert!(ctrl_manifest.contains("item-a"));
        assert!(ctrl_manifest.contains("item-c"));
        assert!(ctrl_manifest.contains("item-no-segments"));
    }

    #[cfg(unix)]
    #[test]
    fn write_rendered_fail_closed_on_os_path_error() {
        // A lowercase name "a".repeat(250) is a legal directory component, and
        // .removing_ plus that name exceeds NAME_MAX (255 bytes), so the staged probe
        // returns an error other than not-found.
        let temporary = tempdir().unwrap();
        let root = temporary.path();
        let stream_dir = root.join("chronicle/20260804/import.ics");
        fs::create_dir_all(&stream_dir).unwrap();
        let long_name = "a".repeat(250);
        let segment_dir = stream_dir.join(&long_name);

        assert!(solstone_core_segment::owner_deleted(&segment_dir).is_err());

        let rendered = RenderedImport {
            source: RegistrySource::Ics,
            files: vec![SegmentFile {
                day: "20260804".to_owned(),
                segment: long_name.clone(),
                name: "calendar.ics",
                contents: "events\n".to_owned(),
                units: 1,
            }],
            items: vec![],
            entries: 1,
            summary: "summary".to_owned(),
        };

        let outcome = write_rendered(root, Some("imp-fail"), &rendered);
        assert!(outcome.error.is_some());
        assert!(!segment_dir.exists());

        // Control layout writes successfully with normal name
        let control_rendered = RenderedImport {
            source: RegistrySource::Ics,
            files: vec![SegmentFile {
                day: "20260804".to_owned(),
                segment: "normal_seg".to_owned(),
                name: "calendar.ics",
                contents: "events\n".to_owned(),
                units: 1,
            }],
            items: vec![],
            entries: 1,
            summary: "summary".to_owned(),
        };
        let control_outcome = write_rendered(root, Some("imp-pass"), &control_rendered);
        assert!(control_outcome.error.is_none());
        assert!(
            root.join("chronicle/20260804/import.ics/normal_seg/calendar.ics")
                .exists()
        );
    }
}
