// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Writing a rendered import into the journal.
//!
//! A source renders itself into segment files plus the rows an owner browses on
//! the Import page. This module writes both. Publication (stream binding and
//! indexing) and the attempt record are the caller's.

use std::fmt;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde_json::Value;
use solstone_core_import::RegistrySource;
use solstone_core_import::text::TextCreated;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use solstone_core_segment::{AiChatSource, ImportSource, Kind, StreamHints};

/// Private mode for everything an import writes about the owner.
const PRIVATE_FILE_MODE: u32 = 0o600;

/// One file written into one segment of the import's stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentFile {
    pub day: String,
    pub segment: String,
    pub name: &'static str,
    pub contents: String,
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
}

/// Render a source for saving, placed on the owner's local days.
///
/// `None` means the source has no text save path here.
pub fn render(
    source: RegistrySource,
    path: &Path,
    import_id: &str,
) -> Option<Result<RenderedImport, String>> {
    let conversations = |plan: Result<crate::ImportPlan, crate::SourceError>| {
        plan.map(|plan| crate::conversations::render(source, plan, import_id))
            .map_err(|error| error.to_string())
    };
    Some(match source {
        RegistrySource::Chatgpt => conversations(crate::chatgpt::plan(path, &Local)),
        RegistrySource::Claude => conversations(crate::claude::plan(path, &Local)),
        RegistrySource::Gemini => conversations(crate::gemini::plan(path, &Local)),
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
    for file in &rendered.files {
        let path = journal
            .join("chronicle")
            .join(&file.day)
            .join(&stream)
            .join(&file.segment)
            .join(file.name);
        if let Err(error) = write_private(&path, file.contents.as_bytes()) {
            return WriteOutcome {
                created,
                error: Some(error),
            };
        }
        created.push(TextCreated {
            day: file.day.clone(),
            segment: file.segment.clone(),
            stream: stream.clone(),
            hints: hints(rendered.source),
            path,
        });
    }
    let error = import_id.and_then(|import_id| {
        let path = journal
            .join("imports")
            .join(import_id)
            .join("content_manifest.jsonl");
        let mut rows = String::new();
        for item in &rendered.items {
            rows.push_str(&item.to_string());
            rows.push('\n');
        }
        write_private(&path, rows.as_bytes()).err()
    });
    WriteOutcome { created, error }
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
