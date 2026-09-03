// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared currentness evaluation for published timeline artifacts.

use std::path::Path;

use solstone_core_brain::fingerprint_sha256;

use crate::{AttemptOutcome, TimelineError, load_timeline_state};

/// Durable currentness of a syntactically and schema-valid timeline artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactCurrentness {
    Current,
    Stale,
    Missing,
}

/// SHA-256 of the exact serialized artifact text recorded at publication.
pub fn artifact_sha256(artifact_text: &str) -> String {
    fingerprint_sha256(artifact_text)
}

/// Evaluate an artifact against the durable timeline publication state.
///
/// `artifact_text` must be the exact text read from the artifact being evaluated.
/// Callers validate and bind the typed artifact before invoking this function.
pub fn evaluate_artifact_currentness(
    journal: &Path,
    subject: &str,
    artifact_input_digest: &str,
    artifact_generated_at_ms: i64,
    artifact_text: &str,
) -> Result<ArtifactCurrentness, TimelineError> {
    let state = load_timeline_state(journal)?;
    let Some(record) = state.artifacts.get(subject) else {
        return Ok(ArtifactCurrentness::Missing);
    };
    if record.input_digest != artifact_input_digest
        || record.artifact_sha256 != artifact_sha256(artifact_text)
    {
        return Ok(ArtifactCurrentness::Stale);
    }
    let has_newer_unsuccessful_attempt = state.attempts.iter().any(|(key, attempt)| {
        matches!(
            attempt.outcome,
            AttemptOutcome::Failed | AttemptOutcome::DurabilityUncertain
        ) && attempt_subject(key, &attempt.attempt_id).is_some_and(|attempt_subject| {
            attempt_subject == subject
                && attempt.started_at_ms > artifact_generated_at_ms
                && attempt.input_digest != artifact_input_digest
        })
    });
    Ok(if has_newer_unsuccessful_attempt {
        ArtifactCurrentness::Stale
    } else {
        ArtifactCurrentness::Current
    })
}

fn attempt_subject<'a>(key: &'a str, attempt_id: &str) -> Option<&'a str> {
    key.strip_suffix(&format!(":{attempt_id}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        ArtifactStateV1, AttemptStateV1, CURRENT_SCHEMA_VERSION, TimelineStateV1,
        save_timeline_state,
    };

    const SUBJECT: &str = "segment:20260520/_default/090000_300";
    const ARTIFACT: &str = "{\"timeline\":\"fixture\"}\n";

    fn state() -> TimelineStateV1 {
        TimelineStateV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: 1,
            artifacts: BTreeMap::from([(
                SUBJECT.to_owned(),
                ArtifactStateV1 {
                    input_digest: "known-input".to_owned(),
                    artifact_sha256: artifact_sha256(ARTIFACT),
                    published_at_ms: 10,
                    generation: 1,
                },
            )]),
            attempts: BTreeMap::new(),
        }
    }

    #[test]
    fn newer_failed_different_input_makes_last_good_artifact_stale() {
        let root = tempfile::tempdir().expect("journal");
        let mut state = state();
        state.attempts.insert(
            format!("{SUBJECT}:newer-failed"),
            AttemptStateV1 {
                attempt_id: "newer-failed".to_owned(),
                input_digest: "newer-input".to_owned(),
                started_at_ms: 11,
                finished_at_ms: Some(12),
                outcome: AttemptOutcome::Failed,
                detail: "fixture failure".to_owned(),
            },
        );
        save_timeline_state(root.path(), &state).expect("state");

        assert_eq!(
            evaluate_artifact_currentness(root.path(), SUBJECT, "known-input", 10, ARTIFACT)
                .expect("currentness"),
            ArtifactCurrentness::Stale
        );
    }

    #[test]
    fn matching_published_artifact_is_current() {
        let root = tempfile::tempdir().expect("journal");
        save_timeline_state(root.path(), &state()).expect("state");

        assert_eq!(
            evaluate_artifact_currentness(root.path(), SUBJECT, "known-input", 10, ARTIFACT)
                .expect("currentness"),
            ArtifactCurrentness::Current
        );
    }

    #[test]
    fn absent_artifact_record_is_missing() {
        let root = tempfile::tempdir().expect("journal");

        assert_eq!(
            evaluate_artifact_currentness(root.path(), SUBJECT, "known-input", 10, ARTIFACT)
                .expect("currentness"),
            ArtifactCurrentness::Missing
        );
    }
}
