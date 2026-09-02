// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use solstone_core_journal_io::{DetailedAtomicOutcome, atomic_replace_detailed};

use crate::{
    ArtifactStateV1, AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION, DayTimelineV1,
    MasterTimelineV1, SegmentBindingV1, SegmentSummaryV1, SegmentTimelineV1, TimelineError,
    TimelineLockRequest, TimelineLockSubject, acquire_timeline_locks, bounded_diagnostic_detail,
    load_timeline_state, origin_for_binding, record_artifact_published, record_attempt_outcome,
    record_attempt_started, segment_directory, validate_day_timeline, validate_master_timeline,
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
    let mut bytes = serde_json::to_vec(timeline)?;
    bytes.push(b'\n');
    publish_timeline(
        journal,
        &subject,
        path,
        &timeline.input_digest,
        timeline.generated_at_ms,
        bytes,
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
    let mut bytes = serde_json::to_vec(timeline)?;
    bytes.push(b'\n');
    publish_timeline(
        journal,
        &subject,
        day_timeline_path(journal, &timeline.day),
        &timeline.source_digest,
        timeline.generated_at_ms,
        bytes,
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
    let mut bytes = serde_json::to_vec(timeline)?;
    bytes.push(b'\n');
    publish_timeline(
        journal,
        subject,
        master_timeline_path(journal),
        &timeline.source_digest,
        timeline.generated_at_ms,
        bytes,
        attempt,
    )
}

fn publish_timeline(
    journal: &Path,
    subject: &str,
    path: PathBuf,
    input_digest: &str,
    generated_at_ms: i64,
    bytes: Vec<u8>,
    attempt: AttemptStateV1,
) -> Result<(), TimelineError> {
    let publication = atomic_replace_detailed(&path, &bytes, 0o600);
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
                artifact_sha256: sha256_hex(&bytes),
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

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
}
