// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One-time conversion of the frozen journal-wide document. Ordinary readers never load it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::state::write_json_strict;
use crate::{
    AttemptStateV1, CURRENT_SCHEMA_VERSION, PublishedArtifactV1, TimelineError,
    TimelineLockRequest, TimelineRecordV1, acquire_timeline_locks, bounded_diagnostic_detail,
    read_timeline_record_at, timeline_record_path, timeline_state_path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyArtifact {
    input_digest: String,
    artifact_sha256: String,
    published_at_ms: i64,
    #[serde(rename = "generation")]
    _generation: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyState {
    schema_version: u32,
    #[serde(rename = "revision")]
    _revision: u64,
    artifacts: BTreeMap<String, LegacyArtifact>,
    attempts: BTreeMap<String, AttemptStateV1>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConversionMarker {
    schema_version: u32,
    refused: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize)]
pub struct TimelineConversionReport {
    pub subjects: usize,
    pub written: usize,
    pub verified: usize,
    pub remaining: BTreeMap<String, String>,
    pub complete: bool,
}

pub fn timeline_conversion_marker_path(journal: &Path) -> PathBuf {
    journal.join("health/timeline/per-artifact.json")
}

fn read_marker(journal: &Path) -> Result<Option<ConversionMarker>, TimelineError> {
    let bytes = match fs::read(timeline_conversion_marker_path(journal)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let marker: ConversionMarker = serde_json::from_slice(&bytes)?;
    if marker.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(TimelineError::SchemaVersionMismatch {
            expected: CURRENT_SCHEMA_VERSION,
            actual: marker.schema_version,
        });
    }
    Ok(Some(marker))
}

/// Gate generation and publication, including force and refresh. No legacy bytes are read.
pub fn ensure_timeline_conversion(journal: &Path, subject: &str) -> Result<(), TimelineError> {
    let refusal = |detail| TimelineError::ConversionRequired {
        subject: subject.to_owned(),
        detail,
    };
    match read_marker(journal).map_err(|error| refusal(format!("conversion marker unavailable: {error}")))? {
        Some(marker) => {
            if let Some(reason) = marker.refused.get(subject) {
                return Err(refusal(format!("{reason}; repair the named path, then use journal maintenance convert-timeline-state --commit --allow-regeneration {subject}; this permits artifact regeneration")));
            }
            Ok(())
        }
        None => match fs::metadata(timeline_state_path(journal)) {
            Ok(_) => Err(refusal("pause the journal service and run journal maintenance convert-timeline-state --commit; conversion has not run".to_owned())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

pub fn read_timeline_conversion_refusals(
    journal: &Path,
) -> Result<BTreeMap<String, String>, TimelineError> {
    ensure_timeline_conversion(journal, "")?;
    Ok(read_marker(journal)?
        .map(|marker| marker.refused)
        .unwrap_or_default())
}

/// Survey by default. A committed pass holds the population lock across read, conversion,
/// verification, and marker publication. It takes no subject locks while holding that lock.
pub fn convert_timeline_state(
    journal: &Path,
    commit: bool,
) -> Result<TimelineConversionReport, TimelineError> {
    let _exclusive = if commit {
        Some(acquire_timeline_locks(
            journal,
            TimelineLockRequest::default(),
        )?)
    } else {
        None
    };
    if let Some(marker) = read_marker(journal)? {
        return Ok(TimelineConversionReport {
            complete: true,
            remaining: marker.refused,
            ..TimelineConversionReport::default()
        });
    }
    let bytes = match fs::read(timeline_state_path(journal)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TimelineConversionReport {
                complete: true,
                ..TimelineConversionReport::default()
            });
        }
        Err(error) => return Err(error.into()),
    };
    let legacy: LegacyState = serde_json::from_slice(&bytes)?;
    if legacy.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(TimelineError::SchemaVersionMismatch {
            expected: CURRENT_SCHEMA_VERSION,
            actual: legacy.schema_version,
        });
    }
    let mut records = BTreeMap::new();
    for (subject, artifact) in legacy.artifacts {
        let record = records
            .entry(subject.clone())
            .or_insert_with(|| TimelineRecordV1::empty(&subject));
        record.published = Some(PublishedArtifactV1 {
            input_digest: artifact.input_digest,
            artifact_sha256: artifact.artifact_sha256,
            published_at_ms: artifact.published_at_ms,
        });
    }
    let mut report = TimelineConversionReport::default();
    for (key, mut attempt) in legacy.attempts {
        let Some(subject) = key.strip_suffix(&format!(":{}", attempt.attempt_id)) else {
            report
                .remaining
                .insert(key, "unspellable legacy attempt key".to_owned());
            continue;
        };
        attempt.detail = bounded_diagnostic_detail(&attempt.detail);
        records
            .entry(subject.to_owned())
            .or_insert_with(|| TimelineRecordV1::empty(subject))
            .attempts
            .push(attempt);
    }
    report.subjects = records.len();
    let mut refused = BTreeMap::new();
    for (subject, record) in records {
        let result = conversion_destination(journal, &subject, &record);
        match result {
            Ok((path, exists)) => {
                if exists && commit {
                    // A previous pass may have stopped after rename but before parent
                    // sync. Matching bytes alone do not establish durable publication.
                    fs::File::open(&path)?.sync_all()?;
                    fs::File::open(path.parent().expect("record has a parent"))?.sync_all()?;
                }
                if !exists && commit {
                    write_json_strict(&path, &record)?;
                    if read_timeline_record_at(&path)?.as_ref() != Some(&record) {
                        return Err(TimelineError::StateUnavailable {
                            subject,
                            detail: format!("conversion verification failed at {}", path.display()),
                        });
                    }
                    report.written += 1;
                }
                if exists || commit {
                    report.verified += 1;
                }
            }
            Err(error) => {
                let detail = bounded_diagnostic_detail(&error.to_string());
                if record.published.is_some() {
                    refused.insert(subject.clone(), detail.clone());
                }
                report.remaining.insert(subject, detail);
            }
        }
    }
    // A write/verification failure must leave the global gate closed. Unplaceable and
    // conflicting subjects are individually refused; they are explicit conversion outcomes.
    if commit {
        write_json_strict(
            &timeline_conversion_marker_path(journal),
            &ConversionMarker {
                schema_version: CURRENT_SCHEMA_VERSION,
                refused,
            },
        )?;
        report.complete = true;
    }
    Ok(report)
}

fn conversion_destination(
    journal: &Path,
    subject: &str,
    expected: &TimelineRecordV1,
) -> Result<(PathBuf, bool), TimelineError> {
    let path = timeline_record_path(journal, subject)?;
    if !path.parent().is_some_and(Path::is_dir) {
        return Err(TimelineError::StateUnavailable {
            subject: subject.to_owned(),
            detail: format!("artifact directory is absent for {}", path.display()),
        });
    }
    match read_timeline_record_at(&path).map_err(|error| TimelineError::StateUnavailable {
        subject: subject.to_owned(),
        detail: format!("{}: {error}", path.display()),
    })? {
        Some(existing) if existing == *expected => Ok((path, true)),
        Some(_) => Err(TimelineError::StateUnavailable {
            subject: subject.to_owned(),
            detail: format!(
                "conflicting record at {}; preserved without changes",
                path.display()
            ),
        }),
        None => Ok((path, false)),
    }
}

/// Explicitly permit regeneration of an omitted legacy publication after operator repair.
pub fn release_timeline_refusal(journal: &Path, subject: &str) -> Result<(), TimelineError> {
    let _exclusive = acquire_timeline_locks(journal, TimelineLockRequest::default())?;
    let mut marker = read_marker(journal)?.ok_or_else(|| TimelineError::ConversionRequired {
        subject: subject.to_owned(),
        detail: "conversion has not run".to_owned(),
    })?;
    marker.refused.remove(subject);
    write_json_strict(&timeline_conversion_marker_path(journal), &marker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AttemptOutcome, TimelineLockSubject, load_timeline_record, record_attempt_started,
    };
    use serde_json::json;

    fn legacy(root: &Path) {
        fs::create_dir_all(root.join("health/timeline")).unwrap();
        fs::create_dir_all(root.join("chronicle/20260101/090000_300")).unwrap();
        fs::create_dir_all(root.join("chronicle/20260101/audio/100000_300")).unwrap();
        let published = json!({"input_digest":"input","artifact_sha256":"sha","published_at_ms":-123,"generation":8});
        fs::write(timeline_state_path(root), serde_json::to_vec(&json!({
            "schema_version":1,"revision":7,
            "artifacts": {"master":published,"day:20260101":published,"segment:20260101/_default/090000_300":published,"segment:20260101/audio/100000_300":published,"day:20260102":published},
            "attempts":{"master:old": {"attempt_id":"old","input_digest":"other","started_at_ms":-123,"finished_at_ms":null,"outcome":"running","detail":"kept"},"unspellable:bad": {"attempt_id":"bad","input_digest":"other","started_at_ms":0,"finished_at_ms":null,"outcome":"running","detail":"unplaceable"}}
        })).unwrap()).unwrap();
    }

    #[test]
    fn conversion_preserves_verdict_history_and_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        legacy(root.path());
        let original = fs::read(timeline_state_path(root.path())).unwrap();
        let plan = convert_timeline_state(root.path(), false).unwrap();
        assert_eq!(plan.subjects, 6);
        assert_eq!(plan.written, 0);
        assert_eq!(plan.remaining.len(), 2);
        assert!(!timeline_conversion_marker_path(root.path()).exists());
        assert!(
            !timeline_record_path(root.path(), "master")
                .unwrap()
                .exists()
        );
        let report = convert_timeline_state(root.path(), true).unwrap();
        assert_eq!(report.written, 4);
        assert_eq!(report.verified, 4);
        assert_eq!(report.remaining.len(), 2);
        assert!(report.complete);
        let record = load_timeline_record(root.path(), "master")
            .unwrap()
            .unwrap();
        assert_eq!(record.published.unwrap().published_at_ms, -123);
        assert_eq!(record.attempts[0].started_at_ms, -123);
        assert_eq!(record.attempts[0].finished_at_ms, None);
        assert_eq!(record.attempts[0].outcome, AttemptOutcome::Running);
        assert!(ensure_timeline_conversion(root.path(), "day:20260102").is_err());
        assert!(ensure_timeline_conversion(root.path(), "unspellable").is_ok());
        let before = fs::read(timeline_record_path(root.path(), "master").unwrap()).unwrap();
        assert_eq!(
            convert_timeline_state(root.path(), true).unwrap().written,
            0
        );
        assert_eq!(
            before,
            fs::read(timeline_record_path(root.path(), "master").unwrap()).unwrap()
        );
        assert_eq!(
            original,
            fs::read(timeline_state_path(root.path())).unwrap()
        );
        release_timeline_refusal(root.path(), "day:20260102").unwrap();
        ensure_timeline_conversion(root.path(), "day:20260102").unwrap();
    }

    #[test]
    fn foreign_or_conflicting_record_is_preserved_and_refused() {
        for foreign in [
            b"owner content".to_vec(),
            serde_json::to_vec(&TimelineRecordV1::empty("master")).unwrap(),
        ] {
            let root = tempfile::tempdir().unwrap();
            legacy(root.path());
            let path = timeline_record_path(root.path(), "master").unwrap();
            fs::write(&path, &foreign).unwrap();
            let report = convert_timeline_state(root.path(), true).unwrap();
            assert_eq!(report.written, 3);
            assert!(report.remaining.contains_key("master"));
            assert_eq!(fs::read(&path).unwrap(), foreign);
            assert!(matches!(
                load_timeline_record(root.path(), "master"),
                Err(TimelineError::ConversionRequired { .. })
            ));
        }
    }

    #[test]
    fn interrupted_conversion_resumes_without_rewriting_verified_records() {
        let root = tempfile::tempdir().unwrap();
        legacy(root.path());
        // An exact partial record from a prior interrupted pass.
        let path = timeline_record_path(root.path(), "day:20260101").unwrap();
        let expected = TimelineRecordV1 {
            schema_version: 1,
            subject: "day:20260101".to_owned(),
            published: Some(PublishedArtifactV1 {
                input_digest: "input".to_owned(),
                artifact_sha256: "sha".to_owned(),
                published_at_ms: -123,
            }),
            attempts: Vec::new(),
        };
        write_json_strict(&path, &expected).unwrap();
        let before = fs::read(&path).unwrap();
        let report = convert_timeline_state(root.path(), true).unwrap();
        assert_eq!(report.written, 3);
        assert_eq!(report.verified, 4);
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn uncertain_record_write_never_opens_the_conversion_gate() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };
        let root = tempfile::tempdir().unwrap();
        legacy(root.path());
        let (result, consumed) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::ParentSync,
            1,
            nix::errno::Errno::EIO as i32,
            || convert_timeline_state(root.path(), true),
        );
        assert!(consumed);
        assert!(result.is_err());
        assert!(!timeline_conversion_marker_path(root.path()).exists());
        assert!(ensure_timeline_conversion(root.path(), "master").is_err());
        let resumed = convert_timeline_state(root.path(), true).unwrap();
        assert!(resumed.complete);
        assert_eq!(resumed.verified, 4);
    }

    #[cfg(unix)]
    #[test]
    fn conversion_excludes_live_publication_without_inverting_lock_order() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, LockOptions, run_with_bound_publication_barrier,
        };
        use std::time::Duration;
        let root = tempfile::tempdir().unwrap();
        legacy(root.path());
        let journal = root.path().to_path_buf();
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::ParentSync,
            1,
            move || {
                assert!(matches!(
                    ensure_timeline_conversion(&journal, "master"),
                    Err(TimelineError::ConversionRequired { .. })
                ));
                let acquisition = acquire_timeline_locks(
                    &journal,
                    TimelineLockRequest {
                        subjects: vec![TimelineLockSubject::Master],
                        options: LockOptions {
                            timeout: Duration::ZERO,
                            ..LockOptions::default()
                        },
                    },
                );
                assert!(matches!(
                    acquisition,
                    Err(TimelineError::LockContention { .. })
                ));
            },
            || convert_timeline_state(root.path(), true),
        );
        assert!(fired);
        assert!(result.unwrap().complete);
        let locks = acquire_timeline_locks(
            root.path(),
            TimelineLockRequest {
                subjects: vec![TimelineLockSubject::Master],
                ..TimelineLockRequest::default()
            },
        )
        .unwrap();
        record_attempt_started(
            root.path(),
            "master",
            AttemptStateV1 {
                attempt_id: "new".to_owned(),
                input_digest: "input".to_owned(),
                started_at_ms: 1,
                finished_at_ms: None,
                outcome: AttemptOutcome::Running,
                detail: String::new(),
            },
            &locks,
        )
        .unwrap();
        let record = load_timeline_record(root.path(), "master")
            .unwrap()
            .unwrap();
        assert!(record.published.is_some());
        assert_eq!(record.attempts.len(), 2);
    }
}
