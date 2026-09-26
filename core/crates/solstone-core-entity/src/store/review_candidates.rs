// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable entity merge-review candidates.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value};
use solstone_core_journal_io::AtomicWriteError;
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::LockError;
use solstone_core_journal_io::LockOptions;
use solstone_core_journal_io::hold_lock;
use solstone_core_journal_io::write_text;
use solstone_core_journal_io::{MalformedPolicy, read_jsonl};

use crate::{EntityTrustLockError, hold_entity_trust_lock};

use super::error::EntityStoreError;
use super::lifecycle::resolve_entity_dir;
use super::merge_payload::{list_entity_merge_payload_ids, load_entity_merge_payload};
use super::paths::review_candidates_path;

const DEFAULT_BASIS: &str = "name-variant";

/// Failure while recording a durable entity merge-review candidate.
#[derive(Debug)]
pub enum EntityReviewCandidateError {
    TrustLock(EntityTrustLockError),
    Store(EntityStoreError),
    Lock(LockError),
    Write(AtomicWriteError),
    RecordedMerge(String),
}

impl fmt::Display for EntityReviewCandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::RecordedMerge(error) => formatter.write_str(error),
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
            Self::RecordedMerge(_) => None,
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
    let census = super::census::scan_identity_census(journal_root)
        .map_err(EntityReviewCandidateError::Store)?;
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
            object.insert("updated_at".to_owned(), Value::String(now.clone()));
            super::review_policy::apply_merge_candidate_review_policy(object, &census, &now);
            return Ok((existing.clone(), false));
        }

        let mut row = serde_json::json!({
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
        super::review_policy::apply_merge_candidate_review_policy(
            row.as_object_mut().expect("candidate object"),
            &census,
            &now,
        );
        rows.push(row.clone());
        Ok((row, true))
    })
}

/// Prepared proposal file; owner decisions are preserved in the after-image.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreparedMergeProposals {
    pub before: Option<String>,
    pub after: String,
}

pub fn prepare_merge_proposals(
    root: &Path,
    proposals: &[Value],
) -> Result<PreparedMergeProposals, String> {
    let _trust = hold_entity_trust_lock(root).map_err(|e| e.to_string())?;
    let census = super::census::scan_identity_census(root).map_err(|e| e.to_string())?;
    let path = review_candidates_path(root).map_err(|e| e.to_string())?;
    let _lock = hold_lock(&path, LockOptions::default()).map_err(|e| e.to_string())?;
    let before = proposal_bytes(&path)?;
    let mut rows: Vec<Value> = before
        .as_deref()
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|e| format!("malformed merge candidate: {e}"))
        })
        .collect::<Result<_, _>>()?;
    if rows.iter().any(|row| !row.is_object()) {
        return Err("malformed merge candidate object".into());
    }
    let now = candidate_now_iso();
    for proposal in proposals {
        for key in [
            "facet",
            "source",
            "source_slug",
            "target",
            "target_slug",
            "day",
            "summary",
        ] {
            if proposal.get(key).and_then(Value::as_str).is_none() {
                return Err(format!("merge proposal missing {key}"));
            }
        }
        let key = candidate_key_for_row(proposal);
        if let Some(existing) = rows
            .iter_mut()
            .find(|row| candidate_key_for_row(row) == key)
        {
            if matches!(
                existing.get("status").and_then(Value::as_str),
                Some("accepted" | "dismissed" | "resolved")
            ) {
                continue;
            }
            let object = existing.as_object_mut().ok_or("malformed candidate")?;
            object.insert("last_surfaced".into(), proposal["day"].clone());
            object.insert("updated_at".into(), Value::String(now.clone()));
            object.insert("evidence".into(), serde_json::json!({"basis":"name-variant", "summary":proposal["summary"], "detection_count":null, "needs":null}));
        } else {
            rows.push(serde_json::json!({
                "facet":proposal["facet"], "source":proposal["source"], "source_slug":proposal["source_slug"],
                "target":proposal["target"], "target_slug":proposal["target_slug"], "status":"open",
                "evidence":{"basis":"name-variant", "summary":proposal["summary"], "detection_count":null, "needs":null},
                "first_surfaced":proposal["day"], "last_surfaced":proposal["day"], "created_at":now, "updated_at":now,
            }));
        }
    }
    for row in rows.iter_mut().filter_map(Value::as_object_mut) {
        super::review_policy::apply_merge_candidate_review_policy(row, &census, &now);
    }
    let after = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("Value serializes") + "\n")
        .collect();
    Ok(PreparedMergeProposals { before, after })
}

fn proposal_bytes(path: &Path) -> Result<Option<String>, String> {
    solstone_core_journal_io::read_optional_text(path).map_err(|error| error.to_string())
}

