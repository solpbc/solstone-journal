// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Which voiceprint rows the journal has evidence for.
//!
//! A row counts toward a person's centroid only when the segment it came from
//! named that person (the segment's speaker attribution listed them in its
//! candidate evidence) or the owner labelled that line as them. Rows written
//! from a guess against every voiceprint, before journal #403's rule stopped
//! that, stay on disk but no longer shape whom a voice is matched to.
//!
//! What each segment named is read from its `talents/speaker_labels.json` and
//! kept in `awareness/voiceprint_evidence.json`, refreshed whenever that file
//! changes. The index is derived: deleting it only costs a rebuild.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use solstone_core_journal_io::{DEFAULT_STREAM, JsonWriteOptions, write_json};

const INDEX_VERSION: u32 = 1;

/// Journal-relative location of the evidence index.
pub fn voiceprint_evidence_path(journal_root: &Path) -> PathBuf {
    journal_root.join("awareness/voiceprint_evidence.json")
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SegmentEvidence {
    /// Modification time of the labels file in ms, or -1 when it is absent.
    modified_ms: i64,
    /// Entity ids the segment's candidate evidence named.
    named: BTreeSet<String>,
    /// Sentence id to the entity the owner labelled it as.
    owner: BTreeMap<i64, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    segments: BTreeMap<String, SegmentEvidence>,
}

/// Per-call view of the evidence index.
#[derive(Debug)]
pub struct VoiceprintEvidence {
    journal_root: PathBuf,
    segments: BTreeMap<String, SegmentEvidence>,
    checked: HashSet<String>,
    dirty: bool,
    /// Live entity id to every id merged into it, itself included.
    aliases: Option<HashMap<String, BTreeSet<String>>>,
}

impl VoiceprintEvidence {
    /// Load the index for `journal_root`; an absent or unreadable one starts empty.
    pub fn load(journal_root: &Path) -> Self {
        let segments = std::fs::read(voiceprint_evidence_path(journal_root))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<IndexFile>(&bytes).ok())
            .filter(|index| index.version == INDEX_VERSION)
            .map(|index| index.segments)
            .unwrap_or_default();
        Self {
            journal_root: journal_root.to_owned(),
            segments,
            checked: HashSet::new(),
            dirty: false,
            aliases: None,
        }
    }

    /// Whether the row described by `metadata` has evidence for `entity_id`.
    pub fn supports(&mut self, entity_id: &str, metadata: &Value) -> bool {
        let Some((segment, sentence_id)) = row_segment(metadata) else {
            return false;
        };
        self.refresh(&segment);
        let ids = self.ids_for(entity_id);
        let Some(evidence) = self.segments.get(&segment) else {
            return false;
        };
        evidence.named.iter().any(|named| ids.contains(named))
            || evidence
                .owner
                .get(&sentence_id)
                .is_some_and(|owner| ids.contains(owner))
    }

    /// Write the index back when this call learned anything new.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let index = IndexFile {
            version: INDEX_VERSION,
            segments: std::mem::take(&mut self.segments),
        };
        let options = JsonWriteOptions {
            indent: None,
            ..JsonWriteOptions::default()
        };
        if let Err(error) = write_json(
            voiceprint_evidence_path(&self.journal_root),
            &index,
            options,
        ) {
            log::warn!("voiceprint evidence index not saved: {error}");
        }
        self.segments = index.segments;
        self.dirty = false;
    }

    fn refresh(&mut self, segment: &str) {
        if !self.checked.insert(segment.to_owned()) {
            return;
        }
        let path = self
            .journal_root
            .join("chronicle")
            .join(segment)
            .join("talents/speaker_labels.json");
        let modified_ms = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(-1, |duration| duration.as_millis() as i64);
        if self
            .segments
            .get(segment)
            .is_some_and(|evidence| evidence.modified_ms == modified_ms)
        {
            return;
        }
        let labels = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let evidence = SegmentEvidence {
            modified_ms,
            named: labels
                .as_ref()
                .and_then(|labels| labels.get("candidate_evidence"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("entity_id").and_then(Value::as_str))
                .map(str::to_owned)
                .collect(),
            owner: labels
                .as_ref()
                .and_then(|labels| labels.get("labels"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|label| {
                    label
                        .get("method")
                        .and_then(Value::as_str)
                        .is_some_and(|method| method.starts_with("user_"))
                })
                .filter_map(|label| {
                    Some((
                        label.get("sentence_id")?.as_i64()?,
                        label.get("speaker")?.as_str()?.to_owned(),
                    ))
                })
                .collect(),
        };
        self.segments.insert(segment.to_owned(), evidence);
        self.dirty = true;
    }

    fn ids_for(&mut self, entity_id: &str) -> BTreeSet<String> {
        let journal_root = &self.journal_root;
        let aliases = self
            .aliases
            .get_or_insert_with(|| merged_aliases(journal_root));
        let mut ids = aliases.get(entity_id).cloned().unwrap_or_default();
        ids.insert(entity_id.to_owned());
        ids
    }
}

/// The chronicle-relative segment directory and sentence id of one row.
fn row_segment(metadata: &Value) -> Option<(String, i64)> {
    let day = metadata.get("day")?.as_str()?;
    let segment_key = metadata.get("segment_key")?.as_str()?;
    let sentence_id = metadata.get("sentence_id")?.as_i64()?;
    let stream = metadata.get("stream").and_then(Value::as_str);
    let direct = match metadata.get("stream_layout").and_then(Value::as_str) {
        Some("direct") => true,
        Some("named") => false,
        Some(_) => return None,
        None => stream.is_none_or(|stream| stream == DEFAULT_STREAM),
    };
    let segment = if direct {
        format!("{day}/{segment_key}")
    } else {
        format!("{day}/{}/{segment_key}", stream?)
    };
    Some((segment, sentence_id))
}

/// Every merged-away id, keyed by the live entity it ended in.
fn merged_aliases(journal_root: &Path) -> HashMap<String, BTreeSet<String>> {
    let merged = solstone_core_entity::read_retired_entities(journal_root).merged();
    let mut out = HashMap::<String, BTreeSet<String>>::new();
    for id in merged.keys() {
        let mut current = id.clone();
        for _ in 0..16 {
            match merged.get(&current) {
                Some(entry) => current = entry.successor.clone(),
                None => break,
            }
        }
        if &current != id {
            out.entry(current).or_default().insert(id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_labels(root: &Path, segment: &str, labels: Value) {
        let path = root
            .join("chronicle")
            .join(segment)
            .join("talents/speaker_labels.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, labels.to_string()).unwrap();
    }

    fn row(sentence_id: i64) -> Value {
        json!({"day": "20261029", "stream": "mic", "stream_layout": "named",
               "segment_key": "100000_300", "source": "audio", "sentence_id": sentence_id})
    }

    #[test]
    fn a_row_counts_when_its_segment_named_the_person_or_the_owner_labelled_it() {
        let root = tempfile::TempDir::new().unwrap();
        write_labels(
            root.path(),
            "20261029/mic/100000_300",
            json!({"candidate_evidence": [{"entity_id": "ada", "sources": ["speakers"]}],
                   "labels": [{"sentence_id": 2, "speaker": "bob", "method": "user_assigned"},
                              {"sentence_id": 3, "speaker": "bob", "method": "acoustic"}]}),
        );
        let mut evidence = VoiceprintEvidence::load(root.path());
        assert!(evidence.supports("ada", &row(1)));
        assert!(evidence.supports("bob", &row(2)));
        assert!(!evidence.supports("bob", &row(3)));
        assert!(!evidence.supports("cy", &row(1)));
        assert!(!evidence.supports("ada", &json!({"stream": "mic", "added_at": 0})));
    }

    #[test]
    fn a_segment_that_named_no_one_supports_no_row() {
        let root = tempfile::TempDir::new().unwrap();
        write_labels(
            root.path(),
            "20261029/mic/100000_300",
            json!({"candidate_evidence": [], "labels": [{"sentence_id": 1, "speaker": "ada", "method": "acoustic_cluster"}]}),
        );
        let mut evidence = VoiceprintEvidence::load(root.path());
        assert!(!evidence.supports("ada", &row(1)));
        let missing = json!({"day": "20261030", "stream": "_default", "segment_key": "1", "source": "audio", "sentence_id": 1});
        assert!(!evidence.supports("ada", &missing));
    }

    #[test]
    fn the_index_is_saved_and_refreshed_when_a_segment_changes() {
        let root = tempfile::TempDir::new().unwrap();
        let segment = "20261029/mic/100000_300";
        write_labels(
            root.path(),
            segment,
            json!({"candidate_evidence": [], "labels": []}),
        );
        let mut evidence = VoiceprintEvidence::load(root.path());
        assert!(!evidence.supports("ada", &row(1)));
        evidence.save();
        assert!(voiceprint_evidence_path(root.path()).is_file());

        write_labels(
            root.path(),
            segment,
            json!({"labels": [{"sentence_id": 1, "speaker": "ada", "method": "user_confirmed"}]}),
        );
        let path = root
            .path()
            .join("chronicle")
            .join(segment)
            .join("talents/speaker_labels.json");
        filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(2_000_000_000, 0))
            .unwrap();
        let mut reloaded = VoiceprintEvidence::load(root.path());
        assert!(reloaded.supports("ada", &row(1)));
    }

    #[test]
    fn rows_direct_and_named_layouts_resolve_their_segment() {
        assert_eq!(
            row_segment(&row(4)),
            Some(("20261029/mic/100000_300".to_owned(), 4))
        );
        let direct =
            json!({"day": "20261029", "stream": "_default", "segment_key": "1", "sentence_id": 1});
        assert_eq!(row_segment(&direct), Some(("20261029/1".to_owned(), 1)));
        let bad = json!({"day": "20261029", "stream_layout": "other", "segment_key": "1", "sentence_id": 1});
        assert_eq!(row_segment(&bad), None);
    }
}
