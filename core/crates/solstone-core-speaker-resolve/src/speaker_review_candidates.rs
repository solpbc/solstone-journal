// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable speaker name-variant review candidates.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, MalformedPolicy, ReadError,
    hold_lock, read_jsonl, write_jsonl,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpeakerReviewCandidateError {
    #[error("speaker review candidates directory failed at {path}: {source}")]
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("speaker review candidates read failed: {0}")]
    Read(#[from] ReadError),
    #[error("speaker review candidates lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("speaker review candidates write failed: {0}")]
    Write(#[from] AtomicWriteError),
}

/// Load name-variant review candidates, skipping malformed JSONL rows.
pub fn load_candidates(journal_root: &Path) -> Result<Vec<Value>, SpeakerReviewCandidateError> {
    Ok(read_jsonl(
        review_candidates_path(journal_root),
        Vec::new(),
        MalformedPolicy::WarnAndSkip,
    )?)
}

/// Mark one name-variant review candidate accepted when it exists.
pub fn accept_candidate(
    journal_root: &Path,
    id_a: &str,
    id_b: &str,
    merge_id: Option<&str>,
) -> Result<Option<Value>, SpeakerReviewCandidateError> {
    mutate_candidates(journal_root, |rows| {
        let existing = find_candidate(rows, id_a, id_b)?;
        let object = existing
            .as_object_mut()
            .expect("candidate reader returns JSON objects");
        object.insert("status".to_owned(), Value::String("accepted".to_owned()));
        if let Some(merge_id) = merge_id.filter(|value| !value.is_empty()) {
            object.insert("merge_id".to_owned(), Value::String(merge_id.to_owned()));
        }
        object.insert("updated_at".to_owned(), Value::String(now_iso()));
        Some(existing.clone())
    })
}

/// Mark one name-variant review candidate dismissed when it exists.
pub fn dismiss_candidate(
    journal_root: &Path,
    id_a: &str,
    id_b: &str,
) -> Result<Option<Value>, SpeakerReviewCandidateError> {
    mutate_candidates(journal_root, |rows| {
        let existing = find_candidate(rows, id_a, id_b)?;
        let dismissed_detection_count = existing
            .get("evidence")
            .and_then(Value::as_object)
            .and_then(|evidence| evidence.get("detection_count"))
            .cloned()
            .unwrap_or(Value::Null);
        let object = existing
            .as_object_mut()
            .expect("candidate reader returns JSON objects");
        object.insert("status".to_owned(), Value::String("dismissed".to_owned()));
        object.insert(
            "dismissed_detection_count".to_owned(),
            dismissed_detection_count,
        );
        object.insert("updated_at".to_owned(), Value::String(now_iso()));
        Some(existing.clone())
    })
}

fn mutate_candidates<T>(
    journal_root: &Path,
    mutate: impl FnOnce(&mut [Value]) -> T,
) -> Result<T, SpeakerReviewCandidateError> {
    let path = review_candidates_path(journal_root);
    create_parent(&path)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let mut rows = load_candidates(journal_root)?;
    let result = mutate(&mut rows);
    write_jsonl(&path, rows, AtomicWriteOptions::default())?;
    Ok(result)
}

fn find_candidate<'a>(rows: &'a mut [Value], id_a: &str, id_b: &str) -> Option<&'a mut Value> {
    let target = candidate_key(id_a, id_b);
    rows.iter_mut().find(|row| {
        candidate_key(
            row.get("source_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            row.get("target_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ) == target
    })
}

fn candidate_key(id_a: &str, id_b: &str) -> String {
    let (left, right) = if id_a <= id_b {
        (id_a, id_b)
    } else {
        (id_b, id_a)
    };
    format!("{left}|{right}")
}

fn review_candidates_path(journal_root: &Path) -> PathBuf {
    journal_root.join("speakers/review-candidates.jsonl")
}

fn create_parent(path: &Path) -> Result<(), SpeakerReviewCandidateError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|source| SpeakerReviewCandidateError::Directory {
        path: parent.to_owned(),
        source,
    })
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    fn temporary_journal(name: &str) -> PathBuf {
        let path = PathBuf::from("/var/tmp").join(format!(
            "solstone-speaker-review-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn row() -> Value {
        json!({
            "source_id": "alice",
            "target_id": "alicia",
            "status": "open",
            "evidence": {"detection_count": 7},
            "updated_at": "20260101T000000Z",
        })
    }

    fn seed(journal: &Path) {
        let path = review_candidates_path(journal);
        create_parent(&path).unwrap();
        write_jsonl(&path, vec![row()], AtomicWriteOptions::default()).unwrap();
    }

    #[test]
    fn accept_marks_row_and_persists_merge_id() {
        let journal = temporary_journal("accept");
        seed(&journal);

        let accepted = accept_candidate(&journal, "alicia", "alice", Some("merge-7"))
            .unwrap()
            .unwrap();

        assert_eq!(accepted["status"], "accepted");
        assert_eq!(accepted["merge_id"], "merge-7");
        assert!(accepted["updated_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(load_candidates(&journal).unwrap()[0]["status"], "accepted");
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn dismiss_captures_detection_count_and_persists() {
        let journal = temporary_journal("dismiss");
        seed(&journal);

        let dismissed = dismiss_candidate(&journal, "alice", "alicia")
            .unwrap()
            .unwrap();

        assert_eq!(dismissed["status"], "dismissed");
        assert_eq!(dismissed["dismissed_detection_count"], 7);
        assert_eq!(
            load_candidates(&journal).unwrap()[0]["dismissed_detection_count"],
            7
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn missing_candidate_returns_none_without_creating_a_row() {
        let journal = temporary_journal("missing");
        seed(&journal);

        assert!(
            accept_candidate(&journal, "missing", "candidate", None)
                .unwrap()
                .is_none()
        );
        assert_eq!(load_candidates(&journal).unwrap().len(), 1);
        fs::remove_dir_all(journal).unwrap();
    }
}
