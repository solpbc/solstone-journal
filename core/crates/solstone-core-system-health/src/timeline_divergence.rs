// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only verification of durable timeline publication state.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use solstone_core_timeline::{
    AttemptOutcome, DayTimelineV1, MasterTimelineV1, SegmentTimelineV1, TimelineError,
    TimelineStateV1, day_subject_key, day_timeline_path, load_timeline_state, master_subject_key,
    master_timeline_path, segment_subject_key, validate_day_timeline, validate_master_timeline,
    validate_segment_timeline,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineDivergenceDiagnosis {
    Clean,
    Stale { detail: String },
    Diverged { detail: String },
    Uncertain { detail: String },
    NoData,
}

#[derive(Debug, Error)]
pub enum TimelineHealthError {
    #[error("could not read timeline artifact {path}: {source}")]
    Artifact { path: PathBuf, source: io::Error },
    #[error("could not scan timeline directory {path}: {source}")]
    Directory { path: PathBuf, source: io::Error },
    #[error("timeline durable state is unavailable: {0}")]
    State(#[source] TimelineError),
}

/// Independently verify artifact schema, state agreement, and day/master agreement.
///
/// This deliberately reads only the small timeline artifacts and their durable state;
/// it does not rescan source activity or recompute curation input digests.
pub fn diagnose_timeline_divergence(
    journal: &Path,
    now: DateTime<Utc>,
) -> Result<TimelineDivergenceDiagnosis, TimelineHealthError> {
    let state = load_timeline_state(journal).map_err(TimelineHealthError::State)?;
    let day_directories = chronicle_day_directories(journal)?;
    let master = read_master(journal)?;
    let mut days = read_standalone_days(journal, &day_directories)?;
    if let ArtifactRead::Valid(master) = &master {
        for day in master_days(master) {
            days.entry(day.clone()).or_insert(read_day(journal, &day)?);
        }
    }
    let segments = read_segment_artifacts(&day_directories)?;

    if is_empty_inventory(&master, &days, &segments, &state) {
        return Ok(TimelineDivergenceDiagnosis::NoData);
    }

    let invalid = invalid_artifacts(&master, &days, &segments);
    if !invalid.is_empty() {
        return Ok(TimelineDivergenceDiagnosis::Stale {
            detail: invalid.join("; "),
        });
    }

    let observed = observed_digests(&master, &days, &segments);
    if let Some(detail) = uncertain_attempt(&state, &observed) {
        return Ok(TimelineDivergenceDiagnosis::Uncertain { detail });
    }

    if let Some(detail) = day_master_divergence(&master, &days) {
        return Ok(TimelineDivergenceDiagnosis::Diverged { detail });
    }

    let stale = stale_reasons(&master, &segments, &state, &observed, now);
    if !stale.is_empty() {
        return Ok(TimelineDivergenceDiagnosis::Stale {
            detail: stale.join("; "),
        });
    }

    Ok(TimelineDivergenceDiagnosis::Clean)
}

enum ArtifactRead<T> {
    Missing,
    Invalid(String),
    Valid(T),
}

struct DayDirectory {
    day: String,
    path: PathBuf,
}

fn read_master(journal: &Path) -> Result<ArtifactRead<MasterTimelineV1>, TimelineHealthError> {
    read_artifact(&master_timeline_path(journal), |bytes| {
        let value = serde_json::from_slice(bytes)?;
        validate_master_timeline(&value)?;
        Ok(value)
    })
}

fn read_day(journal: &Path, day: &str) -> Result<ArtifactRead<DayTimelineV1>, TimelineHealthError> {
    read_artifact(&day_timeline_path(journal, day), |bytes| {
        let value = serde_json::from_slice(bytes)?;
        validate_day_timeline(&value)?;
        if value.day != day {
            return Err(TimelineError::MalformedBinding {
                day: value.day,
                stream: String::new(),
                segment: String::new(),
            });
        }
        Ok(value)
    })
}

fn read_artifact<T>(
    path: &Path,
    parse: impl FnOnce(&[u8]) -> Result<T, TimelineError>,
) -> Result<ArtifactRead<T>, TimelineHealthError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ArtifactRead::Missing),
        Err(source) => {
            return Err(TimelineHealthError::Artifact {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    Ok(match parse(&bytes) {
        Ok(value) => ArtifactRead::Valid(value),
        Err(error) => ArtifactRead::Invalid(error.to_string()),
    })
}

fn chronicle_day_directories(journal: &Path) -> Result<Vec<DayDirectory>, TimelineHealthError> {
    let chronicle = journal.join("chronicle");
    let entries = match fs::read_dir(&chronicle) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(TimelineHealthError::Directory {
                path: chronicle,
                source,
            });
        }
    };
    let mut days = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| TimelineHealthError::Directory {
            path: chronicle.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(day) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        days.push(DayDirectory { day, path });
    }
    days.sort_by(|left, right| left.day.cmp(&right.day));
    Ok(days)
}

