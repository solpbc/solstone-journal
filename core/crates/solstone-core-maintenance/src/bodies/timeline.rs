// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native timeline day and master rollups.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest, GeneratedResponse};
use solstone_core_journal_config::read_journal_config;
use solstone_core_timeline::{
    ArtifactCurrentness, AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION,
    CurationContentPartV1, CurationJobV1, CurationRecordV1, CurationRequestV1, DayTimelineV1,
    GenerationProvenanceV1, HourTimelineV1, InvalidSelectionReason, MasterTimelineV1,
    MonthTimelineEntryV1, MonthTimelineV1, SegmentBindingV1, SegmentTimelineV1,
    TimelineCurationStage, TimelineEntryV1, TimelineError, TimelineKind, TimelineLockRequest,
    TimelineLockSet, TimelineLockSubject, acquire_timeline_locks, bounded_diagnostic_detail,
    curation_input_digest, curation_jobs_digest, day_subject_key, day_timeline_path,
    discover_day_segment_bindings, evaluate_artifact_currentness, master_source_digest,
    master_subject_key, master_timeline_path, new_attempt_id, publish_day_timeline_after_start,
    publish_master_timeline_after_start, record_attempt_outcome, record_attempt_started,
    segment_directory, segment_subject_key, validate_day_timeline, validate_master_timeline,
    validate_segment_timeline,
};

use crate::timezone::{default_rollup_day, resolve_owner_timezone};
use crate::{CliRun, RollupPicker, TimelineServices};

const EXIT_EMPTY: i32 = 66;
const ROLLUP_CONTEXT: &str = "timeline.scratch.rollup";

#[derive(Clone)]
struct SegmentRow {
    binding: SegmentBindingV1,
    hour: String,
    entry: TimelineEntryV1,
}

struct DayScan {
    rows: Vec<SegmentRow>,
    failures: Vec<DayScanFailure>,
}

struct DayScanFailure {
    binding: SegmentBindingV1,
    kind: &'static str,
    detail: String,
}

struct EntryPickResult {
    picks: Vec<TimelineEntryV1>,
    rationale: String,
    input_digest: String,
    provenance: Option<GenerationProvenanceV1>,
}

struct EntryBatchResult {
    key: String,
    candidates: Vec<TimelineEntryV1>,
    result: Result<EntryPickResult, TimelineError>,
}

struct MasterScan {
    days: BTreeMap<String, DayTimelineV1>,
    failures: Vec<MasterScanFailure>,
}

struct MasterScanFailure {
    day: String,
    kind: &'static str,
    detail: String,
}

pub(crate) fn run(
    id: &str,
    args: &[String],
    journal: &Path,
    services: &TimelineServices<'_>,
) -> CliRun {
    match id {
        "timeline:rollup" => match parse_day_args(args) {
            Ok(options) => rollup(journal, options, services),
            Err(error) => usage_error(id, &error),
        },
        "timeline:rollup-day" => match parse_day_args(args) {
            Ok(options) => rollup_day(journal, options, services),
            Err(error) => usage_error(id, &error),
        },
        "timeline:rollup-master" => match parse_master_args(args) {
            Ok(options) => rollup_master(journal, options, services),
            Err(error) => usage_error(id, &error),
        },
        _ => usage_error(id, "unrecognized routine"),
    }
}

#[derive(Default)]
struct DayOptions {
    day: Option<String>,
    top: usize,
    jobs: usize,
    commit: bool,
    force: bool,
    dry_run: bool,
}

#[derive(Default)]
struct MasterOptions {
    top: usize,
    jobs: usize,
    commit: bool,
    force: bool,
    dry_run: bool,
    months: Option<BTreeSet<String>>,
}

fn parse_day_args(args: &[String]) -> Result<DayOptions, String> {
    let mut options = DayOptions {
        top: 4,
        jobs: 5,
        ..DayOptions::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--top" => {
                options.top = parse_usize(args.get(index + 1), "--top")?;
                index += 1;
            }
            "--jobs" => {
                options.jobs = parse_usize(args.get(index + 1), "--jobs")?;
                index += 1;
            }
            "--day" => {
                let day = args
                    .get(index + 1)
                    .ok_or_else(|| "argument --day: expected one argument".to_owned())?;
                if !is_day(day) {
                    return Err("argument --day: day must be YYYYMMDD".to_owned());
                }
                options.day = Some(day.to_owned());
                index += 1;
            }
            "--commit" => options.commit = true,
            "--force" => options.force = true,
            "--dry-run" => options.dry_run = true,
            value => return Err(format!("unrecognized arguments: {value}")),
        }
        index += 1;
    }
    if options.commit && options.dry_run {
        return Err("--commit and --dry-run cannot be combined".to_owned());
    }
    if options.force && !options.commit {
        return Err("--force requires --commit".to_owned());
    }
    Ok(options)
}

fn parse_master_args(args: &[String]) -> Result<MasterOptions, String> {
    let mut options = MasterOptions {
        top: 4,
        jobs: 5,
        ..MasterOptions::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--top" => {
                options.top = parse_usize(args.get(index + 1), "--top")?;
                index += 1;
            }
            "--jobs" => {
                options.jobs = parse_usize(args.get(index + 1), "--jobs")?;
                index += 1;
            }
            "--months" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "argument --months: expected one argument".to_owned())?;
                let months = value
                    .split(',')
                    .map(str::trim)
                    .filter(|month| !month.is_empty())
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>();
                let invalid = months
                    .iter()
                    .filter(|month| !is_month(month))
                    .cloned()
                    .collect::<Vec<_>>();
                if !invalid.is_empty() {
                    return Err(format!(
                        "--months must be comma-separated YYYYMM values: {invalid:?}"
                    ));
                }
                if months.is_empty() {
                    return Err("--months requires at least one YYYYMM value".to_owned());
                }
                options.months = Some(months);
                index += 1;
            }
            "--commit" => options.commit = true,
            "--force" => options.force = true,
            "--dry-run" => options.dry_run = true,
            value => return Err(format!("unrecognized arguments: {value}")),
        }
        index += 1;
    }
    if options.commit && options.dry_run {
        return Err("--commit and --dry-run cannot be combined".to_owned());
    }
    if options.force && !options.commit {
        return Err("--force requires --commit".to_owned());
    }
    if options.commit && options.months.is_some() {
        return Err(
            "--months is preview-only; a canonical master commit must cover every month".to_owned(),
        );
    }
    Ok(options)
}

fn parse_usize(value: Option<&String>, name: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("argument {name}: expected one argument"))?;
    value
        .parse()
        .map_err(|_| format!("argument {name}: invalid int value: '{value}'"))
}

fn rollup(journal: &Path, options: DayOptions, services: &TimelineServices<'_>) -> CliRun {
    let master_options = MasterOptions {
        top: options.top,
        jobs: options.jobs,
        commit: options.commit,
        force: options.force,
        dry_run: options.dry_run,
        months: None,
    };
    let day = rollup_day(journal, options, services);
    if !day_is_ready_for_master(&day) {
        return day;
    }
    rollup_master(journal, master_options, services)
}

fn day_is_ready_for_master(result: &CliRun) -> bool {
    result.exit_code == 0 && (result.stdout.contains("[current]") || result.stdout.contains("[ok "))
}

