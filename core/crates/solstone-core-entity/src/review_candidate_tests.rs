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

    record_candidate(&temporary, "source-two", "target-two", None);
    let accepted = accept_merge_candidate(
        temporary.path(),
        "work",
        "source-two",
        "target-two",
        Some("merge-1"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["merge_id"], "merge-1");
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
