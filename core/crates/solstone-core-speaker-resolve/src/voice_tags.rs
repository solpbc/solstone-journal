// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only voice tags from the speaker candidate pool.
//!
//! The pool already groups diarization clusters from many segments into one
//! voice. This module reads that grouping back so a sentence nobody has named
//! can still carry the same anonymous tag wherever its voice appears. It never
//! writes the pool and never changes a label.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Deserialize;

/// The pool voice a sentence belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceTag {
    /// The pool's candidate id; stable until the voice is merged into another.
    pub voice_id: i64,
    /// The entity an owner or identify confirmed this voice as, if any.
    pub confirmed_entity: Option<String>,
}

#[derive(Deserialize)]
struct PoolFile {
    #[serde(default)]
    candidates: Vec<PoolCandidate>,
}

#[derive(Deserialize)]
struct PoolCandidate {
    cand_id: i64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    confirmed_entity: Option<String>,
    #[serde(default)]
    source_segments: Vec<PoolSource>,
}

#[derive(Deserialize)]
struct PoolSource {
    day: Option<String>,
    stream: Option<String>,
    segment_key: Option<String>,
    source: Option<String>,
    #[serde(default)]
    sentence_ids: Vec<i64>,
}

pub(crate) type SourceKey = (String, String, String, String);
pub(crate) type VoiceIndex = HashMap<SourceKey, BTreeMap<i64, VoiceTag>>;

struct Cached {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    index: Arc<VoiceIndex>,
}

static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

fn pool_path(journal: &Path) -> PathBuf {
    journal.join("awareness/speaker_candidates.json")
}

pub(crate) fn build_index(bytes: &[u8]) -> VoiceIndex {
    let Ok(pool) = serde_json::from_slice::<PoolFile>(bytes) else {
        return VoiceIndex::new();
    };
    let mut candidates = pool.candidates;
    // Lowest id first, so a sentence claimed twice keeps one deterministic tag.
    candidates.sort_by_key(|candidate| candidate.cand_id);
    let mut index = VoiceIndex::new();
    for candidate in candidates {
        if candidate.status.as_deref() == Some("rejected") {
            continue;
        }
        let tag = VoiceTag {
            voice_id: candidate.cand_id,
            confirmed_entity: candidate.confirmed_entity.filter(|id| !id.is_empty()),
        };
        for source in candidate.source_segments {
            let (Some(day), Some(stream), Some(segment_key), Some(source_name)) =
                (source.day, source.stream, source.segment_key, source.source)
            else {
                continue;
            };
            let sentences = index
                .entry((day, stream, segment_key, source_name))
                .or_default();
            for sentence_id in source.sentence_ids {
                sentences.entry(sentence_id).or_insert_with(|| tag.clone());
            }
        }
    }
    index
}

fn load_index(journal: &Path) -> Arc<VoiceIndex> {
    let path = pool_path(journal);
    let Ok(metadata) = fs::metadata(&path) else {
        return Arc::new(VoiceIndex::new());
    };
    let (len, modified) = (metadata.len(), metadata.modified().ok());
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(cached) = cache.as_ref()
        && cached.path == path
        && cached.len == len
        && cached.modified == modified
    {
        return Arc::clone(&cached.index);
    }
    let index = Arc::new(
        fs::read(&path)
            .map(|bytes| build_index(&bytes))
            .unwrap_or_default(),
    );
    *cache = Some(Cached {
        path,
        len,
        modified,
        index: Arc::clone(&index),
    });
    index
}

/// Voice tags for one audio source in one segment, keyed by sentence id.
///
/// A missing or unreadable pool reads as no tags: the transcript simply shows
/// its sentences as unknown, exactly as before voice tags existed.
#[must_use]
pub fn segment_voice_tags(
    journal: &Path,
    day: &str,
    stream: &str,
    segment_key: &str,
    source: &str,
) -> BTreeMap<i64, VoiceTag> {
    load_index(journal)
        .get(&(
            day.to_owned(),
            stream.to_owned(),
            segment_key.to_owned(),
            source.to_owned(),
        ))
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn write_pool(root: &Path, candidates: serde_json::Value) {
        fs::create_dir_all(root.join("awareness")).unwrap();
        fs::write(
            pool_path(root),
            serde_json::to_vec(&json!({"next_id": 99, "candidates": candidates})).unwrap(),
        )
        .unwrap();
    }

    fn source(segment_key: &str, sentence_ids: &[i64]) -> serde_json::Value {
        json!({"day":"20260929","stream_layout":"named","stream":"mic","segment_key":segment_key,"source":"audio","cluster_label":1,"sentence_ids":sentence_ids})
    }

    #[test]
    fn one_voice_tags_its_sentences_in_every_segment_it_spans() {
        let root = tempfile::tempdir().unwrap();
        write_pool(
            root.path(),
            json!([{"cand_id":7,"centroid":[1.0],"status":"pending","confirmed_entity":null,
                    "source_segments":[source("100000_300",&[2,4]), source("100500_300",&[1])]}]),
        );
        let first = segment_voice_tags(root.path(), "20260929", "mic", "100000_300", "audio");
        let second = segment_voice_tags(root.path(), "20260929", "mic", "100500_300", "audio");
        assert_eq!(first.keys().copied().collect::<Vec<_>>(), vec![2, 4]);
        assert_eq!(first[&2].voice_id, 7);
        assert_eq!(second[&1].voice_id, 7);
        assert!(
            segment_voice_tags(root.path(), "20260929", "mic", "100000_300", "other").is_empty()
        );
    }

    #[test]
    fn rejected_voices_carry_no_tag_and_confirmed_voices_carry_their_entity() {
        let root = tempfile::tempdir().unwrap();
        write_pool(
            root.path(),
            json!([
                {"cand_id":3,"centroid":[1.0],"status":"rejected","confirmed_entity":null,"source_segments":[source("100000_300",&[1])]},
                {"cand_id":5,"centroid":[1.0],"status":"confirmed","confirmed_entity":"ryan","source_segments":[source("100000_300",&[2])]}
            ]),
        );
        let tags = segment_voice_tags(root.path(), "20260929", "mic", "100000_300", "audio");
        assert!(!tags.contains_key(&1));
        assert_eq!(tags[&2].confirmed_entity.as_deref(), Some("ryan"));
    }

    #[test]
    fn a_pool_rewrite_is_read_again_and_a_missing_pool_reads_empty() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            segment_voice_tags(root.path(), "20260929", "mic", "100000_300", "audio").is_empty()
        );
        write_pool(
            root.path(),
            json!([{"cand_id":9,"centroid":[1.0],"source_segments":[source("100000_300",&[1])]}]),
        );
        assert_eq!(
            segment_voice_tags(root.path(), "20260929", "mic", "100000_300", "audio")[&1].voice_id,
            9
        );
        // A merge moves the absorbed voice's sources onto the survivor.
        write_pool(
            root.path(),
            json!([{"cand_id":2,"centroid":[1.0],"source_segments":[source("100000_300",&[1]), source("100500_300",&[3])]}]),
        );
        assert_eq!(
            segment_voice_tags(root.path(), "20260929", "mic", "100000_300", "audio")[&1].voice_id,
            2
        );
    }
}