pub fn publish_merge_proposals(
    root: &Path,
    batch: &PreparedMergeProposals,
    allow_before: bool,
    start: impl FnOnce() -> Result<(), String>,
    receipt: impl FnOnce() -> Result<(), String>,
) -> Result<(), crate::ReviewOwnerError> {
    use crate::{ReviewOwnerConflictKind, ReviewOwnerError};
    let _trust =
        hold_entity_trust_lock(root).map_err(|e| ReviewOwnerError::failed(e.to_string()))?;
    let path = review_candidates_path(root).map_err(|e| ReviewOwnerError::failed(e.to_string()))?;
    let _lock = hold_lock(&path, LockOptions::default())
        .map_err(|e| ReviewOwnerError::failed(e.to_string()))?;
    let current = proposal_bytes(&path).map_err(ReviewOwnerError::failed)?;
    if current.as_deref() != Some(batch.after.as_str()) {
        match rebase_merge_proposals(batch, current.as_deref()).map_err(ReviewOwnerError::failed)? {
            Rebase::Published => {}
            Rebase::Pending(bytes) if allow_before => {
                start().map_err(ReviewOwnerError::failed)?;
                write_text(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) })
                    .map_err(|e| ReviewOwnerError::failed(e.to_string()))?;
            }
            Rebase::Pending(_) | Rebase::Conflict => {
                return Err(ReviewOwnerError::conflict(
                    ReviewOwnerConflictKind::MergeProposalsChanged,
                    "conflict: merge proposals changed after preparation",
                ));
            }
        }
    }
    receipt().map_err(ReviewOwnerError::failed)
}

enum Rebase {
    Published,
    Pending(String),
    Conflict,
}

/// The candidate file is journal-wide, while a batch owns only the rows it
/// changes. Rows other facets wrote since preparation are kept as they are; the
/// batch conflicts only when one of its own rows moved.
fn rebase_merge_proposals(
    batch: &PreparedMergeProposals,
    current: Option<&str>,
) -> Result<Rebase, String> {
    let whole_file = || -> Result<Rebase, String> {
        Ok(if current == batch.before.as_deref() {
            Rebase::Pending(batch.after.clone())
        } else {
            Rebase::Conflict
        })
    };
    let (Some(before), Some(after), Some(current_rows)) = (
        keyed_lines(batch.before.as_deref())?,
        keyed_lines(Some(&batch.after))?,
        keyed_lines(current)?,
    ) else {
        return whole_file();
    };
    let before: HashMap<&str, &str> = before.iter().map(|(k, l)| (k.as_str(), *l)).collect();
    let after_keys: HashSet<&str> = after.iter().map(|(k, _)| k.as_str()).collect();
    if before.keys().any(|key| !after_keys.contains(key)) {
        return whole_file();
    }
    let changed: Vec<(&str, &str)> = after
        .iter()
        .map(|(k, l)| (k.as_str(), *l))
        .filter(|(key, line)| before.get(key) != Some(line))
        .collect();
    let now: HashMap<&str, &str> = current_rows.iter().map(|(k, l)| (k.as_str(), *l)).collect();
    if changed.iter().all(|(key, line)| now.get(key) == Some(line)) {
        return Ok(Rebase::Published);
    }
    if !changed
        .iter()
        .all(|(key, _)| now.get(key) == before.get(key))
    {
        return Ok(Rebase::Conflict);
    }
    let replacements: HashMap<&str, &str> = changed.iter().copied().collect();
    let mut bytes = String::new();
    for (key, line) in &current_rows {
        bytes.push_str(replacements.get(key.as_str()).unwrap_or(line));
        bytes.push('\n');
    }
    for (key, line) in &changed {
        if !now.contains_key(key) {
            bytes.push_str(line);
            bytes.push('\n');
        }
    }
    Ok(Rebase::Pending(bytes))
}

/// Candidate lines keyed by facet and pair; `None` when a key repeats.
fn keyed_lines(text: Option<&str>) -> Result<Option<Vec<(String, &str)>>, String> {
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for line in text.unwrap_or_default().lines() {
        if line.trim().is_empty() {
            continue;
        }
        let row: Value =
            serde_json::from_str(line).map_err(|e| format!("malformed merge candidate: {e}"))?;
        if !row.is_object() {
            return Err("malformed merge candidate object".into());
        }
        let key = candidate_key_for_row(&row);
        if !seen.insert(key.clone()) {
            return Ok(None);
        }
        rows.push((key, line));
    }
    Ok(Some(rows))
}

/// Mark one entity merge-review candidate accepted, when it exists.
pub fn find_active_recorded_merge(
    journal_root: &Path,
    source_slug: &str,
    target_slug: &str,
) -> Result<Option<String>, EntityReviewCandidateError> {
    let target_dir = resolve_entity_dir(journal_root, target_slug)
        .map_err(|error| EntityReviewCandidateError::RecordedMerge(error.to_string()))?;
    let ids = list_entity_merge_payload_ids(journal_root, &target_dir)
        .map_err(|error| EntityReviewCandidateError::RecordedMerge(error.to_string()))?;
    let mut matching = None;
    for id in ids {
        let payload = load_entity_merge_payload(journal_root, &target_dir, &id)
            .map_err(|error| EntityReviewCandidateError::RecordedMerge(error.to_string()))?;
        if payload["source_id"] == source_slug && payload["target_id"] == target_slug {
            if matching.is_some() {
                return Err(EntityReviewCandidateError::RecordedMerge(
                    "multiple active merges match the candidate".to_owned(),
                ));
            }
            matching = Some(id);
        }
    }
    Ok(matching)
}

