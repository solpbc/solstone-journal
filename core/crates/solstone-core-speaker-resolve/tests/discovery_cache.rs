// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};
use solstone_core_speaker_resolve::discovery_cache::{
    canonical_members, discovery_cache_path, load_discovery_cache,
    normalize_reviewed_near_match_ids, ReviewedNearMatchIdsError,
};
use solstone_core_speaker_resolve::identify_operations::MemberProvenance;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct TempJournal(PathBuf);

impl TempJournal {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-discovery-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn cache_path(&self) -> PathBuf {
        discovery_cache_path(&self.0)
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_cache(journal: &TempJournal, value: &Value) {
    let path = journal.cache_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

#[test]
fn discovery_cache_read_tolerates_missing_malformed_and_invalid_artifacts() {
    let journal = TempJournal::new();
    assert_eq!(load_discovery_cache(&journal.0), None);

    let path = journal.cache_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{not json}").unwrap();
    assert_eq!(load_discovery_cache(&journal.0), None);

    write_cache(&journal, &json!({"clusters": []}));
    assert_eq!(load_discovery_cache(&journal.0), None);

    write_cache(&journal, &json!(["not-an-object"]));
    assert_eq!(load_discovery_cache(&journal.0), None);
}

#[test]
fn discovery_cache_read_returns_a_valid_cache_without_rewriting_it() {
    let journal = TempJournal::new();
    let cache = json!({"clusters": {"7": [{"day": "20260101"}]}, "generated_at": "now"});
    write_cache(&journal, &cache);

    assert_eq!(load_discovery_cache(&journal.0), Some(cache));
}

#[test]
fn canonical_members_sort_the_shared_provenance_type() {
    let raw = vec![
        json!({
            "day": "20260101", "stream": "mic", "segment_key": "seg-b",
            "source": "audio", "sentence_id": 2,
        }),
        json!({
            "day": "20260101", "stream": "mic", "segment_key": "seg-a",
            "source": "audio", "sentence_id": 1,
        }),
    ];

    assert_eq!(
        canonical_members(&raw).unwrap(),
        vec![
            MemberProvenance {
                day: "20260101".into(),
                stream: "mic".into(),
                segment_key: "seg-a".into(),
                source: "audio".into(),
                sentence_id: 1,
            },
            MemberProvenance {
                day: "20260101".into(),
                stream: "mic".into(),
                segment_key: "seg-b".into(),
                source: "audio".into(),
                sentence_id: 2,
            },
        ],
    );
}

#[test]
fn reviewed_near_match_ids_normalize_none_and_valid_trimmed_ids() {
    assert_eq!(
        normalize_reviewed_near_match_ids(None).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        normalize_reviewed_near_match_ids(Some(&json!([" ent-bob ", "ent-carol"]))).unwrap(),
        vec!["ent-bob", "ent-carol"],
    );
}

#[test]
fn reviewed_near_match_ids_report_each_python_invalid_request_shape() {
    let not_list = normalize_reviewed_near_match_ids(Some(&json!("ent-bob"))).unwrap_err();
    assert_eq!(not_list, ReviewedNearMatchIdsError::NotList);
    assert_eq!(
        not_list.invalid_request_response(),
        json!({
            "status": "invalid_request",
            "error": "reviewed_near_match_entity_ids must be a list",
        }),
    );

    let invalid_item = normalize_reviewed_near_match_ids(Some(&json!([" "]))).unwrap_err();
    assert_eq!(invalid_item, ReviewedNearMatchIdsError::InvalidItem);
    assert_eq!(
        invalid_item.invalid_request_response(),
        json!({
            "status": "invalid_request",
            "error": "reviewed_near_match_entity_ids must contain strings",
        }),
    );
    assert_eq!(
        normalize_reviewed_near_match_ids(Some(&json!([42]))).unwrap_err(),
        ReviewedNearMatchIdsError::InvalidItem,
    );

    let duplicate =
        normalize_reviewed_near_match_ids(Some(&json!(["ent-bob", " ent-bob "]))).unwrap_err();
    assert_eq!(
        duplicate,
        ReviewedNearMatchIdsError::Duplicate {
            entity_id: "ent-bob".into(),
        }
    );
    assert_eq!(
        duplicate.invalid_request_response(),
        json!({
            "status": "invalid_request",
            "error": "reviewed_near_match_entity_ids must be unique",
            "invalid_reviewed_near_match_entity_ids": [
                {"entity_id": "ent-bob", "reason": "duplicate"}
            ],
        }),
    );
}
