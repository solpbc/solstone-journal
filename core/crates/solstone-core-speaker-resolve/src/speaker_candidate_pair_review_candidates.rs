// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable speaker candidate-pair review candidates.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, MalformedPolicy, ReadError,
    hold_lock, read_jsonl, write_jsonl,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpeakerCandidatePairReviewCandidateError {
    #[error("speaker candidate-pair review candidates directory failed at {path}: {source}")]
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("speaker candidate-pair review candidates read failed: {0}")]
    Read(#[from] ReadError),
    #[error("speaker candidate-pair review candidates lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("speaker candidate-pair review candidates write failed: {0}")]
    Write(#[from] AtomicWriteError),
    #[error("invalid speaker candidate-pair review record: {0}")]
    InvalidRow(&'static str),
}

/// One detected pair, addressed by stable source anchors rather than pool IDs.
pub struct CandidatePairSuggestion {
    pub source_anchors: BTreeSet<String>,
    pub target_anchors: BTreeSet<String>,
    pub similarity: f32,
    pub source_intervals: usize,
    pub target_intervals: usize,
    pub source_samples: Vec<Value>,
    pub target_samples: Vec<Value>,
}

/// Record a pair while preserving prior decisions, including after pool merges.
pub fn record_candidate_pair(
    journal_root: &Path,
    suggestion: &CandidatePairSuggestion,
) -> Result<(Option<Value>, bool, bool), SpeakerCandidatePairReviewCandidateError> {
    let source = suggestion.source_anchors.first().ok_or(
        SpeakerCandidatePairReviewCandidateError::InvalidRow("source has no anchors"),
    )?;
    let target = suggestion.target_anchors.first().ok_or(
        SpeakerCandidatePairReviewCandidateError::InvalidRow("target has no anchors"),
    )?;
    let path = review_candidates_path(journal_root);
    create_parent(&path)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let mut rows: Vec<Value> = read_jsonl(&path, Vec::new(), MalformedPolicy::Raise)?;
    if rows.iter().any(|row| !row.is_object()) {
        return Err(SpeakerCandidatePairReviewCandidateError::InvalidRow(
            "expected JSON objects",
        ));
    }
    for row in &rows {
        if row.get("status").and_then(Value::as_str) != Some("dismissed") {
            continue;
        }
        if let (Some(a), Some(b)) = (
            row.get("dismissed_anchor_a").and_then(Value::as_str),
            row.get("dismissed_anchor_b").and_then(Value::as_str),
        ) && ((suggestion.source_anchors.contains(a) && suggestion.target_anchors.contains(b))
            || (suggestion.source_anchors.contains(b) && suggestion.target_anchors.contains(a)))
        {
            return Ok((None, false, true));
        }
    }
    let key = candidate_key(source, target);
    let (anchor_a, anchor_b) = sorted_anchors(source, target);
    let now = now_iso();
    let existing = find_candidate(&mut rows, source, target);
    let created = existing.is_none();
    let mut row = existing.cloned().unwrap_or_else(|| {
        json!({
            "status":"open", "first_surfaced":now, "created_at":now,
        })
    });
    row.as_object_mut().expect("validated object").extend(
        json!({
            "key":key, "anchor_a":anchor_a, "anchor_b":anchor_b,
            "similarity":suggestion.similarity,
            "evidence":{
                "basis":"speaker-candidate-pair", "similarity":suggestion.similarity,
                "source_intervals":suggestion.source_intervals,
                "target_intervals":suggestion.target_intervals,
                "source_samples":suggestion.source_samples,
                "target_samples":suggestion.target_samples,
            },
            "last_surfaced":now, "updated_at":now,
        })
        .as_object()
        .expect("object literal")
        .clone(),
    );
    if let Some(existing) = find_candidate(&mut rows, source, target) {
        *existing = row.clone();
    } else {
        rows.push(row.clone());
    }
    write_jsonl(&path, rows, AtomicWriteOptions::default())?;
    Ok((Some(row), created, false))
}

/// Load candidate-pair review candidates, skipping malformed JSONL rows.
pub fn load_candidates(
    journal_root: &Path,
) -> Result<Vec<Value>, SpeakerCandidatePairReviewCandidateError> {
    Ok(read_jsonl(
        review_candidates_path(journal_root),
        Vec::new(),
        MalformedPolicy::WarnAndSkip,
    )?)
}

/// Mark one candidate-pair review candidate accepted when it exists.
pub fn accept_candidate(
    journal_root: &Path,
    anchor_a: &str,
    anchor_b: &str,
) -> Result<Option<Value>, SpeakerCandidatePairReviewCandidateError> {
    mutate_candidates(journal_root, |rows| {
        let existing = find_candidate(rows, anchor_a, anchor_b)?;
        let object = existing
            .as_object_mut()
            .expect("candidate reader returns JSON objects");
        object.insert("status".to_owned(), Value::String("accepted".to_owned()));
        object.insert("updated_at".to_owned(), Value::String(now_iso()));
        Some(existing.clone())
    })
}

/// Mark one candidate-pair review candidate dismissed when it exists.
pub fn dismiss_candidate(
    journal_root: &Path,
    anchor_a: &str,
    anchor_b: &str,
) -> Result<Option<Value>, SpeakerCandidatePairReviewCandidateError> {
    mutate_candidates(journal_root, |rows| {
        let existing = find_candidate(rows, anchor_a, anchor_b)?;
        let (left, right) = sorted_anchors(anchor_a, anchor_b);
        let timestamp = now_iso();
        let object = existing
            .as_object_mut()
            .expect("candidate reader returns JSON objects");
        object.insert("status".to_owned(), Value::String("dismissed".to_owned()));
        object.insert("dismissed_anchor_a".to_owned(), Value::String(left));
        object.insert("dismissed_anchor_b".to_owned(), Value::String(right));
        object.insert("dismissed_at".to_owned(), Value::String(timestamp.clone()));
        object.insert("updated_at".to_owned(), Value::String(timestamp));
        Some(existing.clone())
    })
}

fn mutate_candidates<T>(
    journal_root: &Path,
    mutate: impl FnOnce(&mut [Value]) -> T,
) -> Result<T, SpeakerCandidatePairReviewCandidateError> {
    let path = review_candidates_path(journal_root);
    create_parent(&path)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let mut rows = load_candidates(journal_root)?;
    let result = mutate(&mut rows);
    write_jsonl(&path, rows, AtomicWriteOptions::default())?;
    Ok(result)
}

fn find_candidate<'a>(
    rows: &'a mut [Value],
    anchor_a: &str,
    anchor_b: &str,
) -> Option<&'a mut Value> {
    let target = candidate_key(anchor_a, anchor_b);
    rows.iter_mut()
        .find(|row| row.get("key").and_then(Value::as_str) == Some(target.as_str()))
}