fn recorded_merge_matches(
    journal_root: &Path,
    source_slug: &str,
    target_slug: &str,
    merge_id: &str,
) -> Result<bool, EntityReviewCandidateError> {
    let target_dir = resolve_entity_dir(journal_root, target_slug)
        .map_err(|error| EntityReviewCandidateError::RecordedMerge(error.to_string()))?;
    let payload = load_entity_merge_payload(journal_root, &target_dir, merge_id)
        .map_err(|error| EntityReviewCandidateError::RecordedMerge(error.to_string()))?;
    Ok(payload["source_id"] == source_slug && payload["target_id"] == target_slug)
}

/// Reconcile every open suggestion whose source identity was merged.
pub fn accept_merge_candidate(
    journal_root: &Path,
    facet: &str,
    source_slug: &str,
    target_slug: &str,
    merge_id: Option<&str>,
) -> Result<Option<Value>, EntityReviewCandidateError> {
    let _trust =
        hold_entity_trust_lock(journal_root).map_err(EntityReviewCandidateError::TrustLock)?;
    if let Some(merge_id) = merge_id
        && !recorded_merge_matches(journal_root, source_slug, target_slug, merge_id)?
    {
        return Err(EntityReviewCandidateError::RecordedMerge(
            "candidate merge has no matching active record".to_owned(),
        ));
    }
    let key = candidate_key(facet, source_slug, target_slug);
    mutate_candidates(journal_root, |rows| {
        if !rows.iter().any(|row| candidate_key_for_row(row) == key) {
            return Ok(None);
        }
        let now = candidate_now_iso();
        for row in rows.iter_mut() {
            if merge_id.is_some() && row.get("status").and_then(Value::as_str) != Some("open") {
                continue;
            }
            let source = row.get("source_slug").and_then(Value::as_str);
            let target = row.get("target_slug").and_then(Value::as_str);
            let same_pair = source == Some(source_slug) && target == Some(target_slug);
            if !same_pair
                && (merge_id.is_none()
                    || (source != Some(source_slug) && target != Some(source_slug)))
            {
                continue;
            }
            let object = row
                .as_object_mut()
                .expect("candidate reader returns objects");
            object.insert("updated_at".to_owned(), Value::String(now.clone()));
            if same_pair {
                object.insert("status".to_owned(), Value::String("accepted".to_owned()));
                if let Some(merge_id) = merge_id.filter(|merge_id| !merge_id.is_empty()) {
                    object.insert("merge_id".to_owned(), Value::String(merge_id.to_owned()));
                }
            } else {
                object.insert("status".to_owned(), Value::String("resolved".to_owned()));
                object.insert(
                    "resolved_by_merge_id".to_owned(),
                    Value::String(merge_id.expect("checked above").to_owned()),
                );
            }
        }
        Ok(rows
            .iter()
            .find(|row| candidate_key_for_row(row) == key)
            .cloned())
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
    let result = read_candidate_rows(&path, MalformedPolicy::Skip)?;
    Ok(result
        .into_iter()
        .filter(|row| {
            facet.is_none_or(|facet| row.get("facet").and_then(Value::as_str) == Some(facet))
        })
        .filter(|row| {
            status.is_none_or(|status| row.get("status").and_then(Value::as_str) == Some(status))
        })
        .collect())
}

pub(crate) fn mutate_candidates<T>(
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
    let mut rows = read_candidate_rows(&path, MalformedPolicy::Raise)
        .map_err(EntityReviewCandidateError::Store)?;
    let result = mutate(&mut rows)?;
    let contents = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("value serializes") + "\n")
        .collect::<String>();
    write_text(&path, &contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(EntityReviewCandidateError::Write)?;
    Ok(result)
}

fn read_candidate_rows(
    path: &Path,
    policy: MalformedPolicy,
) -> Result<Vec<Value>, EntityStoreError> {
    let rows = read_jsonl::<Map<String, Value>>(path, Vec::new(), policy)?;
    for row in &rows {
        if let Some(review) = row.get("review") {
            let valid = review
                .as_object()
                .ok_or("review must be an object")
                .and_then(|review| super::review_policy::validate_review_object(review, true));
            if let Err(detail) = valid {
                return Err(solstone_core_journal_io::ReadError::Io {
                    path: path.to_owned(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, detail),
                }
                .into());
            }
        }
    }
    Ok(rows.into_iter().map(Value::Object).collect())
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
