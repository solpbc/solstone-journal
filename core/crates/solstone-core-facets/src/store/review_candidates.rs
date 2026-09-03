// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable facet review candidates.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, MalformedPolicy, hold_lock,
    read_jsonl, write_text,
};

use crate::{FacetTrustLockError, hold_facet_trust_lock};
use crate::{SpeculativeFacetCandidate, SpeculativeFacetSample};

use super::error::FacetStoreError;
use super::paths::review_candidates_path;

/// Failure while reading or modifying durable facet review candidates.
#[derive(Debug)]
pub enum FacetReviewCandidateError {
    TrustLock(FacetTrustLockError),
    Store(FacetStoreError),
    Lock(LockError),
    Write(AtomicWriteError),
}

impl fmt::Display for FacetReviewCandidateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustLock(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
        }
    }
}

impl Error for FacetReviewCandidateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::TrustLock(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Write(error) => Some(error),
        }
    }
}

/// Load facet review candidates, skipping malformed and non-object rows.
pub fn load_candidates(journal_root: &Path) -> Result<Vec<Value>, FacetReviewCandidateError> {
    let path = review_candidates_path(journal_root).map_err(FacetReviewCandidateError::Store)?;
    read_jsonl(&path, Vec::<Value>::new(), MalformedPolicy::WarnAndSkip)
        .map_err(|error| FacetReviewCandidateError::Store(error.into()))
        .map(|rows| rows.into_iter().filter(Value::is_object).collect())
}

/// Mark a facet review candidate accepted.
pub fn accept_candidate(
    journal_root: &Path,
    name_key: &str,
) -> Result<Option<Value>, FacetReviewCandidateError> {
    modify_candidates(journal_root, |rows| {
        let row = rows
            .iter_mut()
            .find(|row| row.get("name_key").and_then(Value::as_str) == Some(name_key))?;
        let object = row
            .as_object_mut()
            .expect("candidate reader returns objects");
        object.insert("status".to_owned(), Value::String("accepted".to_owned()));
        object.insert("updated_at".to_owned(), Value::String(now_iso()));
        Some(row.clone())
    })
}

/// Mark a facet review candidate dismissed and preserve its count watermark.
pub fn dismiss_candidate(
    journal_root: &Path,
    name_key: &str,
) -> Result<Option<Value>, FacetReviewCandidateError> {
    modify_candidates(journal_root, |rows| {
        let row = rows
            .iter_mut()
            .find(|row| row.get("name_key").and_then(Value::as_str) == Some(name_key))?;
        let object = row
            .as_object_mut()
            .expect("candidate reader returns objects");
        let count = object.get("count").cloned().unwrap_or(Value::Null);
        object.insert("status".to_owned(), Value::String("dismissed".to_owned()));
        object.insert("dismissed_count".to_owned(), count);
        object.insert("updated_at".to_owned(), Value::String(now_iso()));
        Some(row.clone())
    })
}

/// Record a batch of recurring speculative facet candidates.
///
/// Unlike Python's `record_facet_candidate`, this records the complete batch in
/// one locked read-modify-write cycle rather than acquiring the candidate-file
/// lock once per candidate.
///
/// This native path also holds the facet trust lock in addition to the
/// review-candidate file lock; the Python reference holds only its
/// candidate-file lock.
pub fn record_facet_candidates(
    journal_root: &Path,
    day: &str,
    candidates: &[SpeculativeFacetCandidate],
) -> Result<usize, FacetReviewCandidateError> {
    if candidates.is_empty() {
        return Ok(0);
    }

    modify_candidates(journal_root, |rows| {
        let mut touched = 0;
        for candidate in candidates {
            let samples = samples_value(&candidate.samples);
            let now = now_iso();
            if let Some(row) = rows.iter_mut().find(|row| {
                row.get("name_key").and_then(Value::as_str) == Some(candidate.name_key.as_str())
            }) {
                let object = row
                    .as_object_mut()
                    .expect("candidate reader returns objects");
                update_evidence_samples(object, samples);
                object.insert("count".to_owned(), Value::from(candidate.count));
                object.insert("window_days".to_owned(), Value::from(candidate.window_days));
                object.insert("last_surfaced".to_owned(), Value::String(day.to_owned()));
                object.insert("updated_at".to_owned(), Value::String(now));
            } else {
                rows.push(json!({
                    "name": candidate.name,
                    "name_key": candidate.name_key,
                    "status": "open",
                    "count": candidate.count,
                    "window_days": candidate.window_days,
                    "evidence": {"samples": samples},
                    "first_surfaced": day,
                    "last_surfaced": day,
                    "created_at": now,
                    "updated_at": now,
                }));
            }
            touched += 1;
        }
        touched
    })
}

/// Humanize a speculative-facet candidate name for use as a created facet's
/// display title. Older candidates were suggested before naming guidance
/// improved and are stored slug-style (e.g. "low_light_capture"); this never
/// lets that raw punctuation become an owner-visible facet title.
pub fn humanize_facet_title(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Derive the facet directory slug used by the Python facet creator.
pub fn facet_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in title.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            separator = false;
        } else if !slug.is_empty() {
            separator = true;
        }
    }
    slug
}

fn modify_candidates<T>(
    journal_root: &Path,
    modify: impl FnOnce(&mut Vec<Value>) -> T,
) -> Result<T, FacetReviewCandidateError> {
    let _trust =
        hold_facet_trust_lock(journal_root).map_err(FacetReviewCandidateError::TrustLock)?;
    let path = review_candidates_path(journal_root).map_err(FacetReviewCandidateError::Store)?;
    let _lock = hold_lock(
        &path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(FacetReviewCandidateError::Lock)?;
    let mut rows = load_candidates(journal_root)?;
    let result = modify(&mut rows);
    let contents = rows
        .iter()
        .map(|row| serde_json::to_string(row).expect("value serializes") + "\n")
        .collect::<String>();
    write_text(&path, &contents, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(FacetReviewCandidateError::Write)?;
    Ok(result)
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn samples_value(samples: &[SpeculativeFacetSample]) -> Value {
    serde_json::to_value(samples).expect("speculative facet samples serialize")
}

fn update_evidence_samples(object: &mut Map<String, Value>, samples: Value) {
    let evidence = object
        .entry("evidence".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(evidence) = evidence.as_object_mut() {
        evidence.insert("samples".to_owned(), samples);
    } else {
        *evidence = json!({"samples": samples});
    }
}
