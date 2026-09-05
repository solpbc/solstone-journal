// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use solstone_core_journal_io::{DetailedAtomicOutcome, atomic_replace_detailed};

use crate::{
    AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION, DayTimelineV1, MasterTimelineV1,
    PublishedArtifactV1, SegmentBindingV1, SegmentSummaryV1, SegmentTimelineV1, TimelineError,
    TimelineLockRequest, TimelineLockSet, TimelineLockSubject, acquire_timeline_locks,
    artifact_sha256, bounded_diagnostic_detail, origin_for_binding, record_artifact_published,
    record_attempt_outcome, record_attempt_started, resolve_activity_source, segment_directory,
    segment_input_digest, validate_day_timeline, validate_master_timeline,
    validate_segment_timeline,
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
    mut attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    validate_segment_timeline(timeline)?;
    let binding = &timeline.binding;
    let subject = segment_subject_key(binding);
    crate::ensure_timeline_conversion(journal, &segment_subject_key(&timeline.binding))?;
    let locks = acquire_timeline_locks(
        journal,
        TimelineLockRequest {
            subjects: vec![TimelineLockSubject::Segment(binding.clone())],
            ..TimelineLockRequest::default()
        },
    )?;
    attempt.input_digest.clone_from(&timeline.input_digest);
    record_attempt_started(journal, &subject, attempt.clone(), &locks)?;

    if let Err(error) = verify_segment_source(journal, timeline) {
        // A missing or unreadable source cannot be reconstructed into a live
        // snapshot, but it still invalidates the last-good artifact.  Give the
        // failed attempt a deterministic digest distinct from the artifact's
        // source digest before trying to capture more precise changed-source
        // evidence below.
        attempt.input_digest = artifact_sha256(&format!(
            "segment-source-unavailable:{}",
            timeline.input_digest
        ));
        if let Ok(Some(snapshot)) = resolve_activity_source(journal, &timeline.binding)
            && let Some(template) = timeline.source.as_ref()
            && let Ok(source) = source_with_snapshot(journal, template, snapshot)
            && let Ok(current_digest) = segment_input_digest(&timeline.binding, &source)
        {
            attempt.input_digest = current_digest;
        }
        let detail = bounded_diagnostic_detail(&error.to_string());
        record_terminal_failure(
            journal,
            &subject,
            attempt,
            AttemptOutcome::Failed,
            &detail,
            timeline.generated_at_ms,
            &locks,
        )?;
        return Err(error);
    }

    let path = segment_timeline_path(journal, binding)?;
    let serialized = serialize_timeline(timeline)?;
    publish_timeline(
        journal,
        &subject,
        path,
        PublishedArtifactV1 {
            input_digest: timeline.input_digest.clone(),
            artifact_sha256: artifact_sha256(&serialized),
            published_at_ms: timeline.generated_at_ms,
        },
        serialized,
        attempt,
        &locks,
    )
}

fn source_with_snapshot(
    journal: &Path,
    template: &crate::SegmentSourceV1,
    snapshot: crate::ActivitySourceSnapshot,
) -> Result<crate::SegmentSourceV1, TimelineError> {
    match template {
        crate::SegmentSourceV1::GeneratedActivity { schema_version, .. } => {
            Ok(crate::SegmentSourceV1::GeneratedActivity {
                schema_version: *schema_version,
                relative_path: snapshot.relative_path,
                sha256: snapshot.sha256,
            })
        }
        crate::SegmentSourceV1::Continuation {
            schema_version,
            predecessor_segment_key,
            change_evidence_relative_path,
            ..
        } => {
            let change_evidence_text = read_exact_text(journal, change_evidence_relative_path)?;
            Ok(crate::SegmentSourceV1::Continuation {
                schema_version: *schema_version,
                relative_path: snapshot.relative_path,
                sha256: snapshot.sha256,
                predecessor_segment_key: predecessor_segment_key.clone(),
                change_evidence_relative_path: change_evidence_relative_path.clone(),
                change_evidence_sha256: artifact_sha256(&change_evidence_text),
            })
        }
    }
}