fn read_standalone_days(
    journal: &Path,
    directories: &[DayDirectory],
) -> Result<BTreeMap<String, ArtifactRead<DayTimelineV1>>, TimelineHealthError> {
    let mut days = BTreeMap::new();
    for directory in directories {
        let value = read_day(journal, &directory.day)?;
        if !matches!(value, ArtifactRead::Missing) {
            days.insert(directory.day.clone(), value);
        }
    }
    Ok(days)
}

fn read_segment_artifacts(
    directories: &[DayDirectory],
) -> Result<Vec<(PathBuf, ArtifactRead<SegmentTimelineV1>)>, TimelineHealthError> {
    let mut paths = BTreeSet::new();
    for directory in directories {
        let entries =
            fs::read_dir(&directory.path).map_err(|source| TimelineHealthError::Directory {
                path: directory.path.clone(),
                source,
            })?;
        for entry in entries {
            let entry = entry.map_err(|source| TimelineHealthError::Directory {
                path: directory.path.clone(),
                source,
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let direct = path.join("timeline.json");
            if direct.is_file() {
                paths.insert(direct);
                continue;
            }
            let nested = fs::read_dir(&path).map_err(|source| TimelineHealthError::Directory {
                path: path.clone(),
                source,
            })?;
            for nested_entry in nested {
                let nested_entry =
                    nested_entry.map_err(|source| TimelineHealthError::Directory {
                        path: path.clone(),
                        source,
                    })?;
                let nested_path = nested_entry.path();
                if nested_path.is_dir() {
                    let candidate = nested_path.join("timeline.json");
                    if candidate.is_file() {
                        paths.insert(candidate);
                    }
                }
            }
        }
    }
    paths
        .into_iter()
        .map(|path| {
            let value = read_artifact(&path, |bytes| {
                let value = serde_json::from_slice(bytes)?;
                validate_segment_timeline(&value)?;
                Ok(value)
            })?;
            Ok((path, value))
        })
        .collect()
}

fn is_empty_inventory(
    master: &ArtifactRead<MasterTimelineV1>,
    days: &BTreeMap<String, ArtifactRead<DayTimelineV1>>,
    segments: &[(PathBuf, ArtifactRead<SegmentTimelineV1>)],
    state: &TimelineStateV1,
) -> bool {
    matches!(master, ArtifactRead::Missing)
        && days.is_empty()
        && segments.is_empty()
        && state.artifacts.is_empty()
        && state.attempts.is_empty()
}

fn invalid_artifacts(
    master: &ArtifactRead<MasterTimelineV1>,
    days: &BTreeMap<String, ArtifactRead<DayTimelineV1>>,
    segments: &[(PathBuf, ArtifactRead<SegmentTimelineV1>)],
) -> Vec<String> {
    let mut invalid = Vec::new();
    if let ArtifactRead::Invalid(error) = master {
        invalid.push(format!("master artifact invalid: {error}"));
    }
    for (day, artifact) in days {
        if let ArtifactRead::Invalid(error) = artifact {
            invalid.push(format!("day {day} artifact invalid: {error}"));
        }
    }
    for (path, artifact) in segments {
        if let ArtifactRead::Invalid(error) = artifact {
            invalid.push(format!(
                "segment artifact {} invalid: {error}",
                path.display()
            ));
        }
    }
    invalid
}

fn observed_digests(
    master: &ArtifactRead<MasterTimelineV1>,
    days: &BTreeMap<String, ArtifactRead<DayTimelineV1>>,
    segments: &[(PathBuf, ArtifactRead<SegmentTimelineV1>)],
) -> BTreeMap<String, String> {
    let mut observed = BTreeMap::new();
    if let ArtifactRead::Valid(master) = master {
        observed.insert(
            master_subject_key().to_owned(),
            master.source_digest.clone(),
        );
    }
    for artifact in days.values() {
        if let ArtifactRead::Valid(day) = artifact {
            observed.insert(day_subject_key(&day.day), day.source_digest.clone());
        }
    }
    for (_, artifact) in segments {
        if let ArtifactRead::Valid(segment) = artifact {
            observed.insert(
                segment_subject_key(&segment.binding),
                segment.input_digest.clone(),
            );
        }
    }
    observed
}

fn uncertain_attempt(
    state: &TimelineStateV1,
    observed: &BTreeMap<String, String>,
) -> Option<String> {
    state.attempts.iter().find_map(|(key, attempt)| {
        if attempt.outcome != AttemptOutcome::DurabilityUncertain {
            return None;
        }
        let subject = attempt_subject(key, &attempt.attempt_id)?;
        let confirmed = state
            .artifacts
            .get(subject)
            .is_some_and(|artifact| artifact.input_digest == attempt.input_digest)
            && observed
                .get(subject)
                .is_some_and(|digest| digest == &attempt.input_digest);
        (!confirmed).then(|| format!("{subject} publication was not durably confirmed"))
    })
}

fn day_master_divergence(
    master: &ArtifactRead<MasterTimelineV1>,
    days: &BTreeMap<String, ArtifactRead<DayTimelineV1>>,
) -> Option<String> {
    let ArtifactRead::Valid(master) = master else {
        return None;
    };
    let embedded = master
        .months
        .values()
        .flat_map(|month| month.days.iter())
        .collect::<BTreeMap<_, _>>();
    for (day, artifact) in days {
        if matches!(artifact, ArtifactRead::Valid(_)) && !embedded.contains_key(day) {
            return Some(format!(
                "standalone day {day} is absent from the master artifact"
            ));
        }
    }
    for (day, embedded_day) in embedded {
        let Some(ArtifactRead::Valid(day_artifact)) = days.get(day) else {
            return Some(format!("master day {day} has no standalone day artifact"));
        };
        if embedded_day.source_digest != day_artifact.source_digest {
            return Some(format!(
                "day {day} digest differs between master and standalone artifacts"
            ));
        }
    }
    None
}

fn stale_reasons(
    master: &ArtifactRead<MasterTimelineV1>,
    segments: &[(PathBuf, ArtifactRead<SegmentTimelineV1>)],
    state: &TimelineStateV1,
    observed: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let mut stale = Vec::new();
    if matches!(master, ArtifactRead::Missing) {
        stale.push("master timeline artifact is missing".to_owned());
    }
    for subject in observed.keys() {
        let Some(actual) = observed.get(subject) else {
            continue;
        };
        match state.artifacts.get(subject) {
            Some(record) if record.input_digest == *actual => {}
            Some(_) => stale.push(format!("{subject} digest does not match durable state")),
            None => stale.push(format!("{subject} has no durable state record")),
        }
    }
    for subject in state.artifacts.keys() {
        if !observed.contains_key(subject) {
            stale.push(format!(
                "durable state references missing {subject} artifact"
            ));
        }
    }
    for (_, artifact) in segments {
        if matches!(artifact, ArtifactRead::Missing) {
            stale.push("segment timeline artifact is missing".to_owned());
        }
    }
    if let Some(detail) = failed_newer_attempt(state, now) {
        stale.push(detail);
    }
    stale
}

fn failed_newer_attempt(state: &TimelineStateV1, now: DateTime<Utc>) -> Option<String> {
    state.attempts.iter().find_map(|(key, attempt)| {
        if attempt.outcome != AttemptOutcome::Failed
            || attempt.started_at_ms > now.timestamp_millis()
        {
            return None;
        }
        let subject = attempt_subject(key, &attempt.attempt_id)?;
        let artifact = state.artifacts.get(subject)?;
        (artifact.published_at_ms <= attempt.started_at_ms
            && artifact.input_digest != attempt.input_digest)
            .then(|| format!("{subject} has a newer failed re-curation attempt"))
    })
}

fn attempt_subject<'a>(key: &'a str, attempt_id: &str) -> Option<&'a str> {
    key.strip_suffix(&format!(":{attempt_id}"))
}

