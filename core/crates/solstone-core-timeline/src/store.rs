// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{DetailedAtomicOutcome, atomic_replace_detailed};

use crate::{
    ArtifactStateV1, AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION, DayTimelineV1,
    MasterTimelineV1, SegmentBindingV1, SegmentSummaryV1, SegmentTimelineV1, TimelineError,
    TimelineLockRequest, TimelineLockSubject, acquire_timeline_locks, artifact_sha256,
    bounded_diagnostic_detail, load_timeline_state, origin_for_binding, record_artifact_published,
    record_attempt_outcome, record_attempt_started, segment_directory, validate_day_timeline,
    validate_master_timeline, validate_segment_timeline,
};

pub fn segment_timeline_path(
    journal: &Path,
    binding: &SegmentBindingV1,
) -> Result<PathBuf, TimelineError> {
    Ok(segment_directory(journal, binding)?.join("timeline.json"))
}

pub fn publish_segment_timeline(
    journal: &Path,
    timeline: &SegmentTimelineV1,
    attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    validate_segment_timeline(timeline)?;
    let binding = &timeline.binding;
    let subject = segment_subject_key(binding);
    let _locks = acquire_timeline_locks(
        journal,
        TimelineLockRequest {
            days: vec![binding.day.clone()],
            subjects: vec![TimelineLockSubject::Segment(binding.clone())],
            ..TimelineLockRequest::default()
        },
    )?;
    record_attempt_started(journal, &subject, attempt.clone())?;

    let path = segment_timeline_path(journal, binding)?;
    let serialized = serialize_timeline(timeline)?;
    publish_timeline(
        journal,
        &subject,
        path,
        &timeline.input_digest,
        timeline.generated_at_ms,
        serialized,
        attempt,
    )
}

pub fn day_timeline_path(journal: &Path, day: &str) -> PathBuf {
    journal.join("chronicle").join(day).join("timeline.json")
}

pub fn publish_day_timeline(
    journal: &Path,
    timeline: &DayTimelineV1,
    attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    validate_day_timeline(timeline)?;
    let subject = day_subject_key(&timeline.day);
    record_attempt_started(journal, &subject, attempt.clone())?;
    let serialized = serialize_timeline(timeline)?;
    publish_timeline(
        journal,
        &subject,
        day_timeline_path(journal, &timeline.day),
        &timeline.source_digest,
        timeline.generated_at_ms,
        serialized,
        attempt,
    )
}

pub fn master_timeline_path(journal: &Path) -> PathBuf {
    journal.join("timeline.json")
}

pub fn publish_master_timeline(
    journal: &Path,
    timeline: &MasterTimelineV1,
    attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    validate_master_timeline(timeline)?;
    let subject = master_subject_key();
    record_attempt_started(journal, subject, attempt.clone())?;
    let serialized = serialize_timeline(timeline)?;
    publish_timeline(
        journal,
        subject,
        master_timeline_path(journal),
        &timeline.source_digest,
        timeline.generated_at_ms,
        serialized,
        attempt,
    )
}

