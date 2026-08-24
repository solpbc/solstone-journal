// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whole-day catchup composition for the unscoped `journal think --day` mode.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde_json::{Map, Value};
use solstone_core_journal_io::{
    HealthMarkerKind, HealthMarkerState, PublishOutcome, publish_daily_marker_if_current,
    read_health_marker,
};
use solstone_core_journal_stats_cli::{FilesystemBacklogViewReader, FilesystemDocumentWriter};
use solstone_core_system::catchup::{
    SegmentRepairOutcome, record_daily_catchup_progress, record_segment_repair_attempt,
    record_segment_repair_outcome,
};
use solstone_core_system_health::{
    FilesystemHealthLogSource, FilesystemSegmentSource, SegmentIdentity, blocked_segment_keys,
    read_segment_progress, scan_day,
};

use crate::args::ThinkArgs;
use crate::context::ThinkContext;
use crate::daily;
use crate::dispatch::{ModeResult, merge_mode_result};
use crate::helpers;
use crate::run_log::RunLogWriter;
use crate::segment;

#[derive(Clone, Copy)]
struct CompletionScope {
    scoped_stream: bool,
    observed_generation: u64,
}

/// Execute the ordered whole-day catchup lifecycle.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter<std::fs::File>,
    args: &ThinkArgs,
    default_segment_workers: usize,
    timeout: Option<Duration>,
) -> Result<ModeResult, String> {
    let observed_generation = stream_generation(context)?;
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
    let mut total = ModeResult::default();

    log_phase_start(log, context, "sense_batch");
    merge_mode_result(
        &mut total,
        phase_result(
            "sense_batch",
            solstone_core_sense::batch::run_batch_with_environment(
                &context.journal,
                &solstone_core_sense::batch::BatchRequest {
                    day: context.day.clone(),
                    jobs: args.jobs,
                    reprocess: None,
                    segment: None,
                    stream: args.stream.clone(),
                    dry_run: false,
                    verbose: args.verbose,
                    debug: args.debug,
                },
                &BTreeMap::new(),
            ),
        ),
    );
    log_phase_complete(log, context, "sense_batch");

    log_phase_start(log, context, "segment_repair");
    let pre_repair_blocked = match blocked_segments(context) {
        Ok(blocked) => blocked,
        Err(error) => {
            merge_mode_result(&mut total, failed_phase("segment_repair_scan", error));
            BTreeSet::new()
        }
    };
    let repair_segments = pre_repair_blocked
        .iter()
        .map(|identity| (identity.segment.clone(), identity.stream.clone()))
        .collect::<Vec<_>>();
    let repair_attempted = !pre_repair_blocked.is_empty();
    if repair_attempted {
        record_segment_repair_attempt(&context.journal, &context.day, unix_seconds_f64());
    }
    let repair_result = match segment::run_repair_batch(
        context,
        repair_segments,
        args.refresh,
        args.jobs,
        segment_workers,
        timeout,
        skip_talents,
    ) {
        Ok(result) => result,
        Err(error) => failed_phase("segment_repair", error),
    };
    merge_mode_result(&mut total, repair_result.clone());
    let post_repair_blocked = match blocked_segments(context) {
        Ok(blocked) => blocked,
        Err(error) => {
            merge_mode_result(&mut total, failed_phase("segment_repair_scan", error));
            pre_repair_blocked.clone()
        }
    };
    if repair_attempted {
        let cleared = pre_repair_blocked.difference(&post_repair_blocked).count();
        let remaining = post_repair_blocked.len();
        record_segment_repair_outcome(
            &context.journal,
            &context.day,
            SegmentRepairOutcome {
                success: post_repair_blocked.is_empty(),
                timed_out: repair_result.timed_out,
                timeout_seconds: repair_result
                    .timed_out
                    .then(|| timeout.map(|duration| duration.as_secs_f64()))
                    .flatten(),
                ended_at: unix_seconds_f64(),
                cleared: Some(cleared),
                remaining: Some(remaining),
            },
        );
        record_daily_catchup_progress(&context.journal, &context.day, cleared, remaining);
    }
    log_phase_complete(log, context, "segment_repair");

    log_phase_start(log, context, "daily");
    let daily_result = match daily::run(
        context,
        log,
        args.stream.as_deref(),
        args.from_scratch,
        args.jobs,
    ) {
        Ok(result) => result,
        Err(error) => failed_phase("daily", error),
    };
    merge_mode_result(&mut total, daily_result.clone());
    log_phase_complete(log, context, "daily");

    log_phase_start(log, context, "indexer");
    merge_mode_result(
        &mut total,
        phase_result(
            "indexer",
            solstone_core_indexer_store::scan::scan_journal(&context.journal, false),
        ),
    );
    log_phase_complete(log, context, "indexer");

    log_phase_start(log, context, "journal_stats");
    let stats = solstone_core_journal_stats_cli::run_cli(
        &[],
        &context.journal,
        Utc::now(),
        &context.talent_root,
        &context.apps_root,
        &FilesystemBacklogViewReader,
        &FilesystemDocumentWriter,
    );
    if stats.exit_code == 0 {
        merge_mode_result(&mut total, succeeded_phase("journal_stats"));
    } else {
        merge_mode_result(
            &mut total,
            failed_phase("journal_stats", stats.stderr.trim().to_owned()),
        );
    }
    log_phase_complete(log, context, "journal_stats");

    maybe_finalize_completion(
        context,
        log,
        CompletionScope {
            scoped_stream: args.stream.is_some(),
            observed_generation,
        },
        &daily_result,
        &post_repair_blocked,
        &mut total,
        |journal, now_ms, fields| helpers::emit(journal, now_ms, "daily_complete", fields),
    );

    Ok(total)
}