fn rollup_day(journal: &Path, options: DayOptions, services: &TimelineServices<'_>) -> CliRun {
    let day = match options.day {
        Some(day) => day,
        None => {
            let config = match read_config(journal) {
                Ok(config) => config,
                Err(error) => return failure(error),
            };
            default_rollup_day(
                services.now,
                resolve_owner_timezone(&config, services.host_timezone),
            )
            .replace('-', "")
        }
    };
    if !options.commit {
        let scan = match load_day_segments(journal, &day) {
            Ok(scan) => scan,
            Err(error) => return failure(error),
        };
        let entries = match verified_day_entries(&day, scan) {
            Ok(entries) => entries,
            Err(error) => return failure(error),
        };
        if entries.is_empty() {
            return empty_day(&day);
        }
        let digest = match day_source_digest(&entries, options.top) {
            Ok(digest) => digest,
            Err(error) => return failure(error.to_string()),
        };
        return if day_is_current(journal, &day, &digest) {
            success(format!("  [current] {day}: verified timeline is current"))
        } else {
            let status = if day_timeline_path(journal, &day).exists() {
                "stale"
            } else {
                "would_publish"
            };
            success(format!(
                "  [{status}] {day}: {} verified segment candidates; --commit to publish",
                entries.len()
            ))
        };
    }

    let _locks = match acquire_timeline_locks(
        journal,
        TimelineLockRequest {
            days: vec![day.clone()],
            subjects: vec![TimelineLockSubject::Day(day.clone())],
            ..TimelineLockRequest::default()
        },
    ) {
        Ok(locks) => locks,
        Err(error) => return failure(error.to_string()),
    };
    let scan = match load_day_segments(journal, &day) {
        Ok(scan) => scan,
        Err(error) => return failure(error),
    };
    let entries = match verified_day_entries(&day, scan) {
        Ok(entries) => entries,
        Err(error) => return failure(error),
    };
    if entries.is_empty() {
        return empty_day(&day);
    }
    let source_digest = match day_source_digest(&entries, options.top) {
        Ok(digest) => digest,
        Err(error) => return failure(error.to_string()),
    };
    if !options.force && day_is_current(journal, &day, &source_digest) {
        return success(format!("  [current] {day}: verified timeline is current"));
    }
    let generated_at_ms = services.now.timestamp_millis();
    let attempt = day_attempt(&day, &source_digest, generated_at_ms);
    if let Err(error) = record_attempt_started(journal, &day_subject_key(&day), attempt.clone()) {
        return failure(error.to_string());
    }
    let by_hour = group_by_hour(&entries);
    let jobs = by_hour
        .iter()
        .map(|(hour, rows)| EntryBatchInput {
            key: hour.clone(),
            candidates: rows.iter().map(|row| row.entry.clone()).collect(),
        })
        .collect::<Vec<_>>();
    let hour_results = match pick_entry_batch(
        services.picker,
        jobs,
        options.top,
        "hour",
        TimelineCurationStage::Day,
        options.jobs,
    ) {
        Ok(results) => results,
        Err(error) => return day_curation_failure(journal, &day, &attempt, &error),
    };
    let mut hours = BTreeMap::new();
    for result in hour_results {
        let picked = match result.result {
            Ok(picked) => picked,
            Err(error) => return day_curation_failure(journal, &day, &attempt, &error),
        };
        hours.insert(
            result.key,
            HourTimelineV1 {
                source_digest: picked.input_digest.clone(),
                segment_count: result.candidates.len(),
                curation: curation_record(result.candidates.len(), picked),
            },
        );
    }
    let day_candidates = entries
        .iter()
        .map(|row| row.entry.clone())
        .collect::<Vec<_>>();
    let day_picked = match pick_entries(
        services.picker,
        &day_candidates,
        options.top,
        "day",
        TimelineCurationStage::Day,
    ) {
        Ok(picked) => picked,
        Err(error) => return day_curation_failure(journal, &day, &attempt, &error),
    };
    let timeline = DayTimelineV1 {
        schema_version: CURRENT_SCHEMA_VERSION,
        kind: TimelineKind::Day,
        day: day.clone(),
        source_digest,
        generated_at_ms,
        top_n: options.top,
        segment_count: entries.len(),
        hour_count: hours.len(),
        hours,
        day_curation: curation_record(day_candidates.len(), day_picked),
    };
    if let Err(error) = publish_day_timeline_after_start(journal, &timeline, attempt) {
        return failure(error.to_string());
    }
    success(format!(
        "  [ok {day}] → {}",
        day_timeline_path(journal, &day).display()
    ))
}

fn empty_day(day: &str) -> CliRun {
    CliRun {
        stdout: format!("  [empty] {day}: no verified segment timeline.json found\n"),
        stderr: String::new(),
        exit_code: EXIT_EMPTY,
    }
}

fn verified_day_entries(day: &str, scan: DayScan) -> Result<Vec<SegmentRow>, String> {
    if scan.failures.is_empty() {
        return Ok(scan.rows);
    }
    let failures = scan
        .failures
        .into_iter()
        .map(|failure| {
            format!(
                "{} ({}/{}/{}): {}",
                failure.kind,
                failure.binding.day,
                failure.binding.stream,
                failure.binding.segment,
                failure.detail
            )
        })
        .collect::<Vec<_>>();
    Err(format!(
        "day {day} scan failed; no partial rollup will be published: {}",
        failures.join("; ")
    ))
}

fn day_source_digest(rows: &[SegmentRow], top: usize) -> Result<String, TimelineError> {
    curation_jobs_digest(&day_curation_jobs(rows, top))
}

fn day_curation_jobs(rows: &[SegmentRow], top: usize) -> Vec<CurationJobV1> {
    let mut jobs = group_by_hour(rows)
        .into_iter()
        .map(|(hour, rows)| {
            let candidates = rows
                .into_iter()
                .map(|row| row.entry.clone())
                .collect::<Vec<_>>();
            CurationJobV1 {
                scope: format!("hour:{hour}"),
                request: curation_request(&entry_rollup_request(&candidates, top, "hour")),
                candidates,
            }
        })
        .collect::<Vec<_>>();
    let candidates = rows.iter().map(|row| row.entry.clone()).collect::<Vec<_>>();
    jobs.push(CurationJobV1 {
        scope: "day".to_owned(),
        request: curation_request(&entry_rollup_request(&candidates, top, "day")),
        candidates,
    });
    jobs
}

fn day_is_current(journal: &Path, day: &str, source_digest: &str) -> bool {
    let path = day_timeline_path(journal, day);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(timeline) = serde_json::from_str::<DayTimelineV1>(&text) else {
        return false;
    };
    if validate_day_timeline(&timeline).is_err()
        || timeline.day != day
        || timeline.source_digest != source_digest
    {
        return false;
    }
    matches!(
        evaluate_artifact_currentness(
            journal,
            &day_subject_key(day),
            &timeline.source_digest,
            timeline.generated_at_ms,
            &text,
        ),
        Ok(ArtifactCurrentness::Current)
    )
}

fn curation_record(candidate_count: usize, picked: EntryPickResult) -> CurationRecordV1 {
    CurationRecordV1 {
        input_digest: picked.input_digest,
        candidate_count,
        picks: picked.picks,
        rationale: picked.rationale,
        error: None,
        provenance: picked.provenance,
    }
}

fn day_attempt(day: &str, input_digest: &str, started_at_ms: i64) -> AttemptStateV1 {
    AttemptStateV1 {
        attempt_id: new_attempt_id(&format!("day-{day}")),
        input_digest: input_digest.to_owned(),
        started_at_ms,
        finished_at_ms: None,
        outcome: AttemptOutcome::Running,
        detail: String::new(),
    }
}

fn day_curation_failure(
    journal: &Path,
    day: &str,
    attempt: &AttemptStateV1,
    error: &TimelineError,
) -> CliRun {
    let detail = bounded_diagnostic_detail(&error.to_string());
    if let Err(state_error) = record_attempt_outcome(
        journal,
        &day_subject_key(day),
        attempt.clone(),
        AttemptOutcome::Failed,
        &detail,
        attempt.started_at_ms,
    ) {
        return failure(bounded_diagnostic_detail(&format!(
            "terminal timeline state write failed: {state_error}; primary failure: {detail}"
        )));
    }
    failure(detail)
}

fn rollup_master(
    journal: &Path,
    options: MasterOptions,
    services: &TimelineServices<'_>,
) -> CliRun {
    if !options.commit {
        let scan = match load_day_rollups(journal) {
            Ok(scan) => scan,
            Err(error) => return failure(error),
        };
        let days = match verified_master_days(scan, options.months.as_ref()) {
            Ok(days) => days,
            Err(error) => return failure(error),
        };
        return master_preview(journal, &days, options.top);
    }

    let (locks, days) = match lock_master_days(journal, options.months.as_ref()) {
        Ok(value) => value,
        Err(error) => return failure(error),
    };
    let _locks = locks;
    if days.is_empty() {
        return empty_master(journal);
    }
    let candidates = master_candidates(&days);
    if candidates.is_empty() {
        return empty_master_candidates();
    }
    let source_digest = match master_digest(&days, options.top) {
        Ok(digest) => digest,
        Err(error) => return failure(error.to_string()),
    };
    if !options.force && master_is_current(journal, &source_digest) {
        return success("  [current] verified master timeline is current".to_owned());
    }
    let generated_at_ms = services.now.timestamp_millis();
    let attempt = master_attempt(&source_digest, generated_at_ms);
    if let Err(error) = record_attempt_started(journal, master_subject_key(), attempt.clone()) {
        return failure(error.to_string());
    }
    let by_month = group_by_month(&days);
    let jobs = by_month
        .iter()
        .map(|(month, day_keys)| EntryBatchInput {
            key: month.clone(),
            candidates: day_keys
                .iter()
                .flat_map(|day| days[day].day_curation.picks.iter().cloned())
                .collect(),
        })
        .collect::<Vec<_>>();
    let month_results = match pick_entry_batch(
        services.picker,
        jobs,
        options.top,
        "month",
        TimelineCurationStage::Master,
        options.jobs,
    ) {
        Ok(results) => results,
        Err(error) => return master_curation_failure(journal, &attempt, &error),
    };
    let mut months = BTreeMap::new();
    for result in month_results {
        let picked = match result.result {
            Ok(picked) => picked,
            Err(error) => return master_curation_failure(journal, &attempt, &error),
        };
        let day_keys = &by_month[&result.key];
        let month_days = day_keys
            .iter()
            .map(|day| (day.clone(), days[day].clone()))
            .collect();
        months.insert(
            result.key,
            MonthTimelineV1 {
                day_count: day_keys.len(),
                days: month_days,
                month_curation: curation_record(result.candidates.len(), picked),
            },
        );
    }
    let year_picked = match pick_entries(
        services.picker,
        &candidates,
        options.top,
        "year",
        TimelineCurationStage::Master,
    ) {
        Ok(picked) => picked,
        Err(error) => return master_curation_failure(journal, &attempt, &error),
    };
    let year_top = year_picked
        .picks
        .iter()
        .map(|entry| MonthTimelineEntryV1 {
            month: entry.binding.day[..6].to_owned(),
            entry: entry.clone(),
        })
        .collect();
    let timeline = MasterTimelineV1 {
        schema_version: CURRENT_SCHEMA_VERSION,
        kind: TimelineKind::Master,
        source_digest,
        generated_at_ms,
        top_n: options.top,
        months,
        year_top,
        year_curation: curation_record(candidates.len(), year_picked),
    };
    if let Err(error) = publish_master_timeline_after_start(journal, &timeline, attempt) {
        return failure(error.to_string());
    }
    success(format!(
        "  [ok] → {}",
        master_timeline_path(journal).display()
    ))
}