fn candidate_key(anchor_a: &str, anchor_b: &str) -> String {
    let (left, right) = sorted_anchors(anchor_a, anchor_b);
    json!([left, right]).to_string()
}

fn sorted_anchors(anchor_a: &str, anchor_b: &str) -> (String, String) {
    if anchor_a <= anchor_b {
        (anchor_a.to_owned(), anchor_b.to_owned())
    } else {
        (anchor_b.to_owned(), anchor_a.to_owned())
    }
}

fn review_candidates_path(journal_root: &Path) -> PathBuf {
    journal_root.join("speakers/candidate-pair-review-candidates.jsonl")
}

fn create_parent(path: &Path) -> Result<(), SpeakerCandidatePairReviewCandidateError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|source| {
        SpeakerCandidatePairReviewCandidateError::Directory {
            path: parent.to_owned(),
            source,
        }
    })
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn temporary_journal(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("solstone-speaker-pair-{name}-"))
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    fn seed(journal: &Path) {
        let path = review_candidates_path(journal);
        create_parent(&path).unwrap();
        write_jsonl(
            &path,
            vec![json!({
                "key": candidate_key("anchor-a", "anchor-b"),
                "status": "open",
                "updated_at": "20260101T000000Z",
            })],
            AtomicWriteOptions::default(),
        )
        .unwrap();
    }

    #[test]
    fn accept_marks_row_and_persists() {
        let journal = temporary_journal("accept");
        seed(journal.path());

        let accepted = accept_candidate(journal.path(), "anchor-b", "anchor-a")
            .unwrap()
            .unwrap();

        assert_eq!(accepted["status"], "accepted");
        assert_eq!(
            load_candidates(journal.path()).unwrap()[0]["status"],
            "accepted"
        );
    }

    #[test]
    fn dismiss_records_sorted_anchors_and_shared_timestamp() {
        let journal = temporary_journal("dismiss");
        seed(journal.path());

        let dismissed = dismiss_candidate(journal.path(), "anchor-b", "anchor-a")
            .unwrap()
            .unwrap();

        assert_eq!(dismissed["status"], "dismissed");
        assert_eq!(dismissed["dismissed_anchor_a"], "anchor-a");
        assert_eq!(dismissed["dismissed_anchor_b"], "anchor-b");
        assert_eq!(dismissed["dismissed_at"], dismissed["updated_at"]);
    }

    #[test]
    fn missing_candidate_returns_none_without_creating_a_row() {
        let journal = temporary_journal("missing");
        seed(journal.path());

        assert!(
            dismiss_candidate(journal.path(), "missing-a", "missing-b")
                .unwrap()
                .is_none()
        );
        assert_eq!(load_candidates(journal.path()).unwrap().len(), 1);
    }

    #[test]
    fn concurrent_pair_recorders_preserve_each_others_rows() {
        let journal = temporary_journal("concurrent");
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for worker in 0..4 {
                let root = journal.path();
                handles.push(scope.spawn(move || {
                    for pair in 0..6 {
                        let suggestion = CandidatePairSuggestion {
                            source_anchors: BTreeSet::from([format!("source-{worker}-{pair}")]),
                            target_anchors: BTreeSet::from(["target".to_owned()]),
                            similarity: 0.5,
                            source_intervals: 30,
                            target_intervals: 30,
                            source_samples: Vec::new(),
                            target_samples: Vec::new(),
                        };
                        record_candidate_pair(root, &suggestion).unwrap();
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });
        let rows = load_candidates(journal.path()).unwrap();
        assert_eq!(rows.len(), 24);
        let keys = rows
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(keys.len(), 24);
    }
}