fn stream_generation(context: &ThinkContext) -> Result<u64, String> {
    match read_health_marker(&context.journal, &context.day, HealthMarkerKind::Stream)
        .map_err(|error| error.to_string())?
    {
        HealthMarkerState::Versioned { marker, .. } => Ok(marker.generation),
        HealthMarkerState::Absent
        | HealthMarkerState::LegacyEmpty { .. }
        | HealthMarkerState::MalformedNonEmpty { .. } => Ok(0),
    }
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

fn phase_result<T, E: std::fmt::Display>(phase: &str, result: Result<T, E>) -> ModeResult {
    match result {
        Ok(_) => succeeded_phase(phase),
        Err(error) => failed_phase(phase, error),
    }
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

fn maybe_finalize_completion(
    context: &ThinkContext,
    log: &mut RunLogWriter<std::fs::File>,
    scope: CompletionScope,
    daily: &ModeResult,
    segment_blockers: &BTreeSet<SegmentIdentity>,
    total: &mut ModeResult,
    emit: impl FnOnce(&std::path::Path, i64, Map<String, Value>) -> bool,
) {
    if scope.scoped_stream {
        log_completion_fold(log, context, false, true, daily, segment_blockers);
        return;
    }
    let complete = total.failed == 0
        && daily.applicable_units.is_subset(&daily.terminal_units)
        && segment_blockers.is_empty();
    log_completion_fold(log, context, complete, false, daily, segment_blockers);
    if !complete {
        return;
    }
    match publish_daily_marker_if_current(&context.journal, &context.day, scope.observed_generation)
    {
        Ok(PublishOutcome::Published(generation)) => {
            let _ = emit(
                &context.journal,
                context.now_ms,
                Map::from_iter([
                    ("day".to_owned(), Value::String(context.day.clone())),
                    ("generation".to_owned(), Value::from(generation)),
                ]),
            );
        }
        Ok(PublishOutcome::AlreadyCurrent(_) | PublishOutcome::Superseded(_)) => {}
        Err(error) => merge_mode_result(total, failed_phase("daily_marker", error)),
    }
}

fn log_completion_fold(
    log: &mut RunLogWriter<std::fs::File>,
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

fn log_phase_start(log: &mut RunLogWriter<std::fs::File>, context: &ThinkContext, phase: &str) {
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

fn log_phase_complete(log: &mut RunLogWriter<std::fs::File>, context: &ThinkContext, phase: &str) {
    log.log(
        "phase.complete",
        context.now_ms,
        Map::from_iter([
            ("mode".to_owned(), Value::String("daily".to_owned())),
            ("day".to_owned(), Value::String(context.day.clone())),
            ("phase".to_owned(), Value::String(phase.to_owned())),
        ]),
    );
}

fn unix_seconds_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use solstone_core_journal_io::{
        HealthMarkerKind, HealthMarkerState, bump_stream_marker, read_health_marker,
    };
    use tempfile::tempdir;

    use super::*;

    const DAY: &str = "20260813";

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

    fn log(journal: &std::path::Path) -> RunLogWriter<std::fs::File> {
        RunLogWriter::open(&journal.join("chronicle").join(DAY).join("lifecycle.jsonl"))
    }

    fn complete_daily() -> ModeResult {
        ModeResult::default()
    }

    fn marker_generation(journal: &std::path::Path) -> Option<u64> {
        match read_health_marker(journal, DAY, HealthMarkerKind::Daily).unwrap() {
            HealthMarkerState::Versioned { marker, .. } => Some(marker.generation),
            _ => None,
        }
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
        )
        .unwrap();
        assert_eq!(result.failed, 0);
        let rows = fs::read_to_string(
            journal
                .path()
                .join("chronicle")
                .join(DAY)
                .join("lifecycle.jsonl"),
        )
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["event"] == "phase.start")
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
    }

    #[test]
    fn successful_fold_publishes_observed_generation_and_emits_once() {
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
                scoped_stream: false,
                observed_generation: 1,
            },
            &complete_daily(),
            &BTreeSet::new(),
            &mut total,
            |_, _, _| {
                events += 1;
                true
            },
        );
        assert_eq!(marker_generation(journal.path()), Some(1));
        assert_eq!(events, 1);
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
            CompletionScope {
                scoped_stream: false,
                observed_generation: 1,
            },
            &complete_daily(),
            &BTreeSet::new(),
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
            CompletionScope {
                scoped_stream: false,
                observed_generation: 1,
            },
            &complete_daily(),
            &blockers,
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
            },
            &complete_daily(),
            &BTreeSet::new(),
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
        let result = run(&context, &mut log, &args, 1, Some(Duration::from_secs(610))).unwrap();
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
            CompletionScope {
                scoped_stream: false,
                observed_generation: 1,
            },
            &complete_daily(),
            &BTreeSet::new(),
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
            CompletionScope {
                scoped_stream: false,
                observed_generation: 1,
            },
            &complete_daily(),
            &BTreeSet::new(),
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
}