fn master_preview(journal: &Path, days: &BTreeMap<String, DayTimelineV1>, top: usize) -> CliRun {
    if days.is_empty() {
        return empty_master(journal);
    }
    let candidates = master_candidates(days);
    if candidates.is_empty() {
        return empty_master_candidates();
    }
    let digest = match master_digest(days, top) {
        Ok(digest) => digest,
        Err(error) => return failure(error.to_string()),
    };
    if master_is_current(journal, &digest) {
        return success("  [current] verified master timeline is current".to_owned());
    }
    let status = if master_timeline_path(journal).exists() {
        "stale"
    } else {
        "would_publish"
    };
    success(format!(
        "  [{status}] {} verified day artifacts; --commit to publish",
        days.len()
    ))
}

fn empty_master(journal: &Path) -> CliRun {
    CliRun {
        stdout: format!(
            "  [empty] no day-level timeline.json found under {}/chronicle/*/\n",
            journal.display()
        ),
        stderr: String::new(),
        exit_code: EXIT_EMPTY,
    }
}

fn empty_master_candidates() -> CliRun {
    CliRun {
        stdout: "  [empty] no master candidates found\n".to_owned(),
        stderr: String::new(),
        exit_code: EXIT_EMPTY,
    }
}

fn lock_master_days(
    journal: &Path,
    months: Option<&BTreeSet<String>>,
) -> Result<(TimelineLockSet, BTreeMap<String, DayTimelineV1>), String> {
    for _ in 0..3 {
        let scan = load_day_rollups(journal)?;
        let expected = verified_master_days(scan, months)?;
        let locks = acquire_timeline_locks(
            journal,
            TimelineLockRequest {
                days: expected.keys().cloned().collect(),
                subjects: vec![TimelineLockSubject::Master],
                ..TimelineLockRequest::default()
            },
        )
        .map_err(|error| error.to_string())?;
        let scan = load_day_rollups(journal)?;
        let confirmed = verified_master_days(scan, months)?;
        if expected.keys().eq(confirmed.keys()) {
            return Ok((locks, confirmed));
        }
        drop(locks);
    }
    Err("timeline master input population changed while acquiring locks".to_owned())
}

fn verified_master_days(
    scan: MasterScan,
    months: Option<&BTreeSet<String>>,
) -> Result<BTreeMap<String, DayTimelineV1>, String> {
    let matches_month =
        |day: &str| months.is_none_or(|filter| filter.contains(&day[..6].to_owned()));
    let failures = scan
        .failures
        .into_iter()
        .filter(|failure| matches_month(&failure.day))
        .map(|failure| format!("{} ({}): {}", failure.kind, failure.day, failure.detail))
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        return Err(format!(
            "master scan failed; no partial rollup will be published: {}",
            failures.join("; ")
        ));
    }
    Ok(scan
        .days
        .into_iter()
        .filter(|(day, _)| matches_month(day))
        .collect())
}

fn master_candidates(days: &BTreeMap<String, DayTimelineV1>) -> Vec<TimelineEntryV1> {
    days.values()
        .flat_map(|day| day.day_curation.picks.iter().cloned())
        .collect()
}

fn master_digest(
    days: &BTreeMap<String, DayTimelineV1>,
    top: usize,
) -> Result<String, TimelineError> {
    let sources = days
        .iter()
        .map(|(day, timeline)| (day.clone(), timeline.source_digest.clone()))
        .collect::<Vec<_>>();
    master_source_digest(&sources, &master_curation_jobs(days, top))
}

fn master_curation_jobs(days: &BTreeMap<String, DayTimelineV1>, top: usize) -> Vec<CurationJobV1> {
    let mut jobs = group_by_month(days)
        .into_iter()
        .map(|(month, day_keys)| {
            let candidates = day_keys
                .iter()
                .flat_map(|day| days[day].day_curation.picks.iter().cloned())
                .collect::<Vec<_>>();
            CurationJobV1 {
                scope: format!("month:{month}"),
                request: curation_request(&entry_rollup_request(&candidates, top, "month")),
                candidates,
            }
        })
        .collect::<Vec<_>>();
    let candidates = master_candidates(days);
    jobs.push(CurationJobV1 {
        scope: "year".to_owned(),
        request: curation_request(&entry_rollup_request(&candidates, top, "year")),
        candidates,
    });
    jobs
}

fn master_is_current(journal: &Path, source_digest: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(master_timeline_path(journal)) else {
        return false;
    };
    let Ok(timeline) = serde_json::from_str::<MasterTimelineV1>(&text) else {
        return false;
    };
    if validate_master_timeline(&timeline).is_err() || timeline.source_digest != source_digest {
        return false;
    }
    matches!(
        evaluate_artifact_currentness(
            journal,
            master_subject_key(),
            &timeline.source_digest,
            timeline.generated_at_ms,
            &text,
        ),
        Ok(ArtifactCurrentness::Current)
    )
}

fn master_attempt(input_digest: &str, started_at_ms: i64) -> AttemptStateV1 {
    AttemptStateV1 {
        attempt_id: new_attempt_id("master"),
        input_digest: input_digest.to_owned(),
        started_at_ms,
        finished_at_ms: None,
        outcome: AttemptOutcome::Running,
        detail: String::new(),
    }
}

fn master_curation_failure(
    journal: &Path,
    attempt: &AttemptStateV1,
    error: &TimelineError,
) -> CliRun {
    let detail = bounded_diagnostic_detail(&error.to_string());
    if let Err(state_error) = record_attempt_outcome(
        journal,
        master_subject_key(),
        attempt.clone(),
        AttemptOutcome::Failed,
        &detail,
        attempt.started_at_ms,
    ) {
        return failure(bounded_diagnostic_detail(&format!(
            "terminal timeline state write failed: {state_error}; primary failure: {detail}"
        )));
    }
    failure(detail)
}

#[derive(Clone)]
struct EntryBatchInput {
    key: String,
    candidates: Vec<TimelineEntryV1>,
}

fn pick_entry_batch(
    picker: &dyn RollupPicker,
    jobs: Vec<EntryBatchInput>,
    n: usize,
    scope_label: &str,
    stage: TimelineCurationStage,
    max_concurrent: usize,
) -> Result<Vec<EntryBatchResult>, TimelineError> {
    if max_concurrent == 0 {
        return Err(curation_error("max_concurrent must be positive"));
    }
    let mut out = Vec::with_capacity(jobs.len());
    for chunk in jobs.chunks(max_concurrent) {
        let mut completed = Vec::with_capacity(chunk.len());
        std::thread::scope(|scope| {
            let handles = chunk
                .iter()
                .cloned()
                .map(|job| {
                    scope.spawn(move || EntryBatchResult {
                        key: job.key,
                        result: pick_entries(picker, &job.candidates, n, scope_label, stage),
                        candidates: job.candidates,
                    })
                })
                .collect::<Vec<_>>();
            for handle in handles {
                completed.push(
                    handle
                        .join()
                        .map_err(|_| curation_error("timeline rollup worker panicked")),
                );
            }
        });
        for result in completed {
            out.push(result?);
        }
    }
    Ok(out)
}

fn pick_entries(
    picker: &dyn RollupPicker,
    candidates: &[TimelineEntryV1],
    n: usize,
    scope_label: &str,
    stage: TimelineCurationStage,
) -> Result<EntryPickResult, TimelineError> {
    let request = entry_rollup_request(candidates, n, scope_label);
    let input_digest = curation_input_digest(candidates, &curation_request(&request))?;
    if candidates.len() <= n {
        return Ok(EntryPickResult {
            picks: candidates.to_vec(),
            rationale: "fewer than N candidates; returning all".to_owned(),
            input_digest,
            provenance: None,
        });
    }
    let response = picker
        .pick(&request)
        .map_err(|error| curation_error(&error))?;
    let (indices, rationale) = parse_selection(&response, candidates.len(), n, stage)?;
    Ok(EntryPickResult {
        picks: indices
            .into_iter()
            .map(|index| candidates[index].clone())
            .collect(),
        rationale,
        input_digest,
        provenance: Some(generation_provenance(&response)),
    })
}