fn publish_timeline(
    journal: &Path,
    subject: &str,
    path: PathBuf,
    input_digest: &str,
    generated_at_ms: i64,
    serialized: String,
    attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    let publication = atomic_replace_detailed(&path, serialized.as_bytes(), 0o600);
    match publication {
        Err(error) => {
            let detail = bounded_diagnostic_detail(&error.to_string());
            let _ = record_attempt_outcome(
                journal,
                subject,
                attempt,
                AttemptOutcome::Failed,
                &detail,
                generated_at_ms,
            );
            Err(TimelineError::Atomic(error))
        }
        Ok(DetailedAtomicOutcome::Published) => {
            let generation = load_timeline_state(journal)?
                .artifacts
                .get(subject)
                .map(|state| state.generation.saturating_add(1))
                .unwrap_or(1);
            let artifact = ArtifactStateV1 {
                input_digest: input_digest.to_owned(),
                artifact_sha256: artifact_sha256(&serialized),
                published_at_ms: generated_at_ms,
                generation,
            };
            record_artifact_published(journal, subject, attempt, artifact, generated_at_ms)
        }
        Ok(DetailedAtomicOutcome::PublishedDurabilityUncertain { source }) => {
            let detail = bounded_diagnostic_detail(&source.to_string());
            let _ = record_attempt_outcome(
                journal,
                subject,
                attempt,
                AttemptOutcome::DurabilityUncertain,
                &detail,
                generated_at_ms,
            );
            Err(TimelineError::DurabilityUncertain { path, detail })
        }
        Ok(DetailedAtomicOutcome::PublishedParentPathRaced { sync_error }) => {
            let detail = bounded_diagnostic_detail(
                &sync_error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "parent path raced".to_owned()),
            );
            let _ = record_attempt_outcome(
                journal,
                subject,
                attempt,
                AttemptOutcome::Failed,
                &detail,
                generated_at_ms,
            );
            Err(TimelineError::PublicationNotConfirmed { path, detail })
        }
        Ok(DetailedAtomicOutcome::PublishedParentPathUnverified {
            observation,
            sync_error,
        }) => {
            let detail = bounded_diagnostic_detail(&format!(
                "{observation}; {}",
                sync_error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "parent sync was not attempted".to_owned())
            ));
            let _ = record_attempt_outcome(
                journal,
                subject,
                attempt,
                AttemptOutcome::Failed,
                &detail,
                generated_at_ms,
            );
            Err(TimelineError::PublicationNotConfirmed { path, detail })
        }
    }
}

pub fn publish_continuation_summary(
    journal: &Path,
    binding: SegmentBindingV1,
    predecessor_segment_key: String,
    input_digest: String,
    generated_at_ms: i64,
    attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    let timeline = SegmentTimelineV1 {
        schema_version: CURRENT_SCHEMA_VERSION,
        kind: crate::TimelineKind::Segment,
        summary: SegmentSummaryV1 {
            title: "Continued".to_owned(),
            description: "Unchanged from the prior window.".to_owned(),
            origin: origin_for_binding(&binding)?,
            continuation_of: Some(predecessor_segment_key),
        },
        binding,
        input_digest,
        generated_at_ms,
        provenance: None,
    };
    publish_segment_timeline(journal, &timeline, attempt)
}

pub fn segment_subject_key(binding: &SegmentBindingV1) -> String {
    format!(
        "segment:{}/{}/{}",
        binding.day, binding.stream, binding.segment
    )
}

pub fn day_subject_key(day: &str) -> String {
    format!("day:{day}")
}

pub const fn master_subject_key() -> &'static str {
    "master"
}

