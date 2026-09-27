// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use crate::{
    accept_merge_candidate, dismiss_merge_candidate, load_merge_candidates, record_merge_candidate,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-review-candidates-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn load_merge_candidates_filters_object_rows_and_skips_malformed_rows() {
    let temporary = TempDir::new();
    let path = temporary.path().join("entities/review-candidates.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        concat!(
            "{\"facet\":\"work\",\"status\":\"open\",\"source\":\"one\"}\n",
            "not json\n",
            "[\"not an object\"]\n",
            "{\"facet\":\"work\",\"status\":\"accepted\",\"source\":\"two\"}\n",
            "{\"facet\":\"home\",\"status\":\"open\",\"source\":\"three\"}\n",
        ),
    )
    .unwrap();

    let before = fs::read(&path).unwrap();
    assert_eq!(
        load_merge_candidates(temporary.path(), Some("work"), Some("open")).unwrap(),
        vec![json!({"facet":"work","status":"open","source":"one"})]
    );
    assert_eq!(
        load_merge_candidates(temporary.path(), None, Some("open"))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
}

fn record_candidate(
    temporary: &TempDir,
    source_slug: &str,
    target_slug: &str,
    detections: Option<i64>,
) {
    record_merge_candidate(
        temporary.path(),
        "work",
        "20260101",
        source_slug,
        source_slug,
        target_slug,
        target_slug,
        "evidence",
        None,
        detections,
        None,
    )
    .unwrap();
}

#[test]
fn accept_merge_candidate_updates_status_with_optional_merge_id() {
    let temporary = TempDir::new();
    record_candidate(&temporary, "source-one", "target-one", None);
    let accepted =
        accept_merge_candidate(temporary.path(), "work", "source-one", "target-one", None)
            .unwrap()
            .unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert!(accepted.get("merge_id").is_none());
    assert!(accepted["updated_at"].is_string());
}

#[test]
fn dismiss_merge_candidate_preserves_detection_count_watermark() {
    let temporary = TempDir::new();
    record_candidate(&temporary, "source-one", "target-one", Some(7));
    let dismissed = dismiss_merge_candidate(temporary.path(), "work", "source-one", "target-one")
        .unwrap()
        .unwrap();
    assert_eq!(dismissed["status"], "dismissed");
    assert_eq!(dismissed["dismissed_detection_count"], 7);
    assert!(dismissed["updated_at"].is_string());

    record_candidate(&temporary, "source-two", "target-two", None);
    let dismissed = dismiss_merge_candidate(temporary.path(), "work", "source-two", "target-two")
        .unwrap()
        .unwrap();
    assert_eq!(
        dismissed["dismissed_detection_count"],
        serde_json::Value::Null
    );
}

#[test]
fn merge_candidate_status_writers_return_none_when_candidate_is_absent() {
    let temporary = TempDir::new();
    assert_eq!(
        accept_merge_candidate(temporary.path(), "work", "source", "target", None).unwrap(),
        None
    );
    assert_eq!(
        dismiss_merge_candidate(temporary.path(), "work", "source", "target").unwrap(),
        None
    );
}

// The review queue reuses a merge only when the record shows it standing.

fn write_record(temporary: &TempDir, ids: serde_json::Value) {
    let entities = temporary.path().join("entities");
    fs::create_dir_all(&entities).unwrap();
    fs::write(
        entities.join("retired.json"),
        serde_json::to_vec(&json!({ "ids": ids })).unwrap(),
    )
    .unwrap();
}

fn write_log_row(temporary: &TempDir, source: &str, target: &str, merge_id: &str) {
    let logs = temporary.path().join("logs");
    fs::create_dir_all(&logs).unwrap();
    let path = logs.join("entity-merges.jsonl");
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!(
        "{}\n",
        json!({"ts": 1, "merge_id": merge_id, "source_id": source, "target_id": target})
    ));
    fs::write(path, text).unwrap();
}

fn merged(successor: &str, merge_id: &str) -> serde_json::Value {
    json!({"state": "merged", "dir": "ada", "successor": successor, "merge_id": merge_id})
}

fn find(temporary: &TempDir, target: &str) -> Option<String> {
    crate::find_active_recorded_merge(temporary.path(), "ada", target).unwrap()
}

/// Whether accepting the `(ada, target)` candidate with `merge_id` passes the
/// recorded-merge check.
fn accepts(temporary: &TempDir, target: &str, merge_id: &str) -> bool {
    record_candidate(temporary, "ada", target, None);
    match accept_merge_candidate(temporary.path(), "work", "ada", target, Some(merge_id)) {
        Ok(Some(row)) => {
            assert_eq!(row["status"], "accepted");
            true
        }
        Err(crate::EntityReviewCandidateError::RecordedMerge(message))
            if message == "candidate merge has no matching active record" =>
        {
            false
        }
        other => panic!("unexpected accept outcome: {other:?}"),
    }
}

#[test]
fn a_merge_the_record_holds_is_reused_and_matched_exactly() {
    let temporary = TempDir::new();
    write_record(&temporary, json!({"ada": merged("grace", "em_1")}));
    assert_eq!(find(&temporary, "grace").as_deref(), Some("em_1"));
    assert_eq!(find(&temporary, "linus"), None);
    assert!(!accepts(&temporary, "grace", "em_2"));
    assert!(!accepts(&temporary, "linus", "em_1"));
    assert!(accepts(&temporary, "grace", "em_1"));
}