fn parse_selection(
    response: &GeneratedResponse,
    candidate_count: usize,
    n: usize,
    stage: TimelineCurationStage,
) -> Result<(Vec<usize>, String), TimelineError> {
    let payload: Value = serde_json::from_str(&response.text).map_err(|error| {
        curation_error(&format!(
            "rollup payload parse error: {error}; response={:?}",
            response.text
        ))
    })?;
    let payload = payload.as_object().ok_or_else(|| {
        curation_error(&format!(
            "rollup payload must be an object: {:?}",
            response.text
        ))
    })?;
    let raw_indices = payload
        .get("picks")
        .and_then(Value::as_array)
        .ok_or_else(|| curation_error("rollup payload picks must be an array"))?;
    let mut seen = HashSet::new();
    let mut picks = Vec::with_capacity(raw_indices.len().min(n));
    for raw_index in raw_indices {
        let index = raw_index
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| {
                curation_error("rollup payload picks must contain non-negative integers")
            })?;
        if index >= candidate_count {
            return Err(TimelineError::InvalidModelSelection {
                stage,
                index,
                candidate_count,
                reason: InvalidSelectionReason::OutOfRange,
            });
        }
        if !seen.insert(index) {
            return Err(TimelineError::InvalidModelSelection {
                stage,
                index,
                candidate_count,
                reason: InvalidSelectionReason::Duplicate,
            });
        }
        if picks.len() == n {
            continue;
        }
        picks.push(index);
    }
    let rationale = payload
        .get("rationale")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok((picks, rationale))
}

fn curation_error(detail: &str) -> TimelineError {
    TimelineError::CurationFailed {
        detail: bounded_diagnostic_detail(detail),
    }
}

fn generation_provenance(response: &GeneratedResponse) -> GenerationProvenanceV1 {
    GenerationProvenanceV1 {
        model: response.model.clone(),
        finish_reason: response.finish_reason.clone(),
        schema_validation: response.schema_validation.clone().unwrap_or(Value::Null),
        inference: response.inference.clone().unwrap_or(Value::Null),
        usage: response.usage.clone(),
    }
}

fn entry_rollup_request(
    candidates: &[TimelineEntryV1],
    n: usize,
    scope_label: &str,
) -> GenerateRequest {
    let mut lines = vec!["Candidate events:\n".to_owned()];
    for (index, candidate) in candidates.iter().enumerate() {
        lines.push(format!(
            "  [{index}] {} — {}",
            candidate.title, candidate.description
        ));
    }
    rollup_request_with_prompt(lines.join("\n"), n, scope_label)
}

fn rollup_request_with_prompt(prompt: String, n: usize, scope_label: &str) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: ROLLUP_CONTEXT.to_owned(),
        contents: vec![ContentPart::Text { text: prompt }],
        system_instruction: Some(build_system_instruction(scope_label, n)),
        temperature: 0.3,
        max_output_tokens: 2048,
        thinking_budget: None,
        timeout_s: Some(60.0),
        json_output: true,
        json_schema: Some(build_rollup_schema(n)),
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn curation_request(request: &GenerateRequest) -> CurationRequestV1 {
    CurationRequestV1 {
        id: request.id.clone(),
        context: request.context.clone(),
        contents: request
            .contents
            .iter()
            .map(|content| match content {
                ContentPart::Text { text } => CurationContentPartV1::Text { text: text.clone() },
                ContentPart::Image { mime_type, data } => CurationContentPartV1::Image {
                    mime_type: mime_type.clone(),
                    data: data.clone(),
                },
            })
            .collect(),
        system_instruction: request.system_instruction.clone(),
        temperature: request.temperature,
        max_output_tokens: request.max_output_tokens,
        thinking_budget: request.thinking_budget,
        timeout_s: request.timeout_s,
        json_output: request.json_output,
        json_schema: request.json_schema.clone(),
        enforce_responsiveness: request.enforce_responsiveness,
        attempt_index: request.attempt_index,
        exclusive_admission: request.exclusive_admission,
        transport_retries: request.transport_retries,
    }
}

fn build_rollup_schema(n: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "picks": {
                "type": "array",
                "description": format!(
                    "Zero-based indices of the most important events from the candidate list, in order of importance (most important first). Length must be exactly {n} (or fewer if input has fewer)."
                ),
                "items": {"type": "integer"},
            },
            "rationale": {
                "type": "string",
                "description": "ONE sentence, max 100 chars, naming the criterion that drove the pick. For debugging — not shown in the UI.",
            },
        },
        "required": ["picks", "rationale"],
        "additionalProperties": false,
    })
}

fn build_system_instruction(scope_label: &str, n: usize) -> String {
    format!(
        "You are picking the {n} MOST IMPORTANT events from a list of candidate events that occurred during one {scope_label} of a personal life-journal. The picked events become the headline cells in the {scope_label} view of a multi-scale timeline UI.\n\nIMPORTANT-EVENT CRITERIA, in priority order:\n  1. Consequence — decisions, shipments, milestones, externally-visible actions outweigh routine maintenance.\n  2. Specificity — concrete events outweigh generic activity descriptors. 'Trademark Filed' beats 'Email Sent'.\n  3. Diversity — when several candidates describe the same underlying thread (e.g., five 'KDE Crash' debugging segments), pick at most one. Reserve the other slots for distinct events.\n  4. Owner-relevance — events involving identifiable people, decisions, or commitments outweigh tooling housekeeping.\n\nReturn JSON: {{ \"picks\": [<indices>], \"rationale\": \"<short>\" }}.\n  - picks: zero-based indices into the input list, in importance order, length exactly {n} (or fewer if input has fewer).\n  - rationale: one sentence naming the criterion (for debugging).\n\nDo NOT rewrite titles or descriptions. Do NOT invent events. Pick from the given list only."
    )
}

fn load_day_segments(journal: &Path, day: &str) -> Result<DayScan, String> {
    let bindings =
        discover_day_segment_bindings(journal, day).map_err(|error| error.to_string())?;
    let mut rows = Vec::with_capacity(bindings.len());
    let mut failures = Vec::new();
    for binding in bindings {
        let segment_dir = match segment_directory(journal, &binding) {
            Ok(path) => path,
            Err(error) => {
                failures.push(scan_failure(binding, "wrong_shape", error.to_string()));
                continue;
            }
        };
        let timeline_path = segment_dir.join("timeline.json");
        let bytes = match std::fs::read(&timeline_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                failures.push(scan_failure(
                    binding,
                    "missing",
                    format!("{} is missing", timeline_path.display()),
                ));
                continue;
            }
            Err(error) => {
                failures.push(scan_failure(binding, "unreadable", error.to_string()));
                continue;
            }
        };
        let value = match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => value,
            Err(error) => {
                failures.push(scan_failure(binding, "malformed_json", error.to_string()));
                continue;
            }
        };
        let timeline = match serde_json::from_value::<SegmentTimelineV1>(value) {
            Ok(timeline) => timeline,
            Err(error) => {
                failures.push(scan_failure(binding, "wrong_shape", error.to_string()));
                continue;
            }
        };
        if let Err(error) = validate_segment_timeline(&timeline) {
            failures.push(scan_failure(binding, "wrong_shape", error.to_string()));
            continue;
        }
        if timeline.binding != binding {
            failures.push(scan_failure(
                binding,
                "wrong_shape",
                "artifact binding does not match its canonical segment directory".to_owned(),
            ));
            continue;
        }
        let artifact_text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(error) => {
                failures.push(scan_failure(binding, "malformed_json", error.to_string()));
                continue;
            }
        };
        match evaluate_artifact_currentness(
            journal,
            &segment_subject_key(&binding),
            &timeline.input_digest,
            timeline.generated_at_ms,
            artifact_text,
        ) {
            Ok(ArtifactCurrentness::Current) => {}
            Ok(ArtifactCurrentness::Stale) => {
                failures.push(scan_failure(
                    binding,
                    "stale",
                    "artifact does not match durable publication state".to_owned(),
                ));
                continue;
            }
            Ok(ArtifactCurrentness::Missing) => {
                failures.push(scan_failure(
                    binding,
                    "state_missing",
                    "artifact has no durable publication state".to_owned(),
                ));
                continue;
            }
            Err(error) => {
                failures.push(scan_failure(
                    binding,
                    "state_unavailable",
                    error.to_string(),
                ));
                continue;
            }
        }
        let title = timeline.summary.title.trim().to_owned();
        let description = timeline.summary.description.trim().to_owned();
        if title.is_empty() && description.is_empty() {
            failures.push(scan_failure(
                binding,
                "wrong_shape",
                "summary title and description are both empty".to_owned(),
            ));
            continue;
        }
        rows.push(SegmentRow {
            hour: binding.segment[..2].to_owned(),
            entry: TimelineEntryV1 {
                title,
                description,
                origin: timeline.summary.origin,
                binding: binding.clone(),
            },
            binding,
        });
    }
    rows.sort_by(|left, right| {
        left.binding
            .segment
            .cmp(&right.binding.segment)
            .then_with(|| left.binding.stream.cmp(&right.binding.stream))
    });
    Ok(DayScan { rows, failures })
}