fn serialize_timeline<T: serde::Serialize>(timeline: &T) -> Result<String, TimelineError> {
    let mut serialized = serde_json::to_string(timeline)?;
    serialized.push('\n');
    Ok(serialized)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    #[cfg(unix)]
    use nix::errno::Errno;
    #[cfg(unix)]
    use solstone_core_journal_io::{
        BoundPublicationPrimitive, run_with_bound_publication_barrier,
        run_with_bound_publication_fault,
    };

    use super::*;
    use crate::{
        AttemptOutcome, CurationRecordV1, DayTimelineV1, MasterTimelineV1, TimelineKind,
        TimelineStateV1, load_timeline_state,
    };

    fn binding() -> SegmentBindingV1 {
        SegmentBindingV1 {
            day: "20260401".to_owned(),
            stream: "_default".to_owned(),
            segment: "080000_300".to_owned(),
        }
    }

    fn attempt() -> AttemptStateV1 {
        AttemptStateV1 {
            attempt_id: "attempt-1".to_owned(),
            input_digest: "input".to_owned(),
            started_at_ms: 1,
            finished_at_ms: None,
            outcome: AttemptOutcome::Running,
            detail: String::new(),
        }
    }

    fn day_timeline() -> DayTimelineV1 {
        DayTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Day,
            day: "20260401".to_owned(),
            source_digest: "input".to_owned(),
            generated_at_ms: 2,
            top_n: 4,
            segment_count: 1,
            hour_count: 0,
            hours: BTreeMap::new(),
            day_curation: CurationRecordV1 {
                input_digest: "input".to_owned(),
                candidate_count: 1,
                picks: Vec::new(),
                rationale: "fixture".to_owned(),
                error: None,
                provenance: None,
            },
        }
    }

    fn master_timeline() -> MasterTimelineV1 {
        MasterTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Master,
            source_digest: "input".to_owned(),
            generated_at_ms: 2,
            top_n: 4,
            months: BTreeMap::new(),
            year_top: Vec::new(),
            year_curation: CurationRecordV1 {
                input_digest: "input".to_owned(),
                candidate_count: 0,
                picks: Vec::new(),
                rationale: "fixture".to_owned(),
                error: None,
                provenance: None,
            },
        }
    }

    #[test]
    fn continuation_publication_writes_typed_artifact_and_confirmed_state() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("chronicle/20260401/080000_300")).unwrap();

        publish_continuation_summary(
            journal.path(),
            binding(),
            "070000_300".to_owned(),
            "input".to_owned(),
            2,
            attempt(),
        )
        .unwrap();

        let path = journal
            .path()
            .join("chronicle/20260401/080000_300/timeline.json");
        let timeline =
            serde_json::from_slice::<SegmentTimelineV1>(&fs::read(path).unwrap()).unwrap();
        assert_eq!(timeline.summary.title, "Continued");
        assert_eq!(
            timeline.summary.continuation_of.as_deref(),
            Some("070000_300")
        );

        let state = load_timeline_state(journal.path()).unwrap();
        assert_eq!(state.artifacts.len(), 1);
        assert!(
            state
                .attempts
                .values()
                .any(|value| value.outcome == AttemptOutcome::Published)
        );
        assert_ne!(state, TimelineStateV1::empty());
    }

    #[test]
    fn day_publication_writes_typed_artifact_and_day_subject_state() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
        let timeline = day_timeline();

        publish_day_timeline(journal.path(), &timeline, attempt()).unwrap();

        let published = serde_json::from_slice::<DayTimelineV1>(
            &fs::read(day_timeline_path(journal.path(), &timeline.day)).unwrap(),
        )
        .unwrap();
        assert_eq!(published, timeline);
        let state = load_timeline_state(journal.path()).unwrap();
        assert_eq!(
            state.artifacts[&day_subject_key("20260401")].input_digest,
            "input"
        );
        assert!(
            state
                .attempts
                .values()
                .any(|attempt| attempt.outcome == AttemptOutcome::Published)
        );
    }

    #[test]
    fn master_publication_writes_typed_artifact_and_master_subject_state() {
        let journal = tempfile::tempdir().unwrap();
        let timeline = master_timeline();

        publish_master_timeline(journal.path(), &timeline, attempt()).unwrap();

        let published = serde_json::from_slice::<MasterTimelineV1>(
            &fs::read(master_timeline_path(journal.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(published, timeline);
        let state = load_timeline_state(journal.path()).unwrap();
        assert_eq!(state.artifacts[master_subject_key()].input_digest, "input");
        assert!(
            state
                .attempts
                .values()
                .any(|attempt| attempt.outcome == AttemptOutcome::Published)
        );
    }

    #[cfg(unix)]
    #[test]
    fn publication_faults_never_advance_current_artifact_state() {
        struct Case {
            name: &'static str,
            ordinal: usize,
            expected_attempt: Option<AttemptOutcome>,
        }

        for case in [
            Case {
                name: "durable_state_write",
                ordinal: 1,
                expected_attempt: None,
            },
            Case {
                name: "artifact_write",
                ordinal: 2,
                expected_attempt: Some(AttemptOutcome::Failed),
            },
        ] {
            let journal = tempfile::tempdir().unwrap();
            fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
            let (result, consumed) = run_with_bound_publication_fault(
                BoundPublicationPrimitive::TempCreate,
                case.ordinal,
                Errno::EIO as i32,
                || publish_day_timeline(journal.path(), &day_timeline(), attempt()),
            );

            assert!(consumed, "{} fault must be consumed", case.name);
            assert!(
                matches!(result, Err(TimelineError::Atomic(_))),
                "{}",
                case.name
            );
            assert_no_current_artifact(journal.path(), &day_subject_key("20260401"));
            let state = load_timeline_state(journal.path()).unwrap();
            match case.expected_attempt {
                Some(outcome) => assert!(state.attempts.values().any(|attempt| {
                    attempt.outcome == outcome
                        && attempt.detail.len() <= crate::MAX_DIAGNOSTIC_DETAIL_BYTES
                })),
                None => assert!(state.attempts.is_empty()),
            }
            assert!(!day_timeline_path(journal.path(), "20260401").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn lock_acquisition_failure_never_starts_or_publishes_a_segment_attempt() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("health/timeline")).unwrap();
        fs::write(
            journal.path().join("health/timeline/locks"),
            b"not a directory",
        )
        .unwrap();

        let result = publish_segment_timeline(
            journal.path(),
            &SegmentTimelineV1 {
                schema_version: CURRENT_SCHEMA_VERSION,
                kind: crate::TimelineKind::Segment,
                binding: binding(),
                input_digest: "input".to_owned(),
                generated_at_ms: 2,
                summary: SegmentSummaryV1 {
                    title: "Title".to_owned(),
                    description: "Description".to_owned(),
                    origin: "20260401/080000_300".to_owned(),
                    continuation_of: None,
                },
                provenance: None,
            },
            attempt(),
        );

        assert!(matches!(result, Err(TimelineError::LockContention { .. })));
        assert_no_current_artifact(journal.path(), &segment_subject_key(&binding()));
        assert!(
            load_timeline_state(journal.path())
                .unwrap()
                .attempts
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn durability_uncertainty_records_only_an_uncertain_attempt() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
        let (result, consumed) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::ParentSync,
            2,
            Errno::EIO as i32,
            || publish_day_timeline(journal.path(), &day_timeline(), attempt()),
        );

        assert!(consumed);
        assert!(matches!(
            result,
            Err(TimelineError::DurabilityUncertain { .. })
        ));
        assert!(day_timeline_path(journal.path(), "20260401").is_file());
        assert_no_current_artifact(journal.path(), &day_subject_key("20260401"));
        assert!(
            load_timeline_state(journal.path())
                .unwrap()
                .attempts
                .values()
                .any(|attempt| {
                    attempt.outcome == AttemptOutcome::DurabilityUncertain
                        && attempt.detail.len() <= crate::MAX_DIAGNOSTIC_DETAIL_BYTES
                })
        );
    }

    #[cfg(unix)]
    #[test]
    fn parent_path_ambiguities_never_confirm_current_artifact_state() {
        for replace_parent in [false, true] {
            let journal = tempfile::tempdir().unwrap();
            let parent = journal.path().join("chronicle/20260401");
            fs::create_dir_all(&parent).unwrap();
            let relocated = journal.path().join("relocated-day");
            let parent_for_barrier = parent.clone();
            let relocated_for_barrier = relocated.clone();
            let (result, fired) = run_with_bound_publication_barrier(
                BoundPublicationPrimitive::ParentSync,
                2,
                move || {
                    fs::rename(&parent_for_barrier, &relocated_for_barrier).unwrap();
                    if replace_parent {
                        fs::create_dir(&parent_for_barrier).unwrap();
                    }
                },
                || publish_day_timeline(journal.path(), &day_timeline(), attempt()),
            );

            assert!(fired);
            let error = result.expect_err("parent ambiguity must not confirm publication");
            let TimelineError::PublicationNotConfirmed { detail, .. } = error else {
                panic!("expected a publication-not-confirmed error");
            };
            if replace_parent {
                assert!(detail.contains("parent path raced"));
            } else {
                assert!(!detail.contains("parent path raced"));
            }
            assert!(detail.len() <= crate::MAX_DIAGNOSTIC_DETAIL_BYTES);
            assert_no_current_artifact(journal.path(), &day_subject_key("20260401"));
            assert!(
                load_timeline_state(journal.path())
                    .unwrap()
                    .attempts
                    .values()
                    .any(|attempt| {
                        attempt.outcome == AttemptOutcome::Failed
                            && attempt.detail.len() <= crate::MAX_DIAGNOSTIC_DETAIL_BYTES
                    })
            );
            if replace_parent {
                assert!(!parent.join("timeline.json").exists());
            } else {
                assert!(!parent.exists());
            }
            assert!(relocated.join("timeline.json").is_file());
        }
    }

    #[cfg(unix)]
    fn assert_no_current_artifact(journal: &Path, subject: &str) {
        let state = load_timeline_state(journal).unwrap();
        assert!(!state.artifacts.contains_key(subject));
        assert!(
            state
                .attempts
                .values()
                .all(|attempt| attempt.outcome != AttemptOutcome::Published)
        );
    }
}