pub fn verify_segment_source(
    journal: &Path,
    timeline: &SegmentTimelineV1,
) -> Result<(), TimelineError> {
    let expected =
        timeline
            .source
            .as_ref()
            .ok_or_else(|| TimelineError::InvalidSourceEvidence {
                detail: "segment artifact has no source binding".to_owned(),
            })?;
    let current = resolve_activity_source(journal, &timeline.binding)?.ok_or_else(|| {
        TimelineError::InvalidSourceEvidence {
            detail: format!("source {:?} is missing", expected.relative_path()),
        }
    })?;
    if current.relative_path != expected.relative_path() {
        return Err(TimelineError::InvalidSourceEvidence {
            detail: format!(
                "source path changed from {:?} to {:?}",
                expected.relative_path(),
                current.relative_path
            ),
        });
    }
    if current.sha256 != expected.sha256() {
        return Err(TimelineError::InvalidSourceEvidence {
            detail: format!(
                "source {:?} changed: expected {}, got {}",
                expected.relative_path(),
                expected.sha256(),
                current.sha256
            ),
        });
    }
    if let crate::SegmentSourceV1::Continuation {
        change_evidence_relative_path,
        change_evidence_sha256,
        ..
    } = expected
    {
        verify_exact_file_sha(
            journal,
            change_evidence_relative_path,
            change_evidence_sha256,
        )?;
    }
    let digest = segment_input_digest(&timeline.binding, expected)?;
    if digest != timeline.input_digest {
        return Err(TimelineError::DigestMismatch {
            expected: digest,
            actual: timeline.input_digest.clone(),
        });
    }
    Ok(())
}

fn read_exact_text(journal: &Path, relative_path: &str) -> Result<String, TimelineError> {
    let path = solstone_core_journal_io::contained_path(journal, relative_path)?;
    let bytes = std::fs::read(&path).map_err(|error| TimelineError::InvalidSourceEvidence {
        detail: format!("cannot read {relative_path}: {error}"),
    })?;
    String::from_utf8(bytes).map_err(|error| TimelineError::InvalidSourceEvidence {
        detail: format!("source {relative_path} is not UTF-8: {error}"),
    })
}

fn verify_exact_file_sha(
    journal: &Path,
    relative_path: &str,
    expected_sha256: &str,
) -> Result<(), TimelineError> {
    let text = read_exact_text(journal, relative_path)?;
    let actual = artifact_sha256(&text);
    if actual == expected_sha256 {
        Ok(())
    } else {
        Err(TimelineError::InvalidSourceEvidence {
            detail: format!(
                "source {relative_path:?} changed: expected {expected_sha256}, got {actual}"
            ),
        })
    }
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
    crate::ensure_timeline_conversion(journal, &day_subject_key(&timeline.day))?;
    let locks = acquire_timeline_locks(
        journal,
        TimelineLockRequest {
            subjects: vec![TimelineLockSubject::Day(timeline.day.clone())],
            ..TimelineLockRequest::default()
        },
    )?;
    let subject = day_subject_key(&timeline.day);
    record_attempt_started(journal, &subject, attempt.clone(), &locks)?;
    publish_validated_day_timeline(journal, timeline, attempt, &locks)
}

pub fn publish_day_timeline_after_start(
    journal: &Path,
    timeline: &DayTimelineV1,
    attempt: AttemptStateV1,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    let subject = day_subject_key(&timeline.day);
    if let Err(error) = validate_day_timeline(timeline) {
        let detail = bounded_diagnostic_detail(&error.to_string());
        record_terminal_failure(
            journal,
            &subject,
            attempt,
            AttemptOutcome::Failed,
            &detail,
            timeline.generated_at_ms,
            locks,
        )?;
        return Err(error);
    }
    publish_validated_day_timeline(journal, timeline, attempt, locks)
}

