// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{DetailedAtomicOutcome, atomic_replace_detailed, contained_path};
use uuid::Uuid;

use crate::{
    CURRENT_SCHEMA_VERSION, SegmentBindingV1, TimelineError, TimelineLockSet, TimelineLockSubject,
};

pub const TIMELINE_RECORD_NAME: &str = "timeline.state.json";
pub const MAX_DIAGNOSTIC_DETAIL_BYTES: usize = 512;
const TRUNCATION_MARKER: &str = "...[truncated]";

pub fn new_attempt_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

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
pub struct PublishedArtifactV1 {
    pub input_digest: String,
    pub artifact_sha256: String,
    pub published_at_ms: i64,
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
pub struct TimelineRecordV1 {
    pub schema_version: u32,
    pub subject: String,
    pub published: Option<PublishedArtifactV1>,
    pub attempts: Vec<AttemptStateV1>,
}

impl TimelineRecordV1 {
    pub fn empty(subject: &str) -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            subject: subject.to_owned(),
            published: None,
            attempts: Vec::new(),
        }
    }
}

pub fn timeline_state_path(journal: &Path) -> PathBuf {
    journal.join("health/timeline/state.json")
}

pub(crate) fn parse_subject(subject: &str) -> Result<TimelineLockSubject, TimelineError> {
    let invalid = || TimelineError::StateUnavailable {
        subject: subject.to_owned(),
        detail: "invalid timeline subject".to_owned(),
    };
    let day_valid = |day: &str| day.len() == 8 && day.bytes().all(|b| b.is_ascii_digit());
    if subject == "master" {
        return Ok(TimelineLockSubject::Master);
    }
    if let Some(day) = subject.strip_prefix("day:") {
        return if day_valid(day) {
            Ok(TimelineLockSubject::Day(day.to_owned()))
        } else {
            Err(invalid())
        };
    }
    if let Some(key) = subject.strip_prefix("segment:") {
        let parts = key.split('/').collect::<Vec<_>>();
        if parts.len() == 3
            && day_valid(parts[0])
            && parts
                .iter()
                .all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.contains(['\\', ':', '\0']))
        {
            return Ok(TimelineLockSubject::Segment(SegmentBindingV1 {
                day: parts[0].to_owned(),
                stream: parts[1].to_owned(),
                segment: parts[2].to_owned(),
            }));
        }
    }
    Err(invalid())
}

pub fn timeline_record_path(journal: &Path, subject: &str) -> Result<PathBuf, TimelineError> {
    let relative = match parse_subject(subject)? {
        TimelineLockSubject::Master => TIMELINE_RECORD_NAME.to_owned(),
        TimelineLockSubject::Day(day) => format!("chronicle/{day}/{TIMELINE_RECORD_NAME}"),
        TimelineLockSubject::Segment(binding) => {
            let origin = crate::origin_for_binding(&binding)?;
            format!("chronicle/{origin}/{TIMELINE_RECORD_NAME}")
        }
    };
    // Resolve containment through the parent only. Resolving the final component
    // would hide a record symlink from read_timeline_record_at and redirect writes.
    let parent = Path::new(&relative)
        .parent()
        .expect("record path has a parent");
    let parent = if parent.as_os_str().is_empty() {
        solstone_core_journal_io::realpath_non_strict(journal)?
    } else {
        contained_path(journal, &parent.to_string_lossy())?
    };
    Ok(parent.join(TIMELINE_RECORD_NAME))
}

pub fn read_timeline_record_at(path: &Path) -> Result<Option<TimelineRecordV1>, TimelineError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(TimelineError::StateUnavailable { subject: path.display().to_string(), detail: "record path is a symbolic link; preserve its target and repair this path before regeneration".to_owned() }),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let record: TimelineRecordV1 = serde_json::from_slice(&bytes)?;
    if record.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(TimelineError::SchemaVersionMismatch {
            expected: CURRENT_SCHEMA_VERSION,
            actual: record.schema_version,
        });
    }
    Ok(Some(record))
}

pub fn load_timeline_record(
    journal: &Path,
    subject: &str,
) -> Result<Option<TimelineRecordV1>, TimelineError> {
    crate::ensure_timeline_conversion(journal, subject)?;
    let path = timeline_record_path(journal, subject)?;
    let record =
        read_timeline_record_at(&path).map_err(|error| state_error(subject, &path, &error))?;
    if let Some(record) = &record
        && record.subject != subject
    {
        return Err(TimelineError::StateUnavailable {
            subject: subject.to_owned(),
            detail: format!("record at {} belongs to {}", path.display(), record.subject),
        });
    }
    Ok(record)
}

fn state_error(subject: &str, path: &Path, error: &TimelineError) -> TimelineError {
    TimelineError::StateUnavailable {
        subject: subject.to_owned(),
        detail: format!(
            "{}: {error}; remove this record to regenerate the artifact",
            path.display()
        ),
    }
}

