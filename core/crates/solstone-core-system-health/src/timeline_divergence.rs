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
    ArtifactCurrentness, AttemptOutcome, DayTimelineV1, MasterTimelineV1, SegmentTimelineV1,
    TimelineError, TimelineStateV1, day_subject_key, day_timeline_path,
    discover_day_segment_bindings, evaluate_artifact_currentness, load_timeline_state,
    master_subject_key, master_timeline_path, resolve_eligible_activity_source,
    segment_subject_key, segment_timeline_path, validate_day_timeline, validate_master_timeline,
    validate_segment_timeline, verify_segment_source,
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
    #[error("timeline source population is unavailable: {0}")]
    Source(#[source] TimelineError),
}

/// Cap how many reasons a diagnosis enumerates.
///
/// These lists are one entry per artifact, and on a journal mid-schema-migration that is
/// every artifact: the owner's journal produced a `timeline_divergence` detail of
/// **24,622,411 characters** on 2026-09-03 — 94,176 near-identical `unknown field \`title\``
/// entries plus 340 day artifacts. A detail that large is not a diagnosis; it cannot be read,
/// it bloats `doctor --json`, and nothing downstream can render it. The entries repeat, so a
/// bounded sample plus an honest total carries the same information.
fn summarize_reasons(reasons: &[String]) -> String {
    const MAX_REASONS: usize = 20;
    if reasons.len() <= MAX_REASONS {
        return reasons.join("; ");
    }
    format!(
        "{}; (+{} more of {} total)",
        reasons[..MAX_REASONS].join("; "),
        reasons.len() - MAX_REASONS,
        reasons.len()
    )
}

