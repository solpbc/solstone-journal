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
    #[error("invalid speaker review candidate: {0}")]
    InvalidRow(&'static str),
    #[error("speaker keep-separate read failed: {0}")]
    KeepSeparate(#[from] crate::keep_separate::KeepSeparateError),
}

/// Load name-variant review candidates, skipping malformed JSONL rows.
pub fn load_candidates(journal_root: &Path) -> Result<Vec<Value>, SpeakerReviewCandidateError> {
    Ok(read_jsonl(
        review_candidates_path(journal_root),
        Vec::new(),
        MalformedPolicy::WarnAndSkip,
    )?)
}

/// Record a detected name variant without reopening an accepted or dismissed pair.
pub fn record_name_variant_candidate(
    journal_root: &Path,
    candidate: &crate::name_variant_scan::NameVariantCandidate,
) -> Result<(Value, bool, bool), SpeakerReviewCandidateError> {
    let path = review_candidates_path(journal_root);
    create_parent(&path)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let mut rows: Vec<Value> = read_jsonl(&path, Vec::new(), MalformedPolicy::Raise)?;
    if rows.iter().any(|row| !row.is_object()) {
        return Err(SpeakerReviewCandidateError::InvalidRow(
            "expected JSON objects",
        ));
    }
    let existing = find_candidate(&mut rows, &candidate.source_id, &candidate.target_id);
    let created = existing.is_none();
    let mut row = existing.cloned().unwrap_or_else(|| serde_json::json!({}));
    let mut evidence = row
        .get("evidence")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let count = evidence
        .get("detection_count")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(SpeakerReviewCandidateError::InvalidRow(
            "detection count overflow",
        ))?;
    let suppressed_now = crate::keep_separate::find_assertion(
        journal_root,
        &candidate.source_id,
        &candidate.target_id,
    )?
    .is_some_and(|assertion| {
        assertion
            .sources
            .iter()
            .any(|source| count <= source.detection_count)
    });
    let now = now_iso();
    evidence.extend(serde_json::json!({
        "basis": "speaker-name-variant",
        "summary": format!("{} and {} have matching speaker voiceprints (similarity {:.4}).", candidate.source_label, candidate.target_label, candidate.similarity),
        "similarity": candidate.similarity,
        "detection_count": count,
        "readiness": candidate.readiness,
    }).as_object().expect("object literal").clone());
    let object = row.as_object_mut().expect("validated object");
    object.extend(
        serde_json::json!({
            "source_id": candidate.source_id, "source_label": candidate.source_label,
            "target_id": candidate.target_id, "target_label": candidate.target_label,
            "similarity": candidate.similarity, "readiness": candidate.readiness,
            "evidence": evidence, "last_surfaced": now, "updated_at": now,
        })
        .as_object()
        .expect("object literal")
        .clone(),
    );
    if created {
        object.insert("first_surfaced".into(), Value::String(now.clone()));
        object.insert("created_at".into(), Value::String(now));
        object.insert("status".into(), Value::String("open".into()));
    }
    let terminal = matches!(
        object.get("status").and_then(Value::as_str),
        Some("accepted" | "dismissed")
    );
    let suppressed = suppressed_now && !terminal;
    if !terminal {
        if suppressed {
            object.insert("status".into(), Value::String("suppressed".into()));
            object.insert("suppressed_by_keep_separate".into(), Value::Bool(true));
            object.insert("suppressed_detection_count".into(), Value::from(count));
        } else if object.get("suppressed_by_keep_separate") == Some(&Value::Bool(true)) {
            object.insert("status".into(), Value::String("open".into()));
            object.remove("suppressed_by_keep_separate");
            object.remove("suppressed_detection_count");
        }
    }
    if let Some(existing) = find_candidate(&mut rows, &candidate.source_id, &candidate.target_id) {
        *existing = row.clone();
    } else {
        rows.push(row.clone());
    }
    write_jsonl(&path, rows, AtomicWriteOptions::default())?;
    Ok((row, created, suppressed))
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
    use serde_json::json;

    use super::*;

    fn temporary_journal(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("solstone-speaker-review-{name}-"))
            .tempdir_in("/var/tmp")
            .unwrap()
    }

    fn detection() -> crate::name_variant_scan::NameVariantCandidate {
        crate::name_variant_scan::NameVariantCandidate {
            source_id: "alice".into(),
            source_label: "Alice".into(),
            target_id: "alicia".into(),
            target_label: "Alicia".into(),
            similarity: 0.97,
            readiness: "ready".into(),
        }
    }

    #[test]
    fn recording_updates_one_symmetric_pair_and_preserves_extra_evidence() {
        let journal = temporary_journal("record");
        let (first, created, suppressed) =
            record_name_variant_candidate(journal.path(), &detection()).unwrap();
        assert!(created && !suppressed);
        assert_eq!(first["evidence"]["detection_count"], 1);
        let mut stored = first.clone();
        stored["evidence"]["retained"] = json!("original");
        write_jsonl(
            review_candidates_path(journal.path()),
            vec![stored],
            AtomicWriteOptions::default(),
        )
        .unwrap();
        let mut reverse = detection();
        std::mem::swap(&mut reverse.source_id, &mut reverse.target_id);
        std::mem::swap(&mut reverse.source_label, &mut reverse.target_label);
        let (second, created, suppressed) =
            record_name_variant_candidate(journal.path(), &reverse).unwrap();
        assert!(!created && !suppressed);
        assert_eq!(second["evidence"]["detection_count"], 2);
        assert_eq!(second["evidence"]["retained"], "original");
        assert_eq!(second["first_surfaced"], first["first_surfaced"]);
        assert_eq!(second["created_at"], first["created_at"]);
        assert_eq!(load_candidates(journal.path()).unwrap().len(), 1);
    }

    #[test]
    fn keep_separate_watermark_expires_but_owner_terminal_choices_never_reopen() {
        let journal = temporary_journal("suppression");
        crate::keep_separate::record_keep_separate_assertion(
            journal.path(),
            "alice",
            "alicia",
            "review",
            None,
            1,
        )
        .unwrap();
        let (first, _, suppressed) =
            record_name_variant_candidate(journal.path(), &detection()).unwrap();
        assert!(suppressed);
        assert_eq!(first["status"], "suppressed");
        let (second, _, suppressed) =
            record_name_variant_candidate(journal.path(), &detection()).unwrap();
        assert!(!suppressed);
        assert_eq!(second["status"], "open");
        assert!(second.get("suppressed_by_keep_separate").is_none());
        dismiss_candidate(journal.path(), "alice", "alicia").unwrap();
        assert_eq!(
            record_name_variant_candidate(journal.path(), &detection())
                .unwrap()
                .0["status"],
            "dismissed"
        );
        accept_candidate(journal.path(), "alice", "alicia", Some("merge-1")).unwrap();
        crate::keep_separate::record_keep_separate_assertion(
            journal.path(),
            "alice",
            "alicia",
            "review",
            None,
            99,
        )
        .unwrap();
        let (accepted, _, suppressed) =
            record_name_variant_candidate(journal.path(), &detection()).unwrap();
        assert!(!suppressed);
        assert_eq!(accepted["status"], "accepted");
        assert_eq!(accepted["merge_id"], "merge-1");
    }

    #[test]
    fn concurrent_recorders_keep_every_detection_in_one_pair() {
        let journal = temporary_journal("concurrent");
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let root = journal.path();
                scope.spawn(move || {
                    for _ in 0..10 {
                        record_name_variant_candidate(root, &detection()).unwrap();
                    }
                });
            }
        });
        let rows = load_candidates(journal.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["evidence"]["detection_count"], 40);
    }

    #[test]
    fn recording_refuses_damaged_stores_without_rewriting_them() {
        let journal = temporary_journal("damaged");
        let path = review_candidates_path(journal.path());
        create_parent(&path).unwrap();
        for bad in ["{bad\n", "42\n"] {
            fs::write(&path, bad).unwrap();
            assert!(record_name_variant_candidate(journal.path(), &detection()).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        }
        seed(journal.path());
        let before = fs::read(&path).unwrap();
        fs::write(
            journal.path().join("speakers/keep-separate.jsonl"),
            "{bad\n",
        )
        .unwrap();
        assert!(record_name_variant_candidate(journal.path(), &detection()).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
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
        seed(journal.path());

        let accepted = accept_candidate(journal.path(), "alicia", "alice", Some("merge-7"))
            .unwrap()
            .unwrap();

        assert_eq!(accepted["status"], "accepted");
        assert_eq!(accepted["merge_id"], "merge-7");
        assert!(accepted["updated_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(
            load_candidates(journal.path()).unwrap()[0]["status"],
            "accepted"
        );
    }

    #[test]
    fn dismiss_captures_detection_count_and_persists() {
        let journal = temporary_journal("dismiss");
        seed(journal.path());

        let dismissed = dismiss_candidate(journal.path(), "alice", "alicia")
            .unwrap()
            .unwrap();

        assert_eq!(dismissed["status"], "dismissed");
        assert_eq!(dismissed["dismissed_detection_count"], 7);
        assert_eq!(
            load_candidates(journal.path()).unwrap()[0]["dismissed_detection_count"],
            7
        );
    }

    #[test]
    fn missing_candidate_returns_none_without_creating_a_row() {
        let journal = temporary_journal("missing");
        seed(journal.path());

        assert!(
            accept_candidate(journal.path(), "missing", "candidate", None)
                .unwrap()
                .is_none()
        );
        assert_eq!(load_candidates(journal.path()).unwrap().len(), 1);
    }
}