fn update_record(
    journal: &Path,
    subject: &str,
    locks: &TimelineLockSet,
    update: impl FnOnce(&mut TimelineRecordV1),
) -> Result<(), TimelineError> {
    locks.require_subject(journal, subject)?;
    crate::ensure_timeline_conversion(journal, subject)?;
    let path = timeline_record_path(journal, subject)?;
    let mut record = read_timeline_record_at(&path)
        .map_err(|error| state_error(subject, &path, &error))?
        .filter(|record| record.subject == subject)
        .unwrap_or_else(|| TimelineRecordV1::empty(subject));
    update(&mut record);
    write_json_strict(&path, &record)
}

#[cfg(test)]
fn save_timeline_record(
    journal: &Path,
    record: &TimelineRecordV1,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    if record.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(TimelineError::SchemaVersionMismatch {
            expected: CURRENT_SCHEMA_VERSION,
            actual: record.schema_version,
        });
    }
    update_record(journal, &record.subject, locks, |target| {
        *target = record.clone();
        for attempt in &mut target.attempts {
            attempt.detail = bounded_diagnostic_detail(&attempt.detail);
        }
    })
}

fn put_attempt(record: &mut TimelineRecordV1, attempt: AttemptStateV1) {
    record
        .attempts
        .retain(|previous| previous.attempt_id != attempt.attempt_id);
    record.attempts.push(attempt);
}

pub fn record_attempt_started(
    journal: &Path,
    subject: &str,
    mut attempt: AttemptStateV1,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    attempt.outcome = AttemptOutcome::Running;
    attempt.finished_at_ms = None;
    attempt.detail = bounded_diagnostic_detail(&attempt.detail);
    update_record(journal, subject, locks, |record| {
        put_attempt(record, attempt)
    })
}

pub fn record_attempt_outcome(
    journal: &Path,
    subject: &str,
    mut attempt: AttemptStateV1,
    outcome: AttemptOutcome,
    detail: &str,
    finished_at_ms: i64,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    attempt.outcome = outcome;
    attempt.finished_at_ms = Some(finished_at_ms);
    attempt.detail = bounded_diagnostic_detail(detail);
    update_record(journal, subject, locks, |record| {
        put_attempt(record, attempt)
    })
}

pub fn record_artifact_published(
    journal: &Path,
    subject: &str,
    mut attempt: AttemptStateV1,
    published: PublishedArtifactV1,
    finished_at_ms: i64,
    locks: &TimelineLockSet,
) -> Result<(), TimelineError> {
    attempt.outcome = AttemptOutcome::Published;
    attempt.finished_at_ms = Some(finished_at_ms);
    attempt.detail.clear();
    update_record(journal, subject, locks, |record| {
        put_attempt(record, attempt);
        record
            .attempts
            .retain(|attempt| attempt.started_at_ms >= published.published_at_ms);
        record.published = Some(published);
    })
}

