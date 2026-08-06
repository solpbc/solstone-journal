// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable facet review candidates.

use std::error::Error;
use std::fmt;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, LockError, LockOptions, MalformedPolicy, hold_lock,
    read_jsonl, write_text,
};

use crate::{FacetTrustLockError, hold_facet_trust_lock};

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
