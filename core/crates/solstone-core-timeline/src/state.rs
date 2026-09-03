// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    DetailedAtomicOutcome, LockOptions, atomic_replace_detailed, hold_lock,
};
use uuid::Uuid;

use crate::{CURRENT_SCHEMA_VERSION, TimelineError};

pub const MAX_DIAGNOSTIC_DETAIL_BYTES: usize = 512;

pub fn new_attempt_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}
const TRUNCATION_MARKER: &str = "...[truncated]";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Running,
    Published,
    Failed,
    DurabilityUncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStateV1 {
    pub input_digest: String,
    pub artifact_sha256: String,
    pub published_at_ms: i64,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptStateV1 {
    pub attempt_id: String,
    pub input_digest: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub outcome: AttemptOutcome,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineStateV1 {
    pub schema_version: u32,
    pub revision: u64,
    pub artifacts: BTreeMap<String, ArtifactStateV1>,
    pub attempts: BTreeMap<String, AttemptStateV1>,
}

impl TimelineStateV1 {
    pub fn empty() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: 0,
            artifacts: BTreeMap::new(),
            attempts: BTreeMap::new(),
        }
    }
}

pub fn timeline_state_path(journal: &Path) -> PathBuf {
    journal.join("health/timeline/state.json")
}

pub fn bounded_diagnostic_detail(value: &str) -> String {
    if value.len() <= MAX_DIAGNOSTIC_DETAIL_BYTES {
        return value.to_owned();
    }
    let prefix_limit = MAX_DIAGNOSTIC_DETAIL_BYTES - TRUNCATION_MARKER.len();
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        if next > prefix_limit {
            break;
        }
        end = next;
    }
    format!("{}{}", &value[..end], TRUNCATION_MARKER)
}

pub fn load_timeline_state(journal: &Path) -> Result<TimelineStateV1, TimelineError> {
    load_timeline_state_at(&timeline_state_path(journal))
}

pub fn save_timeline_state(journal: &Path, state: &TimelineStateV1) -> Result<(), TimelineError> {
    let path = timeline_state_path(journal);
    let _lock = hold_state_lock(&path)?;
    let mut state = state.clone();
    normalize_state_for_persistence(&mut state);
    save_timeline_state_at(&path, &state)
}

pub fn update_timeline_state<T>(
    journal: &Path,
    update: impl FnOnce(&mut TimelineStateV1) -> Result<T, TimelineError>,
) -> Result<T, TimelineError> {
    let path = timeline_state_path(journal);
    let _lock = hold_state_lock(&path)?;
    let mut state = load_timeline_state_at(&path)?;
    let result = update(&mut state)?;
    state.revision = state.revision.saturating_add(1);
    normalize_state_for_persistence(&mut state);
    save_timeline_state_at(&path, &state)?;
    Ok(result)
}

pub fn record_attempt_started(
    journal: &Path,
    subject: &str,
    mut attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    attempt.outcome = AttemptOutcome::Running;
    attempt.finished_at_ms = None;
    attempt.detail = bounded_diagnostic_detail(&attempt.detail);
    update_timeline_state(journal, |state| {
        state
            .attempts
            .insert(attempt_key(subject, &attempt), attempt);
        Ok(())
    })
}

pub fn record_attempt_outcome(
    journal: &Path,
    subject: &str,
    mut attempt: AttemptStateV1,
    outcome: AttemptOutcome,
    detail: &str,
    finished_at_ms: i64,
) -> Result<(), TimelineError> {
    attempt.outcome = outcome;
    attempt.finished_at_ms = Some(finished_at_ms);
    attempt.detail = bounded_diagnostic_detail(detail);
    update_timeline_state(journal, |state| {
        state
            .attempts
            .insert(attempt_key(subject, &attempt), attempt);
        Ok(())
    })
}

pub fn record_artifact_published(
    journal: &Path,
    subject: &str,
    mut attempt: AttemptStateV1,
    artifact: ArtifactStateV1,
    finished_at_ms: i64,
) -> Result<(), TimelineError> {
    attempt.outcome = AttemptOutcome::Published;
    attempt.finished_at_ms = Some(finished_at_ms);
    attempt.detail.clear();
    update_timeline_state(journal, |state| {
        state.artifacts.insert(subject.to_owned(), artifact);
        state
            .attempts
            .insert(attempt_key(subject, &attempt), attempt);
        Ok(())
    })
}

pub(crate) fn publication_is_confirmed(outcome: &DetailedAtomicOutcome) -> bool {
    matches!(outcome, DetailedAtomicOutcome::Published)
}

fn attempt_key(subject: &str, attempt: &AttemptStateV1) -> String {
    format!("{subject}:{}", attempt.attempt_id)
}

fn normalize_state_for_persistence(state: &mut TimelineStateV1) {
    for attempt in state.attempts.values_mut() {
        attempt.detail = bounded_diagnostic_detail(&attempt.detail);
    }
}

fn hold_state_lock(path: &Path) -> Result<solstone_core_journal_io::FileLock, TimelineError> {
    hold_lock(
        path,
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|error| TimelineError::LockContention {
        detail: bounded_diagnostic_detail(&error.to_string()),
    })
}

