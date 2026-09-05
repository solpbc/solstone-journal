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
    let has_newer_incomplete_attempt = state.attempts.iter().any(|(key, attempt)| {
        matches!(
            attempt.outcome,
            AttemptOutcome::Running | AttemptOutcome::Failed | AttemptOutcome::DurabilityUncertain
        ) && attempt_subject(key, &attempt.attempt_id).is_some_and(|attempt_subject| {
            attempt_subject == subject
                && attempt.started_at_ms >= artifact_generated_at_ms
                && attempt.input_digest != artifact_input_digest
        })
    });
    Ok(if has_newer_incomplete_attempt {
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
    use super::*;
    use crate::{ArtifactStateV1, AttemptStateV1, TimelineStateV1, save_timeline_state};

    const SUBJECT: &str = "segment:20260520/_default/090000_300";
    const OTHER: &str = "segment:20260520/audio/090000_300";
    const ARTIFACT: &str = "{\"timeline\":\"fixture\"}\n";
    const INPUT: &str = "known-input";

    // These verdicts are recorded against the original shared-document implementation.
    // Change only fixture persistence when moving state; the verdicts are the contract.
    #[test]
    fn frozen_currentness_verdict_table() {
        use ArtifactCurrentness::{Current, Missing, Stale};
        use AttemptOutcome::{DurabilityUncertain, Failed, Published, Running};

        struct Case {
            name: &'static str,
            published: bool,
            input: &'static str,
            text: &'static str,
            attempts: Vec<(&'static str, &'static str, i64, AttemptOutcome)>,
            expected: ArtifactCurrentness,
        }
        let mut cases = vec![
            Case {
                name: "no state",
                published: false,
                input: INPUT,
                text: ARTIFACT,
                attempts: vec![],
                expected: Missing,
            },
            Case {
                name: "matching publication",
                published: true,
                input: INPUT,
                text: ARTIFACT,
                attempts: vec![],
                expected: Current,
            },
            Case {
                name: "input mismatch",
                published: true,
                input: "changed",
                text: ARTIFACT,
                attempts: vec![],
                expected: Stale,
            },
            Case {
                name: "artifact bytes mismatch",
                published: true,
                input: INPUT,
                text: "changed artifact",
                attempts: vec![],
                expected: Stale,
            },
            Case {
                name: "attempt without publication",
                published: false,
                input: INPUT,
                text: ARTIFACT,
                attempts: vec![(SUBJECT, "changed", 11, Running)],
                expected: Missing,
            },
            Case {
                name: "later completion does not erase abandoned attempt",
                published: true,
                input: INPUT,
                text: ARTIFACT,
                attempts: vec![
                    (SUBJECT, "changed", 10, Running),
                    (SUBJECT, INPUT, 12, Published),
                ],
                expected: Stale,
            },
            Case {
                name: "later failed same-input refresh does not erase abandoned attempt",
                published: true,
                input: INPUT,
                text: ARTIFACT,
                attempts: vec![
                    (SUBJECT, "changed", 10, Running),
                    (SUBJECT, INPUT, 12, Failed),
                ],
                expected: Stale,
            },
            Case {
                name: "published different-input attempt is complete",
                published: true,
                input: INPUT,
                text: ARTIFACT,
                attempts: vec![(SUBJECT, "changed", 11, Published)],
                expected: Current,
            },
        ];
        for outcome in [Running, Failed, DurabilityUncertain] {
            for (name, subject, input, started, expected) in [
                ("older incomplete attempt", SUBJECT, "changed", 9, Current),
                (
                    "equal-time incomplete attempt",
                    SUBJECT,
                    "changed",
                    10,
                    Stale,
                ),
                ("newer incomplete attempt", SUBJECT, "changed", 11, Stale),
                ("same-input refresh", SUBJECT, INPUT, 11, Current),
                ("other subject", OTHER, "changed", 11, Current),
            ] {
                cases.push(Case {
                    name,
                    published: true,
                    input: INPUT,
                    text: ARTIFACT,
                    attempts: vec![(subject, input, started, outcome.clone())],
                    expected,
                });
            }
        }
        for case in cases {
            let root = tempfile::tempdir().expect("journal");
            let mut state = TimelineStateV1::empty();
            if case.published {
                state.artifacts.insert(
                    SUBJECT.to_owned(),
                    ArtifactStateV1 {
                        input_digest: INPUT.to_owned(),
                        artifact_sha256: artifact_sha256(ARTIFACT),
                        published_at_ms: 10,
                        generation: 1,
                    },
                );
            }
            for (index, (subject, input, started, outcome)) in case.attempts.iter().enumerate() {
                let id = format!("attempt-{index}");
                state.attempts.insert(
                    format!("{subject}:{id}"),
                    AttemptStateV1 {
                        attempt_id: id,
                        input_digest: (*input).to_owned(),
                        started_at_ms: *started,
                        finished_at_ms: (*outcome != Running).then_some(*started + 1),
                        outcome: outcome.clone(),
                        detail: String::new(),
                    },
                );
            }
            save_timeline_state(root.path(), &state).expect("state");
            assert_eq!(
                evaluate_artifact_currentness(root.path(), SUBJECT, case.input, 10, case.text)
                    .expect("currentness"),
                case.expected,
                "{}: {:?}",
                case.name,
                case.attempts,
            );
        }
    }
}
