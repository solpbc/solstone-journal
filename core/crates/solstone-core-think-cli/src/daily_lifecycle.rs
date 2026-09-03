// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whole-day catchup composition for the unscoped `journal think --day` mode.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_journal_io::{
    HealthMarkerKind, HealthMarkerState, PublishOutcome, publish_daily_marker_if_current,
    read_health_marker,
};
use solstone_core_system::catchup::{
    SegmentRepairOutcome, read_raw_input_fingerprint, record_daily_catchup_progress,
    record_segment_repair_attempt, record_segment_repair_outcome,
};
use solstone_core_system_health::{
    FilesystemHealthLogSource, FilesystemSegmentSource, SegmentIdentity, blocked_segment_keys,
    read_segment_progress, scan_day,
};

use crate::args::ThinkArgs;
use crate::context::ThinkContext;
use crate::daily;
use crate::dispatch::{DEFAULT_THINK_TIMEOUT, ModeResult, merge_mode_result};
use crate::helpers;
use crate::phase_process::{
    NativePhaseProcessRunner, PhaseProcessOutcome, PhaseProcessRunner, journal_command,
};
use crate::run_log::RunLogWriter;
use crate::segment;

#[derive(Clone)]
struct CompletionScope {
    scoped_stream: bool,
    observed_generation: u64,
    observed_fingerprint: String,
}

#[derive(Clone, Default)]
struct SegmentPhaseOutcome {
    result: ModeResult,
    blockers: Option<BTreeSet<SegmentIdentity>>,
}

const REQUIRED_PHASES: [&str; 5] = [
    "sense_batch",
    "segment_repair",
    "daily",
    "indexer",
    "journal_stats",
];
const SKIP_SEGMENT_REPAIR_FAILED: &str = "segment_repair_failed";
const SKIP_READINESS_UNAVAILABLE: &str = "readiness_unavailable";
const SKIP_BLOCKED: &str = "blocked";
const SENSE_PHASE_TIMEOUT: Duration = Duration::from_secs(1_800);
const INDEXER_PHASE_TIMEOUT: Duration = Duration::from_secs(3_600);
const STATS_PHASE_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy)]
struct PhaseBudget {
    timeout: Duration,
    scope: &'static str,
}

impl PhaseBudget {
    const fn aggregate(timeout: Duration) -> Self {
        Self {
            timeout,
            scope: "aggregate",
        }
    }

    const fn per_unit(timeout: Duration, scope: &'static str) -> Self {
        Self { timeout, scope }
    }
}

/// Execute the ordered whole-day catchup lifecycle.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    args: &ThinkArgs,
    default_segment_workers: usize,
    timeout: Option<Duration>,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) -> Result<ModeResult, String> {
    run_with_phase_process(
        context,
        log,
        args,
        default_segment_workers,
        timeout,
        &NativePhaseProcessRunner,
        sense_child_environment,
    )
}

fn run_with_phase_process(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    args: &ThinkArgs,
    default_segment_workers: usize,
    timeout: Option<Duration>,
    phase_process: &dyn PhaseProcessRunner,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) -> Result<ModeResult, String> {
    let observed_generation = stream_generation(context)?;
    let observed_fingerprint = read_raw_input_fingerprint(&context.journal, &context.day)
        .map_err(|error| error.to_string())?;
    // This snapshot describes the whole-day catchup: Sense may resolve an
    // existing blocker before repair begins, and that is still real daily
    // progress.  The repair phase takes its own snapshot after Sense below.
    let lifecycle_blockers = blocked_segments(context)?;
    let segment_workers = usize::try_from(
        args.segment_workers
            .unwrap_or(i64::try_from(default_segment_workers).expect("worker count fits i64")),
    )
    .expect("validated segment workers");
    let skip_talents = args
        .skip_talents
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let force_all_repairs = force_all_repairs(args);
    let mut daily_result = ModeResult::default();
    let segment_outcome = std::cell::RefCell::new(SegmentPhaseOutcome::default());
    let mut total = run_required_phases(
        log,
        context,
        &REQUIRED_PHASES,
        |phase| match phase {
            "sense_batch" => Some(PhaseBudget::aggregate(SENSE_PHASE_TIMEOUT)),
            "segment_repair" => {
                timeout.map(|timeout| PhaseBudget::per_unit(timeout, "per_segment"))
            }
            "daily" => Some(PhaseBudget::per_unit(
                DEFAULT_THINK_TIMEOUT,
                "per_priority_group",
            )),
            "indexer" => Some(PhaseBudget::aggregate(INDEXER_PHASE_TIMEOUT)),
            "journal_stats" => Some(PhaseBudget::aggregate(STATS_PHASE_TIMEOUT)),
            _ => None,
        },
        |phase, phase_log| match phase {
            "sense_batch" => {
                let request = solstone_core_sense::batch::BatchRequest {
                    day: context.day.clone(),
                    jobs: args.jobs,
                    reprocess: None,
                    segment: None,
                    stream: args.stream.clone(),
                    dry_run: false,
                    verbose: args.verbose,
                    debug: args.debug,
                };
                match solstone_core_sense::batch::run_batch_for_whole_day_with_environment_and_timeout(
                    &context.journal,
                    &request,
                    sense_child_environment,
                    Some(SENSE_PHASE_TIMEOUT),
                ) {
                    Ok(()) => succeeded_phase("sense_batch"),
                    Err(solstone_core_sense::batch::BatchError::TimedOut { .. }) => {
                        timed_out_phase("sense_batch", None)
                    }
                    Err(error) => failed_phase("sense_batch", error),
                }
            }
            "segment_repair" => {
                let refresh = force_all_repairs;
                let jobs = args.jobs;
                let repair_timeout = timeout;
                let repair_skip_talents = skip_talents.clone();
                // `run_repair_batch_with_activity` already honours the
                // segment timeout and must finish its durable activity tail
                // before the later daily/index/stats phases may observe it.
                segment_outcome.replace(run_segment_repair_phase(
                    context,
                    phase_log,
                    refresh,
                    jobs,
                    segment_workers,
                    repair_timeout,
                    repair_skip_talents,
                    args.no_activity_prompts,
                    refresh,
                ));
                segment_outcome.borrow().result.clone()
            }
            "daily" => {
                daily_result = match daily::run(
                    context,
                    phase_log,
                    args.stream.as_deref(),
                    args.from_scratch,
                    args.jobs,
                ) {
                    Ok(mut result) => {
                        if let Err(error) = refresh_daily_terminal_fold(context, &mut result) {
                            merge_mode_result(
                                &mut result,
                                failed_phase("daily_completion_input", error),
                            );
                        }
                        result
                    }
                    Err(error) => failed_phase("daily", error),
                };
                daily_result.clone()
            }
            "indexer" => {
                let command = journal_command(&["indexer", "--rescan"]);
                run_process_phase(
                    phase_process,
                    "indexer",
                    command,
                    context,
                    INDEXER_PHASE_TIMEOUT,
                )
            }
            "journal_stats" => {
                let command = journal_command(&["journal-stats"]);
                run_process_phase(
                    phase_process,
                    "journal_stats",
                    command,
                    context,
                    STATS_PHASE_TIMEOUT,
                )
            }
            _ => unreachable!("required phase registry is closed"),
        },
        |phase| {
            if phase == "daily" {
                daily_dependency_skip(&segment_outcome.borrow())
            } else {
                None
            }
        },
    );

    let segment_outcome = segment_outcome.into_inner();
    let no_blockers = BTreeSet::new();
    let post_repair_blocked = segment_outcome.blockers.as_ref().unwrap_or(&no_blockers);
    let lifecycle_progress = segment_outcome
        .blockers
        .as_ref()
        .map(|blockers| blocker_progress(&lifecycle_blockers, blockers));

    maybe_finalize_completion(
        context,
        log,
        CompletionScope {
            scoped_stream: args.stream.is_some(),
            observed_generation,
            observed_fingerprint,
        },
        &daily_result,
        post_repair_blocked,
        lifecycle_progress,
        &mut total,
        |journal, now_ms, fields| helpers::emit(journal, now_ms, "daily_complete", fields),
    );

    Ok(total)
}

