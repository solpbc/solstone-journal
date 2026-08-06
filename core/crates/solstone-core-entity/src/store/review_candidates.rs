// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable entity merge-review candidates.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value};
use solstone_core_journal_io::AtomicWriteError;
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::LockError;
use solstone_core_journal_io::LockOptions;
use solstone_core_journal_io::MalformedPolicy;
use solstone_core_journal_io::hold_lock;
use solstone_core_journal_io::read_jsonl;
use solstone_core_journal_io::write_text;

use crate::{EntityTrustLockError, hold_entity_trust_lock};

use super::error::EntityStoreError;
use super::paths::review_candidates_path;

const DEFAULT_BASIS: &str = "name-variant";

/// Failure while recording a durable entity merge-review candidate.
#[derive(Debug)]
pub enum EntityReviewCandidateError {
    TrustLock(EntityTrustLockError),
    Store(EntityStoreError),
    Lock(LockError),
    Write(AtomicWriteError),
}

impl fmt::Display for EntityReviewCandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl Error for EntityReviewCandidateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Write(error) => Some(error),
        }
    }
}

/// Create or update one entity merge-review candidate.
#[allow(clippy::too_many_arguments)]
pub fn record_merge_candidate(
    journal_root: &Path,
    facet: &str,
    day: &str,
    source: &str,
    source_slug: &str,
    target: &str,
    target_slug: &str,
    evidence: &str,
    basis: Option<&str>,
    detections: Option<i64>,
    needs: Option<i64>,
) -> Result<(Value, bool), EntityReviewCandidateError> {
    let _trust =
        hold_entity_trust_lock(journal_root).map_err(EntityReviewCandidateError::TrustLock)?;
    let key = candidate_key(facet, source_slug, target_slug);
    let basis = basis.unwrap_or(DEFAULT_BASIS);
    mutate_candidates(journal_root, |rows| {
        let now = candidate_now_iso();
        if let Some(existing) = rows
            .iter_mut()
            .find(|row| candidate_key_for_row(row) == key)
        {
            let object = existing
                .as_object_mut()
                .expect("candidate reader returns objects");
            let evidence_value = object
                .entry("evidence".to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            let evidence_object = evidence_value
                .as_object_mut()
                .expect("candidate evidence is an object");
            evidence_object.insert("basis".to_owned(), Value::String(basis.to_owned()));
            evidence_object.insert("summary".to_owned(), Value::String(evidence.to_owned()));
            if let Some(detections) = detections {
                evidence_object.insert("detection_count".to_owned(), Value::from(detections));
            }
            if let Some(needs) = needs {
                evidence_object.insert("needs".to_owned(), Value::from(needs));
            }
            object.insert("last_surfaced".to_owned(), Value::String(day.to_owned()));
            object.insert("updated_at".to_owned(), Value::String(now));
            return Ok((existing.clone(), false));
        }

        let row = serde_json::json!({
            "facet": facet,
            "source": source,
            "source_slug": source_slug,
            "target": target,
            "target_slug": target_slug,
            "status": "open",
            "evidence": {
                "basis": basis,
                "summary": evidence,
                "detection_count": detections,
                "needs": needs,
            },
            "first_surfaced": day,
            "last_surfaced": day,
            "created_at": now,
            "updated_at": now,
        });
        rows.push(row.clone());
        Ok((row, true))
    })
}

/// Mark one entity merge-review candidate accepted, when it exists.
pub fn accept_merge_candidate(
    journal_root: &Path,
    facet: &str,
    source_slug: &str,
    target_slug: &str,
    merge_id: Option<&str>,
) -> Result<Option<Value>, EntityReviewCandidateError> {
    let _trust =
        hold_entity_trust_lock(journal_root).map_err(EntityReviewCandidateError::TrustLock)?;
    let key = candidate_key(facet, source_slug, target_slug);
    mutate_candidates(journal_root, |rows| {
        let Some(existing) = rows
            .iter_mut()
            .find(|row| candidate_key_for_row(row) == key)
        else {
            return Ok(None);
        };
        let object = existing
            .as_object_mut()
            .expect("candidate reader returns objects");
        object.insert("status".to_owned(), Value::String("accepted".to_owned()));
        if let Some(merge_id) = merge_id.filter(|merge_id| !merge_id.is_empty()) {
            object.insert("merge_id".to_owned(), Value::String(merge_id.to_owned()));
        }
        object.insert("updated_at".to_owned(), Value::String(candidate_now_iso()));
        Ok(Some(existing.clone()))
    })
}

/// Mark one entity merge-review candidate dismissed, when it exists.
pub fn dismiss_merge_candidate(
    journal_root: &Path,
    facet: &str,
    source_slug: &str,
    target_slug: &str,
) -> Result<Option<Value>, EntityReviewCandidateError> {
    let _trust =
        hold_entity_trust_lock(journal_root).map_err(EntityReviewCandidateError::TrustLock)?;
    let key = candidate_key(facet, source_slug, target_slug);
    mutate_candidates(journal_root, |rows| {
        let Some(existing) = rows
            .iter_mut()
            .find(|row| candidate_key_for_row(row) == key)
        else {
            return Ok(None);
        };
        let dismissed_detection_count = existing
            .get("evidence")
            .and_then(Value::as_object)
            .and_then(|evidence| evidence.get("detection_count"))
            .cloned()
            .unwrap_or(Value::Null);
        let object = existing
            .as_object_mut()
            .expect("candidate reader returns objects");
        object.insert("status".to_owned(), Value::String("dismissed".to_owned()));
        object.insert(
            "dismissed_detection_count".to_owned(),
            dismissed_detection_count,
        );
        object.insert("updated_at".to_owned(), Value::String(candidate_now_iso()));
        Ok(Some(existing.clone()))
    })
}

/// Load durable entity merge-review candidates, optionally filtered by facet and status.
pub fn load_merge_candidates(
    journal_root: &Path,
    facet: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<Value>, EntityStoreError> {
    let path = review_candidates_path(journal_root)?;
    let rows = read_jsonl(&path, Vec::<Value>::new(), MalformedPolicy::WarnAndSkip)?;
    Ok(rows
        .into_iter()
        .filter(Value::is_object)
        .filter(|row| {
            facet.is_none_or(|facet| row.get("facet").and_then(Value::as_str) == Some(facet))
        })
        .filter(|row| {
            status.is_none_or(|status| row.get("status").and_then(Value::as_str) == Some(status))
        })
        .collect())
}

fn mutate_candidates<T>(
    journal_root: &Path,
    mutate: impl FnOnce(&mut Vec<Value>) -> Result<T, EntityReviewCandidateError>,
) -> Result<T, EntityReviewCandidateError> {
    let path = review_candidates_path(journal_root).map_err(EntityReviewCandidateError::Store)?;
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(EntityReviewCandidateError::Lock)?;
    let mut rows = read_jsonl(&path, Vec::<Value>::new(), MalformedPolicy::WarnAndSkip)
        .map_err(|error| EntityReviewCandidateError::Store(EntityStoreError::from(error)))?
        .into_iter()
        .filter(Value::is_object)
        .collect();
    let result = mutate(&mut rows)?;
    let contents = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("value serializes") + "\n")
        .collect::<String>();
    write_text(&path, &contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(EntityReviewCandidateError::Write)?;
    Ok(result)
}

fn candidate_key(facet: &str, source_slug: &str, target_slug: &str) -> String {
    format!("{facet}|{source_slug}|{target_slug}")
}

fn candidate_key_for_row(row: &Value) -> String {
    candidate_key(
        row.get("facet").and_then(Value::as_str).unwrap_or_default(),
        row.get("source_slug")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        row.get("target_slug")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )
}

fn candidate_now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}
