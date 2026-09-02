// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native timeline day and master rollups.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest, GeneratedResponse};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};
use solstone_core_timeline::{
    AttemptOutcome, AttemptStateV1, CURRENT_SCHEMA_VERSION, CurationContentPartV1,
    CurationRecordV1, CurationRequestV1, DayTimelineV1, GenerationProvenanceV1, HourTimelineV1,
    InvalidSelectionReason, SegmentBindingV1, SegmentTimelineV1, TimelineCurationStage,
    TimelineEntryV1, TimelineError, TimelineKind, TimelineLockRequest, TimelineLockSubject,
    acquire_timeline_locks, bounded_diagnostic_detail, curation_input_digest, day_subject_key,
    day_timeline_path, discover_day_segment_bindings, load_timeline_state, publish_day_timeline,
    record_attempt_outcome, segment_directory, validate_day_timeline, validate_segment_timeline,
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

struct PickResult {
    picks: Vec<Value>,
    rationale: String,
}

struct BatchResult {
    key: String,
    result: Result<PickResult, TimelineError>,
}

static DAY_ATTEMPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn run(
    id: &str,
    args: &[String],
    journal: &Path,
    services: &TimelineServices<'_>,
) -> CliRun {
    match id {
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
                options.months = (!months.is_empty()).then_some(months);
                index += 1;
            }
            "--force" => options.force = true,
            "--dry-run" => options.dry_run = true,
            value => return Err(format!("unrecognized arguments: {value}")),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_usize(value: Option<&String>, name: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("argument {name}: expected one argument"))?;
    value
        .parse()
        .map_err(|_| format!("argument {name}: invalid int value: '{value}'"))
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
    let by_hour = group_by_hour(&entries);
    let jobs = by_hour
        .iter()
        .map(|(hour, rows)| EntryBatchInput {
            key: hour.clone(),
            candidates: rows.iter().map(|row| row.entry.clone()).collect(),
        })
        .collect::<Vec<_>>();
    let hour_results =
        match pick_entry_batch(services.picker, jobs, options.top, "hour", options.jobs) {
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
    let day_picked = match pick_entries(services.picker, &day_candidates, options.top, "day") {
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
    if let Err(error) = publish_day_timeline(journal, &timeline, attempt) {
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
    let candidates = rows.iter().map(|row| row.entry.clone()).collect::<Vec<_>>();
    let request = entry_rollup_request(&candidates, top, "day");
    curation_input_digest(&candidates, &curation_request(&request))
}

fn day_is_current(journal: &Path, day: &str, source_digest: &str) -> bool {
    let path = day_timeline_path(journal, day);
    let timeline = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<DayTimelineV1>(&bytes).ok());
    let Some(timeline) = timeline else {
        return false;
    };
    if validate_day_timeline(&timeline).is_err()
        || timeline.day != day
        || timeline.source_digest != source_digest
    {
        return false;
    }
    load_timeline_state(journal)
        .ok()
        .and_then(|state| state.artifacts.get(&day_subject_key(day)).cloned())
        .is_some_and(|artifact| artifact.input_digest == source_digest)
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
    let sequence = DAY_ATTEMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    AttemptStateV1 {
        attempt_id: format!("day-{day}-{sequence}"),
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
    let _ = record_attempt_outcome(
        journal,
        &day_subject_key(day),
        attempt.clone(),
        AttemptOutcome::Failed,
        &detail,
        attempt.started_at_ms,
    );
    failure(detail)
}

fn rollup_master(
    journal: &Path,
    options: MasterOptions,
    services: &TimelineServices<'_>,
) -> CliRun {
    let out_path = journal.join("timeline.json");
    if out_path.exists() && !options.force && !options.dry_run {
        return success(format!(
            "  [skip] {}: already exists (use --force to overwrite)",
            out_path.display()
        ));
    }
    let day_rollups = match load_day_rollups(journal) {
        Ok(rollups) => rollups,
        Err(error) => return failure(error),
    };
    if day_rollups.is_empty() {
        return CliRun {
            stdout: format!(
                "  [empty] no day-level timeline.json found under {}/chronicle/*/\n",
                journal.display()
            ),
            stderr: String::new(),
            exit_code: EXIT_EMPTY,
        };
    }
    let mut by_month = group_by_month(&day_rollups);
    if let Some(filter) = &options.months {
        by_month.retain(|month, _| filter.contains(month));
        if by_month.is_empty() {
            return success(format!(
                "  [empty] no overlap between --months {:?} and journal",
                filter.iter().collect::<Vec<_>>()
            ));
        }
    }
    if options.dry_run {
        return success(format!(
            "\n== dry run ==\ndays with day-level timeline.json: {}\nmonths covered                   : {}",
            day_rollups.len(),
            by_month.len()
        ));
    }
    let jobs = by_month
        .iter()
        .filter_map(|(month, days)| {
            let events = days
                .iter()
                .flat_map(|day| {
                    day_rollups[day]
                        .get("day_top")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(event_value)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            (!events.is_empty()).then_some(BatchResultInput {
                key: month.clone(),
                events,
            })
        })
        .collect::<Vec<_>>();
    if jobs.is_empty() {
        return success("  [empty] no month candidates found".to_owned());
    }
    let config = match read_config(journal) {
        Ok(config) => config,
        Err(error) => return failure(error),
    };
    let model = match services.model_resolver.resolve(&config) {
        Ok(model) => model,
        Err(error) => return failure(error),
    };
    let month_results = match pick_batch(
        services.picker,
        jobs.clone(),
        options.top,
        "month",
        options.jobs,
    ) {
        Ok(results) => results,
        Err(error) => {
            let detail = error.to_string();
            jobs.into_iter()
                .map(|job| BatchResult {
                    key: job.key,
                    result: Err(curation_error(&detail)),
                })
                .collect()
        }
    };
    let mut months = Map::new();
    let mut year_top = Vec::new();
    let mut stdout = String::new();
    for result in month_results {
        let days = by_month.get(&result.key).cloned().unwrap_or_default();
        let (month_top, month_rationale) = match result.result {
            Ok(picked) => (picked.picks, picked.rationale),
            Err(error) => {
                let detail = error.to_string();
                stdout.push_str(&format!(
                    "  [month-err {}] {}\n",
                    result.key,
                    truncate(&detail, 120)
                ));
                (Vec::new(), format!("ERROR: {}", truncate(&detail, 200)))
            }
        };
        if let Some(head) = month_top.first() {
            year_top.push(json!({
                "month": result.key,
                "title": string_field(head, "title"),
                "description": string_field(head, "description"),
                "origin": string_field(head, "origin"),
            }));
        }
        let embedded_days = days
            .iter()
            .map(|day| (day.clone(), day_rollups[day].clone()))
            .collect::<Map<_, _>>();
        months.insert(
            result.key,
            json!({
                "month_top": month_top,
                "month_rationale": month_rationale,
                "day_count": days.len(),
                "days": embedded_days,
            }),
        );
    }
    let payload = json!({
        "generated_at": services.now.timestamp(),
        "model": model,
        "top_n": options.top,
        "year_top": year_top,
        "months": months,
    });
    if let Err(error) = write_json(&out_path, &payload) {
        return failure(error);
    }
    stdout.push_str(&format!("\n[ok] wrote {}\n", out_path.display()));
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

#[derive(Clone)]
struct BatchResultInput {
    key: String,
    events: Vec<Value>,
}

fn pick_batch(
    picker: &dyn RollupPicker,
    jobs: Vec<BatchResultInput>,
    n: usize,
    scope_label: &str,
    max_concurrent: usize,
) -> Result<Vec<BatchResult>, TimelineError> {
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
                    scope.spawn(move || BatchResult {
                        key: job.key,
                        result: pick_one(picker, &job.events, n, scope_label),
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

fn pick_one(
    picker: &dyn RollupPicker,
    events: &[Value],
    n: usize,
    scope_label: &str,
) -> Result<PickResult, TimelineError> {
    if events.len() <= n {
        return Ok(PickResult {
            picks: events.to_vec(),
            rationale: "fewer than N candidates; returning all".to_owned(),
        });
    }
    let request = rollup_request(events, n, scope_label);
    let response = picker
        .pick(&request)
        .map_err(|error| curation_error(&error))?;
    let (indices, rationale) =
        parse_selection(&response, events.len(), n, TimelineCurationStage::Master)?;
    Ok(PickResult {
        picks: indices
            .into_iter()
            .map(|index| events[index].clone())
            .collect(),
        rationale,
    })
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
                        result: pick_entries(picker, &job.candidates, n, scope_label),
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
    let (indices, rationale) =
        parse_selection(&response, candidates.len(), n, TimelineCurationStage::Day)?;
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

fn rollup_request(events: &[Value], n: usize, scope_label: &str) -> GenerateRequest {
    rollup_request_with_prompt(build_user_prompt(events), n, scope_label)
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

fn build_user_prompt(events: &[Value]) -> String {
    let mut lines = vec!["Candidate events:\n".to_owned()];
    for (index, event) in events.iter().enumerate() {
        lines.push(format!(
            "  [{index}] {} — {}",
            string_field(event, "title"),
            string_field(event, "description")
        ));
    }
    lines.join("\n")
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

/// Write the timeline entry for one chronicle segment by atomic replacement.
pub fn write_segment_timeline(segment: &Path, payload: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    atomic_replace(
        segment.join("timeline.json"),
        &bytes,
        AtomicWriteOptions::default(),
    )
    .map_err(|error| error.to_string())
}

/// Write the deterministic continuation entry for a redundant segment.
pub fn write_continuation_summary(
    segment: &Path,
    predecessor_segment_key: &str,
) -> Result<(), String> {
    // Preserve solstone/apps/timeline/talent/segment_summary.py:70-88.
    write_segment_timeline(
        segment,
        &json!({
            "title": "Continued",
            "description": "Unchanged from the prior window.",
            "origin": origin_for_segment(segment),
            "continuation_of": predecessor_segment_key,
        }),
    )
}

fn group_by_hour(rows: &[SegmentRow]) -> BTreeMap<String, Vec<&SegmentRow>> {
    let mut grouped = BTreeMap::<String, Vec<&SegmentRow>>::new();
    for row in rows {
        grouped.entry(row.hour.clone()).or_default().push(row);
    }
    grouped
}

fn load_day_rollups(journal: &Path) -> Result<BTreeMap<String, Value>, String> {
    let chronicle = journal.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(BTreeMap::new());
    }
    let mut days = std::fs::read_dir(&chronicle)
        .map_err(|error| error.to_string())?
        .flatten()
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
    for day_dir in days {
        let path = day_dir.join("timeline.json");
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let object = value
            .as_object()
            .ok_or_else(|| format!("timeline.json at {} must be a JSON object", path.display()))?;
        if !object.get("day_top").is_some_and(is_truthy) {
            continue;
        }
        let day = day_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        rollups.insert(day, value);
    }
    Ok(rollups)
}

fn group_by_month(day_rollups: &BTreeMap<String, Value>) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for day in day_rollups.keys() {
        grouped
            .entry(day[..6].to_owned())
            .or_default()
            .push(day.clone());
    }
    grouped
}

fn event_value(event: &Value) -> Value {
    json!({
        "title": string_field(event, "title"),
        "description": string_field(event, "description"),
        "origin": string_field(event, "origin"),
    })
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_month(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn read_config(journal: &Path) -> Result<Map<String, Value>, String> {
    read_journal_config(journal)
        .map_err(|error| error.to_string())
        .map(|read| read.config.unwrap_or_default())
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
}

fn truncate(value: &str, limit: usize) -> &str {
    value.get(..limit).unwrap_or(value)
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
        "timeline:rollup-day" => {
            " [--day YYYYMMDD] [--commit | --dry-run] [--force] [--top TOP] [--jobs JOBS]"
        }
        _ => " [--top TOP] [--jobs JOBS] [--force] [--dry-run] [--months MONTHS]",
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
        day_source_digest, load_day_segments, pick_entries, pick_one, rollup_request, run,
        write_segment_timeline,
    };
    use crate::timezone::HostTimezoneSource;
    use crate::{GenerateModelResolver, RollupPicker, TimelineServices};
    use chrono::{TimeZone, Utc};
    use serde_json::{Map, Value, json};
    use solstone_core_generate::{ContentPart, GenerateRequest, GeneratedResponse};
    use solstone_core_timeline::{
        AttemptOutcome, CURRENT_SCHEMA_VERSION, SegmentBindingV1, SegmentSummaryV1,
        SegmentTimelineV1, TimelineKind, load_timeline_state,
    };
    use std::collections::VecDeque;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[test]
    fn segment_timeline_writer_atomically_replaces_existing_entry() {
        // Derived from solstone/apps/timeline/talent/segment_summary.py:70-88, :198-207.
        let journal = tempfile::tempdir().unwrap();
        let segment = journal.path().join("chronicle/20260101/090000_300");
        std::fs::create_dir_all(&segment).unwrap();
        let timeline = segment.join("timeline.json");
        std::fs::write(&timeline, b"{\"title\":\"old\"}\n").unwrap();
        let old_inode = std::fs::metadata(&timeline).unwrap().ino();

        write_segment_timeline(&segment, &json!({"title":"new"})).unwrap();

        assert_eq!(read_json(&timeline), json!({"title":"new"}));
        assert_ne!(std::fs::metadata(&timeline).unwrap().ino(), old_inode);
    }

    struct Model {
        value: &'static str,
        calls: AtomicUsize,
    }

    impl Model {
        const fn new(value: &'static str) -> Self {
            Self {
                value,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl GenerateModelResolver for Model {
        fn resolve(&self, _config: &Map<String, Value>) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.value.to_owned())
        }
    }

    fn services<'a>(picker: &'a Picker, model: &'a Model) -> TimelineServices<'a> {
        static HOST: Host = Host("UTC");
        TimelineServices {
            now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap(),
            host_timezone: &HOST,
            picker,
            model_resolver: model,
        }
    }

    #[test]
    fn invalid_model_selections_fail_and_short_circuit_never_calls_the_model() {
        let picker = Picker::canned([Ok("{\"picks\":[2,2],\"rationale\":\"duplicate\"}")]);
        let events = vec![
            event("Alpha", "first", "a"),
            event("Bravo", "second", "b"),
            event("Charlie", "third", "c"),
        ];
        assert!(matches!(
            pick_one(&picker, &events, 2, "hour"),
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
        assert_eq!(
            rollup_request(&events, 2, "hour")
                .system_instruction
                .unwrap(),
            request.system_instruction.unwrap()
        );

        let entries = vec![
            entry("Alpha", "first", "a", "080000_1"),
            entry("Bravo", "second", "b", "080100_2"),
        ];
        let picker = Picker::canned([Ok("{\"picks\":[3],\"rationale\":\"bad\"}")]);
        assert!(matches!(
            pick_entries(&picker, &entries, 1, "day"),
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
        let short_circuit = pick_entries(&picker, &entries[..1], 1, "day").unwrap();
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
        assert_eq!(model.calls.load(Ordering::SeqCst), 0);
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
    fn master_happy_path_embeds_full_days_and_filters_empty_rollups() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("January", "first", "one")], "extra":"kept"}),
        );
        write_day_rollup(
            journal.path(),
            "20260102",
            json!({"day_top":[event("January Two", "second", "two")]}),
        );
        write_day_rollup(journal.path(), "20260201", json!({"day_top":[]}));
        let picker = Picker::canned([]);
        let model = Model::new("month-model");
        let result = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        let payload = read_json(&journal.path().join("timeline.json"));
        assert_eq!(payload["months"]["202601"]["day_count"], 2);
        assert_eq!(
            payload["months"]["202601"]["days"]["20260101"]["extra"],
            "kept"
        );
        assert_eq!(payload["year_top"][0]["month"], "202601");
        assert!(payload["months"].get("202602").is_none());
    }

    #[test]
    fn master_existing_output_skips_and_force_replaces_it() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("January", "event", "one")]}),
        );
        std::fs::write(journal.path().join("timeline.json"), b"{\"old\":true}\n").unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let skipped = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(skipped.exit_code, 0);
        assert!(skipped.stdout.contains("[skip]"));
        assert_eq!(model.calls.load(Ordering::SeqCst), 0);
        let forced = run(
            "timeline:rollup-master",
            &["--force".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(forced.exit_code, 0);
        assert!(
            read_json(&journal.path().join("timeline.json"))
                .get("months")
                .is_some()
        );
    }

    #[test]
    fn master_empty_wrong_shape_and_no_overlap_keep_their_distinct_exits() {
        let journal = tempfile::tempdir().unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let empty = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(empty.exit_code, 66);
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("A", "b", "c")]}),
        );
        let no_overlap = run(
            "timeline:rollup-master",
            &["--months".to_owned(), "202602".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(no_overlap.exit_code, 0);
        assert!(!journal.path().join("timeline.json").exists());
        std::fs::write(
            journal.path().join("chronicle/20260101/timeline.json"),
            b"[]",
        )
        .unwrap();
        let wrong_shape = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(wrong_shape.exit_code, 1);
        assert!(!journal.path().join("timeline.json").exists());
    }

    #[test]
    fn master_batch_failure_and_per_month_failure_publish_error_records() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("Jan", "x", "jan"), event("Jan Two", "x", "jan-two")]}),
        );
        write_day_rollup(
            journal.path(),
            "20260201",
            json!({"day_top":[event("Feb", "y", "feb"), event("Feb Two", "y", "feb-two")]}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let whole = run(
            "timeline:rollup-master",
            &["--jobs".to_owned(), "0".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(whole.exit_code, 0);
        let payload = read_json(&journal.path().join("timeline.json"));
        assert!(
            payload["months"]["202601"]["month_rationale"]
                .as_str()
                .unwrap()
                .starts_with("ERROR: ")
        );
        assert_eq!(payload["months"]["202602"]["day_count"], 1);
        assert_eq!(payload["year_top"], json!([]));

        std::fs::remove_file(journal.path().join("timeline.json")).unwrap();
        let picker = Picker::error_when("Feb");
        let partial = run(
            "timeline:rollup-master",
            &["--top".to_owned(), "1".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(partial.exit_code, 0);
        let payload = read_json(&journal.path().join("timeline.json"));
        assert!(
            payload["months"]["202602"]["month_rationale"]
                .as_str()
                .unwrap()
                .starts_with("ERROR: ")
        );
        assert_eq!(payload["months"]["202601"]["month_top"][0]["title"], "Jan");
        assert_eq!(payload["year_top"].as_array().unwrap().len(), 1);
    }

    fn event(title: &str, description: &str, origin: &str) -> Value {
        json!({"title": title, "description": description, "origin": origin})
    }

    fn write_segment(journal: &Path, day: &str, stream: &str, segment: &str, value: Value) {
        let path = journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment)
            .join("timeline.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let binding = SegmentBindingV1 {
            day: day.to_owned(),
            stream: stream.to_owned(),
            segment: segment.to_owned(),
        };
        let timeline = SegmentTimelineV1 {
            schema_version: CURRENT_SCHEMA_VERSION,
            kind: TimelineKind::Segment,
            binding: binding.clone(),
            input_digest: "fixture-input".to_owned(),
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
        std::fs::write(path, serde_json::to_vec(&timeline).unwrap()).unwrap();
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

    fn write_day_rollup(journal: &Path, day: &str, value: Value) {
        let path = journal.join("chronicle").join(day).join("timeline.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
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