fn run_process_phase(
    runner: &dyn PhaseProcessRunner,
    phase: &'static str,
    command: Result<Vec<String>, String>,
    context: &ThinkContext,
    timeout: Duration,
) -> ModeResult {
    let command = match command {
        Ok(command) => command,
        Err(error) => return failed_phase(phase, error),
    };
    match runner.run(phase, command, &context.journal, &context.day, timeout) {
        PhaseProcessOutcome::Exited(0) => succeeded_phase(phase),
        PhaseProcessOutcome::Exited(code) => failed_phase(phase, format!("exit {code}")),
        PhaseProcessOutcome::TimedOut { cleanup_error } => {
            timed_out_phase(phase, cleanup_error.as_deref())
        }
        PhaseProcessOutcome::Failed(error) => failed_phase(phase, error),
    }
}

#[allow(clippy::too_many_arguments)] // Explicit lifecycle inputs keep the production/test seam narrow.
fn run_segment_repair_phase(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    refresh: bool,
    jobs: i64,
    segment_workers: usize,
    timeout: Option<Duration>,
    skip_talents: Vec<String>,
    no_activity_prompts: bool,
    force_all: bool,
) -> SegmentPhaseOutcome {
    // This is intentionally after Sense.  Segment-repair health only claims
    // blockers it saw when repair began; Sense's work belongs to the whole-day
    // progress snapshot captured by `run`.
    let pre_repair_blocked = match blocked_segments(context) {
        Ok(blocked) => blocked,
        Err(error) => {
            return SegmentPhaseOutcome {
                result: failed_phase("segment_repair_scan", error),
                blockers: None,
            };
        }
    };
    let repair_targets = match if force_all {
        all_segments(context)
    } else {
        Ok(pre_repair_blocked.clone())
    } {
        Ok(blocked) => blocked,
        Err(error) => {
            return SegmentPhaseOutcome {
                result: failed_phase("segment_repair_scan", error),
                blockers: None,
            };
        }
    };
    let repair_segments = repair_targets
        .iter()
        .map(|identity| (identity.segment.clone(), identity.stream.clone()))
        .collect::<Vec<_>>();
    let repair_attempted = !repair_segments.is_empty();
    if repair_attempted {
        record_segment_repair_attempt(&context.journal, &context.day, unix_seconds_f64());
    }
    let mut result = match segment::run_repair_batch_with_activity(
        context,
        log,
        repair_segments,
        refresh,
        jobs,
        segment_workers,
        timeout,
        skip_talents,
        no_activity_prompts,
    ) {
        Ok(result) => result,
        Err(error) => failed_phase("segment_repair", error),
    };
    let post_repair_blocked = match blocked_segments(context) {
        Ok(blocked) => Some(blocked),
        Err(error) => {
            merge_mode_result(&mut result, failed_phase("segment_repair_scan", error));
            None
        }
    };
    if let Some(post_repair_blocked) = post_repair_blocked.as_ref() {
        let progress = blocker_progress(&pre_repair_blocked, post_repair_blocked);
        if repair_attempted {
            let (cleared, remaining) = progress;
            record_segment_repair_outcome(
                &context.journal,
                &context.day,
                SegmentRepairOutcome {
                    success: result.failed == 0 && remaining == 0,
                    timed_out: result.timed_out,
                    timeout_seconds: result
                        .timed_out
                        .then(|| timeout.map(|duration| duration.as_secs_f64()))
                        .flatten(),
                    ended_at: unix_seconds_f64(),
                    cleared: Some(cleared),
                    remaining: Some(remaining),
                },
            );
        }
    }
    SegmentPhaseOutcome {
        result,
        blockers: post_repair_blocked,
    }
}

/// Measure a phase against the exact blocker set it owned at admission.
fn blocker_progress(
    baseline: &BTreeSet<SegmentIdentity>,
    remaining: &BTreeSet<SegmentIdentity>,
) -> (usize, usize) {
    (baseline.difference(remaining).count(), remaining.len())
}

fn daily_dependency_skip(repair: &SegmentPhaseOutcome) -> Option<&'static str> {
    if repair.blockers.is_none() {
        return Some(SKIP_READINESS_UNAVAILABLE);
    }
    if repair.result.failed != 0 || repair.result.timed_out {
        return Some(SKIP_SEGMENT_REPAIR_FAILED);
    }
    if repair
        .blockers
        .as_ref()
        .is_some_and(|blockers| !blockers.is_empty())
    {
        return Some(SKIP_BLOCKED);
    }
    None
}

fn skipped_phase(phase: &str, reason: &'static str) -> ModeResult {
    ModeResult {
        skipped: 1,
        skipped_names: vec![format!("{phase} ({reason})")],
        ..ModeResult::default()
    }
}

fn run_required_phases(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    phases: &[&str],
    phase_budget: impl Fn(&str) -> Option<PhaseBudget>,
    mut execute: impl FnMut(&str, &mut RunLogWriter) -> ModeResult,
    mut skip: impl FnMut(&str) -> Option<&'static str>,
) -> ModeResult {
    let mut total = ModeResult::default();
    for phase in phases {
        let started = Instant::now();
        log_phase_start(log, context, phase);
        let result = if let Some(reason) = skip(phase) {
            skipped_phase(phase, reason)
        } else {
            execute(phase, log)
        };
        log_phase_complete(
            log,
            context,
            phase,
            started.elapsed(),
            phase_budget(phase),
            &result,
        );
        merge_mode_result(&mut total, result);
    }
    total
}

fn stream_generation(context: &ThinkContext) -> Result<u64, String> {
    match read_health_marker(&context.journal, &context.day, HealthMarkerKind::Stream)
        .map_err(|error| error.to_string())?
    {
        HealthMarkerState::Versioned { marker, .. } => Ok(marker.generation),
        HealthMarkerState::Absent | HealthMarkerState::LegacyEmpty { .. } => Ok(0),
        HealthMarkerState::MalformedNonEmpty { .. } => {
            Err("stream health marker is malformed".to_owned())
        }
    }
}

fn force_all_repairs(args: &ThinkArgs) -> bool {
    args.refresh || args.from_scratch
}

fn blocked_segments(context: &ThinkContext) -> Result<BTreeSet<SegmentIdentity>, String> {
    let source = FilesystemSegmentSource;
    let (_, _, segments) = scan_day(&source, &context.journal, &context.day, Utc::now())
        .map_err(|error| error.to_string())?;
    let inputs = segments.into_iter().map(Into::into).collect::<Vec<_>>();
    let health = FilesystemHealthLogSource::new(&context.journal);
    let progress =
        read_segment_progress(&health, &context.day).map_err(|error| error.to_string())?;
    Ok(blocked_segment_keys(&inputs, &progress.value))
}

fn all_segments(context: &ThinkContext) -> Result<BTreeSet<SegmentIdentity>, String> {
    let source = FilesystemSegmentSource;
    let (_, _, segments) = scan_day(&source, &context.journal, &context.day, Utc::now())
        .map_err(|error| error.to_string())?;
    Ok(segments
        .into_iter()
        .map(|segment| SegmentIdentity {
            stream: Some(segment.stream),
            segment: segment.key,
        })
        .collect())
}