fn publish_validated_day_timeline(
    journal: &Path,
    timeline: &DayTimelineV1,
    attempt: AttemptStateV1,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    let subject = day_subject_key(&timeline.day);
    let serialized = serialize_timeline(timeline)?;
    publish_timeline(
        journal,
        &subject,
        day_timeline_path(journal, &timeline.day),
        PublishedArtifactV1 {
            input_digest: timeline.source_digest.clone(),
            artifact_sha256: artifact_sha256(&serialized),
            published_at_ms: timeline.generated_at_ms,
        },
        serialized,
        attempt,
        locks,
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
    crate::ensure_timeline_conversion(journal, master_subject_key())?;
    let locks = acquire_timeline_locks(
        journal,
        TimelineLockRequest {
            subjects: vec![TimelineLockSubject::Master],
            ..TimelineLockRequest::default()
        },
    )?;
    let subject = master_subject_key();
    record_attempt_started(journal, subject, attempt.clone(), &locks)?;
    publish_validated_master_timeline(journal, timeline, attempt, &locks)
}

pub fn publish_master_timeline_after_start(
    journal: &Path,
    timeline: &MasterTimelineV1,
    attempt: AttemptStateV1,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    let subject = master_subject_key();
    if let Err(error) = validate_master_timeline(timeline) {
        let detail = bounded_diagnostic_detail(&error.to_string());
        record_terminal_failure(
            journal,
            subject,
            attempt,
            AttemptOutcome::Failed,
            &detail,
            timeline.generated_at_ms,
            locks,
        )?;
        return Err(error);
    }
    publish_validated_master_timeline(journal, timeline, attempt, locks)
}

fn publish_validated_master_timeline(
    journal: &Path,
    timeline: &MasterTimelineV1,
    attempt: AttemptStateV1,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    let subject = master_subject_key();
    let serialized = serialize_timeline(timeline)?;
    publish_timeline(
        journal,
        subject,
        master_timeline_path(journal),
        PublishedArtifactV1 {
            input_digest: timeline.source_digest.clone(),
            artifact_sha256: artifact_sha256(&serialized),
            published_at_ms: timeline.generated_at_ms,
        },
        serialized,
        attempt,
        locks,
    )
}

fn publish_timeline(
    journal: &Path,
    subject: &str,
    path: PathBuf,
    published: PublishedArtifactV1,
    serialized: String,
    attempt: AttemptStateV1,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    locks.require_subject(journal, subject)?;
    crate::ensure_timeline_conversion(journal, subject)?;
    let generated_at_ms = published.published_at_ms;
    let publication = atomic_replace_detailed(&path, serialized.as_bytes(), 0o600);
    match publication {
        Err(error) => {
            let detail = bounded_diagnostic_detail(&error.to_string());
            record_terminal_failure(
                journal,
                subject,
                attempt.clone(),
                AttemptOutcome::Failed,
                &detail,
                generated_at_ms,
                locks,
            )?;
            Err(TimelineError::Atomic(error))
        }
        Ok(DetailedAtomicOutcome::Published) => {
            record_artifact_published(journal, subject, attempt, published, generated_at_ms, locks)
        }
        Ok(DetailedAtomicOutcome::PublishedDurabilityUncertain { source }) => {
            let detail = bounded_diagnostic_detail(&source.to_string());
            record_terminal_failure(
                journal,
                subject,
                attempt,
                AttemptOutcome::DurabilityUncertain,
                &detail,
                generated_at_ms,
                locks,
            )?;
            Err(TimelineError::DurabilityUncertain { path, detail })
        }
        Ok(DetailedAtomicOutcome::PublishedParentPathRaced { sync_error }) => {
            let detail = bounded_diagnostic_detail(
                &sync_error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "parent path raced".to_owned()),
            );
            record_terminal_failure(
                journal,
                subject,
                attempt,
                AttemptOutcome::Failed,
                &detail,
                generated_at_ms,
                locks,
            )?;
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
            record_terminal_failure(
                journal,
                subject,
                attempt,
                AttemptOutcome::Failed,
                &detail,
                generated_at_ms,
                locks,
            )?;
            Err(TimelineError::PublicationNotConfirmed { path, detail })
        }
    }
}