/// Independently verify live source coverage, artifact schema, durable state, and rollup agreement.
pub fn diagnose_timeline_divergence(
    journal: &Path,
    _now: DateTime<Utc>,
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
    let SegmentScan {
        artifacts: segments,
        eligible_paths: eligible_segment_paths,
        unreadable_days,
    } = read_segment_artifacts(journal, &day_directories)?;

    if is_empty_inventory(&master, &days, &segments, &state) {
        return Ok(TimelineDivergenceDiagnosis::NoData);
    }

    let invalid = invalid_artifacts(&master, &days, &segments);
    if !invalid.is_empty() {
        return Ok(TimelineDivergenceDiagnosis::Stale {
            detail: summarize_reasons(&invalid),
        });
    }

    // Divergence is a completeness-dependent claim: "the day says N segments, the
    // source has M" is only meaningful if the source was fully enumerated. When a day
    // could not be read, skipping it silently makes its segments look *missing* and
    // reports a false `Diverged`. Say the population is incomplete instead, and say
    // which days -- an operator can act on that; a fabricated divergence wastes them.
    //
    // Artifact validity above is unaffected: it judges only what was actually read.
    if !unreadable_days.is_empty() {
        return Ok(TimelineDivergenceDiagnosis::Uncertain {
            detail: format!(
                "read {} of {} chronicle day(s); could not enumerate segment bindings for {}",
                day_directories.len().saturating_sub(unreadable_days.len()),
                day_directories.len(),
                summarize_reasons(&unreadable_days)
            ),
        });
    }

    let observed = observed_digests(&master, &days, &segments);
    if let Some(detail) = uncertain_attempt(&state, &observed) {
        return Ok(TimelineDivergenceDiagnosis::Uncertain { detail });
    }

    if let Some(detail) = segment_day_divergence(&days, &segments, &eligible_segment_paths) {
        return Ok(TimelineDivergenceDiagnosis::Diverged { detail });
    }

    if let Some(detail) = day_master_divergence(&master, &days) {
        return Ok(TimelineDivergenceDiagnosis::Diverged { detail });
    }

    let stale = stale_reasons(journal, &master, &days, &segments, &state, &observed)?;
    if !stale.is_empty() {
        return Ok(TimelineDivergenceDiagnosis::Stale {
            detail: summarize_reasons(&stale),
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

struct SegmentScan {
    artifacts: Vec<(PathBuf, ArtifactRead<SegmentTimelineV1>)>,
    eligible_paths: BTreeSet<PathBuf>,
    /// Days whose segment bindings could not be enumerated, with the reason.
    ///
    /// 🔴 These used to abort the whole diagnosis. A journal accumulates historical
    /// shapes the current identity grammar cannot spell -- suze carries 21 `_default`
    /// stream directories from 2026-01-08..02-13 holding 982 segments -- and one of
    /// them made `journal doctor` report the entire timeline check as a hard error,
    /// hiding every day it *could* read. Coverage that is partial is reported as
    /// partial; it is not grounds for refusing to look at the rest.
    unreadable_days: Vec<String>,
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
    journal: &Path,
    directories: &[DayDirectory],
) -> Result<SegmentScan, TimelineHealthError> {
    let mut paths = BTreeSet::new();
    let mut eligible_paths = BTreeSet::new();
    let mut unreadable_days = Vec::new();
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
    for directory in directories {
        let bindings = match discover_day_segment_bindings(journal, &directory.day) {
            Ok(bindings) => bindings,
            Err(error) => {
                unreadable_days.push(format!("{}: {error}", directory.day));
                continue;
            }
        };
        for binding in bindings {
            match resolve_eligible_activity_source(journal, &binding) {
                Ok(Some(_)) => match segment_timeline_path(journal, &binding) {
                    Ok(path) => {
                        eligible_paths.insert(path.clone());
                        paths.insert(path);
                    }
                    Err(error) => {
                        unreadable_days.push(format!("{}: {error}", directory.day));
                    }
                },
                Err(_) => match segment_timeline_path(journal, &binding) {
                    Ok(path) => {
                        paths.insert(path);
                    }
                    Err(error) => {
                        unreadable_days.push(format!("{}: {error}", directory.day));
                    }
                },
                Ok(None) => {}
            }
        }
    }
    let artifacts = paths
        .into_iter()
        .map(|path| {
            let canonical_path = path.clone();
            let value = read_artifact(&path, |bytes| {
                let value = serde_json::from_slice(bytes)?;
                validate_segment_timeline(&value)?;
                let expected_path = segment_timeline_path(journal, &value.binding)?;
                if expected_path != canonical_path {
                    return Err(TimelineError::InvalidSourceEvidence {
                        detail: format!(
                            "segment artifact path {} does not match its binding",
                            canonical_path.display()
                        ),
                    });
                }
                verify_segment_source(journal, &value)?;
                Ok(value)
            })?;
            Ok((path, value))
        })
        .collect::<Result<Vec<_>, TimelineHealthError>>()?;
    Ok(SegmentScan {
        artifacts,
        eligible_paths,
        unreadable_days,
    })
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
        (!observed.contains_key(subject))
            .then(|| format!("{subject} publication was not durably confirmed"))
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

fn segment_day_divergence(
    days: &BTreeMap<String, ArtifactRead<DayTimelineV1>>,
    segments: &[(PathBuf, ArtifactRead<SegmentTimelineV1>)],
    eligible_paths: &BTreeSet<PathBuf>,
) -> Option<String> {
    let mut segment_counts = BTreeMap::<String, usize>::new();
    for (path, artifact) in segments {
        if !eligible_paths.contains(path) {
            continue;
        }
        if let ArtifactRead::Valid(segment) = artifact {
            *segment_counts
                .entry(segment.binding.day.clone())
                .or_default() += 1;
        }
    }
    for (day, artifact) in days {
        let ArtifactRead::Valid(timeline) = artifact else {
            continue;
        };
        let current_count = segment_counts.remove(day).unwrap_or_default();
        if timeline.segment_count != current_count {
            return Some(format!(
                "day {day} segment count is {}, but {current_count} current source-backed segment summaries exist",
                timeline.segment_count
            ));
        }
    }
    segment_counts
        .into_iter()
        .find(|(_, count)| *count > 0)
        .map(|(day, count)| {
            format!(
                "day {day} has {count} current source-backed segment summaries but no standalone day artifact"
            )
        })
}

fn stale_reasons(
    journal: &Path,
    master: &ArtifactRead<MasterTimelineV1>,
    days: &BTreeMap<String, ArtifactRead<DayTimelineV1>>,
    segments: &[(PathBuf, ArtifactRead<SegmentTimelineV1>)],
    state: &TimelineStateV1,
    observed: &BTreeMap<String, String>,
) -> Result<Vec<String>, TimelineHealthError> {
    let mut stale = Vec::new();
    if matches!(master, ArtifactRead::Missing) {
        stale.push("master timeline artifact is missing".to_owned());
    }
    if let ArtifactRead::Valid(timeline) = master {
        append_currentness_reason(
            &mut stale,
            journal,
            &master_timeline_path(journal),
            master_subject_key(),
            &timeline.source_digest,
            timeline.generated_at_ms,
        )?;
    }
    for artifact in days.values() {
        if let ArtifactRead::Valid(timeline) = artifact {
            append_currentness_reason(
                &mut stale,
                journal,
                &day_timeline_path(journal, &timeline.day),
                &day_subject_key(&timeline.day),
                &timeline.source_digest,
                timeline.generated_at_ms,
            )?;
        }
    }
    for (path, artifact) in segments {
        if let ArtifactRead::Valid(timeline) = artifact {
            append_currentness_reason(
                &mut stale,
                journal,
                path,
                &segment_subject_key(&timeline.binding),
                &timeline.input_digest,
                timeline.generated_at_ms,
            )?;
        }
    }
    for subject in state.artifacts.keys() {
        if !observed.contains_key(subject) {
            stale.push(format!(
                "durable state references missing {subject} artifact"
            ));
        }
    }
    for (path, artifact) in segments {
        if matches!(artifact, ArtifactRead::Missing) {
            stale.push(format!(
                "segment timeline artifact {} is missing",
                path.display()
            ));
        }
    }
    Ok(stale)
}

fn append_currentness_reason(
    stale: &mut Vec<String>,
    journal: &Path,
    path: &Path,
    subject: &str,
    input_digest: &str,
    generated_at_ms: i64,
) -> Result<(), TimelineHealthError> {
    let artifact_text =
        fs::read_to_string(path).map_err(|source| TimelineHealthError::Artifact {
            path: path.to_path_buf(),
            source,
        })?;
    match evaluate_artifact_currentness(
        journal,
        subject,
        input_digest,
        generated_at_ms,
        &artifact_text,
    )
    .map_err(TimelineHealthError::State)?
    {
        ArtifactCurrentness::Current => {}
        ArtifactCurrentness::Stale => {
            stale.push(format!(
                "{subject} does not match durable publication state"
            ));
        }
        ArtifactCurrentness::Missing => {
            stale.push(format!("{subject} has no durable state record"));
        }
    }
    Ok(())
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
        publish_segment_timeline, segment_subject_key, timeline_state_path,
    };

    use super::{TimelineDivergenceDiagnosis, diagnose_timeline_divergence, summarize_reasons};

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
        let binding = binding();
        let source = solstone_core_timeline::SegmentSourceV1::GeneratedActivity {
            schema_version: solstone_core_timeline::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: format!("chronicle/{DAY}/090000_300/talents/activity.md"),
            sha256: solstone_core_timeline::artifact_sha256("fixture activity"),
        };
        SegmentTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Segment,
            input_digest: solstone_core_timeline::segment_input_digest(&binding, &source).unwrap(),
            binding,
            source: Some(source),
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

    fn artifact(input_digest: &str, timeline: &impl Serialize) -> ArtifactStateV1 {
        ArtifactStateV1 {
            input_digest: input_digest.to_owned(),
            artifact_sha256: solstone_core_timeline::artifact_sha256(
                &serde_json::to_string(timeline).expect("timeline JSON"),
            ),
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
        fs::create_dir_all(root.join("chronicle").join(DAY).join("090000_300/talents"))
            .expect("activity parent");
        fs::write(
            root.join("chronicle")
                .join(DAY)
                .join("090000_300/talents/activity.md"),
            "fixture activity",
        )
        .expect("activity source");
        let state = solstone_core_timeline::TimelineStateV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: 1,
            artifacts: BTreeMap::from([
                (
                    master_subject_key().to_owned(),
                    artifact(&master.source_digest, &master),
                ),
                (day_subject_key(DAY), artifact(&day.source_digest, &day)),
                (
                    segment_subject_key(&segment.binding),
                    artifact(&segment.input_digest, &segment),
                ),
            ]),
            attempts: BTreeMap::new(),
        };
        write_json(&timeline_state_path(root), &state);
    }

    fn publish_extra_segment(root: &Path, segment_name: &str, source_text: &str) {
        let mut added = segment();
        added.binding.segment = segment_name.to_owned();
        let source_path = root
            .join("chronicle")
            .join(DAY)
            .join(segment_name)
            .join("talents/activity.md");
        fs::create_dir_all(source_path.parent().expect("source parent")).expect("source parent");
        fs::write(&source_path, source_text).expect("source");
        added.source = Some(solstone_core_timeline::SegmentSourceV1::GeneratedActivity {
            schema_version: solstone_core_timeline::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: format!("chronicle/{DAY}/{segment_name}/talents/activity.md"),
            sha256: solstone_core_timeline::artifact_sha256(source_text),
        });
        added.input_digest = solstone_core_timeline::segment_input_digest(
            &added.binding,
            added.source.as_ref().expect("source"),
        )
        .expect("input digest");
        publish_segment_timeline(
            root,
            &added,
            AttemptStateV1 {
                attempt_id: format!("added-{segment_name}"),
                input_digest: added.input_digest.clone(),
                started_at_ms: 2,
                finished_at_ms: None,
                outcome: AttemptOutcome::Running,
                detail: String::new(),
            },
        )
        .expect("added segment publishes");
    }

    fn diagnose(root: &Path) -> TimelineDivergenceDiagnosis {
        diagnose_timeline_divergence(root, Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap())
            .expect("diagnosis")
    }

    /// A journal accumulates historical stream shapes the current identity grammar
    /// cannot spell. On the founder's journal that is 21 `_default` stream directories
    /// (2026-01-08..02-13, 982 segments), and one of them made `journal doctor` report
    /// the whole timeline check as a hard error -- hiding every day it could read.
    ///
    /// Partial coverage must be reported as partial, never as a refusal to look.
    // AC: a diagnosis detail stays readable no matter how many artifacts are invalid. On
    // 2026-09-03 the owner's journal produced a 24,622,411-character detail because every
    // artifact predated the V1 schema and each got its own entry. The entries repeat, so a
    // bounded sample plus an honest total says the same thing and can actually be read.
    #[test]
    fn a_diagnosis_detail_is_bounded_and_reports_the_true_total() {
        let few: Vec<String> = (0..5).map(|i| format!("reason {i}")).collect();
        assert_eq!(summarize_reasons(&few), few.join("; "));
        assert!(!summarize_reasons(&few).contains("more of"));

        let many: Vec<String> = (0..94_176).map(|i| format!("reason {i}")).collect();
        let summary = summarize_reasons(&many);
        assert!(
            summary.len() < 2_000,
            "detail must stay readable, got {} chars",
            summary.len()
        );
        assert!(
            summary.starts_with("reason 0; reason 1;"),
            "keeps real examples"
        );
        assert!(
            summary.ends_with("(+94156 more of 94176 total)"),
            "reports the true total, got tail {:?}",
            &summary[summary.len().saturating_sub(40)..]
        );
    }

    #[test]
    fn an_unspellable_legacy_stream_directory_yields_uncertain_not_a_hard_error() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        // The exact shape from the live journal: chronicle/<day>/_default/<segment>/
        let legacy = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("_default")
            .join("100414_304");
        fs::create_dir_all(&legacy).expect("legacy stream directory");

        match diagnose_timeline_divergence(
            root.path(),
            Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
        ) {
            Ok(TimelineDivergenceDiagnosis::Uncertain { detail }) => {
                assert!(detail.contains("_default"), "{detail}");
                assert!(detail.contains("could not enumerate"), "{detail}");
            }
            other => panic!("expected Uncertain naming the unreadable day, got {other:?}"),
        }
    }

    #[test]
    fn clean_requires_current_segment_day_and_master_artifacts() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        assert_eq!(diagnose(root.path()), TimelineDivergenceDiagnosis::Clean);
    }

    #[test]
    fn source_backed_segment_without_a_summary_is_not_invisible() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        let missing = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("100000_300/talents");
        fs::create_dir_all(&missing).expect("missing summary source parent");
        fs::write(missing.join("activity.md"), "unpublished activity")
            .expect("missing summary source");

        assert!(matches!(
            diagnose(root.path()),
            TimelineDivergenceDiagnosis::Stale { detail }
                if detail.contains("100000_300") && detail.contains("missing")
        ));
    }

    #[test]
    fn newly_summarized_segment_makes_older_day_rollup_diverge() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        publish_extra_segment(root.path(), "100000_300", "later activity");

        assert!(matches!(
            diagnose(root.path()),
            TimelineDivergenceDiagnosis::Diverged { detail }
                if detail.contains(DAY) && detail.contains("segment count")
        ));
    }

    #[test]
    fn blank_activity_artifact_does_not_expand_the_day_population() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        publish_extra_segment(root.path(), "100000_300", " \n");

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
    fn stale_names_newer_failed_attempt_for_a_different_input() {
        let root = tempfile::tempdir().expect("root");
        write_clean(root.path());
        let path = timeline_state_path(root.path());
        let mut state: solstone_core_timeline::TimelineStateV1 =
            serde_json::from_slice(&fs::read(&path).expect("state")).expect("state JSON");
        state.attempts.insert(
            "master:newer-failed".to_owned(),
            AttemptStateV1 {
                attempt_id: "newer-failed".to_owned(),
                input_digest: "new-master-input".to_owned(),
                started_at_ms: 2,
                finished_at_ms: Some(2),
                outcome: AttemptOutcome::Failed,
                detail: "fixture failure".to_owned(),
            },
        );
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
                    artifact(&master.source_digest, &master),
                ),
                (
                    day_subject_key(DAY),
                    artifact(&standalone.source_digest, &standalone),
                ),
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