fn load_timeline_state_at(path: &Path) -> Result<TimelineStateV1, TimelineError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TimelineStateV1::empty());
        }
        Err(error) => return Err(TimelineError::Io(error)),
    };
    let state = serde_json::from_slice::<TimelineStateV1>(&bytes)?;
    if state.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(TimelineError::SchemaVersionMismatch {
            expected: CURRENT_SCHEMA_VERSION,
            actual: state.schema_version,
        });
    }
    Ok(state)
}

fn save_timeline_state_at(path: &Path, state: &TimelineStateV1) -> Result<(), TimelineError> {
    if state.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(TimelineError::SchemaVersionMismatch {
            expected: CURRENT_SCHEMA_VERSION,
            actual: state.schema_version,
        });
    }
    let mut bytes = serde_json::to_vec(state)?;
    bytes.push(b'\n');
    let outcome = atomic_replace_detailed(path, &bytes, 0o600)?;
    if publication_is_confirmed(&outcome) {
        return Ok(());
    }
    match outcome {
        DetailedAtomicOutcome::Published => unreachable!("confirmed publication returned early"),
        DetailedAtomicOutcome::PublishedDurabilityUncertain { source } => {
            Err(TimelineError::DurabilityUncertain {
                path: path.to_path_buf(),
                detail: bounded_diagnostic_detail(&source.to_string()),
            })
        }
        DetailedAtomicOutcome::PublishedParentPathRaced { sync_error } => {
            Err(TimelineError::PublicationNotConfirmed {
                path: path.to_path_buf(),
                detail: bounded_diagnostic_detail(
                    &sync_error
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "parent path raced".to_owned()),
                ),
            })
        }
        DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error,
        } => Err(TimelineError::PublicationNotConfirmed {
            path: path.to_path_buf(),
            detail: bounded_diagnostic_detail(&format!(
                "{observation}; {}",
                sync_error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "parent sync was not attempted".to_owned())
            )),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use solstone_core_journal_io::DetailedAtomicOutcome;

    use super::*;

    #[test]
    fn generated_attempt_ids_are_distinct_and_keep_the_subject_prefix() {
        let first = new_attempt_id("day-20260401");
        let second = new_attempt_id("day-20260401");

        assert_ne!(first, second);
        assert!(first.starts_with("day-20260401-"));
        assert!(second.starts_with("day-20260401-"));
    }

    #[test]
    fn detail_at_limit_is_unchanged() {
        let value = "x".repeat(MAX_DIAGNOSTIC_DETAIL_BYTES);
        assert_eq!(bounded_diagnostic_detail(&value), value);
    }

    #[test]
    fn detail_over_limit_is_marked_and_bounded() {
        let value = "x".repeat(MAX_DIAGNOSTIC_DETAIL_BYTES + 1);
        let bounded = bounded_diagnostic_detail(&value);
        assert!(bounded.ends_with(TRUNCATION_MARKER));
        assert!(bounded.len() <= MAX_DIAGNOSTIC_DETAIL_BYTES);
    }

    #[test]
    fn detail_truncation_respects_utf8_boundaries() {
        let prefix = "x".repeat(MAX_DIAGNOSTIC_DETAIL_BYTES - TRUNCATION_MARKER.len() - 1);
        let bounded = bounded_diagnostic_detail(&format!("{prefix}💥{}", "x".repeat(12)));
        assert!(bounded.is_char_boundary(bounded.len()));
        assert!(bounded.ends_with(TRUNCATION_MARKER));
        assert!(bounded.len() <= MAX_DIAGNOSTIC_DETAIL_BYTES);
    }

    #[test]
    fn only_confirmed_publication_is_success() {
        assert!(publication_is_confirmed(&DetailedAtomicOutcome::Published));
        assert!(!publication_is_confirmed(
            &DetailedAtomicOutcome::PublishedDurabilityUncertain {
                source: io::Error::other("sync"),
            }
        ));
        assert!(!publication_is_confirmed(
            &DetailedAtomicOutcome::PublishedParentPathRaced { sync_error: None }
        ));
        assert!(!publication_is_confirmed(
            &DetailedAtomicOutcome::PublishedParentPathUnverified {
                observation: io::Error::other("observe"),
                sync_error: None,
            }
        ));
    }

    #[test]
    fn saved_state_round_trips_under_the_state_lock() {
        let journal = tempfile::tempdir().unwrap();
        let state = TimelineStateV1::empty();

        save_timeline_state(journal.path(), &state).unwrap();

        assert_eq!(load_timeline_state(journal.path()).unwrap(), state);
    }

    #[test]
    fn whole_state_save_bounds_attempt_detail() {
        let journal = tempfile::tempdir().unwrap();
        let mut state = TimelineStateV1::empty();
        state.attempts.insert(
            "segment:attempt".to_owned(),
            AttemptStateV1 {
                attempt_id: "attempt".to_owned(),
                input_digest: "digest".to_owned(),
                started_at_ms: 1,
                finished_at_ms: None,
                outcome: AttemptOutcome::Running,
                detail: "x".repeat(MAX_DIAGNOSTIC_DETAIL_BYTES + 1),
            },
        );

        save_timeline_state(journal.path(), &state).unwrap();

        assert!(
            load_timeline_state(journal.path())
                .unwrap()
                .attempts
                .values()
                .all(|attempt| attempt.detail.len() <= MAX_DIAGNOSTIC_DETAIL_BYTES)
        );
    }
}