fn record_terminal_failure(
    journal: &Path,
    subject: &str,
    attempt: AttemptStateV1,
    outcome: AttemptOutcome,
    primary: &str,
    finished_at_ms: i64,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    record_attempt_outcome(
        journal,
        subject,
        attempt,
        outcome,
        primary,
        finished_at_ms,
        locks,
    )
    .map_err(|state_error| TimelineError::TerminalStateWriteFailed {
        primary: bounded_diagnostic_detail(primary),
        state: bounded_diagnostic_detail(&state_error.to_string()),
    })
}

pub fn publish_continuation_summary(
    journal: &Path,
    binding: SegmentBindingV1,
    predecessor_segment_key: String,
    generated_at_ms: i64,
    mut attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    let snapshot = resolve_activity_source(journal, &binding)?.ok_or_else(|| {
        TimelineError::InvalidSourceEvidence {
            detail: "continuation activity source is missing".to_owned(),
        }
    })?;
    let change_evidence_relative_path = format!(
        "chronicle/{}/talents/change.json",
        origin_for_binding(&binding)?
    );
    let change_evidence_text = read_exact_text(journal, &change_evidence_relative_path)?;
    let source = crate::SegmentSourceV1::Continuation {
        schema_version: crate::SEGMENT_SOURCE_SCHEMA_VERSION,
        relative_path: snapshot.relative_path,
        sha256: snapshot.sha256,
        predecessor_segment_key: predecessor_segment_key.clone(),
        change_evidence_relative_path,
        change_evidence_sha256: artifact_sha256(&change_evidence_text),
    };
    let input_digest = segment_input_digest(&binding, &source)?;
    attempt.input_digest.clone_from(&input_digest);
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
        source: Some(source),
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
        TimelineRecordV1, load_timeline_record,
    };

    fn poisoned_journal() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("health/timeline")).unwrap();
        fs::write(
            crate::timeline_conversion_marker_path(root.path()),
            br#"{"schema_version":1,"refused":{}}"#,
        )
        .unwrap();
        fs::write(
            crate::timeline_state_path(root.path()),
            b"poisoned legacy document",
        )
        .unwrap();
        root
    }

    fn read_record(journal: &Path, subject: &str) -> Result<TimelineRecordV1, TimelineError> {
        Ok(load_timeline_record(journal, subject)?
            .unwrap_or_else(|| TimelineRecordV1::empty(subject)))
    }

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
            started_at_ms: 2,
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

    fn published_continuation() -> (tempfile::TempDir, SegmentTimelineV1, PathBuf) {
        let journal = poisoned_journal();
        let talents = journal.path().join("chronicle/20260401/080000_300/talents");
        fs::create_dir_all(&talents).unwrap();
        fs::write(talents.join("activity.md"), "activity").unwrap();
        fs::write(
            talents.join("change.json"),
            r#"{"change_class":"redundant"}"#,
        )
        .unwrap();
        let mut continuation_attempt = attempt();
        continuation_attempt.attempt_id = "attempt-2".to_owned();
        continuation_attempt.started_at_ms = 2;
        publish_continuation_summary(
            journal.path(),
            binding(),
            "070000_300".to_owned(),
            2,
            continuation_attempt,
        )
        .unwrap();
        let path = segment_timeline_path(journal.path(), &binding()).unwrap();
        let timeline =
            serde_json::from_slice::<SegmentTimelineV1>(&fs::read(&path).unwrap()).unwrap();
        (journal, timeline, path)
    }

    #[test]
    fn continuation_publication_writes_typed_artifact_and_confirmed_state() {
        let (journal, timeline, path) = published_continuation();
        assert_eq!(timeline.summary.title, "Continued");
        assert_eq!(
            timeline.summary.continuation_of.as_deref(),
            Some("070000_300")
        );

        let state = read_record(journal.path(), &segment_subject_key(&binding())).unwrap();
        assert!(state.published.is_some());
        assert!(
            state
                .attempts
                .iter()
                .any(|value| value.outcome == AttemptOutcome::Published)
        );
        assert_ne!(state, TimelineRecordV1::empty(&day_subject_key("20260401")));

        let last_good = fs::read(&path).unwrap();
        fs::write(
            journal
                .path()
                .join("chronicle/20260401/080000_300/talents/change.json"),
            r#"{"change_class":"meaningful"}"#,
        )
        .unwrap();
        let mut stale_attempt = attempt();
        stale_attempt.attempt_id = "attempt-3".to_owned();
        stale_attempt.started_at_ms = 3;
        publish_segment_timeline(journal.path(), &timeline, stale_attempt)
            .expect_err("changed continuation evidence must not publish");
        assert_eq!(fs::read(&path).unwrap(), last_good);
        let state = read_record(journal.path(), &segment_subject_key(&binding())).unwrap();
        let failed = state
            .attempts
            .iter()
            .find(|attempt| attempt.attempt_id == "attempt-3")
            .expect("terminal failed attempt");
        assert_eq!(failed.outcome, AttemptOutcome::Failed);
        assert_ne!(failed.input_digest, timeline.input_digest);
    }

    #[test]
    fn missing_or_unreadable_change_evidence_invalidates_a_continuation() {
        for replacement in [None, Some(vec![0xff, 0xfe])] {
            let (journal, timeline, path) = published_continuation();
            let change = journal
                .path()
                .join("chronicle/20260401/080000_300/talents/change.json");
            match replacement {
                None => fs::remove_file(change).unwrap(),
                Some(bytes) => fs::write(change, bytes).unwrap(),
            }
            let last_good = fs::read(&path).unwrap();
            let mut stale_attempt = attempt();
            stale_attempt.attempt_id = "attempt-3".to_owned();
            stale_attempt.started_at_ms = 3;
            publish_segment_timeline(journal.path(), &timeline, stale_attempt)
                .expect_err("unavailable continuation evidence must not publish");

            assert_eq!(fs::read(&path).unwrap(), last_good);
            let state = read_record(journal.path(), &segment_subject_key(&binding())).unwrap();
            let failed = state
                .attempts
                .iter()
                .find(|attempt| attempt.attempt_id == "attempt-3")
                .expect("terminal failed attempt");
            assert_ne!(failed.input_digest, timeline.input_digest);
            assert_eq!(
                crate::evaluate_artifact_currentness(
                    journal.path(),
                    &segment_subject_key(&binding()),
                    &timeline.input_digest,
                    timeline.generated_at_ms,
                    &String::from_utf8(last_good).unwrap(),
                )
                .unwrap(),
                crate::ArtifactCurrentness::Stale
            );
        }
    }

    #[test]
    fn changed_activity_cannot_publish_a_stale_segment_result() {
        let journal = poisoned_journal();
        let activity = journal
            .path()
            .join("chronicle/20260401/080000_300/talents/activity.md");
        fs::create_dir_all(activity.parent().unwrap()).unwrap();
        fs::write(&activity, "activity V1").unwrap();
        let snapshot = resolve_activity_source(journal.path(), &binding())
            .unwrap()
            .unwrap();
        let source = crate::SegmentSourceV1::GeneratedActivity {
            schema_version: crate::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: snapshot.relative_path,
            sha256: snapshot.sha256,
        };
        let input_digest = segment_input_digest(&binding(), &source).unwrap();
        let timeline = SegmentTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: crate::TimelineKind::Segment,
            binding: binding(),
            input_digest: input_digest.clone(),
            source: Some(source),
            generated_at_ms: 2,
            summary: SegmentSummaryV1 {
                title: "V1 result".to_owned(),
                description: "Generated from activity V1".to_owned(),
                origin: "20260401/080000_300".to_owned(),
                continuation_of: None,
            },
            provenance: None,
        };
        let mut first_attempt = attempt();
        first_attempt.input_digest.clone_from(&input_digest);
        publish_segment_timeline(journal.path(), &timeline, first_attempt).unwrap();
        let path = segment_timeline_path(journal.path(), &binding()).unwrap();
        let last_good = fs::read(&path).unwrap();

        fs::write(&activity, "activity V2").unwrap();
        let mut stale_attempt = attempt();
        stale_attempt.attempt_id = "attempt-2".to_owned();
        stale_attempt.started_at_ms = 3;
        let error = publish_segment_timeline(journal.path(), &timeline, stale_attempt)
            .expect_err("V1 result must not publish after activity V2");

        assert!(matches!(error, TimelineError::InvalidSourceEvidence { .. }));
        assert_eq!(fs::read(&path).unwrap(), last_good);
        let state = read_record(journal.path(), &segment_subject_key(&binding())).unwrap();
        let failed = state
            .attempts
            .iter()
            .find(|attempt| attempt.attempt_id == "attempt-2")
            .expect("terminal failed attempt");
        assert_eq!(failed.outcome, AttemptOutcome::Failed);
        assert_ne!(failed.input_digest, input_digest);
        let artifact_text = String::from_utf8(last_good).unwrap();
        assert_eq!(
            crate::evaluate_artifact_currentness(
                journal.path(),
                &segment_subject_key(&binding()),
                &timeline.input_digest,
                timeline.generated_at_ms,
                &artifact_text,
            )
            .unwrap(),
            crate::ArtifactCurrentness::Stale
        );
    }

    #[test]
    fn missing_or_unreadable_activity_invalidates_the_last_good_segment_result() {
        for replacement in [None, Some(vec![0xff, 0xfe])] {
            let journal = poisoned_journal();
            let activity = journal
                .path()
                .join("chronicle/20260401/080000_300/talents/activity.md");
            fs::create_dir_all(activity.parent().unwrap()).unwrap();
            fs::write(&activity, "activity V1").unwrap();
            let snapshot = resolve_activity_source(journal.path(), &binding())
                .unwrap()
                .unwrap();
            let source = crate::SegmentSourceV1::GeneratedActivity {
                schema_version: crate::SEGMENT_SOURCE_SCHEMA_VERSION,
                relative_path: snapshot.relative_path,
                sha256: snapshot.sha256,
            };
            let input_digest = segment_input_digest(&binding(), &source).unwrap();
            let timeline = SegmentTimelineV1 {
                schema_version: CURRENT_SCHEMA_VERSION,
                kind: crate::TimelineKind::Segment,
                binding: binding(),
                input_digest: input_digest.clone(),
                source: Some(source),
                generated_at_ms: 2,
                summary: SegmentSummaryV1 {
                    title: "V1 result".to_owned(),
                    description: "Generated from activity V1".to_owned(),
                    origin: "20260401/080000_300".to_owned(),
                    continuation_of: None,
                },
                provenance: None,
            };
            publish_segment_timeline(journal.path(), &timeline, attempt()).unwrap();
            let path = segment_timeline_path(journal.path(), &binding()).unwrap();
            let last_good = fs::read(&path).unwrap();

            match replacement {
                None => fs::remove_file(&activity).unwrap(),
                Some(bytes) => fs::write(&activity, bytes).unwrap(),
            }
            let mut stale_attempt = attempt();
            stale_attempt.attempt_id = "attempt-2".to_owned();
            stale_attempt.started_at_ms = 3;
            publish_segment_timeline(journal.path(), &timeline, stale_attempt)
                .expect_err("missing or unreadable source must prevent publication");

            assert_eq!(fs::read(&path).unwrap(), last_good);
            let state = read_record(journal.path(), &segment_subject_key(&binding())).unwrap();
            let failed = state
                .attempts
                .iter()
                .find(|attempt| attempt.attempt_id == "attempt-2")
                .expect("terminal failed attempt");
            assert_eq!(failed.outcome, AttemptOutcome::Failed);
            assert_ne!(failed.input_digest, input_digest);
            assert_eq!(
                crate::evaluate_artifact_currentness(
                    journal.path(),
                    &segment_subject_key(&binding()),
                    &timeline.input_digest,
                    timeline.generated_at_ms,
                    &String::from_utf8(last_good).unwrap(),
                )
                .unwrap(),
                crate::ArtifactCurrentness::Stale
            );
        }
    }

    #[test]
    fn day_publication_writes_typed_artifact_and_day_subject_state() {
        let journal = poisoned_journal();
        fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
        let timeline = day_timeline();

        publish_day_timeline(journal.path(), &timeline, attempt()).unwrap();

        let published = serde_json::from_slice::<DayTimelineV1>(
            &fs::read(day_timeline_path(journal.path(), &timeline.day)).unwrap(),
        )
        .unwrap();
        assert_eq!(published, timeline);
        let state = read_record(journal.path(), &day_subject_key("20260401")).unwrap();
        assert_eq!(state.published.as_ref().unwrap().input_digest, "input");
        assert!(
            state
                .attempts
                .iter()
                .any(|attempt| attempt.outcome == AttemptOutcome::Published)
        );
    }

    #[test]
    fn day_validation_is_nonmutating_before_start_and_terminal_after_start() {
        let journal = poisoned_journal();
        let mut invalid = day_timeline();
        invalid.kind = crate::TimelineKind::Segment;

        let public_error = publish_day_timeline(journal.path(), &invalid, attempt())
            .expect_err("public API must reject malformed input");
        assert!(matches!(
            public_error,
            TimelineError::SchemaKindMismatch { .. }
        ));
        assert_eq!(
            read_record(journal.path(), &day_subject_key("20260401")).unwrap(),
            TimelineRecordV1::empty(&day_subject_key("20260401"))
        );

        let locks = acquire_timeline_locks(
            journal.path(),
            TimelineLockRequest {
                subjects: vec![TimelineLockSubject::Day("20260401".to_owned())],
                ..TimelineLockRequest::default()
            },
        )
        .unwrap();
        fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
        record_attempt_started(
            journal.path(),
            &day_subject_key("20260401"),
            attempt(),
            &locks,
        )
        .unwrap();
        let started_error =
            publish_day_timeline_after_start(journal.path(), &invalid, attempt(), &locks)
                .expect_err("started attempt must reject malformed input");
        assert!(matches!(
            started_error,
            TimelineError::SchemaKindMismatch { .. }
        ));
        assert!(
            read_record(journal.path(), &day_subject_key("20260401"))
                .unwrap()
                .attempts
                .iter()
                .any(|attempt| attempt.outcome == AttemptOutcome::Failed)
        );
    }

    #[test]
    fn master_publication_writes_typed_artifact_and_master_subject_state() {
        let journal = poisoned_journal();
        let timeline = master_timeline();

        publish_master_timeline(journal.path(), &timeline, attempt()).unwrap();

        let published = serde_json::from_slice::<MasterTimelineV1>(
            &fs::read(master_timeline_path(journal.path())).unwrap(),
        )
        .unwrap();
        assert_eq!(published, timeline);
        let state = read_record(journal.path(), master_subject_key()).unwrap();
        assert_eq!(state.published.as_ref().unwrap().input_digest, "input");
        assert!(
            state
                .attempts
                .iter()
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
            let journal = poisoned_journal();
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
            let state = read_record(journal.path(), &day_subject_key("20260401")).unwrap();
            match case.expected_attempt {
                Some(outcome) => assert!(state.attempts.iter().any(|attempt| {
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
    fn publication_failure_surfaces_a_second_terminal_state_write_failure() {
        let journal = poisoned_journal();
        fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
        let locks = acquire_timeline_locks(
            journal.path(),
            TimelineLockRequest {
                subjects: vec![TimelineLockSubject::Day("20260401".to_owned())],
                ..TimelineLockRequest::default()
            },
        )
        .unwrap();
        fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
        record_attempt_started(
            journal.path(),
            &day_subject_key("20260401"),
            attempt(),
            &locks,
        )
        .unwrap();
        let (result, consumed) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::TempCreate,
            1,
            Errno::EIO as i32,
            || {
                record_terminal_failure(
                    journal.path(),
                    &day_subject_key("20260401"),
                    attempt(),
                    AttemptOutcome::Failed,
                    "artifact publication failed",
                    3,
                    &locks,
                )
            },
        );

        assert!(consumed, "terminal-state fault must be consumed");
        let error = result.expect_err("publication and terminal state cannot both succeed");
        let TimelineError::TerminalStateWriteFailed { primary, state } = error else {
            panic!("expected combined terminal-state failure, got {error:?}");
        };
        assert!(!primary.is_empty());
        assert!(!state.is_empty());
        assert!(primary.len() <= crate::MAX_DIAGNOSTIC_DETAIL_BYTES);
        assert!(state.len() <= crate::MAX_DIAGNOSTIC_DETAIL_BYTES);
        assert!(
            read_record(journal.path(), &day_subject_key("20260401"))
                .unwrap()
                .attempts
                .iter()
                .any(|attempt| attempt.outcome == AttemptOutcome::Running)
        );
        assert_no_current_artifact(journal.path(), &day_subject_key("20260401"));
    }

    #[cfg(unix)]
    #[test]
    fn lock_acquisition_failure_never_starts_or_publishes_a_segment_attempt() {
        let journal = poisoned_journal();
        fs::create_dir_all(journal.path().join("health/timeline")).unwrap();
        fs::write(
            journal.path().join("health/timeline/locks"),
            b"not a directory",
        )
        .unwrap();

        let source = crate::SegmentSourceV1::GeneratedActivity {
            schema_version: crate::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: "chronicle/20260401/080000_300/talents/activity.md".to_owned(),
            sha256: "a".repeat(64),
        };
        let input_digest = segment_input_digest(&binding(), &source).unwrap();
        let result = publish_segment_timeline(
            journal.path(),
            &SegmentTimelineV1 {
                schema_version: CURRENT_SCHEMA_VERSION,
                kind: crate::TimelineKind::Segment,
                binding: binding(),
                input_digest,
                source: Some(source),
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
            read_record(journal.path(), &segment_subject_key(&binding()))
                .unwrap()
                .attempts
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn durability_uncertainty_records_only_an_uncertain_attempt() {
        let journal = poisoned_journal();
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
            read_record(journal.path(), &day_subject_key("20260401"))
                .unwrap()
                .attempts
                .iter()
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
            let journal = poisoned_journal();
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
            let detail = match error {
                TimelineError::PublicationNotConfirmed { detail, .. } if replace_parent => detail,
                TimelineError::TerminalStateWriteFailed { primary, state } if !replace_parent => {
                    assert!(!state.is_empty());
                    primary
                }
                other => panic!("unexpected parent-move failure: {other:?}"),
            };
            if replace_parent {
                assert!(detail.contains("parent path raced"));
            } else {
                assert!(!detail.contains("parent path raced"));
            }
            assert!(detail.len() <= crate::MAX_DIAGNOSTIC_DETAIL_BYTES);
            assert_no_current_artifact(journal.path(), &day_subject_key("20260401"));
            let remaining = read_record(journal.path(), &day_subject_key("20260401")).unwrap();
            if replace_parent {
                assert!(
                    remaining
                        .attempts
                        .iter()
                        .any(|attempt| attempt.outcome == AttemptOutcome::Failed)
                );
            } else {
                assert!(remaining.attempts.is_empty());
                let moved =
                    crate::read_timeline_record_at(&relocated.join(crate::TIMELINE_RECORD_NAME))
                        .unwrap()
                        .unwrap();
                assert!(moved.published.is_none());
                assert!(
                    moved
                        .attempts
                        .iter()
                        .any(|attempt| attempt.outcome == AttemptOutcome::Running)
                );
            }
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
        let state = read_record(journal, subject).unwrap();
        assert!(state.published.is_none());
        assert!(
            state
                .attempts
                .iter()
                .all(|attempt| attempt.outcome != AttemptOutcome::Published)
        );
    }
    #[test]
    fn after_start_publisher_rejects_the_wrong_subject_before_writing_artifact() {
        let journal = poisoned_journal();
        fs::create_dir_all(journal.path().join("chronicle/20260401")).unwrap();
        let locks = acquire_timeline_locks(
            journal.path(),
            TimelineLockRequest {
                subjects: vec![TimelineLockSubject::Master],
                ..TimelineLockRequest::default()
            },
        )
        .unwrap();
        assert!(matches!(
            publish_day_timeline_after_start(journal.path(), &day_timeline(), attempt(), &locks),
            Err(TimelineError::LockContention { .. })
        ));
        assert!(!day_timeline_path(journal.path(), "20260401").exists());
        assert!(
            !crate::timeline_record_path(journal.path(), "day:20260401")
                .unwrap()
                .exists()
        );
    }
}