pub(crate) fn publication_is_confirmed(outcome: &DetailedAtomicOutcome) -> bool {
    matches!(outcome, DetailedAtomicOutcome::Published)
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

pub(crate) fn write_json_strict(path: &Path, value: &impl Serialize) -> Result<(), TimelineError> {
    let mut bytes = serde_json::to_vec(value)?;
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
}

#[cfg(test)]
mod record_tests {
    use super::*;
    use crate::{
        ArtifactCurrentness, TimelineLockRequest, acquire_timeline_locks, artifact_sha256,
        evaluate_artifact_currentness,
    };

    fn fixture() -> (tempfile::TempDir, TimelineLockSet) {
        let root = tempfile::tempdir().unwrap();
        let locks = acquire_timeline_locks(
            root.path(),
            TimelineLockRequest {
                subjects: vec![TimelineLockSubject::Master],
                ..TimelineLockRequest::default()
            },
        )
        .unwrap();
        write_json_strict(
            &crate::timeline_conversion_marker_path(root.path()),
            &serde_json::json!({"schema_version":1,"refused":{}}),
        )
        .unwrap();
        fs::write(
            timeline_state_path(root.path()),
            b"poisoned legacy document",
        )
        .unwrap();
        (root, locks)
    }

    fn attempt(id: &str, input: &str, started: i64) -> AttemptStateV1 {
        AttemptStateV1 {
            attempt_id: id.to_owned(),
            input_digest: input.to_owned(),
            started_at_ms: started,
            finished_at_ms: None,
            outcome: AttemptOutcome::Running,
            detail: String::new(),
        }
    }

    fn publication() -> PublishedArtifactV1 {
        PublishedArtifactV1 {
            input_digest: "input".to_owned(),
            artifact_sha256: artifact_sha256("artifact"),
            published_at_ms: 10,
        }
    }

    #[cfg(unix)]
    #[test]
    fn record_symlink_inside_the_journal_cannot_redirect_reads_or_writes() {
        let (root, locks) = fixture();
        let target = root.path().join("preserved.json");
        let original = serde_json::to_vec(&TimelineRecordV1::empty("master")).unwrap();
        fs::write(&target, &original).unwrap();
        let path = root.path().join(TIMELINE_RECORD_NAME);
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(load_timeline_record(root.path(), "master").is_err());
        assert!(
            record_attempt_started(root.path(), "master", attempt("new", "input", 10), &locks)
                .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), original);
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }

    #[test]
    fn interrupted_refresh_preserves_publication_and_retention_preserves_verdict() {
        let (root, locks) = fixture();
        record_artifact_published(
            root.path(),
            "master",
            attempt("first", "input", 10),
            publication(),
            10,
            &locks,
        )
        .unwrap();
        record_attempt_started(
            root.path(),
            "master",
            attempt("old", "different", 9),
            &locks,
        )
        .unwrap();
        record_attempt_started(
            root.path(),
            "master",
            attempt("equal", "different", 10),
            &locks,
        )
        .unwrap();
        let record = load_timeline_record(root.path(), "master")
            .unwrap()
            .unwrap();
        assert_eq!(record.published, Some(publication()));
        assert_eq!(
            evaluate_artifact_currentness(root.path(), "master", "input", 10, "artifact").unwrap(),
            ArtifactCurrentness::Stale
        );
        record_artifact_published(
            root.path(),
            "master",
            attempt("refresh", "input", 11),
            publication(),
            12,
            &locks,
        )
        .unwrap();
        let record = load_timeline_record(root.path(), "master")
            .unwrap()
            .unwrap();
        assert!(!record.attempts.iter().any(|a| a.attempt_id == "old"));
        assert!(record.attempts.iter().any(|a| a.attempt_id == "equal"));
        assert_eq!(
            evaluate_artifact_currentness(root.path(), "master", "input", 10, "artifact").unwrap(),
            ArtifactCurrentness::Stale
        );
    }

    #[test]
    fn corrupt_record_refuses_but_relocated_subject_can_publish() {
        let (root, locks) = fixture();
        let path = timeline_record_path(root.path(), "master").unwrap();
        for bytes in [
            b"not JSON".as_slice(),
            br#"{"schema_version":99,"subject":"master","published":null,"attempts":[]}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(matches!(
                load_timeline_record(root.path(), "master"),
                Err(TimelineError::StateUnavailable { .. })
            ));
            let error =
                record_attempt_started(root.path(), "master", attempt("new", "input", 10), &locks)
                    .unwrap_err();
            assert!(error.to_string().contains("regenerate"));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        write_json_strict(&path, &TimelineRecordV1::empty("day:20260101")).unwrap();
        assert!(matches!(
            load_timeline_record(root.path(), "master"),
            Err(TimelineError::StateUnavailable { .. })
        ));
        record_artifact_published(
            root.path(),
            "master",
            attempt("new", "input", 10),
            publication(),
            10,
            &locks,
        )
        .unwrap();
        assert_eq!(
            load_timeline_record(root.path(), "master")
                .unwrap()
                .unwrap()
                .published,
            Some(publication())
        );
    }

    #[test]
    fn writes_require_the_exact_subject_and_journal_lock() {
        let (root, locks) = fixture();
        let other = tempfile::tempdir().unwrap();
        for (journal, subject) in [(root.path(), "day:20260101"), (other.path(), "master")] {
            assert!(matches!(
                record_attempt_started(journal, subject, attempt("new", "input", 10), &locks),
                Err(TimelineError::LockContention { .. })
            ));
            assert!(!timeline_record_path(journal, subject).unwrap().exists());
        }
        for subject in [
            "day:../escape",
            "segment:20260101/../escape",
            "segment:20260101/a/b/c",
        ] {
            assert!(timeline_record_path(root.path(), subject).is_err());
        }
    }

    #[test]
    fn lock_free_reader_observes_only_complete_records() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        let (root, locks) = fixture();
        let mut first = TimelineRecordV1::empty("master");
        first.published = Some(publication());
        let mut second = first.clone();
        second.attempts.push(attempt("next", "different", 11));
        save_timeline_record(root.path(), &first, &locks).unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let finish = done.clone();
        let path = timeline_record_path(root.path(), "master").unwrap();
        let old = first.clone();
        let new = second.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut count = 0;
            loop {
                let value = read_timeline_record_at(&path).unwrap().unwrap();
                assert!(value == old || value == new);
                count += 1;
                if count == 1 {
                    ready_tx.send(()).unwrap();
                }
                if finish.load(Ordering::Acquire) {
                    return count;
                }
            }
        });
        ready_rx.recv().unwrap();
        for index in 0..20 {
            save_timeline_record(
                root.path(),
                if index % 2 == 0 { &second } else { &first },
                &locks,
            )
            .unwrap();
        }
        done.store(true, Ordering::Release);
        assert!(reader.join().unwrap() > 0);
    }
    #[test]
    fn sidecars_are_inert_to_segment_discovery_in_both_layouts() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260101/090000_300")).unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260101/audio/100000_300")).unwrap();
        let before = crate::discover_day_segment_bindings(root.path(), "20260101").unwrap();
        assert_eq!(before.len(), 2);
        for subject in [
            "master",
            "day:20260101",
            "segment:20260101/_default/090000_300",
            "segment:20260101/audio/100000_300",
        ] {
            write_json_strict(
                &timeline_record_path(root.path(), subject).unwrap(),
                &TimelineRecordV1::empty(subject),
            )
            .unwrap();
        }
        assert_eq!(
            crate::discover_day_segment_bindings(root.path(), "20260101").unwrap(),
            before
        );
    }
}