fn master_days(master: &MasterTimelineV1) -> BTreeSet<String> {
    master
        .months
        .values()
        .flat_map(|month| month.days.keys().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path};

    use chrono::{TimeZone, Utc};
    use serde::Serialize;
    use solstone_core_timeline::{
        ArtifactStateV1, AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION, CurationRecordV1,
        DayTimelineV1, MasterTimelineV1, SegmentBindingV1, SegmentSummaryV1, SegmentTimelineV1,
        TimelineKind, day_subject_key, day_timeline_path, master_subject_key, master_timeline_path,
        segment_subject_key, timeline_state_path,
    };

    use super::{TimelineDivergenceDiagnosis, diagnose_timeline_divergence};

    const DAY: &str = "20260520";

    fn binding() -> SegmentBindingV1 {
        SegmentBindingV1 {
            day: DAY.to_owned(),
            stream: "_default".to_owned(),
            segment: "090000_300".to_owned(),
        }
    }

    fn curation(digest: &str) -> CurationRecordV1 {
        CurationRecordV1 {
            input_digest: digest.to_owned(),
            candidate_count: 0,
            picks: Vec::new(),
            rationale: String::new(),
            error: None,
            provenance: None,
        }
    }

    fn day(digest: &str) -> DayTimelineV1 {
        DayTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Day,
            day: DAY.to_owned(),
            source_digest: digest.to_owned(),
            generated_at_ms: 1,
            top_n: 1,
            segment_count: 1,
            hour_count: 0,
            hours: BTreeMap::new(),
            day_curation: curation(digest),
        }
    }

    fn master(day: DayTimelineV1) -> MasterTimelineV1 {
        MasterTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Master,
            source_digest: "master-digest".to_owned(),
            generated_at_ms: 1,
            top_n: 1,
            months: BTreeMap::from([(
                "202605".to_owned(),
                solstone_core_timeline::MonthTimelineV1 {
                    day_count: 1,
                    days: BTreeMap::from([(DAY.to_owned(), day)]),
                    month_curation: curation("month-digest"),
                },
            )]),
            year_top: Vec::new(),
            year_curation: curation("master-digest"),
        }
    }

    fn segment() -> SegmentTimelineV1 {
        SegmentTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Segment,
            binding: binding(),
            input_digest: "segment-digest".to_owned(),
            generated_at_ms: 1,
            summary: SegmentSummaryV1 {
                title: "Timeline".to_owned(),
                description: "Summary".to_owned(),
                origin: format!("{DAY}/090000_300"),
                continuation_of: None,
            },
            provenance: None,
        }
    }

    fn artifact(input_digest: &str) -> ArtifactStateV1 {
        ArtifactStateV1 {
            input_digest: input_digest.to_owned(),
            artifact_sha256: "sha256".to_owned(),
            published_at_ms: 1,
            generation: 1,
        }
    }

    fn write_json(path: &Path, value: &impl Serialize) {
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        fs::write(path, serde_json::to_vec(value).expect("JSON")).expect("JSON");
    }

    fn write_clean(root: &Path) {
        let day = day("day-digest");
        let master = master(day.clone());
        let segment = segment();
        write_json(&day_timeline_path(root, DAY), &day);
        write_json(&master_timeline_path(root), &master);
        write_json(
            &root
                .join("chronicle")
                .join(DAY)
                .join("090000_300/timeline.json"),
            &segment,
        );
        let state = solstone_core_timeline::TimelineStateV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: 1,
            artifacts: BTreeMap::from([
                (
                    master_subject_key().to_owned(),
                    artifact(&master.source_digest),
                ),
                (day_subject_key(DAY), artifact(&day.source_digest)),
                (
                    segment_subject_key(&segment.binding),
                    artifact(&segment.input_digest),
                ),
            ]),
            attempts: BTreeMap::new(),
        };
        write_json(&timeline_state_path(root), &state);
    }

    fn diagnose(root: &Path) -> TimelineDivergenceDiagnosis {
        diagnose_timeline_divergence(root, Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap())
            .expect("diagnosis")
    }

    #[test]
    fn clean_requires_current_segment_day_and_master_artifacts() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        assert_eq!(diagnose(root.path()), TimelineDivergenceDiagnosis::Clean);
    }

    #[test]
    fn stale_names_durable_state_digest_mismatch() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        let path = timeline_state_path(root.path());
        let mut state: solstone_core_timeline::TimelineStateV1 =
            serde_json::from_slice(&fs::read(&path).expect("state")).expect("state JSON");
        state
            .artifacts
            .get_mut(master_subject_key())
            .expect("master")
            .input_digest = "different".to_owned();
        write_json(&path, &state);
        assert!(matches!(
            diagnose(root.path()),
            TimelineDivergenceDiagnosis::Stale { detail } if detail.contains("master")
        ));
    }

    #[test]
    fn diverged_names_disagreement_between_master_and_standalone_day() {
        let root = tempfile::tempdir().expect("root");
        let standalone = day("standalone-digest");
        let embedded = day("embedded-digest");
        let master = master(embedded);
        write_json(&day_timeline_path(root.path(), DAY), &standalone);
        write_json(&master_timeline_path(root.path()), &master);
        let state = solstone_core_timeline::TimelineStateV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: 1,
            artifacts: BTreeMap::from([
                (
                    master_subject_key().to_owned(),
                    artifact(&master.source_digest),
                ),
                (day_subject_key(DAY), artifact(&standalone.source_digest)),
            ]),
            attempts: BTreeMap::new(),
        };
        write_json(&timeline_state_path(root.path()), &state);
        assert!(matches!(
            diagnose(root.path()),
            TimelineDivergenceDiagnosis::Diverged { detail } if detail.contains(DAY)
        ));
    }

    #[test]
    fn uncertain_names_unconfirmed_durability_attempt() {
        let root = tempfile::tempdir().expect("root");
        let state = solstone_core_timeline::TimelineStateV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: 1,
            artifacts: BTreeMap::new(),
            attempts: BTreeMap::from([(
                "master:attempt-1".to_owned(),
                AttemptStateV1 {
                    attempt_id: "attempt-1".to_owned(),
                    input_digest: "uncertain-digest".to_owned(),
                    started_at_ms: 1,
                    finished_at_ms: Some(1),
                    outcome: AttemptOutcome::DurabilityUncertain,
                    detail: "sync failed".to_owned(),
                },
            )]),
        };
        write_json(&timeline_state_path(root.path()), &state);
        assert!(matches!(
            diagnose(root.path()),
            TimelineDivergenceDiagnosis::Uncertain { detail } if detail.contains("master")
        ));
    }

    #[test]
    fn no_data_is_not_an_error_for_a_fresh_journal() {
        let root = tempfile::tempdir().expect("root");
        assert_eq!(diagnose(root.path()), TimelineDivergenceDiagnosis::NoData);
    }

    #[test]
    fn diagnosis_is_read_only() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        let snapshot = |path: &Path| {
            let mut rows = walk(path, path);
            rows.sort();
            rows
        };
        let before = snapshot(root.path());
        let _ = diagnose(root.path());
        assert_eq!(before, snapshot(root.path()));
    }

    fn walk(root: &Path, path: &Path) -> Vec<(String, Vec<u8>)> {
        let mut rows = Vec::new();
        for entry in fs::read_dir(path).expect("directory").flatten() {
            let entry_path = entry.path();
            let relative = entry_path
                .strip_prefix(root)
                .expect("relative")
                .display()
                .to_string();
            if entry_path.is_dir() {
                rows.push((format!("{relative}/"), Vec::new()));
                rows.extend(walk(root, &entry_path));
            } else {
                rows.push((relative, fs::read(entry_path).expect("file")));
            }
        }
        rows
    }
}