fn scan_failure(binding: SegmentBindingV1, kind: &'static str, detail: String) -> DayScanFailure {
    DayScanFailure {
        binding,
        kind,
        detail: bounded_diagnostic_detail(&detail),
    }
}

pub fn origin_for_segment(segment: &Path) -> String {
    let mut after_chronicle = false;
    let mut parts = Vec::new();
    for component in segment.components() {
        let part = component.as_os_str().to_string_lossy();
        if after_chronicle {
            parts.push(part.into_owned());
        } else if part == "chronicle" {
            after_chronicle = true;
        }
    }
    if after_chronicle {
        parts.join("/")
    } else {
        segment
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned()
    }
}

fn group_by_hour(rows: &[SegmentRow]) -> BTreeMap<String, Vec<&SegmentRow>> {
    let mut grouped = BTreeMap::<String, Vec<&SegmentRow>>::new();
    for row in rows {
        grouped.entry(row.hour.clone()).or_default().push(row);
    }
    grouped
}

fn load_day_rollups(journal: &Path) -> Result<MasterScan, String> {
    let chronicle = journal.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(MasterScan {
            days: BTreeMap::new(),
            failures: Vec::new(),
        });
    }
    let mut days = std::fs::read_dir(&chronicle)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_day)
        })
        .collect::<Vec<_>>();
    days.sort();
    let mut rollups = BTreeMap::new();
    let mut failures = Vec::new();
    for day_dir in days {
        let day = day_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let path = day_dir.join("timeline.json");
        if !path.exists() {
            continue;
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                failures.push(master_scan_failure(day, "unreadable", error.to_string()));
                continue;
            }
        };
        let timeline = match serde_json::from_slice::<DayTimelineV1>(&bytes) {
            Ok(timeline) => timeline,
            Err(error) => {
                let kind = if serde_json::from_slice::<Value>(&bytes).is_err() {
                    "malformed_json"
                } else {
                    "wrong_shape"
                };
                failures.push(master_scan_failure(day, kind, error.to_string()));
                continue;
            }
        };
        if let Err(error) = validate_day_timeline(&timeline) {
            failures.push(master_scan_failure(day, "wrong_shape", error.to_string()));
            continue;
        }
        if timeline.day != day {
            failures.push(master_scan_failure(
                day,
                "wrong_shape",
                "artifact day does not match its directory".to_owned(),
            ));
            continue;
        }
        let artifact_text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(error) => {
                failures.push(master_scan_failure(
                    day,
                    "malformed_json",
                    error.to_string(),
                ));
                continue;
            }
        };
        match evaluate_artifact_currentness(
            journal,
            &day_subject_key(&day),
            &timeline.source_digest,
            timeline.generated_at_ms,
            artifact_text,
        ) {
            Ok(ArtifactCurrentness::Current) => {}
            Ok(ArtifactCurrentness::Stale) => {
                failures.push(master_scan_failure(
                    day,
                    "stale",
                    "artifact does not match durable publication state".to_owned(),
                ));
                continue;
            }
            Ok(ArtifactCurrentness::Missing) => {
                failures.push(master_scan_failure(
                    day,
                    "state_missing",
                    "artifact has no durable publication state".to_owned(),
                ));
                continue;
            }
            Err(error) => {
                failures.push(master_scan_failure(
                    day,
                    "state_unavailable",
                    error.to_string(),
                ));
                continue;
            }
        }
        rollups.insert(day, timeline);
    }
    Ok(MasterScan {
        days: rollups,
        failures,
    })
}

fn master_scan_failure(day: String, kind: &'static str, detail: String) -> MasterScanFailure {
    MasterScanFailure {
        day,
        kind,
        detail: bounded_diagnostic_detail(&detail),
    }
}

fn group_by_month(day_rollups: &BTreeMap<String, DayTimelineV1>) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for day in day_rollups.keys() {
        grouped
            .entry(day[..6].to_owned())
            .or_default()
            .push(day.clone());
    }
    grouped
}

fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_month(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn read_config(journal: &Path) -> Result<Map<String, Value>, String> {
    read_journal_config(journal)
        .map_err(|error| error.to_string())
        .map(|read| read.config.unwrap_or_default())
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout: format!("{stdout}\n"),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(error: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("timeline maintenance: {error}\n"),
        exit_code: 1,
    }
}

fn usage_error(id: &str, detail: &str) -> CliRun {
    let usage = match id {
        "timeline:rollup" | "timeline:rollup-day" => {
            " [--day YYYYMMDD] [--commit | --dry-run] [--force] [--top TOP] [--jobs JOBS]"
        }
        _ => " [--commit | --dry-run] [--force] [--top TOP] [--jobs JOBS] [--months MONTHS]",
    };
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "usage: journal maintenance run {id}{usage}\njournal maintenance run {id}: error: {detail}\n"
        ),
        exit_code: 2,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "temporary journals and canned model responses are test-only"
)]
mod tests {
    use super::{
        SegmentRow, day_curation_jobs, day_source_digest, load_day_segments, master_curation_jobs,
        master_digest, pick_entries, run,
    };
    use crate::timezone::HostTimezoneSource;
    use crate::{RollupPicker, TimelineServices};
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
    use solstone_core_generate::{ContentPart, GenerateRequest, GeneratedResponse};
    use solstone_core_timeline::{
        AttemptOutcome, CURRENT_SCHEMA_VERSION, CurationRecordV1, DayTimelineV1, SegmentBindingV1,
        SegmentSummaryV1, SegmentTimelineV1, TimelineEntryV1, TimelineKind, curation_jobs_digest,
        load_timeline_state, publish_day_timeline, publish_segment_timeline,
        record_attempt_outcome, segment_subject_key,
    };
    use std::collections::{BTreeMap, VecDeque};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    struct Host(&'static str);

    impl HostTimezoneSource for Host {
        fn usable_iana_key(&self) -> Option<String> {
            Some(self.0.to_owned())
        }
    }

    struct Picker {
        replies: Mutex<VecDeque<Result<GeneratedResponse, String>>>,
        requests: Mutex<Vec<GenerateRequest>>,
        fail_when: Option<&'static str>,
    }

    impl Picker {
        fn canned(replies: impl IntoIterator<Item = Result<&'static str, &'static str>>) -> Self {
            Self {
                replies: Mutex::new(
                    replies
                        .into_iter()
                        .map(|reply| reply.map(generated_response).map_err(str::to_owned))
                        .collect(),
                ),
                requests: Mutex::new(Vec::new()),
                fail_when: None,
            }
        }

        fn error_when(text: &'static str) -> Self {
            Self {
                replies: Mutex::new(VecDeque::new()),
                requests: Mutex::new(Vec::new()),
                fail_when: Some(text),
            }
        }

        fn call_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl RollupPicker for Picker {
        fn pick(&self, request: &GenerateRequest) -> Result<GeneratedResponse, String> {
            self.requests.lock().unwrap().push(request.clone());
            let contents = match request.contents.first() {
                Some(ContentPart::Text { text }) => text,
                _ => "",
            };
            if self
                .fail_when
                .is_some_and(|needle| contents.contains(needle))
            {
                return Err(format!("canned failure for {contents}"));
            }
            self.replies.lock().unwrap().pop_front().unwrap_or_else(|| {
                Ok(generated_response(
                    "{\"picks\":[0],\"rationale\":\"canned\"}",
                ))
            })
        }
    }

    fn generated_response(text: impl Into<String>) -> GeneratedResponse {
        GeneratedResponse {
            id: None,
            text: text.into(),
            model: "fixture-byo-model".to_owned(),
            usage: json!({"input_tokens": 7, "output_tokens": 3}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: Some(json!({"valid": true})),
            input_budget: None,
            request_budget: None,
            inference: Some(json!({"provider": "fixture-byo"})),
            hints_applied: Vec::new(),
        }
    }

    struct Model;

    impl Model {
        const fn new(_: &'static str) -> Self {
            Self
        }
    }

    fn services<'a>(picker: &'a Picker, _model: &'a Model) -> TimelineServices<'a> {
        static HOST: Host = Host("UTC");
        TimelineServices {
            now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap(),
            host_timezone: &HOST,
            picker,
        }
    }

    #[test]
    fn invalid_model_selections_fail_and_short_circuit_never_calls_the_model() {
        let picker = Picker::canned([Ok("{\"picks\":[2,2],\"rationale\":\"duplicate\"}")]);
        let entries = vec![
            entry("Alpha", "first", "a", "080000_1"),
            entry("Bravo", "second", "b", "080100_2"),
            entry("Charlie", "third", "c", "080200_3"),
        ];
        assert!(matches!(
            pick_entries(
                &picker,
                &entries,
                2,
                "month",
                solstone_core_timeline::TimelineCurationStage::Master,
            ),
            Err(
                solstone_core_timeline::TimelineError::InvalidModelSelection {
                    stage: solstone_core_timeline::TimelineCurationStage::Master,
                    index: 2,
                    candidate_count: 3,
                    reason: solstone_core_timeline::InvalidSelectionReason::Duplicate,
                }
            )
        ));
        let request = picker.requests.lock().unwrap().pop().unwrap();
        assert_eq!(request.context, "timeline.scratch.rollup");
        assert_eq!(request.temperature, 0.3);
        assert_eq!(request.max_output_tokens, 2048);
        assert_eq!(request.timeout_s, Some(60.0));
        assert!(request.json_output);
        assert_eq!(
            request.contents,
            vec![ContentPart::Text {
                text: "Candidate events:\n\n  [0] Alpha — first\n  [1] Bravo — second\n  [2] Charlie — third".to_owned()
            }]
        );
        assert_eq!(
            request.json_schema.unwrap(),
            json!({
                "type": "object",
                "properties": {
                    "picks": {"type": "array", "description": "Zero-based indices of the most important events from the candidate list, in order of importance (most important first). Length must be exactly 2 (or fewer if input has fewer).", "items": {"type": "integer"}},
                    "rationale": {"type": "string", "description": "ONE sentence, max 100 chars, naming the criterion that drove the pick. For debugging — not shown in the UI."}
                },
                "required": ["picks", "rationale"],
                "additionalProperties": false
            })
        );
        assert!(request.system_instruction.unwrap().contains("one month"));

        let entries = &entries[..2];
        let picker = Picker::canned([Ok("{\"picks\":[3],\"rationale\":\"bad\"}")]);
        assert!(matches!(
            pick_entries(
                &picker,
                entries,
                1,
                "day",
                solstone_core_timeline::TimelineCurationStage::Day,
            ),
            Err(
                solstone_core_timeline::TimelineError::InvalidModelSelection {
                    stage: solstone_core_timeline::TimelineCurationStage::Day,
                    index: 3,
                    candidate_count: 2,
                    reason: solstone_core_timeline::InvalidSelectionReason::OutOfRange,
                }
            )
        ));

        let picker = Picker::canned([]);
        let short_circuit = pick_entries(
            &picker,
            &entries[..1],
            1,
            "day",
            solstone_core_timeline::TimelineCurationStage::Day,
        )
        .unwrap();
        assert_eq!(short_circuit.picks, entries[..1]);
        assert_eq!(picker.call_count(), 0);
    }

    #[test]
    fn day_preview_is_safe_and_currentness_requires_a_valid_matching_artifact() {
        let journal = tempfile::tempdir().unwrap();
        let day = "20260301";
        write_segment(
            journal.path(),
            day,
            "field.audio",
            "080000_1",
            json!({"title":"New", "description":"event"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let preview = run(
            "timeline:rollup-day",
            &["--day".to_owned(), day.to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(preview.exit_code, 0);
        assert!(preview.stdout.contains("[would_publish]"));
        assert_eq!(picker.call_count(), 0);
        let output = journal
            .path()
            .join("chronicle")
            .join(day)
            .join("timeline.json");
        assert!(!output.exists());

        let committed = run(
            "timeline:rollup-day",
            &["--day".to_owned(), day.to_owned(), "--commit".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(committed.exit_code, 0);
        assert_eq!(
            read_json(&output)["day_curation"]["picks"][0]["title"],
            "New"
        );

        let current = run(
            "timeline:rollup-day",
            &["--day".to_owned(), day.to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert!(current.stdout.contains("[current]"));

        std::fs::write(&output, b"{\"old\":true}\n").unwrap();
        let stale = run(
            "timeline:rollup-day",
            &["--day".to_owned(), day.to_owned(), "--dry-run".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert!(stale.stdout.contains("[stale]"));

        let force_without_commit = run(
            "timeline:rollup-day",
            &["--day".to_owned(), day.to_owned(), "--force".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(force_without_commit.exit_code, 2);
        assert!(
            force_without_commit
                .stderr
                .contains("--force requires --commit")
        );

        let contradictory_modes = run(
            "timeline:rollup-day",
            &[
                "--day".to_owned(),
                day.to_owned(),
                "--commit".to_owned(),
                "--dry-run".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(contradictory_modes.exit_code, 2);
        assert!(
            contradictory_modes
                .stderr
                .contains("--commit and --dry-run cannot be combined")
        );
    }

    #[test]
    fn day_empty_and_dry_run_follow_the_reference_boundaries() {
        let journal = tempfile::tempdir().unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let empty = run(
            "timeline:rollup-day",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--dry-run".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(empty.exit_code, 66);
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"Dry", "description":"run"}),
        );
        let dry = run(
            "timeline:rollup-day",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--dry-run".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(dry.exit_code, 0);
        assert_eq!(picker.call_count(), 0);
        assert!(
            !journal
                .path()
                .join("chronicle/20260301/timeline.json")
                .exists()
        );
    }

    #[test]
    fn preview_stays_read_only_when_commit_would_fail_timeline_lock_acquisition() {
        let journal = tempfile::tempdir().unwrap();
        let day = "20260301";
        write_segment(
            journal.path(),
            day,
            "field.audio",
            "080000_1",
            json!({"title":"Preview", "description":"must not write"}),
        );
        std::fs::remove_dir_all(journal.path().join("health/timeline/locks")).unwrap();
        std::fs::write(
            journal.path().join("health/timeline/locks"),
            b"not a directory",
        )
        .unwrap();
        let before = file_snapshot(journal.path());
        let picker = Picker::canned([]);
        let model = Model::new("model");

        let preview = run(
            "timeline:rollup",
            &["--day".to_owned(), day.to_owned(), "--dry-run".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );

        assert_eq!(preview.exit_code, 0);
        assert!(preview.stdout.contains("[would_publish]"));
        assert_eq!(file_snapshot(journal.path()), before);

        let commit = run(
            "timeline:rollup",
            &["--day".to_owned(), day.to_owned(), "--commit".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );

        assert_eq!(commit.exit_code, 1);
        assert!(commit.stderr.contains("timeline lock contention"));
        assert_eq!(file_snapshot(journal.path()), before);
    }

    fn file_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn collect(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    collect(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        std::fs::read(path).unwrap(),
                    );
                }
            }
        }

        let mut files = BTreeMap::new();
        collect(root, root, &mut files);
        files
    }

    #[test]
    fn day_scan_reports_every_failure_mode_and_aborts_without_a_partial_artifact() {
        let journal = tempfile::tempdir().unwrap();
        let day = "20260301";
        write_segment(
            journal.path(),
            day,
            "field.audio",
            "080000_1",
            json!({"title":"Good", "description":"row"}),
        );
        create_segment_dir(journal.path(), day, "field.audio", "090000_2");
        let unreadable = journal
            .path()
            .join("chronicle")
            .join(day)
            .join("field.audio")
            .join("100000_3")
            .join("timeline.json");
        std::fs::create_dir_all(&unreadable).unwrap();
        write_raw_segment(journal.path(), day, "field.audio", "110000_4", b"{not json");
        write_raw_segment(journal.path(), day, "field.audio", "120000_5", b"[]");
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let result = run(
            "timeline:rollup-day",
            &["--day".to_owned(), day.to_owned(), "--commit".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 1);
        for kind in ["missing", "unreadable", "malformed_json", "wrong_shape"] {
            assert!(
                result.stderr.contains(kind),
                "missing {kind}: {}",
                result.stderr
            );
        }
        assert!(
            !journal
                .path()
                .join("chronicle/20260301/timeline.json")
                .exists()
        );
    }

    #[test]
    fn day_origin_falls_back_to_path_but_embedded_origin_wins() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "first.audio",
            "080000_1",
            json!({"title":"Path", "description":"origin"}),
        );
        write_segment(
            journal.path(),
            "20260301",
            "second.audio",
            "080100_2",
            json!({"title":"Embedded", "description":"origin", "origin":"custom-origin"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let result = run(
            "timeline:rollup-day",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--commit".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        let payload = read_json(&journal.path().join("chronicle/20260301/timeline.json"));
        assert_eq!(
            payload["day_curation"]["picks"][0]["origin"],
            "20260301/first.audio/080000_1"
        );
        assert_eq!(
            payload["day_curation"]["picks"][1]["origin"],
            "custom-origin"
        );
    }

    #[test]
    fn day_curation_failures_are_fail_closed() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"bad-hour", "description":"x"}),
        );
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080100_2",
            json!({"title":"bad-hour-more", "description":"x"}),
        );
        let picker = Picker::error_when("bad-hour");
        let model = Model::new("model");
        let hour_error = run(
            "timeline:rollup-day",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--commit".to_owned(),
                "--top".to_owned(),
                "1".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(hour_error.exit_code, 1);
        assert!(
            !journal
                .path()
                .join("chronicle/20260301/timeline.json")
                .exists()
        );
        assert!(
            load_timeline_state(journal.path())
                .unwrap()
                .attempts
                .values()
                .any(|attempt| attempt.outcome == AttemptOutcome::Failed)
        );
    }

    #[test]
    fn day_rollup_records_response_provenance_and_order_sensitive_source_digest() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"One", "description":"event"}),
        );
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080100_2",
            json!({"title":"Two", "description":"event"}),
        );
        let picker = Picker::canned([
            Ok("{\"picks\":[1],\"rationale\":\"hour\"}"),
            Ok("{\"picks\":[0],\"rationale\":\"day\"}"),
        ]);
        let model = Model::new("resolved-model");
        let result = run(
            "timeline:rollup-day",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--commit".to_owned(),
                "--top".to_owned(),
                "1".to_owned(),
                "--jobs".to_owned(),
                "1".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(picker.call_count(), 2);
        let scan = load_day_segments(journal.path(), "20260301").unwrap();
        let forward = day_source_digest(&scan.rows, 1).unwrap();
        let mut reversed = scan.rows;
        reversed.reverse();
        assert_ne!(forward, day_source_digest(&reversed, 1).unwrap());
        let timeline = read_json(&journal.path().join("chronicle/20260301/timeline.json"));
        assert_eq!(timeline["source_digest"], Value::String(forward));
        assert_eq!(
            timeline["day_curation"]["provenance"]["model"],
            "fixture-byo-model"
        );
        assert_eq!(
            timeline["day_curation"]["provenance"]["inference"]["provider"],
            "fixture-byo"
        );
        for lock in [
            "health/timeline/locks/population.lock",
            "health/timeline/locks/days/20260301.order.lock",
            "health/timeline/locks/subjects/day/20260301.attempt.lock",
        ] {
            assert!(journal.path().join(lock).exists(), "missing {lock}");
        }
        let invalid = run(
            "timeline:rollup-day",
            &["--day".to_owned(), "not-a-day".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(invalid.exit_code, 2);
        assert!(invalid.stderr.contains("day must be YYYYMMDD"));
    }

    #[test]
    fn day_preview_names_a_segment_with_a_newer_failed_input_as_stale() {
        let journal = tempfile::tempdir().unwrap();
        let day = "20260301";
        let stream = "field.audio";
        let segment = "080000_1";
        write_segment(
            journal.path(),
            day,
            stream,
            segment,
            json!({"title":"Current", "description":"event"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        assert_eq!(
            run(
                "timeline:rollup-day",
                &["--day".to_owned(), day.to_owned(), "--commit".to_owned()],
                journal.path(),
                &services(&picker, &model),
            )
            .exit_code,
            0
        );
        let binding = SegmentBindingV1 {
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
        };
        let attempt = solstone_core_timeline::AttemptStateV1 {
            attempt_id: "newer-failed".to_owned(),
            input_digest: "changed-input".to_owned(),
            started_at_ms: 2,
            finished_at_ms: None,
            outcome: AttemptOutcome::Running,
            detail: String::new(),
        };
        record_attempt_outcome(
            journal.path(),
            &segment_subject_key(&binding),
            attempt,
            AttemptOutcome::Failed,
            "fixture failure",
            3,
        )
        .unwrap();

        let preview = run(
            "timeline:rollup-day",
            &["--day".to_owned(), day.to_owned(), "--dry-run".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(preview.exit_code, 1);
        assert!(preview.stderr.contains("stale"));
    }

    #[test]
    fn day_source_digest_covers_each_hour_request() {
        let rows = vec![
            SegmentRow {
                binding: SegmentBindingV1 {
                    day: "20260301".to_owned(),
                    stream: "_default".to_owned(),
                    segment: "080000_1".to_owned(),
                },
                hour: "08".to_owned(),
                entry: entry("One", "event", "one", "080000_1"),
            },
            SegmentRow {
                binding: SegmentBindingV1 {
                    day: "20260301".to_owned(),
                    stream: "_default".to_owned(),
                    segment: "090000_1".to_owned(),
                },
                hour: "09".to_owned(),
                entry: entry("Two", "event", "two", "090000_1"),
            },
        ];
        let baseline = day_source_digest(&rows, 1).unwrap();
        let mut jobs = day_curation_jobs(&rows, 1);
        jobs[0].request.system_instruction = Some("changed hour prompt".to_owned());

        assert_ne!(baseline, curation_jobs_digest(&jobs).unwrap());
    }

    #[test]
    fn day_default_uses_the_owner_timezone_seam() {
        let journal = tempfile::tempdir().unwrap();
        write_config(
            journal.path(),
            json!({"identity": {"timezone": "Asia/Tokyo"}}),
        );
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"Owner date", "description":"wins"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let result = run(
            "timeline:rollup-day",
            &["--commit".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            read_json(&journal.path().join("chronicle/20260301/timeline.json"))["day"],
            "20260301"
        );
    }

    #[test]
    fn orchestration_stops_after_a_day_failure_without_running_master() {
        let journal = tempfile::tempdir().unwrap();
        for index in 0..5 {
            write_segment(
                journal.path(),
                "20260301",
                "field.audio",
                &format!("08{index:02}00_{index}"),
                json!({"title":"bad event", "description":"fails day curation"}),
            );
        }
        write_day_rollup(
            journal.path(),
            "20260302",
            (0..5)
                .map(|index| {
                    day_entry(
                        "20260302",
                        "Other",
                        "master input",
                        &format!("09{index:02}00_{index}"),
                    )
                })
                .collect(),
        );
        let picker = Picker::error_when("bad event");
        let model = Model::new("model");

        let result = run(
            "timeline:rollup",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--commit".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );

        assert_eq!(result.exit_code, 1);
        assert_eq!(picker.call_count(), 1);
        assert!(!journal.path().join("timeline.json").exists());
    }

    #[test]
    fn orchestration_runs_master_after_a_current_day() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"Current", "description":"day"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let day = run(
            "timeline:rollup-day",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--commit".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(day.exit_code, 0);

        let result = run(
            "timeline:rollup",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--commit".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );

        assert_eq!(result.exit_code, 0);
        assert!(journal.path().join("timeline.json").exists());
    }

    #[test]
    fn orchestration_publishes_day_then_master_and_preview_never_writes() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"New", "description":"day"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let preview = run(
            "timeline:rollup",
            &["--day".to_owned(), "20260301".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(preview.exit_code, 0);
        assert!(preview.stdout.contains("[would_publish]"));
        assert_eq!(picker.call_count(), 0);
        assert!(
            !journal
                .path()
                .join("chronicle/20260301/timeline.json")
                .exists()
        );
        assert!(!journal.path().join("timeline.json").exists());

        let committed = run(
            "timeline:rollup",
            &[
                "--day".to_owned(),
                "20260301".to_owned(),
                "--commit".to_owned(),
            ],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(committed.exit_code, 0);
        assert!(
            journal
                .path()
                .join("chronicle/20260301/timeline.json")
                .exists()
        );
        assert!(journal.path().join("timeline.json").exists());
    }

    #[test]
    fn master_preview_currentness_locks_and_cli_modes_are_safe() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            vec![day_entry("20260101", "January", "first", "080000_1")],
        );
        write_day_rollup(
            journal.path(),
            "20260102",
            vec![day_entry("20260102", "January two", "second", "080000_2")],
        );
        let picker = Picker::canned([
            Ok("{\"picks\":[0],\"rationale\":\"month\"}"),
            Ok("{\"picks\":[1],\"rationale\":\"year\"}"),
        ]);
        let model = Model::new("unused");
        let preview = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(preview.exit_code, 0);
        assert!(preview.stdout.contains("[would_publish]"));
        assert_eq!(picker.call_count(), 0);

        let committed = run(
            "timeline:rollup-master",
            &["--commit".to_owned(), "--top".to_owned(), "1".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(committed.exit_code, 0);
        let timeline = serde_json::from_slice::<solstone_core_timeline::MasterTimelineV1>(
            &std::fs::read(journal.path().join("timeline.json")).unwrap(),
        )
        .unwrap();
        solstone_core_timeline::validate_master_timeline(&timeline).unwrap();
        assert_eq!(timeline.months["202601"].day_count, 2);
        assert_eq!(
            timeline.year_curation.provenance.unwrap().model,
            "fixture-byo-model"
        );
        for lock in [
            "health/timeline/locks/population.lock",
            "health/timeline/locks/days/20260101.order.lock",
            "health/timeline/locks/days/20260102.order.lock",
            "health/timeline/locks/subjects/master.attempt.lock",
        ] {
            assert!(journal.path().join(lock).exists(), "missing {lock}");
        }
        let current = run(
            "timeline:rollup-master",
            &["--top".to_owned(), "1".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert!(current.stdout.contains("[current]"));
        std::fs::write(journal.path().join("timeline.json"), b"{\"old\":true}\n").unwrap();
        let stale = run(
            "timeline:rollup-master",
            &["--dry-run".to_owned(), "--top".to_owned(), "1".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert!(stale.stdout.contains("[stale]"));
        let force_without_commit = run(
            "timeline:rollup-master",
            &["--force".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(force_without_commit.exit_code, 2);
        let contradictory_modes = run(
            "timeline:rollup-master",
            &["--commit".to_owned(), "--dry-run".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(contradictory_modes.exit_code, 2);

        let artifact_before = std::fs::read(journal.path().join("timeline.json")).unwrap();
        let state_before =
            std::fs::read(journal.path().join("health/timeline/state.json")).unwrap();
        let calls_before = picker.call_count();
        for month_value in ["202601", "", ",,,"] {
            let filtered_commit = run(
                "timeline:rollup-master",
                &[
                    "--commit".to_owned(),
                    "--months".to_owned(),
                    month_value.to_owned(),
                ],
                journal.path(),
                &services(&picker, &model),
            );
            assert_eq!(filtered_commit.exit_code, 2, "value={month_value:?}");
            assert_eq!(picker.call_count(), calls_before);
            assert_eq!(
                std::fs::read(journal.path().join("timeline.json")).unwrap(),
                artifact_before
            );
            assert_eq!(
                std::fs::read(journal.path().join("health/timeline/state.json")).unwrap(),
                state_before
            );
        }
    }

    #[test]
    fn master_scan_reports_invalid_day_artifacts_without_partial_publication() {
        let journal = tempfile::tempdir().unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        write_day_rollup(
            journal.path(),
            "20260101",
            vec![day_entry("20260101", "Good", "entry", "080000_1")],
        );
        std::fs::create_dir_all(journal.path().join("chronicle/20260102")).unwrap();
        std::fs::write(
            journal.path().join("chronicle/20260102/timeline.json"),
            b"[]",
        )
        .unwrap();
        std::fs::create_dir_all(journal.path().join("chronicle/20260103")).unwrap();
        std::fs::write(
            journal.path().join("chronicle/20260103/timeline.json"),
            b"{not json",
        )
        .unwrap();
        std::fs::create_dir_all(journal.path().join("chronicle/20260104/timeline.json")).unwrap();
        let result = run(
            "timeline:rollup-master",
            &["--commit".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 1);
        for kind in ["wrong_shape", "malformed_json", "unreadable"] {
            assert!(
                result.stderr.contains(kind),
                "missing {kind}: {}",
                result.stderr
            );
        }
        assert!(!journal.path().join("timeline.json").exists());
    }

    #[test]
    fn master_invalid_model_selection_is_fail_closed() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            vec![day_entry("20260101", "Jan", "first", "080000_1")],
        );
        write_day_rollup(
            journal.path(),
            "20260102",
            vec![day_entry("20260102", "Jan two", "second", "080000_2")],
        );
        let picker = Picker::canned([Ok("{\"picks\":[0,0],\"rationale\":\"duplicate\"}")]);
        let model = Model::new("model");
        let result = run(
            "timeline:rollup-master",
            &["--commit".to_owned(), "--top".to_owned(), "1".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("duplicate"));
        assert!(!journal.path().join("timeline.json").exists());
        assert!(
            load_timeline_state(journal.path())
                .unwrap()
                .attempts
                .values()
                .any(|attempt| attempt.outcome == AttemptOutcome::Failed)
        );
    }

    #[test]
    fn master_source_digest_covers_each_month_request() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            vec![day_entry("20260101", "January", "first", "080000_1")],
        );
        write_day_rollup(
            journal.path(),
            "20260201",
            vec![day_entry("20260201", "February", "second", "080000_2")],
        );
        let days = ["20260101", "20260201"]
            .into_iter()
            .map(|day| {
                let timeline = serde_json::from_slice::<DayTimelineV1>(
                    &std::fs::read(
                        journal
                            .path()
                            .join("chronicle")
                            .join(day)
                            .join("timeline.json"),
                    )
                    .unwrap(),
                )
                .unwrap();
                (day.to_owned(), timeline)
            })
            .collect::<BTreeMap<_, _>>();
        let baseline = master_digest(&days, 1).unwrap();
        let mut jobs = master_curation_jobs(&days, 1);
        jobs[0].request.system_instruction = Some("changed month prompt".to_owned());

        assert_ne!(baseline, curation_jobs_digest(&jobs).unwrap());
    }

    fn write_segment(journal: &Path, day: &str, stream: &str, segment: &str, value: Value) {
        let segment_dir = journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment);
        std::fs::create_dir_all(segment_dir.join("talents")).unwrap();
        std::fs::write(segment_dir.join("talents/activity.md"), "fixture activity").unwrap();
        let binding = SegmentBindingV1 {
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
        };
        let snapshot = solstone_core_timeline::resolve_activity_source(journal, &binding)
            .unwrap()
            .unwrap();
        let source = solstone_core_timeline::SegmentSourceV1::GeneratedActivity {
            schema_version: solstone_core_timeline::SEGMENT_SOURCE_SCHEMA_VERSION,
            relative_path: snapshot.relative_path,
            sha256: snapshot.sha256,
        };
        let timeline = SegmentTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Segment,
            binding: binding.clone(),
            input_digest: solstone_core_timeline::segment_input_digest(&binding, &source).unwrap(),
            source: Some(source),
            generated_at_ms: 1,
            summary: SegmentSummaryV1 {
                title: value
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                description: value
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                origin: value
                    .get("origin")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| format!("{day}/{stream}/{segment}")),
                continuation_of: None,
            },
            provenance: None,
        };
        publish_segment_timeline(
            journal,
            &timeline,
            solstone_core_timeline::AttemptStateV1 {
                attempt_id: format!("fixture-segment-{day}-{stream}-{segment}"),
                input_digest: timeline.input_digest.clone(),
                started_at_ms: timeline.generated_at_ms,
                finished_at_ms: None,
                outcome: AttemptOutcome::Running,
                detail: String::new(),
            },
        )
        .unwrap();
    }

    fn create_segment_dir(journal: &Path, day: &str, stream: &str, segment: &str) {
        std::fs::create_dir_all(
            journal
                .join("chronicle")
                .join(day)
                .join(stream)
                .join(segment),
        )
        .unwrap();
    }

    fn entry(
        title: &str,
        description: &str,
        origin: &str,
        segment: &str,
    ) -> solstone_core_timeline::TimelineEntryV1 {
        solstone_core_timeline::TimelineEntryV1 {
            title: title.to_owned(),
            description: description.to_owned(),
            origin: origin.to_owned(),
            binding: SegmentBindingV1 {
                day: "20260301".to_owned(),
                stream: "field.audio".to_owned(),
                segment: segment.to_owned(),
            },
        }
    }

    fn write_raw_segment(journal: &Path, day: &str, stream: &str, segment: &str, contents: &[u8]) {
        let path = journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment)
            .join("timeline.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn day_entry(day: &str, title: &str, description: &str, segment: &str) -> TimelineEntryV1 {
        TimelineEntryV1 {
            title: title.to_owned(),
            description: description.to_owned(),
            origin: format!("{day}/field.audio/{segment}"),
            binding: SegmentBindingV1 {
                day: day.to_owned(),
                stream: "field.audio".to_owned(),
                segment: segment.to_owned(),
            },
        }
    }

    fn write_day_rollup(journal: &Path, day: &str, picks: Vec<TimelineEntryV1>) {
        let path = journal.join("chronicle").join(day).join("timeline.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let candidate_count = picks.len();
        let curation = CurationRecordV1 {
            input_digest: format!("fixture-{day}"),
            candidate_count,
            picks,
            rationale: "fixture".to_owned(),
            error: None,
            provenance: None,
        };
        let timeline = DayTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Day,
            day: day.to_owned(),
            source_digest: format!("fixture-source-{day}"),
            generated_at_ms: 1,
            top_n: 4,
            segment_count: candidate_count,
            hour_count: 0,
            hours: BTreeMap::new(),
            day_curation: curation,
        };
        publish_day_timeline(
            journal,
            &timeline,
            solstone_core_timeline::AttemptStateV1 {
                attempt_id: format!("fixture-day-{day}"),
                input_digest: timeline.source_digest.clone(),
                started_at_ms: timeline.generated_at_ms,
                finished_at_ms: None,
                outcome: AttemptOutcome::Running,
                detail: String::new(),
            },
        )
        .unwrap();
    }

    fn write_config(journal: &Path, config: Value) {
        let path = journal.join("config/journal.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, config.to_string()).unwrap();
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
}