fn refresh_daily_terminal_fold(
    context: &ThinkContext,
    result: &mut ModeResult,
) -> Result<(), String> {
    let source = FilesystemHealthLogSource::new(&context.journal);
    let completed = solstone_core_system_health::read_completed_units(&source, &context.day)
        .map_err(|error| error.to_string())?;
    let deterministic =
        solstone_core_system_health::read_daily_deterministic_failures(&source, &context.day)
            .map_err(|error| error.to_string())?;
    if completed.malformed_line_count != 0 || deterministic.malformed_line_count != 0 {
        return Err(format!(
            "daily terminal health records contain malformed lines (completed={}, deterministic={})",
            completed.malformed_line_count, deterministic.malformed_line_count
        ));
    }
    let completed = completed.value;
    let deterministic = deterministic.value;
    result.terminal_units.clear();
    result.capped_units.clear();
    for unit in &result.applicable_units {
        let completed_key = solstone_core_system_health::CompletedUnit {
            mode: "daily".to_owned(),
            name: unit.0.clone(),
            facet: unit.1.clone(),
        };
        if completed.contains(&completed_key) {
            result.terminal_units.insert(unit.clone());
            continue;
        }
        let failure_key = solstone_core_system_health::DailyUnit {
            name: unit.0.clone(),
            facet: unit.1.clone(),
        };
        if deterministic.get(&failure_key).is_some_and(|failure| {
            solstone_core_system_health::daily_failure_capped(&failure.reason_code, failure.count)
        }) {
            result.terminal_units.insert(unit.clone());
            result.capped_units.insert(unit.clone());
        }
    }
    Ok(())
}

fn succeeded_phase(phase: &str) -> ModeResult {
    ModeResult {
        success: 1,
        success_names: vec![phase.to_owned()],
        ..ModeResult::default()
    }
}

fn failed_phase(phase: &str, error: impl std::fmt::Display) -> ModeResult {
    ModeResult {
        failed: 1,
        failed_names: vec![format!("{phase} ({error})")],
        ..ModeResult::default()
    }
}

fn timed_out_phase(phase: &str, cleanup_error: Option<&str>) -> ModeResult {
    ModeResult {
        failed: 1,
        timed_out: true,
        failed_names: vec![cleanup_error.map_or_else(
            || format!("{phase} (wall_clock_exceeded)"),
            |error| format!("{phase} (wall_clock_exceeded; cleanup failed: {error})"),
        )],
        ..ModeResult::default()
    }
}

#[allow(clippy::too_many_arguments)] // Completion folding keeps its admitted state and emit seam explicit.
fn maybe_finalize_completion(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    scope: CompletionScope,
    daily: &ModeResult,
    segment_blockers: &BTreeSet<SegmentIdentity>,
    progress: Option<(usize, usize)>,
    total: &mut ModeResult,
    emit: impl FnOnce(&std::path::Path, i64, Map<String, Value>) -> bool,
) {
    if scope.scoped_stream {
        log_completion_fold(log, context, false, true, daily, segment_blockers);
        return;
    }
    let completion_candidate = total.failed == 0
        && daily.applicable_units.is_subset(&daily.terminal_units)
        && segment_blockers.is_empty();
    if let Some((cleared, remaining)) = progress {
        record_daily_catchup_progress(&context.journal, &context.day, cleared, remaining);
    }
    if !completion_candidate {
        if total.failed == 0 {
            let nonterminal_daily = daily
                .applicable_units
                .difference(&daily.terminal_units)
                .count();
            merge_mode_result(
                total,
                failed_phase(
                    "daily_incomplete",
                    format!(
                        "{nonterminal_daily} daily unit(s) non-terminal; {} segment blocker(s) remain",
                        segment_blockers.len()
                    ),
                ),
            );
        }
        log_completion_fold(log, context, false, false, daily, segment_blockers);
        return;
    }
    let capped_units = match capped_unit_payload(context, daily) {
        Ok(units) => units,
        Err(error) => {
            merge_mode_result(total, failed_phase("daily_completion_input", error));
            log_completion_fold(log, context, false, false, daily, segment_blockers);
            return;
        }
    };
    match publish_daily_marker_if_current(
        &context.journal,
        &context.day,
        scope.observed_generation,
        &scope.observed_fingerprint,
        || {
            read_raw_input_fingerprint(&context.journal, &context.day)
                .map_err(|error| error.to_string())
        },
    ) {
        Ok(PublishOutcome::Published(generation)) => {
            log_completion_fold(log, context, true, false, daily, segment_blockers);
            let mut fields = Map::from_iter([
                ("day".to_owned(), Value::String(context.day.clone())),
                ("generation".to_owned(), Value::from(generation)),
                ("success".to_owned(), Value::from(daily.success)),
                ("failed".to_owned(), Value::from(daily.failed)),
            ]);
            if let Some((cleared, remaining)) = progress {
                fields.insert("cleared".to_owned(), Value::from(cleared));
                fields.insert("remaining".to_owned(), Value::from(remaining));
            }
            if !capped_units.is_empty() {
                fields.insert("capped_daily_units".to_owned(), Value::Array(capped_units));
            }
            let _ = emit(&context.journal, context.now_ms, fields);
        }
        Ok(PublishOutcome::AlreadyCurrent(_) | PublishOutcome::Superseded(_)) => {
            log_completion_fold(log, context, false, false, daily, segment_blockers);
        }
        Ok(PublishOutcome::InputChanged(_)) => {
            merge_mode_result(
                total,
                failed_phase(
                    "daily_input_changed",
                    "raw input changed during whole-day processing",
                ),
            );
            log_completion_fold(log, context, false, false, daily, segment_blockers);
        }
        Err(error) => {
            merge_mode_result(total, failed_phase("daily_marker", error));
            log_completion_fold(log, context, false, false, daily, segment_blockers);
        }
    }
}

