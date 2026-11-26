// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Connection admission for segment topics. The owner's index category is
//! independent of the source permissions required to serve the topic.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{JournalRoot, read_relative_file_bounded};

use super::{AdmittedCategory, Family, produce_chunks};

pub const PROVENANCE_FILE: &str = "topics.provenance.json";
/// Set by the Sense writer on validated output, never by the model.
pub const SENSE_INPUT_KINDS: &str = "_topic_input_kinds";

/// The source projections Sense can load in a topic-producing segment:
/// speech and conversation, screen and browser text, imported notes,
/// documents, calendar and body prose, and photo descriptions.
pub const POSSIBLE_INPUTS: &[AdmittedCategory] = {
    use AdmittedCategory::*;
    &[
        Transcripts,
        Messages,
        ScreenText,
        Body,
        Notes,
        Documents,
        Calendar,
        Photos,
    ]
};

pub fn kind_words(kinds: &BTreeSet<AdmittedCategory>) -> Vec<&'static str> {
    kinds.iter().map(|kind| kind.as_str()).collect()
}

pub fn parse_kinds<'a>(
    words: impl IntoIterator<Item = &'a str>,
) -> Option<BTreeSet<AdmittedCategory>> {
    words
        .into_iter()
        .map(|word| {
            AdmittedCategory::from_stored_word(word).filter(|kind| POSSIBLE_INPUTS.contains(kind))
        })
        .collect()
}

/// Read strictly on purpose. A record this version can't read whole, one a
/// later version wrote with another field included, verifies nothing here:
/// the topic then needs every source permission a topic can draw on. The
/// owner's own search doesn't read this record, and Sense writes it new with
/// each topic text, so nothing a later version wrote is lost or shown.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provenance {
    sha256: String,
    kinds: Option<Vec<String>>,
}

pub fn provenance(text: &str, kinds: Option<&BTreeSet<AdmittedCategory>>) -> serde_json::Value {
    serde_json::json!({
        "sha256": text_digest(text.as_bytes()),
        "kinds": kinds.map(kind_words),
    })
}

fn text_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The segment containing this indexed topic, relative to the chronicle.
pub fn segment_path(path: &str) -> Option<&str> {
    let path = path.strip_prefix("chronicle/").unwrap_or(path);
    let segment = path.strip_suffix("/talents/topics.md")?;
    let mut parts = segment.split('/');
    if !crate::segment::is_date_key(parts.next()?) {
        return None;
    }
    let second = parts.next()?;
    let name = match (parts.next(), parts.next()) {
        (None, None) => second,
        (Some(name), None) if !matches!(second, "" | "." | "..") => name,
        _ => return None,
    };
    crate::segment::segment_key(name).map(|_| segment)
}

#[derive(Debug, PartialEq, Eq)]
pub struct TopicDocument {
    pub text: String,
    /// Absent when the text predates provenance or its record cannot be verified.
    pub kinds: Option<BTreeSet<AdmittedCategory>>,
}

impl TopicDocument {
    pub fn read(journal: &Path, path: &str) -> Option<Self> {
        let segment = segment_path(path)?;
        let dir = Path::new("chronicle").join(segment).join("talents");
        let root = JournalRoot::open(journal).ok()?;
        let text =
            read_relative_file_bounded(&root, &dir.join("topics.md"), 4 * 1024 * 1024).ok()??;
        let kinds = read_relative_file_bounded(&root, &dir.join(PROVENANCE_FILE), 4096)
            .ok()
            .flatten()
            .and_then(|file| {
                let record: Provenance = serde_json::from_slice(&file.bytes).ok()?;
                if record.sha256 != text_digest(&text.bytes) {
                    return None;
                }
                parse_kinds(record.kinds?.iter().map(String::as_str))
                    .filter(|kinds| !kinds.is_empty())
            });
        Some(Self {
            text: String::from_utf8(text.bytes).ok()?,
            kinds,
        })
    }

    pub fn required_kinds(&self) -> impl Iterator<Item = AdmittedCategory> + '_ {
        self.kinds.iter().flatten().copied().chain(
            self.kinds
                .is_none()
                .then_some(POSSIBLE_INPUTS)
                .into_iter()
                .flatten()
                .copied(),
        )
    }

    pub fn selected_kinds(&self) -> impl Iterator<Item = AdmittedCategory> + '_ {
        // Keep an older topic findable under its historical search filter.
        // The wire response omits the kind label since the sources are unknown.
        self.kinds.iter().flatten().copied().chain(
            self.kinds
                .is_none()
                .then_some(AdmittedCategory::Transcripts),
        )
    }

    pub fn matches_chunk(&self, path: &str, index: i64, text: &str) -> bool {
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        produce_chunks(Family::Markdown, path, &self.text)
            .chunks
            .get(index)
            .is_some_and(|chunk| chunk.content == text)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn a_later_versions_record_releases_the_topic_only_with_every_source() {
        let journal = tempfile::TempDir::new().unwrap();
        let dir = journal.path().join("chronicle/20260308/120000_300/talents");
        fs::create_dir_all(&dir).unwrap();
        let text = "a topic";
        fs::write(dir.join("topics.md"), text).unwrap();
        let path = "chronicle/20260308/120000_300/talents/topics.md";
        let notes = BTreeSet::from([AdmittedCategory::Notes]);

        let mut record = provenance(text, Some(&notes));
        fs::write(dir.join(PROVENANCE_FILE), record.to_string()).unwrap();
        let read = TopicDocument::read(journal.path(), path).unwrap();
        assert_eq!(read.kinds, Some(notes));

        // A field this version doesn't know may narrow who can be served.
        record["withheld_from"] = serde_json::json!(["notes"]);
        fs::write(dir.join(PROVENANCE_FILE), record.to_string()).unwrap();
        let read = TopicDocument::read(journal.path(), path).unwrap();
        assert_eq!(read.text, text);
        assert_eq!(read.kinds, None);
        assert_eq!(
            read.required_kinds().collect::<Vec<_>>(),
            POSSIBLE_INPUTS.to_vec()
        );
    }
}