#[test]
fn only_a_complete_merged_entry_answers_and_the_log_never_does() {
    let entries = [
        json!({"state": "deleted", "dir": "ada"}),
        json!({"state": "renamed", "dir": "ada", "successor": "grace", "merge_id": "em_1"}),
        json!({"state": "merged", "dir": "ada", "successor": "grace"}),
        json!({"state": "merged", "successor": "grace", "merge_id": "em_1"}),
        json!({"state": "merged", "dir": "ada", "successor": "", "merge_id": "em_1"}),
        json!({"state": "merged", "dir": "ada", "successor": "grace", "merge_id": "em_1", "seeded": true}),
        json!("merged"),
    ];
    for entry in entries {
        let temporary = TempDir::new();
        write_log_row(&temporary, "ada", "grace", "em_1");
        write_record(&temporary, json!({ "ada": entry.clone() }));
        assert_eq!(find(&temporary, "grace"), None, "{entry}");
        assert!(!accepts(&temporary, "grace", "em_1"), "{entry}");
    }

    // A log row with no record entry is no answer either.
    let temporary = TempDir::new();
    write_log_row(&temporary, "ada", "grace", "em_1");
    assert_eq!(find(&temporary, "grace"), None);
    write_record(&temporary, json!({"ada": merged("grace", "em_1")}));
    assert_eq!(find(&temporary, "grace").as_deref(), Some("em_1"));
}

#[test]
fn a_source_with_anything_at_its_id_or_directory_is_not_merged_away() {
    for folder in ["ada", "ada_dir"] {
        let temporary = TempDir::new();
        write_record(
            &temporary,
            json!({"ada": {"state": "merged", "dir": "ada_dir", "successor": "grace", "merge_id": "em_1"}}),
        );
        fs::create_dir_all(temporary.path().join("entities").join(folder)).unwrap();
        fs::write(
            temporary
                .path()
                .join("entities")
                .join(folder)
                .join("entity.json"),
            b"{\"id\": ",
        )
        .unwrap();
        assert_eq!(find(&temporary, "grace"), None, "{folder}");
        assert!(!accepts(&temporary, "grace", "em_1"), "{folder}");
        fs::remove_dir_all(temporary.path().join("entities").join(folder)).unwrap();
        assert_eq!(
            find(&temporary, "grace").as_deref(),
            Some("em_1"),
            "{folder}"
        );
    }
}

#[test]
fn another_entity_in_a_folder_named_by_the_id_does_not_hide_the_merge() {
    let temporary = TempDir::new();
    write_record(
        &temporary,
        json!({"ada": {"state": "merged", "dir": "ada_dir", "successor": "grace", "merge_id": "em_1"}}),
    );
    let folder = temporary.path().join("entities/ada");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("entity.json"),
        br#"{"id": "linus", "name": "Linus"}"#,
    )
    .unwrap();
    assert_eq!(find(&temporary, "grace").as_deref(), Some("em_1"));
    assert!(accepts(&temporary, "grace", "em_1"));

    // The same folder naming the merged id, or naming none, is that entity.
    for identity in [
        &br#"{"id": "ada", "name": "Ada"}"#[..],
        br#"{"name": "Ada"}"#,
    ] {
        fs::write(folder.join("entity.json"), identity).unwrap();
        assert_eq!(find(&temporary, "grace"), None);
    }
}

#[test]
fn a_source_live_in_another_folder_is_not_merged_away() {
    let temporary = TempDir::new();
    write_record(&temporary, json!({"ada": merged("grace", "em_1")}));
    let folder = temporary.path().join("entities/restored");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("entity.json"),
        br#"{"id": "ada", "name": "Ada"}"#,
    )
    .unwrap();
    assert_eq!(find(&temporary, "grace"), None);
    assert!(!accepts(&temporary, "grace", "em_1"));
    fs::remove_dir_all(&folder).unwrap();
    assert_eq!(find(&temporary, "grace").as_deref(), Some("em_1"));
}

#[test]
fn a_damaged_record_is_an_error_not_a_fallback_to_the_log() {
    let temporary = TempDir::new();
    write_log_row(&temporary, "ada", "grace", "em_1");
    fs::create_dir_all(temporary.path().join("entities")).unwrap();
    fs::write(temporary.path().join("entities/retired.json"), b"{broken").unwrap();
    assert!(matches!(
        crate::find_active_recorded_merge(temporary.path(), "ada", "grace"),
        Err(crate::EntityReviewCandidateError::RecordedMerge(_))
    ));
    record_candidate(&temporary, "ada", "grace", None);
    assert!(matches!(
        accept_merge_candidate(temporary.path(), "work", "ada", "grace", Some("em_1")),
        Err(crate::EntityReviewCandidateError::RecordedMerge(message))
            if message != "candidate merge has no matching active record"
    ));
}

#[test]
fn a_merge_into_an_entity_later_merged_or_deleted_still_stands() {
    let temporary = TempDir::new();
    write_record(
        &temporary,
        json!({
            "ada": merged("grace", "em_1"),
            "grace": {"state": "merged", "dir": "grace", "successor": "linus", "merge_id": "em_2"},
        }),
    );
    assert_eq!(find(&temporary, "grace").as_deref(), Some("em_1"));
    assert_eq!(find(&temporary, "linus"), None);
    assert!(accepts(&temporary, "grace", "em_1"));
}