fn capped_unit_payload(context: &ThinkContext, daily: &ModeResult) -> Result<Vec<Value>, String> {
    if daily.capped_units.is_empty() {
        return Ok(Vec::new());
    }
    let failures = solstone_core_system_health::read_daily_deterministic_failures(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .map_err(|error| error.to_string())?
    .value;
    Ok(daily
        .capped_units
        .iter()
        .filter_map(|(name, facet)| {
            let key = solstone_core_system_health::DailyUnit {
                name: name.clone(),
                facet: facet.clone(),
            };
            failures.get(&key).map(|failure| {
                Value::Object(Map::from_iter([
                    ("name".to_owned(), Value::String(name.clone())),
                    (
                        "facet".to_owned(),
                        facet.clone().map_or(Value::Null, Value::String),
                    ),
                    (
                        "reason_code".to_owned(),
                        Value::String(failure.reason_code.clone()),
                    ),
                    ("count".to_owned(), Value::from(failure.count)),
                ]))
            })
        })
        .collect())
}

fn log_completion_fold(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    complete: bool,
    scoped_stream: bool,
    daily: &ModeResult,
    segment_blockers: &BTreeSet<SegmentIdentity>,
) {
    let capped_units = daily
        .capped_units
        .iter()
        .map(|(name, facet)| crate::dispatch::item_label(name, facet.as_deref()))
        .collect::<Vec<_>>();
    log.log(
        "daily.completion",
        context.now_ms,
        Map::from_iter([
            ("mode".to_owned(), Value::String("daily".to_owned())),
            ("day".to_owned(), Value::String(context.day.clone())),
            ("complete".to_owned(), Value::Bool(complete)),
            ("scoped_stream".to_owned(), Value::Bool(scoped_stream)),
            (
                "applicable_units".to_owned(),
                Value::from(daily.applicable_units.len()),
            ),
            (
                "terminal_units".to_owned(),
                Value::from(daily.terminal_units.len()),
            ),
            ("capped_units".to_owned(), Value::from(capped_units)),
            (
                "segment_blockers".to_owned(),
                Value::from(segment_blockers.len()),
            ),
        ]),
    );
}

fn log_phase_start(log: &mut RunLogWriter, context: &ThinkContext, phase: &str) {
    log.log(
        "phase.start",
        context.now_ms,
        Map::from_iter([
            ("mode".to_owned(), Value::String("daily".to_owned())),
            ("day".to_owned(), Value::String(context.day.clone())),
            ("phase".to_owned(), Value::String(phase.to_owned())),
        ]),
    );
}

fn skip_reason_code(result: &ModeResult) -> Option<&str> {
    result.skipped_names.first().and_then(|name| {
        name.rsplit_once(" (")
            .and_then(|(_, rest)| rest.strip_suffix(')'))
    })
}

fn log_phase_complete(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    phase: &str,
    duration: Duration,
    budget: Option<PhaseBudget>,
    result: &ModeResult,
) {
    let skipped = result.skipped != 0;
    let success = result.failed == 0 && !result.timed_out && !skipped;
    let mut fields = Map::from_iter([
        ("mode".to_owned(), Value::String("daily".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("phase".to_owned(), Value::String(phase.to_owned())),
        ("success".to_owned(), Value::Bool(success)),
        (
            "duration_ms".to_owned(),
            Value::from(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
        ),
    ]);
    if skipped {
        fields.insert("skipped".to_owned(), Value::Bool(true));
    }
    if let Some(budget) = budget {
        fields.insert("bounded".to_owned(), Value::Bool(true));
        fields.insert(
            "timeout_seconds".to_owned(),
            Value::from(budget.timeout.as_secs_f64()),
        );
        fields.insert(
            "timeout_scope".to_owned(),
            Value::String(budget.scope.to_owned()),
        );
    } else {
        fields.insert("bounded".to_owned(), Value::Bool(false));
    }
    if result.timed_out {
        fields.insert(
            "reason_code".to_owned(),
            Value::String("wall_clock_exceeded".to_owned()),
        );
    } else if skipped {
        if let Some(reason) = skip_reason_code(result) {
            fields.insert("reason_code".to_owned(), Value::String(reason.to_owned()));
        }
    } else if result.failed != 0 {
        fields.insert("reason_code".to_owned(), Value::String("failed".to_owned()));
    }
    log.log("phase.complete", context.now_ms, fields);
}

fn unix_seconds_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, VecDeque};
    use std::fs;
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    use std::sync::Arc;
    use std::sync::Mutex;

    use solstone_core_cortex_client::{
        CortexRequest, UseCompletion, UseEndState, WaitForUsesReport,
    };
    use solstone_core_journal_io::{
        HealthMarkerKind, HealthMarkerState, JournalRoot, bump_stream_marker,
        operational_log::catalog_oplogs, read_health_marker,
    };
    use tempfile::tempdir;

    use super::*;

    const DAY: &str = "20260813";

    struct RecordingPhaseProcessRunner {
        outcomes: Mutex<VecDeque<PhaseProcessOutcome>>,
        calls: Mutex<Vec<(&'static str, Vec<String>)>>,
    }

    impl RecordingPhaseProcessRunner {
        fn new(outcomes: impl IntoIterator<Item = PhaseProcessOutcome>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl PhaseProcessRunner for RecordingPhaseProcessRunner {
        fn run(
            &self,
            phase: &'static str,
            command: Vec<String>,
            _journal: &std::path::Path,
            _day: &str,
            _timeout: Duration,
        ) -> PhaseProcessOutcome {
            self.calls.lock().unwrap().push((phase, command));
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("one injected outcome per phase process")
        }
    }

    struct TerminalCortex {
        end_state: UseEndState,
    }

    impl crate::context::CortexBoundary for TerminalCortex {
        fn dispatch(
            &self,
            _runtime: &tokio::runtime::Runtime,
            request: &CortexRequest,
        ) -> Result<String, crate::context::DispatchFailure> {
            Ok(format!("use-{}", request.name))
        }

        fn wait(
            &self,
            _runtime: &tokio::runtime::Runtime,
            use_ids: &[String],
            _deadline: Option<Duration>,
        ) -> Result<WaitForUsesReport, String> {
            Ok(WaitForUsesReport {
                completed: use_ids
                    .iter()
                    .map(|use_id| {
                        (
                            use_id.clone(),
                            UseCompletion {
                                end_state: self.end_state,
                                finish_fields: Default::default(),
                            },
                        )
                    })
                    .collect(),
                timed_out: Vec::new(),
            })
        }
    }

    fn context(journal: &std::path::Path) -> ThinkContext {
        let day_dir = crate::day::create_day(journal, DAY).unwrap();
        let roots = tempdir().unwrap();
        let talent_root = roots.keep().join("talent");
        let apps_root = talent_root.parent().unwrap().join("apps");
        fs::create_dir_all(&talent_root).unwrap();
        fs::create_dir_all(&apps_root).unwrap();
        ThinkContext::new(journal, DAY.to_owned(), day_dir, 1_785_000_000_000)
            .unwrap()
            .with_talent_roots(talent_root, apps_root)
    }

    fn log(journal: &std::path::Path) -> RunLogWriter {
        RunLogWriter::open(journal, DAY, "daily")
    }

    fn health_log(journal: &std::path::Path) -> RunLogWriter {
        RunLogWriter::open(journal, DAY, "daily")
    }

    fn complete_daily() -> ModeResult {
        ModeResult::default()
    }

    fn completion_scope(journal: &std::path::Path, generation: u64) -> CompletionScope {
        CompletionScope {
            scoped_stream: false,
            observed_generation: generation,
            observed_fingerprint: read_raw_input_fingerprint(journal, DAY).unwrap(),
        }
    }

    fn marker_generation(journal: &std::path::Path) -> Option<u64> {
        match read_health_marker(journal, DAY, HealthMarkerKind::Daily).unwrap() {
            HealthMarkerState::Versioned { marker, .. } => Some(marker.generation),
            _ => None,
        }
    }

    fn terminal_context(journal: &std::path::Path, end_state: UseEndState) -> ThinkContext {
        let context = context(journal);
        fs::write(
            context.talent_root.join("fresh.md"),
            "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
        )
        .unwrap();
        context.with_boundary(Arc::new(TerminalCortex { end_state }))
    }

    fn blocker(key: &str) -> SegmentIdentity {
        SegmentIdentity {
            stream: Some("default".to_owned()),
            segment: key.to_owned(),
        }
    }

    #[test]
    fn repair_and_whole_day_progress_have_distinct_admission_baselines() {
        // `old` existed before Sense and Sense clears it. `new` is introduced
        // by Sense and repair clears it. The counts must be attributed to the
        // phase that actually owned the blocker at its admission point.
        let before_sense = BTreeSet::from([blocker("old")]);
        let before_repair = BTreeSet::from([blocker("new")]);
        let after_repair = BTreeSet::new();

        assert_eq!(blocker_progress(&before_sense, &after_repair), (1, 0));
        assert_eq!(blocker_progress(&before_repair, &after_repair), (1, 0));
        assert!(!before_repair.contains(&blocker("old")));
        assert!(before_repair.contains(&blocker("new")));
    }

    #[test]
    fn repair_progress_does_not_credit_a_blocker_sense_already_cleared() {
        let before_sense = BTreeSet::from([blocker("sense-cleared"), blocker("repair-cleared")]);
        let before_repair = BTreeSet::from([blocker("repair-cleared")]);
        let after_repair = BTreeSet::new();

        assert_eq!(blocker_progress(&before_sense, &after_repair), (2, 0));
        assert_eq!(blocker_progress(&before_repair, &after_repair), (1, 0));
    }

    #[test]
    fn phase_order_is_logged_for_an_empty_fixture_day() {
        let journal = tempdir().unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let result = run(
            &context,
            &mut log,
            &ThinkArgs::default(),
            1,
            Some(Duration::from_secs(610)),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(result.failed, 0);
        let rows = lifecycle_rows(journal.path(), "phase.start")
            .iter()
            .map(|row| row["phase"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            rows,
            [
                "sense_batch",
                "segment_repair",
                "daily",
                "indexer",
                "journal_stats"
            ]
        );
        let completions = lifecycle_rows(journal.path(), "phase.complete");
        assert_eq!(completions[0]["timeout_scope"], "aggregate");
        assert_eq!(completions[1]["timeout_scope"], "per_segment");
        assert_eq!(completions[2]["timeout_scope"], "per_priority_group");
        assert_eq!(completions[3]["timeout_scope"], "aggregate");
        assert_eq!(completions[4]["timeout_scope"], "aggregate");
    }

    #[test]
    fn newly_dispatched_daily_unit_is_durably_folded_before_publication() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = terminal_context(journal.path(), UseEndState::Finish);
        let mut log = health_log(journal.path());

        let result = run(
            &context,
            &mut log,
            &ThinkArgs::default(),
            1,
            Some(Duration::from_secs(610)),
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(result.failed, 0);
        assert_eq!(marker_generation(journal.path()), Some(1));
        let completed = solstone_core_system_health::read_completed_units(
            &FilesystemHealthLogSource::new(journal.path()),
            DAY,
        )
        .unwrap();
        assert_eq!(completed.malformed_line_count, 0);
        assert!(
            completed
                .value
                .contains(&solstone_core_system_health::CompletedUnit {
                    mode: "daily".to_owned(),
                    name: "fresh".to_owned(),
                    facet: None,
                })
        );
    }

    #[test]
    fn newly_dispatched_daily_failure_is_durable_and_withholds_publication() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = terminal_context(journal.path(), UseEndState::Error);
        let mut log = health_log(journal.path());

        let result = run(
            &context,
            &mut log,
            &ThinkArgs::default(),
            1,
            Some(Duration::from_secs(610)),
            &BTreeMap::new(),
        )
        .unwrap();

        assert!(result.failed > 0);
        assert_eq!(marker_generation(journal.path()), None);
        let rows = lifecycle_rows(journal.path(), "talent.fail");
        assert!(rows.iter().any(|row| {
            row["event"] == "talent.fail"
                && row["mode"] == "daily"
                && row["name"] == "fresh"
                && row["use_id"] == "use-fresh"
                && row["state"] == "error"
        }));
    }

    #[test]
    fn malformed_daily_terminal_fold_withholds_publication() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let malformed = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join("health/malformed.jsonl");
        fs::create_dir_all(malformed.parent().unwrap()).unwrap();
        fs::write(malformed, b"{not json}\n").unwrap();
        let mut log = log(journal.path());

        let result = run(
            &context,
            &mut log,
            &ThinkArgs::default(),
            1,
            Some(Duration::from_secs(610)),
            &BTreeMap::new(),
        )
        .unwrap();

        assert!(result.failed > 0);
        assert_eq!(marker_generation(journal.path()), None);
    }

    #[test]
    fn successful_fold_publishes_observed_generation_and_emits_once() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut total = succeeded_phase("all");
        let mut events = 0;
        let mut emitted = None;
        maybe_finalize_completion(
            &context,
            &mut log,
            completion_scope(journal.path(), 1),
            &complete_daily(),
            &BTreeSet::new(),
            Some((2, 0)),
            &mut total,
            |_, _, fields| {
                events += 1;
                emitted = Some(fields);
                true
            },
        );
        assert_eq!(marker_generation(journal.path()), Some(1));
        assert_eq!(events, 1);
        let completion = lifecycle_rows(journal.path(), "daily.completion")
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(completion["complete"], true);
        let emitted = emitted.expect("completion payload");
        assert_eq!(emitted["day"], DAY);
        assert_eq!(emitted["generation"], 1);
        assert_eq!(emitted["success"], 0);
        assert_eq!(emitted["failed"], 0);
        assert_eq!(emitted["cleared"], 2);
        assert_eq!(emitted["remaining"], 0);
    }

    #[test]
    fn failed_phase_withholds_marker_and_event() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut total = failed_phase("indexer", "failed");
        let mut events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            completion_scope(journal.path(), 1),
            &complete_daily(),
            &BTreeSet::new(),
            None,
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );
        assert_eq!(marker_generation(journal.path()), None);
        assert_eq!(events, 0);
        assert_eq!(
            total.failed, 1,
            "existing phase failure is not double-counted"
        );
        assert_eq!(total.failed_names, ["indexer (failed)"]);
    }

    #[test]
    fn remaining_segment_blocker_withholds_marker_and_event() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut total = succeeded_phase("all");
        let blockers = BTreeSet::from([SegmentIdentity {
            stream: Some("default".to_owned()),
            segment: "090000_300".to_owned(),
        }]);
        let mut events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            completion_scope(journal.path(), 1),
            &complete_daily(),
            &blockers,
            Some((0, 1)),
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );
        assert_eq!(marker_generation(journal.path()), None);
        assert_eq!(events, 0);
        assert_eq!(total.failed, 1);
        assert!(total.failed_names[0].starts_with("daily_incomplete ("));
        assert_eq!(crate::mode_outcome(total).exit_code, 1);
    }

    #[test]
    fn nonterminal_daily_unit_withholds_completion_and_returns_failure() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut daily = ModeResult::default();
        daily.applicable_units.insert(("pending".to_owned(), None));
        let mut total = succeeded_phase("all");
        let mut events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            completion_scope(journal.path(), 1),
            &daily,
            &BTreeSet::new(),
            None,
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );

        assert_eq!(marker_generation(journal.path()), None);
        assert_eq!(events, 0);
        assert_eq!(total.failed, 1);
        assert!(total.failed_names[0].starts_with("daily_incomplete ("));
        assert_eq!(crate::mode_outcome(total).exit_code, 1);
    }

    #[test]
    fn scoped_stream_suppresses_marker_publication_and_event() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut total = succeeded_phase("all");
        let mut events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            CompletionScope {
                scoped_stream: true,
                observed_generation: 1,
                observed_fingerprint: read_raw_input_fingerprint(journal.path(), DAY).unwrap(),
            },
            &complete_daily(),
            &BTreeSet::new(),
            None,
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );
        assert_eq!(marker_generation(journal.path()), None);
        assert_eq!(events, 0);
    }

    #[test]
    fn scoped_stream_default_lifecycle_never_publishes_daily_marker() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let args = ThinkArgs {
            stream: Some("default".to_owned()),
            ..ThinkArgs::default()
        };
        let result = run(
            &context,
            &mut log,
            &args,
            1,
            Some(Duration::from_secs(610)),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(result.failed, 0);
        assert_eq!(marker_generation(journal.path()), None);
    }

    #[test]
    fn already_current_marker_does_not_republish_or_reemit() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut first = succeeded_phase("all");
        let mut first_events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            completion_scope(journal.path(), 1),
            &complete_daily(),
            &BTreeSet::new(),
            None,
            &mut first,
            |_, _, _| {
                first_events += 1;
                true
            },
        );
        let mut second = succeeded_phase("all");
        let mut second_events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            completion_scope(journal.path(), 1),
            &complete_daily(),
            &BTreeSet::new(),
            None,
            &mut second,
            |_, _, _| {
                second_events += 1;
                true
            },
        );
        assert_eq!(marker_generation(journal.path()), Some(1));
        assert_eq!(first_events, 1);
        assert_eq!(second_events, 0);
    }

    #[test]
    fn changed_raw_input_advances_dirty_generation_and_withholds_publication() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let scope = completion_scope(journal.path(), 1);
        let segment = journal.path().join("chronicle").join(DAY).join("090000_60");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("audio.json"), b"late raw input").unwrap();
        let mut log = log(journal.path());
        let mut total = succeeded_phase("all");
        let mut events = 0;

        maybe_finalize_completion(
            &context,
            &mut log,
            scope,
            &complete_daily(),
            &BTreeSet::new(),
            None,
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );

        assert_eq!(marker_generation(journal.path()), None);
        assert_eq!(events, 0);
        assert_eq!(total.failed, 1);
        let completion = lifecycle_rows(journal.path(), "daily.completion")
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(completion["complete"], false);
        assert!(matches!(
            read_health_marker(journal.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned {
                marker: solstone_core_journal_io::HealthMarker { generation: 2, .. },
                ..
            }
        ));
    }

    #[test]
    fn raw_mutation_before_admission_invalidates_an_old_same_generation_daily_marker() {
        let journal = tempdir().unwrap();
        let context = context(journal.path());
        let segment = journal.path().join("chronicle").join(DAY).join("090000_60");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("audio.json"), b"before").unwrap();
        assert_eq!(bump_stream_marker(journal.path(), DAY).unwrap(), 1);
        let old_fingerprint = read_raw_input_fingerprint(journal.path(), DAY).unwrap();
        assert_eq!(
            publish_daily_marker_if_current(journal.path(), DAY, 1, &old_fingerprint, || Ok(
                old_fingerprint.clone()
            ),)
            .unwrap(),
            PublishOutcome::Published(1)
        );

        // Model a dirty writer crash after durable raw mutation but before its
        // required stream-generation bump. Admission sees the new raw bytes at
        // the old generation, while daily.updated still proves only the old
        // fingerprint.
        fs::write(segment.join("audio.json"), b"after").unwrap();
        let scope = completion_scope(journal.path(), 1);
        let mut log = log(journal.path());
        let mut total = succeeded_phase("all");
        let mut events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            scope,
            &complete_daily(),
            &BTreeSet::new(),
            None,
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );

        assert_eq!(events, 0);
        assert_eq!(total.failed, 1);
        assert!(matches!(
            read_health_marker(journal.path(), DAY, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned {
                marker: solstone_core_journal_io::HealthMarker { generation: 2, .. },
                ..
            }
        ));
        assert_eq!(
            solstone_core_journal_io::day_marker_pair_status(journal.path(), DAY).unwrap(),
            solstone_core_journal_io::DayMarkerPairStatus::Dirty
        );
    }

    #[test]
    fn sense_media_projection_does_not_withhold_same_generation_completion() {
        let journal = tempdir().unwrap();
        let context = context(journal.path());
        let segment = journal.path().join("chronicle").join(DAY).join("090000_60");
        fs::create_dir_all(&segment).unwrap();
        let raw = b"raw audio bytes";
        fs::write(segment.join("audio.wav"), raw).unwrap();
        assert_eq!(bump_stream_marker(journal.path(), DAY).unwrap(), 1);
        let scope = completion_scope(journal.path(), 1);

        fs::write(
            segment.join("audio.jsonl"),
            format!(
                "{{\"raw\":\"audio.wav\",\"_solstone_processing\":{{\"schema\":\"solstone.processing.v1\",\"state\":\"analyzed\",\"handler\":\"transcribe\",\"input_size\":{}}}}}\n{{\"start\":\"09:00:00\",\"text\":\"derived\"}}\n",
                raw.len()
            ),
        )
        .unwrap();

        let mut log = log(journal.path());
        let mut total = succeeded_phase("all");
        let mut events = 0;
        maybe_finalize_completion(
            &context,
            &mut log,
            scope,
            &complete_daily(),
            &BTreeSet::new(),
            None,
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );

        assert_eq!(marker_generation(journal.path()), Some(1));
        assert_eq!(events, 1);
        assert_eq!(total.failed, 0);
    }

    fn lifecycle_rows(journal: &std::path::Path, event: &str) -> Vec<Value> {
        let day = chrono::NaiveDate::parse_from_str(DAY, "%Y%m%d").unwrap();
        catalog_oplogs(JournalRoot::open(journal).unwrap(), &[day])
            .unwrap()
            .into_catalogued_entries()
            .into_iter()
            .filter(|(entry, _)| {
                entry.name().source().display_slug() == "think"
                    && entry.name().run().display_slug() == "daily"
            })
            .flat_map(|(entry, mut file)| {
                file.seek(SeekFrom::Start(entry.payload_offset() as u64))
                    .unwrap();
                BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .filter_map(|line| serde_json::from_str(&line).ok())
                    .collect::<Vec<Value>>()
            })
            .filter(|row| row["event"] == event)
            .collect()
    }

    fn policy_run(
        context: &ThinkContext,
        log: &mut RunLogWriter,
        repair: SegmentPhaseOutcome,
        mut execute: impl FnMut(&str) -> ModeResult,
    ) -> (ModeResult, Vec<String>) {
        let repair = std::cell::RefCell::new(repair);
        let invoked = std::cell::RefCell::new(Vec::new());
        let total = run_required_phases(
            log,
            context,
            &REQUIRED_PHASES,
            |_| None,
            |phase, _| {
                invoked.borrow_mut().push(phase.to_owned());
                if phase == "segment_repair" {
                    return repair.borrow().result.clone();
                }
                execute(phase)
            },
            |phase| {
                if phase == "daily" {
                    daily_dependency_skip(&repair.borrow())
                } else {
                    None
                }
            },
        );
        (total, invoked.into_inner())
    }

    fn succeeded_repair() -> SegmentPhaseOutcome {
        SegmentPhaseOutcome {
            result: succeeded_phase("segment_repair"),
            blockers: Some(BTreeSet::new()),
        }
    }

    fn blocked_repair() -> SegmentPhaseOutcome {
        SegmentPhaseOutcome {
            result: succeeded_phase("segment_repair"),
            blockers: Some(BTreeSet::from([blocker("090000_300")])),
        }
    }

    fn failed_repair(timed_out: bool) -> SegmentPhaseOutcome {
        SegmentPhaseOutcome {
            result: if timed_out {
                timed_out_phase("segment_repair", None)
            } else {
                failed_phase("segment_repair", "injected")
            },
            blockers: Some(BTreeSet::new()),
        }
    }

    fn unread_repair(detail: &str) -> SegmentPhaseOutcome {
        SegmentPhaseOutcome {
            result: failed_phase("segment_repair_scan", detail),
            blockers: None,
        }
    }

    #[test]
    fn daily_dependency_skip_reasons_are_stable() {
        assert_eq!(daily_dependency_skip(&succeeded_repair()), None);
        assert_eq!(daily_dependency_skip(&blocked_repair()), Some(SKIP_BLOCKED));
        assert_eq!(
            daily_dependency_skip(&failed_repair(false)),
            Some(SKIP_SEGMENT_REPAIR_FAILED)
        );
        assert_eq!(
            daily_dependency_skip(&failed_repair(true)),
            Some(SKIP_SEGMENT_REPAIR_FAILED)
        );
        assert_eq!(
            daily_dependency_skip(&unread_repair("missing")),
            Some(SKIP_READINESS_UNAVAILABLE)
        );
        assert_eq!(
            daily_dependency_skip(&unread_repair("malformed")),
            Some(SKIP_READINESS_UNAVAILABLE)
        );
        assert_eq!(
            daily_dependency_skip(&unread_repair("unreadable")),
            Some(SKIP_READINESS_UNAVAILABLE)
        );
    }

    #[test]
    fn all_green_zero_blockers_runs_every_phase_in_order() {
        let journal = tempdir().unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut daily_calls = 0;
        let (total, invoked) = policy_run(&context, &mut log, succeeded_repair(), |phase| {
            if phase == "daily" {
                daily_calls += 1;
            }
            succeeded_phase(phase)
        });
        assert_eq!(invoked, REQUIRED_PHASES);
        assert_eq!(daily_calls, 1);
        assert_eq!(total.failed, 0);
        assert_eq!(total.skipped, 0);
        assert_eq!(
            total.success_names,
            [
                "sense_batch",
                "segment_repair",
                "daily",
                "indexer",
                "journal_stats"
            ]
        );
        let starts = lifecycle_rows(journal.path(), "phase.start");
        let completions = lifecycle_rows(journal.path(), "phase.complete");
        assert_eq!(starts.len(), 5);
        assert_eq!(completions.len(), 5);
        assert!(completions.iter().all(|row| row["success"] == true));
        assert!(completions.iter().all(|row| row.get("skipped").is_none()));
    }

    #[test]
    fn sense_failure_or_timeout_still_runs_repair_and_skips_daily_when_blocked() {
        for sense in [
            failed_phase("sense_batch", "injected"),
            timed_out_phase("sense_batch", None),
        ] {
            let journal = tempdir().unwrap();
            bump_stream_marker(journal.path(), DAY).unwrap();
            let context = context(journal.path());
            let mut log = log(journal.path());
            let mut daily_calls = 0;
            let sense_result = sense.clone();
            let (mut total, invoked) = policy_run(&context, &mut log, blocked_repair(), |phase| {
                if phase == "daily" {
                    daily_calls += 1;
                }
                if phase == "sense_batch" {
                    return sense_result.clone();
                }
                succeeded_phase(phase)
            });
            assert_eq!(
                invoked,
                ["sense_batch", "segment_repair", "indexer", "journal_stats"]
            );
            assert_eq!(daily_calls, 0);
            assert_eq!(total.skipped_names, ["daily (blocked)"]);
            assert!(total.failed > 0);
            assert!(!total.success_names.iter().any(|name| name == "daily"));
            maybe_finalize_completion(
                &context,
                &mut log,
                completion_scope(journal.path(), 1),
                &complete_daily(),
                &BTreeSet::from([blocker("090000_300")]),
                Some((0, 1)),
                &mut total,
                |_, _, _| true,
            );
            assert_eq!(marker_generation(journal.path()), None);
        }
    }

    #[test]
    fn sense_failure_with_clear_blockers_still_runs_daily_once() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let mut daily_calls = 0;
        let (mut total, invoked) = policy_run(&context, &mut log, succeeded_repair(), |phase| {
            if phase == "daily" {
                daily_calls += 1;
            }
            if phase == "sense_batch" {
                return failed_phase("sense_batch", "injected");
            }
            succeeded_phase(phase)
        });
        assert_eq!(invoked, REQUIRED_PHASES);
        assert_eq!(daily_calls, 1);
        assert_eq!(total.failed, 1);
        assert_eq!(total.failed_names, ["sense_batch (injected)"]);
        assert_eq!(total.skipped, 0);
        maybe_finalize_completion(
            &context,
            &mut log,
            completion_scope(journal.path(), 1),
            &complete_daily(),
            &BTreeSet::new(),
            Some((0, 0)),
            &mut total,
            |_, _, _| true,
        );
        assert_eq!(marker_generation(journal.path()), None);
        assert_eq!(total.failed, 1);
    }

    #[test]
    fn remaining_blockers_skip_daily_and_zero_blocker_twin_runs_daily() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let blocked_context = context(journal.path());
        let mut blocked_log = log(journal.path());
        let mut daily_calls = 0;
        let (mut blocked_total, blocked_invoked) = policy_run(
            &blocked_context,
            &mut blocked_log,
            blocked_repair(),
            |phase| {
                if phase == "daily" {
                    daily_calls += 1;
                }
                succeeded_phase(phase)
            },
        );
        assert_eq!(
            blocked_invoked,
            ["sense_batch", "segment_repair", "indexer", "journal_stats"]
        );
        assert_eq!(daily_calls, 0);
        assert_eq!(blocked_total.skipped_names, ["daily (blocked)"]);
        maybe_finalize_completion(
            &blocked_context,
            &mut blocked_log,
            completion_scope(journal.path(), 1),
            &complete_daily(),
            &BTreeSet::from([blocker("090000_300")]),
            Some((0, 1)),
            &mut blocked_total,
            |_, _, _| true,
        );
        assert_eq!(marker_generation(journal.path()), None);
        assert_eq!(blocked_total.failed, 1);

        let clear_journal = tempdir().unwrap();
        bump_stream_marker(clear_journal.path(), DAY).unwrap();
        let clear_context = context(clear_journal.path());
        let mut clear_log = log(clear_journal.path());
        daily_calls = 0;
        let (mut clear_total, clear_invoked) = policy_run(
            &clear_context,
            &mut clear_log,
            succeeded_repair(),
            |phase| {
                if phase == "daily" {
                    daily_calls += 1;
                }
                succeeded_phase(phase)
            },
        );
        assert_eq!(clear_invoked, REQUIRED_PHASES);
        assert_eq!(daily_calls, 1);
        maybe_finalize_completion(
            &clear_context,
            &mut clear_log,
            completion_scope(clear_journal.path(), 1),
            &complete_daily(),
            &BTreeSet::new(),
            Some((1, 0)),
            &mut clear_total,
            |_, _, _| true,
        );
        assert_eq!(marker_generation(clear_journal.path()), Some(1));
        assert_eq!(clear_total.failed, 0);
    }

    #[test]
    fn unread_post_repair_readiness_skips_daily_for_each_fixture_shape() {
        for detail in ["missing", "malformed", "unreadable"] {
            let journal = tempdir().unwrap();
            bump_stream_marker(journal.path(), DAY).unwrap();
            let context = context(journal.path());
            let mut log = log(journal.path());
            let mut daily_calls = 0;
            let (mut total, invoked) =
                policy_run(&context, &mut log, unread_repair(detail), |phase| {
                    if phase == "daily" {
                        daily_calls += 1;
                    }
                    succeeded_phase(phase)
                });
            assert_eq!(
                invoked,
                ["sense_batch", "segment_repair", "indexer", "journal_stats"],
                "{detail}"
            );
            assert_eq!(daily_calls, 0, "{detail}");
            assert_eq!(
                total.skipped_names,
                ["daily (readiness_unavailable)"],
                "{detail}"
            );
            assert_eq!(total.failed, 1, "{detail}");
            assert_eq!(
                total.failed_names,
                [format!("segment_repair_scan ({detail})")],
                "{detail}"
            );
            maybe_finalize_completion(
                &context,
                &mut log,
                completion_scope(journal.path(), 1),
                &complete_daily(),
                &BTreeSet::new(),
                None,
                &mut total,
                |_, _, _| true,
            );
            assert_eq!(marker_generation(journal.path()), None, "{detail}");
            assert_eq!(total.failed, 1, "{detail}");
        }
    }

    #[test]
    fn later_maintenance_phases_run_after_every_upstream_failure() {
        let cases: [(&str, SegmentPhaseOutcome, ModeResult); 4] = [
            (
                "repair-failed",
                failed_repair(false),
                succeeded_phase("sense_batch"),
            ),
            (
                "repair-timeout",
                failed_repair(true),
                succeeded_phase("sense_batch"),
            ),
            (
                "daily-failed",
                succeeded_repair(),
                failed_phase("daily", "injected"),
            ),
            (
                "daily-timeout",
                succeeded_repair(),
                timed_out_phase("daily", None),
            ),
        ];
        for (label, repair, daily) in cases {
            let journal = tempdir().unwrap();
            let context = context(journal.path());
            let mut log = log(journal.path());
            let daily_result = daily.clone();
            let expect_daily = daily_dependency_skip(&repair).is_none();
            let mut daily_calls = 0;
            let (total, invoked) = policy_run(&context, &mut log, repair, |phase| {
                if phase == "daily" {
                    daily_calls += 1;
                    return daily_result.clone();
                }
                succeeded_phase(phase)
            });
            assert!(
                invoked.ends_with(&["indexer".to_owned(), "journal_stats".to_owned()]),
                "{label}: {invoked:?}"
            );
            assert_eq!(daily_calls, usize::from(expect_daily), "{label}");
            if !expect_daily {
                assert_eq!(
                    total.skipped_names,
                    ["daily (segment_repair_failed)"],
                    "{label}"
                );
            }
            assert_eq!(total.failed, 1, "{label}");
            assert_eq!(
                total
                    .failed_names
                    .iter()
                    .filter(|name| name.as_str() == total.failed_names[0].as_str())
                    .count(),
                1,
                "{label} retains each failure once"
            );
        }
    }

    #[test]
    fn indexer_failure_still_runs_statistics() {
        let journal = tempdir().unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let runner = RecordingPhaseProcessRunner::new([
            PhaseProcessOutcome::Exited(7),
            PhaseProcessOutcome::Exited(0),
        ]);

        let result = run_with_phase_process(
            &context,
            &mut log,
            &ThinkArgs::default(),
            1,
            Some(Duration::from_secs(610)),
            &runner,
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(result.failed, 1);
        assert!(!result.timed_out);
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "indexer");
        assert_eq!(
            calls[0]
                .1
                .iter()
                .rev()
                .take(2)
                .rev()
                .cloned()
                .collect::<Vec<_>>(),
            ["indexer", "--rescan"]
        );
        assert_eq!(calls[1].0, "journal_stats");
        assert_eq!(calls[1].1.last().map(String::as_str), Some("journal-stats"));
        assert_eq!(marker_generation(journal.path()), None);
    }

    #[test]
    fn statistics_failure_fails_the_attempt_and_withholds_completion() {
        let journal = tempdir().unwrap();
        bump_stream_marker(journal.path(), DAY).unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let runner = RecordingPhaseProcessRunner::new([
            PhaseProcessOutcome::Exited(0),
            PhaseProcessOutcome::TimedOut {
                cleanup_error: None,
            },
        ]);

        let result = run_with_phase_process(
            &context,
            &mut log,
            &ThinkArgs::default(),
            1,
            Some(Duration::from_secs(610)),
            &runner,
            &BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(result.failed, 1);
        assert!(result.timed_out);
        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].1.last().map(String::as_str), Some("--rescan"));
        assert_eq!(calls[1].0, "journal_stats");
        assert_eq!(calls[1].1.last().map(String::as_str), Some("journal-stats"));
        assert_eq!(marker_generation(journal.path()), None);
    }

    #[test]
    fn phase_account_logs_one_start_and_terminal_including_skips() {
        let journal = tempdir().unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let sentinel = "SENTINEL_STDERR_/journal/path/raw";
        let (_total, invoked) = policy_run(&context, &mut log, blocked_repair(), |phase| {
            if phase == "sense_batch" {
                return failed_phase("sense_batch", format!("command failed: {sentinel} exit 1"));
            }
            succeeded_phase(phase)
        });
        assert_eq!(
            invoked,
            ["sense_batch", "segment_repair", "indexer", "journal_stats"]
        );
        let starts = lifecycle_rows(journal.path(), "phase.start");
        let completions = lifecycle_rows(journal.path(), "phase.complete");
        assert_eq!(starts.len(), 5);
        assert_eq!(completions.len(), 5);
        let phases = completions
            .iter()
            .map(|row| row["phase"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(phases, REQUIRED_PHASES);
        let daily = completions
            .iter()
            .find(|row| row["phase"] == "daily")
            .unwrap();
        assert_eq!(daily["success"], false);
        assert_eq!(daily["skipped"], true);
        assert_eq!(daily["reason_code"], SKIP_BLOCKED);
        assert!(daily.get("reason").is_none());
        let sense = completions
            .iter()
            .find(|row| row["phase"] == "sense_batch")
            .unwrap();
        assert_eq!(sense["success"], false);
        assert_eq!(sense["reason_code"], "failed");
        let rendered = serde_json::to_string(sense).unwrap();
        assert!(
            !rendered.contains(sentinel),
            "raw failure text leaked into {rendered}"
        );
        let repair = completions
            .iter()
            .find(|row| row["phase"] == "segment_repair")
            .unwrap();
        assert_eq!(repair["success"], true);
        assert_ne!(
            repair.get("reason_code").and_then(Value::as_str),
            Some("wall_clock_exceeded")
        );
    }

    #[test]
    fn mixed_outcome_counts_each_phase_once_and_does_not_count_skips_as_success() {
        let journal = tempdir().unwrap();
        let context = context(journal.path());
        let mut log = log(journal.path());
        let (total, invoked) = policy_run(&context, &mut log, blocked_repair(), |phase| {
            if phase == "sense_batch" {
                return failed_phase("sense_batch", "injected");
            }
            succeeded_phase(phase)
        });
        assert_eq!(
            invoked,
            ["sense_batch", "segment_repair", "indexer", "journal_stats"]
        );
        assert_eq!(total.failed, 1);
        assert_eq!(total.failed_names, ["sense_batch (injected)"]);
        assert_eq!(total.skipped, 1);
        assert_eq!(total.skipped_names, ["daily (blocked)"]);
        assert_eq!(
            total.success_names,
            ["segment_repair", "indexer", "journal_stats"]
        );
        assert!(!total.success_names.iter().any(|name| name == "daily"));
        assert_eq!(
            blocker_progress(
                &BTreeSet::from([blocker("090000_300")]),
                &BTreeSet::from([blocker("090000_300")])
            ),
            (0, 1)
        );
    }

    #[test]
    fn bounded_phase_timeout_is_a_terminal_failure() {
        let result = timed_out_phase("injected", None);

        assert!(result.timed_out);
        assert_eq!(result.failed, 1);
        assert_eq!(result.failed_names, ["injected (wall_clock_exceeded)"]);
    }

    #[test]
    fn from_scratch_and_refresh_both_force_the_full_segment_set() {
        assert!(force_all_repairs(&ThinkArgs {
            from_scratch: true,
            ..ThinkArgs::default()
        }));
        assert!(force_all_repairs(&ThinkArgs {
            refresh: true,
            ..ThinkArgs::default()
        }));
        assert!(!force_all_repairs(&ThinkArgs::default()));
    }
}
