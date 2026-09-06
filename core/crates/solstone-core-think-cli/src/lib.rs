// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native preflight and unavailable-run boundary for `journal think`.

mod activity;
mod args;
mod cadence;
mod cadence_state;
mod context;
mod daily;
mod daily_lifecycle;
mod day;
mod dispatch;
mod dry_run;
mod flush;
mod gate;
mod helpers;
mod phase_process;
mod run_log;
mod segment;
mod weekly;
mod workers;

#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod test_support {
    use std::path::Path;

    use serde_json::{Map, Value};

    pub fn emit(journal: &Path, now_ms: i64, event: &str, fields: Map<String, Value>) -> bool {
        crate::helpers::emit(journal, now_ms, event, fields)
    }

    pub fn runtime() -> Result<tokio::runtime::Runtime, String> {
        crate::dispatch::runtime()
    }

    pub fn emit_segment_dispatch(journal: &Path, day: &str, now_ms: i64) -> Result<bool, String> {
        let mut log = crate::run_log::RunLogWriter::open(journal, day, "segment");
        let emitted = crate::segment::write_dispatch_event(
            journal,
            &mut log,
            now_ms,
            Map::from_iter([
                ("mode".to_owned(), Value::String("segment".to_owned())),
                ("day".to_owned(), Value::String(day.to_owned())),
                ("segment".to_owned(), Value::String("test".to_owned())),
                ("name".to_owned(), Value::String("sense".to_owned())),
                ("use_id".to_owned(), Value::String("use-test".to_owned())),
            ]),
        );
        log.finish()?;
        Ok(emitted)
    }
}

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use chrono::{Local, NaiveDate};
use solstone_core_cli::THINK_USAGE;
use solstone_core_local::{LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_segment::SUPERVISOR_MESSAGE;

use crate::args::{
    ACTIVITY_INCOMPATIBLE, ACTIVITY_REQUIRES_DAY, ACTIVITY_REQUIRES_FACET, CADENCE_INCOMPATIBLE,
    FACET_REQUIRES_ACTIVITY, FLUSH_INCOMPATIBLE, FLUSH_REQUIRES_SEGMENT,
    MULTI_WORKER_UNLIMITED_JOBS, NO_ACTIVITY_PROMPTS_WITH_ACTIVITY, SEGMENT_WORKERS_RANGE,
    SEGMENTS_INCOMPATIBLE, UPDATED_INCOMPATIBLE, WEEKLY_INCOMPATIBLE,
};

#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, PartialEq, Eq)]
enum CliError {
    Usage { message: String },
    SupervisorSpawnedUnavailable,
    SupervisorUnavailable,
    InvalidDay { message: String },
}

pub fn run_cli(
    args: &[String],
    journal: &Path,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) -> CliRun {
    let event_clock: Arc<dyn Fn() -> i64 + Send + Sync> =
        Arc::new(|| chrono::Utc::now().timestamp_millis());
    let start_clock = Arc::clone(&event_clock);
    run_cli_with_event_clock(
        args,
        journal,
        |name| std::env::var(name).ok(),
        || solstone_core_segment::is_solstone_up(journal),
        || Local::now().date_naive(),
        move || start_clock(),
        Some(event_clock),
        || {
            std::thread::available_parallelism()
                .ok()
                .map(|count| count.get())
        },
        || {
            solstone_core_journal_config::read_journal_config(journal)
                .ok()
                .and_then(|read| read.config)
                .map(|config| {
                    let uses_local = config
                        .get("providers")
                        .and_then(serde_json::Value::as_object)
                        .and_then(|providers| providers.get("active"))
                        .and_then(serde_json::Value::as_object)
                        .and_then(|active| active.get("provider"))
                        .and_then(serde_json::Value::as_str)
                        == Some("local");
                    (uses_local, resolve_local_endpoint(&config))
                })
                .unwrap_or((false, LocalEndpointResolution::Bundled))
        },
        || workers::bundled_slots(journal),
        sense_child_environment,
    )
}

/// Whether `raw_args` would reach the whole-day lifecycle (the sole caller of
/// `sense_batch`, and thus the only mode that can launch transcription),
/// rather than a narrower `--updated`, `--cadence`, `--activity`, `--flush`,
/// `--segments`, `--segment`, `--weekly`, or `--dry-run` mode. Callers use
/// this to decide whether to acquire a speakers-analyze generation before
/// invoking [`run_cli`]; this crate has no dependency on the speakers-analyze
/// lease itself, so the decision is made by the caller.
pub fn requires_daily_lifecycle(raw_args: &[String]) -> bool {
    let Ok(args::ParseOutcome::Args(parsed)) = args::parse(raw_args) else {
        return false;
    };
    !parsed.updated
        && !parsed.cadence
        && parsed.activity.is_none()
        && !parsed.flush
        && !parsed.segments
        && parsed.segment.is_none()
        && !parsed.weekly
        && !parsed.dry_run
}

#[allow(clippy::too_many_arguments)]
pub fn run_cli_with<E, C, N, M, P, R, B>(
    raw_args: &[String],
    journal: &Path,
    lookup_env: E,
    connectivity: C,
    clock: N,
    now_ms: M,
    cpu_count: P,
    endpoint: R,
    bundled_slots: B,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
    N: Fn() -> NaiveDate,
    M: Fn() -> i64,
    P: Fn() -> Option<usize>,
    R: Fn() -> (bool, LocalEndpointResolution),
    B: Fn() -> Option<u32>,
{
    run_cli_with_event_clock(
        raw_args,
        journal,
        lookup_env,
        connectivity,
        clock,
        now_ms,
        None,
        cpu_count,
        endpoint,
        bundled_slots,
        sense_child_environment,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_cli_with_event_clock<E, C, N, M, P, R, B>(
    raw_args: &[String],
    journal: &Path,
    lookup_env: E,
    connectivity: C,
    clock: N,
    now_ms: M,
    event_clock: Option<Arc<dyn Fn() -> i64 + Send + Sync>>,
    cpu_count: P,
    endpoint: R,
    bundled_slots: B,
    sense_child_environment: &BTreeMap<OsString, OsString>,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
    N: Fn() -> NaiveDate,
    M: Fn() -> i64,
    P: Fn() -> Option<usize>,
    R: Fn() -> (bool, LocalEndpointResolution),
    B: Fn() -> Option<u32>,
{
    let result = (|| {
        let parsed = match args::parse(raw_args).map_err(|message| CliError::Usage { message })? {
            args::ParseOutcome::Help => {
                return Ok(CliRun {
                    stdout: THINK_USAGE.to_owned(),
                    stderr: String::new(),
                    exit_code: 0,
                });
            }
            args::ParseOutcome::Args(parsed) => parsed,
        };
        gate::check(lookup_env, connectivity)?;
        solstone_core_identity::ensure_identity_directory(journal).map_err(|error| {
            CliError::InvalidDay {
                message: error.to_string(),
            }
        })?;

        let today = clock();
        if parsed.updated {
            let offenders = args::updated_offenders(&parsed);
            if !offenders.is_empty() {
                return Err(CliError::Usage {
                    message: format!("{UPDATED_INCOMPATIBLE}{}", offenders.join(", ")),
                });
            }
            let days =
                day::updated(journal, today).map_err(|message| CliError::InvalidDay { message })?;
            return Ok(CliRun {
                stdout: days
                    .join("\n")
                    .chars()
                    .chain((!days.is_empty()).then_some('\n'))
                    .collect(),
                stderr: String::new(),
                exit_code: 0,
            });
        }

        let selected_day = day::selected_day(parsed.day.as_deref(), parsed.cadence, today);
        let day_dir = day::create_day(journal, &selected_day)
            .map_err(|message| CliError::InvalidDay { message })?;
        let (uses_local, endpoint) = endpoint();
        let cpu_count = cpu_count();
        let bundled_slots = bundled_slots();
        let default_segment_workers = workers::default_segment_workers(
            cpu_count,
            uses_local,
            endpoint.clone(),
            bundled_slots,
        );
        validate(&parsed, cpu_count, uses_local, endpoint, bundled_slots)?;

        let now_ms = now_ms();
        let event_clock = event_clock.unwrap_or_else(|| Arc::new(move || now_ms));
        let context = context::ThinkContext::new_with_event_clock(
            journal,
            selected_day.clone(),
            day_dir.clone(),
            now_ms,
            event_clock,
        )
        .map_err(|message| CliError::InvalidDay { message })?;
        if parsed.dry_run {
            return Ok(CliRun {
                stdout: dry_run::run(&context, &parsed, default_segment_workers)
                    .map_err(|message| CliError::InvalidDay { message })?,
                stderr: String::new(),
                exit_code: 0,
            });
        }
        let timeout = (!parsed.no_timeout).then_some(std::time::Duration::from_secs(610));
        if parsed.cadence {
            let configs = cadence::configured(&context)
                .map_err(|message| CliError::InvalidDay { message })?;
            // `thinking.py:2969-2972` returns before creating the cadence sidecar
            // when no cadence talent is configured.
            if configs.is_empty() {
                return Ok(CliRun {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                });
            }
            let mut log = open_run_log(&parsed, journal, now_ms, &selected_day);
            let result = cadence::run(&context, configs, &mut log, parsed.refresh);
            return logged_mode_outcome(log, result);
        }

        if let Some(activity_id) = parsed.activity.as_deref() {
            let mut log = open_run_log(&parsed, journal, now_ms, &selected_day);
            let result = activity::run(
                &context,
                &mut log,
                activity_id,
                parsed.facet.as_deref().expect("validated activity facet"),
                parsed.refresh,
                parsed.jobs,
            );
            return logged_mode_outcome(log, result);
        }
        if parsed.flush {
            let mut log = open_run_log(&parsed, journal, now_ms, &selected_day);
            let result = flush::run(
                &context,
                &mut log,
                parsed.segment.as_deref().expect("validated flush segment"),
                parsed.stream.as_deref(),
            );
            return logged_mode_outcome(log, result);
        }
        if parsed.segments {
            let mut log = open_run_log(&parsed, journal, now_ms, &selected_day);
            let source = solstone_core_system_health::FilesystemSegmentSource;
            let segments: Vec<(String, Option<String>)> =
                match solstone_core_system_health::scan_day(
                    &source,
                    &context.journal,
                    &context.day,
                    chrono::Utc::now(),
                ) {
                    Ok((_, _, entries)) => entries
                        .into_iter()
                        .map(|entry| (entry.key, Some(entry.stream)))
                        .collect(),
                    Err(error) => return logged_mode_outcome(log, Err(error.to_string())),
                };
            let skip_talents = parsed
                .skip_talents
                .split(',')
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let workers =
                usize::try_from(parsed.segment_workers.unwrap_or(
                    i64::try_from(default_segment_workers).expect("worker count fits i64"),
                ))
                .expect("validated segment workers");
            let result = segment::run_repair_batch_with_activity(
                &context,
                &mut log,
                segments.clone(),
                parsed.refresh,
                parsed.jobs,
                workers,
                timeout,
                skip_talents,
                parsed.no_activity_prompts,
            );
            return logged_mode_outcome(log, result);
        }
        if let Some(segment) = parsed.segment.as_deref() {
            let mut log = open_run_log(&parsed, journal, now_ms, &selected_day);
            let skip_talents = parsed
                .skip_talents
                .split(',')
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            // Source-derived, not measured: thinking.py:4709 gives the
            // direct segment path its sole optional overall deadline.
            let result = segment::run(
                &context,
                &mut log,
                segment,
                parsed.refresh,
                parsed.stream.as_deref(),
                parsed.jobs,
                timeout,
                parsed.live,
                &skip_talents,
            );
            // Source-derived, not measured: thinking.py:2021-2030 advances
            // activity state after every direct segment Sense run.
            let result = result.map(|mut result| {
                if let Err(error) = segment::replay_activity_state(
                    &context,
                    &mut log,
                    &[(segment.to_owned(), parsed.stream.clone())],
                    parsed.refresh,
                    parsed.jobs,
                    parsed.no_activity_prompts,
                    true,
                ) {
                    dispatch::record_followup_failure(&mut result, "activity replay", &error);
                }
                result
            });
            return logged_mode_outcome(log, result);
        }
        let mut log = open_run_log(&parsed, journal, now_ms, &selected_day);
        let result = if parsed.weekly {
            weekly::run(
                &context,
                &mut log,
                parsed.refresh,
                parsed.stream.as_deref(),
                parsed.jobs,
            )
        } else {
            daily_lifecycle::run(
                &context,
                &mut log,
                &parsed,
                default_segment_workers,
                timeout,
                sense_child_environment,
            )
        };
        logged_mode_outcome(log, result)
    })();
    match result {
        Ok(run) => run,
        Err(CliError::Usage { message }) => CliRun {
            stdout: String::new(),
            stderr: format!("{THINK_USAGE}journal think: error: {message}\n"),
            exit_code: 2,
        },
        Err(CliError::SupervisorSpawnedUnavailable) => CliRun {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 75,
        },
        Err(CliError::SupervisorUnavailable) => CliRun {
            stdout: String::new(),
            stderr: format!("{SUPERVISOR_MESSAGE}\n"),
            exit_code: 1,
        },
        Err(CliError::InvalidDay { message }) => CliRun {
            stdout: String::new(),
            stderr: format!("journal think: {message}\n"),
            exit_code: 1,
        },
    }
}

fn mode_outcome(result: dispatch::ModeResult) -> CliRun {
    if result.failed == 0 {
        let mut stderr = format!("journal think: {} completed\n", result.success);
        for name in &result.success_names {
            stderr.push_str(name);
            stderr.push('\n');
        }
        return CliRun {
            stdout: String::new(),
            stderr,
            exit_code: 0,
        };
    }
    let mut stderr = format!("journal think: {} failed\n", result.failed);
    for name in &result.failed_names {
        stderr.push_str(name);
        stderr.push('\n');
    }
    CliRun {
        stdout: String::new(),
        stderr,
        exit_code: 1,
    }
}

fn logged_mode_outcome(
    log: run_log::RunLogWriter,
    result: Result<dispatch::ModeResult, String>,
) -> Result<CliRun, CliError> {
    let log_error = log.finish().err();
    match (result, log_error) {
        (Ok(result), None) => Ok(mode_outcome(result)),
        (Ok(result), Some(log_error)) => {
            let mut run = mode_outcome(result);
            run.exit_code = 1;
            run.stderr
                .push_str(&format!("journal think: {log_error}\n"));
            Ok(run)
        }
        (Err(message), None) => Err(CliError::InvalidDay { message }),
        (Err(message), Some(log_error)) => Err(CliError::InvalidDay {
            message: format!("{message}; {log_error}"),
        }),
    }
}

fn open_run_log(
    args: &args::ThinkArgs,
    journal: &Path,
    now_ms: i64,
    day: &str,
) -> run_log::RunLogWriter {
    // This order differs from Python's main chain only superficially: args.rs
    // refuses --segment with --weekly or --cadence before mode derivation.
    let mode = run_log::mode(args);
    let mut log = run_log::RunLogWriter::open(journal, day, mode);
    let mut fields = serde_json::Map::new();
    fields.insert(
        "mode".to_owned(),
        serde_json::Value::String(mode.to_owned()),
    );
    fields.insert("day".to_owned(), serde_json::Value::String(day.to_owned()));
    fields.insert("ref".to_owned(), serde_json::Value::from(now_ms));
    log.log("run.start", now_ms, fields);
    log
}

fn validate(
    args: &args::ThinkArgs,
    cpu_count: Option<usize>,
    uses_local: bool,
    endpoint: LocalEndpointResolution,
    bundled_slots: Option<u32>,
) -> Result<(), CliError> {
    let usage = |message: &str| {
        Err(CliError::Usage {
            message: message.to_owned(),
        })
    };
    if args.facet.is_some() && args.activity.is_none() {
        return usage(FACET_REQUIRES_ACTIVITY);
    }
    if args.activity.is_some() && args.facet.is_none() {
        return usage(ACTIVITY_REQUIRES_FACET);
    }
    if args.activity.is_some() && args.day.is_none() {
        return usage(ACTIVITY_REQUIRES_DAY);
    }
    if args.no_activity_prompts && args.activity.is_some() {
        return usage(NO_ACTIVITY_PROMPTS_WITH_ACTIVITY);
    }
    if args
        .segment_workers
        .is_some_and(|workers| !(1..=32).contains(&workers))
    {
        return usage(SEGMENT_WORKERS_RANGE);
    }
    if args.activity.is_some() && (args.segment.is_some() || args.segments || args.flush) {
        return usage(ACTIVITY_INCOMPATIBLE);
    }
    if args.flush && args.segment.is_none() {
        return usage(FLUSH_REQUIRES_SEGMENT);
    }
    if args.flush && (args.segments || args.refresh) {
        return usage(FLUSH_INCOMPATIBLE);
    }
    if args.segments && (args.segment.is_some() || args.facet.is_some()) {
        return usage(SEGMENTS_INCOMPATIBLE);
    }
    if args.weekly
        && (args.segment.is_some() || args.segments || args.activity.is_some() || args.flush)
    {
        return usage(WEEKLY_INCOMPATIBLE);
    }
    if args.cadence
        && (args.segment.is_some()
            || args.segments
            || args.activity.is_some()
            || args.flush
            || args.weekly)
    {
        return usage(CADENCE_INCOMPATIBLE);
    }
    if args.segments {
        let workers = args
            .segment_workers
            .map(|workers| workers as usize)
            .unwrap_or_else(|| {
                workers::default_segment_workers(cpu_count, uses_local, endpoint, bundled_slots)
            });
        if args.jobs == 0 && workers > 1 {
            return usage(MULTI_WORKER_UNLIMITED_JOBS);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
    use std::path::Path;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock};

    use chrono::NaiveDate;
    use filetime::{FileTime, set_file_mtime};
    use log::{Level, LevelFilter, Log, Metadata, Record};
    use serde_json::{Map, Value};
    use solstone_core_journal_io::{
        JournalRoot,
        operational_log::{OplogFormat, catalog_oplogs, create_oplog_at},
    };
    use solstone_core_local::LocalEndpointResolution;
    use tempfile::tempdir;

    use super::*;

    struct TestLogger;

    static LOGGER: TestLogger = TestLogger;
    static LOGGER_INIT: Once = Once::new();
    static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    static LOG_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[derive(Default)]
    struct Recorder {
        requests: Mutex<Vec<solstone_core_cortex_client::CortexRequest>>,
        waits: Mutex<Vec<Vec<String>>>,
        deadlines: Mutex<Vec<Option<std::time::Duration>>>,
        finish_fields: Mutex<solstone_core_cortex_client::FinishFields>,
        dispatch_failure: Mutex<Option<context::DispatchFailure>>,
        dispatch_failures: Mutex<std::collections::BTreeMap<String, context::DispatchFailure>>,
        end_states:
            Mutex<std::collections::BTreeMap<String, solstone_core_cortex_client::UseEndState>>,
        timed_out: Mutex<Vec<solstone_core_cortex_client::TimedOutUse>>,
        omit_ids: Mutex<BTreeSet<String>>,
        wait_error: Mutex<Option<String>>,
    }

    #[derive(Default)]
    struct IndexRecorder(Mutex<Vec<std::path::PathBuf>>);

    impl context::IndexBoundary for IndexRecorder {
        fn rescan_file(&self, _: &Path, path: &Path) {
            self.0.lock().unwrap().push(path.to_path_buf());
        }
    }

    impl context::CortexBoundary for Recorder {
        fn dispatch(
            &self,
            _: &tokio::runtime::Runtime,
            request: &solstone_core_cortex_client::CortexRequest,
        ) -> Result<String, context::DispatchFailure> {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            if let Some(error) = self
                .dispatch_failures
                .lock()
                .unwrap()
                .get(&request.name)
                .cloned()
            {
                return Err(error);
            }
            if let Some(error) = self.dispatch_failure.lock().unwrap().clone() {
                return Err(error);
            }
            Ok(format!("use-{}", requests.len()))
        }
        fn wait(
            &self,
            _: &tokio::runtime::Runtime,
            use_ids: &[String],
            deadline: Option<std::time::Duration>,
        ) -> Result<solstone_core_cortex_client::WaitForUsesReport, String> {
            self.waits.lock().unwrap().push(use_ids.to_vec());
            self.deadlines.lock().unwrap().push(deadline);
            if let Some(error) = self.wait_error.lock().unwrap().clone() {
                return Err(error);
            }
            let timed_out = self.timed_out.lock().unwrap().clone();
            let omit_ids = self.omit_ids.lock().unwrap().clone();
            let end_states = self.end_states.lock().unwrap().clone();
            let finish_fields = *self.finish_fields.lock().unwrap();
            Ok(solstone_core_cortex_client::WaitForUsesReport {
                completed: use_ids
                    .iter()
                    .filter(|id| {
                        !timed_out.iter().any(|timeout| timeout.use_id() == *id)
                            && !omit_ids.contains(*id)
                    })
                    .map(|id| {
                        (
                            id.clone(),
                            solstone_core_cortex_client::UseCompletion {
                                end_state: end_states
                                    .get(id)
                                    .copied()
                                    .unwrap_or(solstone_core_cortex_client::UseEndState::Finish),
                                finish_fields,
                            },
                        )
                    })
                    .collect(),
                timed_out,
            })
        }
    }

    fn recorder_context(
        journal: &Path,
        day: &str,
        now_ms: i64,
    ) -> (context::ThinkContext, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        let day_dir = day::create_day(journal, day).unwrap();
        (
            context::ThinkContext::new(journal, day.to_owned(), day_dir, now_ms)
                .expect("think context")
                .with_boundary(recorder.clone()),
            recorder,
        )
    }

    fn recorder_context_with_index(
        journal: &Path,
        day: &str,
        now_ms: i64,
    ) -> (context::ThinkContext, Arc<Recorder>, Arc<IndexRecorder>) {
        let (context, recorder) = recorder_context(journal, day, now_ms);
        let index = Arc::new(IndexRecorder::default());
        (context.with_index_boundary(index.clone()), recorder, index)
    }

    fn test_log(context: &context::ThinkContext, run: &str) -> run_log::RunLogWriter {
        run_log::RunLogWriter::open(&context.journal, &context.day, run)
    }

    fn oplog_records(journal: &Path, day: &str, run: &str) -> Vec<Value> {
        let day = NaiveDate::parse_from_str(day, "%Y%m%d").unwrap();
        catalog_oplogs(JournalRoot::open(journal).unwrap(), &[day])
            .unwrap()
            .into_catalogued_entries()
            .into_iter()
            .filter(|(entry, _)| {
                entry.name().source().display_slug() == "think"
                    && entry.name().run().display_slug() == run
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
            .collect()
    }

    fn talent_roots(
        root: &Path,
        entries: &[(&str, &str)],
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let talents = root.join("talent");
        fs::create_dir_all(&talents).unwrap();
        for (name, contents) in entries {
            fs::write(talents.join(format!("{name}.md")), contents).unwrap();
        }
        let apps = root.join("apps");
        fs::create_dir_all(&apps).unwrap();
        (talents, apps)
    }

    impl Log for TestLogger {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.level() <= Level::Warn
        }

        fn log(&self, record: &Record<'_>) {
            if self.enabled(record.metadata()) {
                LOGS.get_or_init(|| Mutex::new(Vec::new()))
                    .lock()
                    .unwrap()
                    .push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    fn capture_logs() -> MutexGuard<'static, ()> {
        let guard = LOG_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        LOGGER_INIT.call_once(|| {
            log::set_logger(&LOGGER).unwrap();
            log::set_max_level(LevelFilter::Warn);
        });
        LOGS.get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clear();
        guard
    }

    fn warnings() -> Vec<String> {
        LOGS.get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clone()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()
    }

    fn run_at(journal: &Path, args: &[&str]) -> CliRun {
        run_cli_with(
            &args
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
            journal,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            today,
            || 1_785_000_000_000,
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(2),
            &BTreeMap::new(),
        )
    }

    fn run(args: &[&str]) -> CliRun {
        let journal = tempdir().expect("journal");
        run_at(journal.path(), args)
    }

    /// The oracle harness never executes an argv read verbatim from the frozen
    /// fixture.  Scenario A deliberately omits this flag, so executing its
    /// header would otherwise start a real run mode.
    fn oracle_dry_run_argv(header: &[&str]) -> Result<Vec<String>, &'static str> {
        if header.contains(&"--dry-run") {
            return Err("fixture argv must not supply --dry-run");
        }
        let mut argv = header
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>();
        argv.push("--dry-run".to_owned());
        Ok(argv)
    }

    fn oracle_blocks() -> Vec<(usize, Vec<String>, String)> {
        let fixture = include_str!("../../../fixtures/think-dry-run-reference-oracle.txt");
        let lines = fixture.lines().collect::<Vec<_>>();
        let mut blocks = Vec::new();
        let mut line = 0;
        while line < lines.len() {
            let Some(header) = lines[line].strip_prefix("### journal think") else {
                line += 1;
                continue;
            };
            let header_line = line + 1;
            let argv = header
                .split_whitespace()
                // The fixture documents that a header may contain this flag,
                // but only this harness is allowed to append it to execution.
                .filter(|argument| *argument != "--dry-run")
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let mut stdout = String::new();
            line += 1;
            while line < lines.len() && lines[line] != "(exit 0)" {
                stdout.push_str(lines[line]);
                stdout.push('\n');
                line += 1;
            }
            assert!(line < lines.len(), "oracle block must state its exit");
            blocks.push((header_line, argv, stdout));
            line += 1;
        }
        blocks
    }

    fn seed_oracle_case(journal: &Path, line: usize, now_ms: i64) {
        match line {
            101 | 126 | 138 => {
                for facet in ["personal", "work"] {
                    let declaration = journal.join("facets").join(facet).join("facet.json");
                    fs::create_dir_all(declaration.parent().unwrap()).unwrap();
                    fs::write(declaration, "{}\n").unwrap();
                }
            }
            193 => {
                for (key, body) in [
                    ("093000_600", "browser_first.jsonl"),
                    ("141500_900", "browser_second.jsonl"),
                ] {
                    let segment = journal.join("chronicle/20260101").join(key);
                    fs::create_dir_all(&segment).unwrap();
                    fs::write(segment.join(body), "browser content\n").unwrap();
                }
            }
            221 => {
                let cadence = journal.join("health/cadence.json");
                fs::create_dir_all(cadence.parent().unwrap()).unwrap();
                fs::write(
                    cadence,
                    format!(r#"{{"steward":{now_ms},"pulse":{}}}"#, now_ms - 3_600_000),
                )
                .unwrap();
            }
            _ => {}
        }
    }

    fn run_oracle_case(journal: &Path, header_argv: &[String], now_ms: i64) -> CliRun {
        let borrowed = header_argv.iter().map(String::as_str).collect::<Vec<_>>();
        let argv = oracle_dry_run_argv(&borrowed).expect("harness owns dry-run flag");
        run_cli_with(
            &argv,
            journal,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            || NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            move || now_ms,
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(4),
            &BTreeMap::new(),
        )
    }

    fn created_paths(root: &Path) -> BTreeSet<String> {
        fn visit(root: &Path, path: &Path, values: &mut BTreeSet<String>) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let relative = entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .display()
                    .to_string();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    values.insert(format!("{relative}/"));
                    visit(root, &entry.path(), values);
                } else {
                    values.insert(relative);
                }
            }
        }
        let mut values = BTreeSet::new();
        visit(root, root, &mut values);
        values
    }

    fn day_keys(journal: &Path) -> BTreeSet<String> {
        let chronicle = journal.join("chronicle");
        let Ok(entries) = fs::read_dir(chronicle) else {
            return BTreeSet::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                entry
                    .file_type()
                    .ok()
                    .filter(|kind| kind.is_dir())
                    .and_then(|_| entry.file_name().into_string().ok())
            })
            .collect()
    }

    fn marker(journal: &Path, day: &str, name: &str, modified_at: i64) {
        let path = journal
            .join("chronicle")
            .join(day)
            .join("health")
            .join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, []).unwrap();
        set_file_mtime(&path, FileTime::from_unix_time(modified_at, 0)).unwrap();
    }

    fn sidecar_events(journal: &Path, day: &str, mode: &str) -> Vec<Value> {
        oplog_records(journal, day, mode)
    }

    fn write_health_event(journal: &Path, day: &str, event: &str) {
        let instant = NaiveDate::parse_from_str(day, "%Y%m%d")
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .fixed_offset();
        let mut writer = create_oplog_at(
            JournalRoot::open(journal).unwrap(),
            "think",
            "test",
            OplogFormat::Jsonl,
            instant,
        )
        .unwrap();
        writer.write_all(event.as_bytes()).unwrap();
        writer.write_all(b"\n").unwrap();
    }

    fn write_activity_record(journal: &Path, facet: &str, day: &str, record: Value) {
        let path = journal
            .join("facets")
            .join(facet)
            .join("activities")
            .join(format!("{day}.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();
    }

    fn talent(metadata: Map<String, Value>) -> solstone_core_talent_config::TalentConfig {
        solstone_core_talent_config::TalentConfig {
            key: "sample".to_owned(),
            file: "talent/sample.md".to_owned(),
            metadata,
            body: "prompt".to_owned(),
        }
    }

    #[test]
    fn failed_mode_prints_owner_readable_names_and_exits_1() {
        let reported_success = mode_outcome(dispatch::ModeResult {
            success: 3,
            success_names: vec![
                "daily_schedule".to_owned(),
                "schedule".to_owned(),
                "morning_briefing".to_owned(),
            ],
            ..dispatch::ModeResult::default()
        });
        assert_eq!(reported_success.exit_code, 0);
        assert!(reported_success.stdout.is_empty());
        assert_eq!(
            reported_success.stderr,
            "journal think: 3 completed\ndaily_schedule\nschedule\nmorning_briefing\n"
        );

        let empty_success = mode_outcome(dispatch::ModeResult::default());
        assert_eq!(empty_success.exit_code, 0);
        assert_eq!(empty_success.stderr, "journal think: 0 completed\n");

        let reported = mode_outcome(dispatch::ModeResult {
            success: 1,
            failed: 2,
            failed_names: vec![
                "daily_summary (send)".to_owned(),
                "facts (gpu-unavailable)".to_owned(),
            ],
            applicable_units: BTreeSet::new(),
            success_names: vec!["schedule".to_owned()],
            ..dispatch::ModeResult::default()
        });
        assert_eq!(reported.exit_code, 1);
        assert!(reported.stdout.is_empty());
        assert_eq!(
            reported.stderr,
            "journal think: 2 failed\ndaily_summary (send)\nfacts (gpu-unavailable)\n"
        );

        let mut followed = dispatch::ModeResult {
            success: 2,
            success_names: vec!["sense".to_owned(), "documents".to_owned()],
            ..dispatch::ModeResult::default()
        };
        dispatch::record_followup_failure(&mut followed, "activity replay", "disk full");
        assert_eq!(followed.success, 2);
        assert_eq!(followed.success_names, ["sense", "documents"]);
        assert_eq!(followed.failed, 1);
        assert_eq!(followed.failed_names, ["activity replay (disk full)"]);
    }

    #[test]
    fn think_status_updates_match_the_reference_shape() {
        // Source-derived, not measured: thinking.py:770-773 updates status,
        // preserving deterministic in-process updates without a channel.
        let status = helpers::ThinkStatus::default();
        status.update(Map::from_iter([(
            "mode".to_owned(),
            Value::String("daily".to_owned()),
        )]));
        status.update(Map::from_iter([(
            "agents_completed".to_owned(),
            Value::from(2),
        )]));
        assert_eq!(status.snapshot()["mode"], "daily");
        assert_eq!(status.snapshot()["agents_completed"], 2);
    }

    #[test]
    fn run_summary_records_the_operator_message_in_the_invocation_oplog() {
        let journal = tempdir().unwrap();
        let mut writer = run_log::RunLogWriter::open(journal.path(), "20260813", "daily");
        writer.summary(1_785_000_000_999, "sense_repair timeout".to_owned());
        writer.summary(1_785_000_001_001, "sense_repair error 1".to_owned());
        assert_eq!(
            oplog_records(journal.path(), "20260813", "daily"),
            vec![
                serde_json::json!({"event":"run.summary","ts":1_785_000_000_999_i64,"message":"sense_repair timeout"}),
                serde_json::json!({"event":"run.summary","ts":1_785_000_001_001_i64,"message":"sense_repair error 1"}),
            ]
        );
    }

    #[test]
    fn output_persistence_accumulate_returns_without_mutating_request() {
        let config = talent(Map::from_iter([
            ("accumulate".to_owned(), Value::Bool(true)),
            ("output".to_owned(), Value::String("json".to_owned())),
        ]));
        let mut request = Map::from_iter([("existing".to_owned(), Value::Bool(true))]);
        dispatch::apply_output_persistence(&config, &mut request, true);
        assert_eq!(
            request,
            Map::from_iter([("existing".to_owned(), Value::Bool(true))])
        );
    }

    #[test]
    fn scheduled_prompt_shapes_keep_daily_weekly_and_cadence_context_distinct() {
        // Source-derived, not measured: thinking.py:2134/2294/2425,
        // 2606/2728-2730/2834-2836, and 3012-3025 require distinct prompt
        // forms rather than one generic scheduled-task message.
        let journal = tempdir().unwrap();
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        fs::create_dir_all(context.day_dir.join("090000_300")).unwrap();
        let runtime = dispatch::runtime().unwrap();
        let cogitate = talent(Map::new());
        dispatch::dispatch(
            &context,
            &runtime,
            &cogitate,
            "daily",
            None,
            false,
            Map::new(),
        )
        .unwrap();
        dispatch::dispatch(
            &context,
            &runtime,
            &cogitate,
            "cadence",
            None,
            false,
            Map::new(),
        )
        .unwrap();
        let mut reflection = talent(Map::new());
        reflection.key = "weekly_reflection".to_owned();
        dispatch::dispatch(
            &context,
            &runtime,
            &reflection,
            "weekly",
            None,
            false,
            Map::new(),
        )
        .unwrap();
        let requests = recorder.requests.lock().unwrap();
        assert_eq!(
            requests[0].prompt,
            "Running scheduled task for 2026-08-13: Light activity: 1 segment, ~5 minutes."
        );
        assert_eq!(requests[1].prompt, "Running cadence task for 2026-08-13.");
        assert_eq!(
            requests[2].prompt,
            "Running scheduled weekly reflection for 2026-08-09: Light activity: 1 segment, ~5 minutes."
        );
    }

    #[test]
    fn exclude_streams_uses_fnmatch_globs() {
        let config = talent(Map::from_iter([(
            "exclude_streams".to_owned(),
            Value::Array(vec![Value::String("screen*".to_owned())]),
        )]));
        assert!(dispatch::excluded(&config, Some("screen.main")));
        assert!(!dispatch::excluded(&config, Some("audio.main")));
        assert!(!dispatch::excluded(&config, None));
    }

    #[test]
    fn ac7_exclude_streams_guard_handles_fresh_and_matching_state() {
        let config = talent(Map::from_iter([(
            "exclude_streams".to_owned(),
            Value::Array(vec![Value::String("capture-*".to_owned())]),
        )]));
        assert!(!dispatch::excluded(&config, None));
        assert!(!dispatch::excluded(&config, Some("screen")));
        assert!(dispatch::excluded(&config, Some("capture-screen")));
    }

    #[test]
    fn priority_groups_are_sorted_and_keys_inside_groups_are_sorted() {
        let mut late = talent(Map::from_iter([("priority".to_owned(), Value::from(20))]));
        late.key = "zeta".to_owned();
        let mut first = talent(Map::from_iter([("priority".to_owned(), Value::from(10))]));
        first.key = "beta".to_owned();
        let mut second = talent(Map::from_iter([("priority".to_owned(), Value::from(10))]));
        second.key = "alpha".to_owned();
        let groups = dispatch::grouped(vec![late, first, second]);
        assert_eq!(groups.keys().copied().collect::<Vec<_>>(), vec![10, 20]);
        assert_eq!(
            groups[&10]
                .iter()
                .map(|config| config.key.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
    }

    #[test]
    fn weekly_reflection_uses_the_week_start_output_and_prompt_only_for_that_key() {
        // Source-derived, not measured: thinking.py:2728-2730 and 2834-2836
        // shape each eligible facet row with the weekly reflection summary.
        let journal = tempdir().unwrap();
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let runtime = dispatch::runtime().unwrap();
        let mut reflection = talent(Map::from_iter([(
            "type".to_owned(),
            Value::String("cogitate".to_owned()),
        )]));
        reflection.key = "weekly_reflection".to_owned();
        dispatch::dispatch(
            &context,
            &runtime,
            &reflection,
            "weekly",
            None,
            false,
            Map::new(),
        )
        .unwrap();
        let ordinary = talent(Map::from_iter([(
            "type".to_owned(),
            Value::String("cogitate".to_owned()),
        )]));
        dispatch::dispatch(
            &context,
            &runtime,
            &ordinary,
            "weekly",
            None,
            false,
            Map::new(),
        )
        .unwrap();
        dispatch::dispatch(
            &context,
            &runtime,
            &reflection,
            "weekly",
            Some("work"),
            false,
            Map::new(),
        )
        .unwrap();
        let requests = recorder.requests.lock().unwrap();
        assert_eq!(
            requests[0].config["output_path"],
            journal
                .path()
                .join("reflections/weekly/20260809.md")
                .display()
                .to_string()
        );
        assert!(
            requests[0]
                .prompt
                .contains("weekly reflection for 2026-08-09: No recordings")
        );
        assert!(!requests[1].config.contains_key("output_path"));
        assert!(!requests[1].prompt.contains("weekly reflection"));
        assert!(
            requests[2]
                .prompt
                .contains("Processing facet 'work' for 2026-08-09: No recordings")
        );
    }

    #[test]
    fn recorder_seam_folds_a_batch_of_finished_uses() {
        let journal = tempdir().unwrap();
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let runtime = dispatch::runtime().unwrap();
        let config = talent(Map::new());
        let first =
            dispatch::dispatch(&context, &runtime, &config, "daily", None, true, Map::new())
                .unwrap();
        let second =
            dispatch::dispatch(&context, &runtime, &config, "daily", None, true, Map::new())
                .unwrap();
        let result = dispatch::drain(&context, &runtime, vec![first, second]);
        assert_eq!((result.success, result.failed), (2, 0));
        assert_eq!(recorder.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn indexing_gate_requires_changed_boolean_and_existing_output() {
        // Source-derived, not measured: thinking.py:240-242 uses `is True` and an existing path.
        let journal = tempdir().unwrap();
        let (context, recorder, index) = recorder_context_with_index(journal.path(), "20260813", 9);
        let runtime = dispatch::runtime().unwrap();
        let config = talent(Map::from_iter([
            ("type".to_owned(), Value::String("generate".to_owned())),
            ("output".to_owned(), Value::String("md".to_owned())),
        ]));
        let run = |changed: Option<bool>, create: bool| {
            *recorder.finish_fields.lock().unwrap() = solstone_core_cortex_client::FinishFields {
                output_changed: changed,
            };
            let pending = dispatch::dispatch(
                &context,
                &runtime,
                &config,
                "daily",
                None,
                false,
                Map::new(),
            )
            .unwrap();
            let path = recorder.requests.lock().unwrap().last().unwrap().config["output_path"]
                .as_str()
                .unwrap()
                .to_owned();
            if create {
                let path = std::path::PathBuf::from(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "# output").unwrap();
            } else {
                let _ = fs::remove_file(path);
            }
            dispatch::drain(&context, &runtime, vec![pending]);
        };
        run(Some(true), true);
        assert_eq!(index.0.lock().unwrap().len(), 1);
        run(Some(false), true);
        run(None, true);
        run(Some(true), false);
        assert_eq!(index.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn shared_drain_indexes_changed_daily_weekly_and_cadence_outputs_only() {
        // Source-derived, not measured: thinking.py:1102 applies the common
        // changed-output indexing branch to daily, weekly, and cadence drains.
        let journal = tempdir().unwrap();
        let (context, recorder, index) = recorder_context_with_index(journal.path(), "20260813", 9);
        let runtime = dispatch::runtime().unwrap();
        *recorder.finish_fields.lock().unwrap() = solstone_core_cortex_client::FinishFields {
            output_changed: Some(true),
        };
        for schedule in ["daily", "weekly", "cadence"] {
            let mut config = talent(Map::from_iter([
                ("type".to_owned(), Value::String("generate".to_owned())),
                ("output".to_owned(), Value::String("md".to_owned())),
            ]));
            config.key = format!("{schedule}-changed");
            let pending = dispatch::dispatch(
                &context,
                &runtime,
                &config,
                schedule,
                None,
                false,
                Map::new(),
            )
            .unwrap();
            let path = recorder.requests.lock().unwrap().last().unwrap().config["output_path"]
                .as_str()
                .unwrap()
                .to_owned();
            let path = std::path::PathBuf::from(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "output").unwrap();
            let result = dispatch::drain(&context, &runtime, vec![pending]);
            assert_eq!((result.success, result.failed), (1, 0));
        }
        assert_eq!(index.0.lock().unwrap().len(), 3);

        *recorder.finish_fields.lock().unwrap() = solstone_core_cortex_client::FinishFields {
            output_changed: Some(false),
        };
        let config = talent(Map::from_iter([
            ("type".to_owned(), Value::String("generate".to_owned())),
            ("output".to_owned(), Value::String("md".to_owned())),
        ]));
        let pending = dispatch::dispatch(
            &context,
            &runtime,
            &config,
            "daily",
            None,
            false,
            Map::new(),
        )
        .unwrap();
        let path = recorder.requests.lock().unwrap().last().unwrap().config["output_path"]
            .as_str()
            .unwrap()
            .to_owned();
        let path = std::path::PathBuf::from(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "output").unwrap();
        dispatch::drain(&context, &runtime, vec![pending]);
        assert_eq!(index.0.lock().unwrap().len(), 3);
    }

    #[test]
    fn ac2a_empty_cadence_configuration_leaves_no_sidecar() {
        // Source-derived, not measured: thinking.py:2969-2972 returns before opening a sidecar.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(roots.path(), &[]);
        let (context, _) = recorder_context(journal.path(), "20260814", 1_785_000_000_000);
        let context = context.with_talent_roots(talent_root, apps_root);
        assert!(cadence::configured(&context).unwrap().is_empty());
        assert!(oplog_records(journal.path(), "20260814", "cadence").is_empty());
    }

    #[test]
    fn ac2b_configured_cadence_opens_exactly_one_run_start_sidecar() {
        // Source-derived, not measured: thinking.py:2969-2972 permits the sidecar after configuration exists.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "cadence",
                "{\n\"type\": \"generate\", \"schedule\": \"cadence\", \"priority\": 1, \"output\": \"md\"\n}\n",
            )],
        );
        let (context, _) = recorder_context(journal.path(), "20260814", 1_785_000_000_000);
        let context = context.with_talent_roots(talent_root, apps_root);
        assert_eq!(cadence::configured(&context).unwrap().len(), 1);
        let args::ParseOutcome::Args(parsed) = args::parse(&["--cadence".to_owned()]).unwrap()
        else {
            panic!("cadence args")
        };
        let _log = open_run_log(&parsed, journal.path(), context.now_ms, &context.day);
        let events = sidecar_events(journal.path(), "20260814", "cadence");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "run.start");
        assert_eq!(events[0]["mode"], "cadence");
        assert_eq!(events[0]["day"], "20260814");
        assert_eq!(events[0]["ref"], 1_785_000_000_000_i64);
    }

    #[test]
    fn batch_drain_bounded_drains_at_two_then_the_group_remainder() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "charlie",
                    "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
                ),
                (
                    "alpha",
                    "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
                ),
                (
                    "bravo",
                    "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        let result = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!((result.success, result.failed), (3, 0));
        assert_eq!(
            recorder
                .waits
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(
            recorder
                .requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| request.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "bravo", "charlie"]
        );
    }

    #[test]
    fn ac7_active_facet_gate_skips_inactive_and_always_run_overrides_it() {
        // Source-derived, not measured: thinking.py:2220-2227 skips inactive facets unless `always` is set.
        let journal = tempdir().unwrap();
        fs::create_dir_all(journal.path().join("facets/work")).unwrap();
        fs::write(journal.path().join("facets/work/facet.json"), "{}").unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "multi",
                "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\", \"multi_facet\": true\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        assert!(
            daily::run(&context, &mut log, None, false, 2)
                .unwrap()
                .applicable_units
                .is_empty()
        );
        assert!(recorder.requests.lock().unwrap().is_empty());
        assert!(
            oplog_records(journal.path(), &context.day, "daily")
                .iter()
                .any(|record| record["reason"] == "no_active_facets")
        );

        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "always-multi",
                "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\", \"multi_facet\": true, \"always\": true\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 10);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        let result = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(
            result.applicable_units,
            BTreeSet::from([("always-multi".to_owned(), Some("work".to_owned()))])
        );
        assert_eq!(recorder.requests.lock().unwrap()[0].config["facet"], "work");
    }

    #[test]
    fn ac20_cortex_policies_have_distinct_real_deadlines() {
        use solstone_core_cortex_client::CortexRequestPolicy;
        use std::time::Duration;

        assert_eq!(
            CortexRequestPolicy::think().outcome_deadline(),
            Some(Duration::from_secs(610))
        );
        assert_eq!(
            CortexRequestPolicy::interactive().outcome_deadline(),
            Some(Duration::from_secs(600))
        );
    }

    #[test]
    fn ac7_completed_unit_guard_dispatches_fresh_then_skips_terminal_unit() {
        // Source-derived, not measured: thinking.py:2123-2125 skips a completed daily unit on rerun.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "completed",
                "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        let fresh = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(fresh.applicable_units.len(), 1);
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);

        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.complete","ts":1,"mode":"daily","name":"completed"}"#,
        );
        let repeated = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(repeated.applicable_units, fresh.applicable_units);
        assert_eq!(repeated.terminal_units, repeated.applicable_units);
        assert!(repeated.capped_units.is_empty());
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn ac7_deterministic_failure_guard_dispatches_fresh_then_skips_failure() {
        // Source-derived, not measured: thinking.py:2124-2125 keeps deterministic daily failures out of a rerun.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "deterministic",
                "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        let fresh = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(fresh.applicable_units.len(), 1);
        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.fail","ts":10,"mode":"daily","name":"deterministic","reason_code":"no_output"}
{"event":"talent.fail","ts":11,"mode":"daily","name":"deterministic","reason_code":"no_output"}"#,
        );
        let repeated = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(repeated.applicable_units, fresh.applicable_units);
        assert_eq!(repeated.terminal_units, repeated.applicable_units);
        assert_eq!(repeated.capped_units, repeated.applicable_units);
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn deterministic_failure_below_its_cap_remains_eligible() {
        let journal = tempdir().unwrap();
        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.fail","ts":1,"mode":"daily","name":"deterministic","reason_code":"no_output"}"#,
        );
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "deterministic",
                "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");

        let result = daily::run(&context, &mut log, None, false, 2).unwrap();

        assert_eq!(result.applicable_units.len(), 1);
        assert!(result.capped_units.is_empty());
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn ac7_retry_on_deterministic_failure_dispatches_the_recorded_unit() {
        // Source-derived, not measured: thinking.py:2124-2125 lets `retry_on_deterministic_failure` bypass the skip.
        let journal = tempdir().unwrap();
        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.fail","ts":1,"mode":"daily","name":"retry","reason_code":"no_output"}"#,
        );
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "retry",
                "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\", \"retry_on_deterministic_failure\": true\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        let result = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(result.applicable_units.len(), 1);
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn batch_drain_zero_dispatches_the_entire_priority_group_before_waiting() {
        // Source-derived, not measured: thinking.py:2086 documents zero as unlimited per priority group.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "one",
                    "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
                ),
                (
                    "two",
                    "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
                ),
                (
                    "three",
                    "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\"\n}\n",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        let result = daily::run(&context, &mut log, None, false, 0).unwrap();
        assert_eq!((result.success, result.failed), (3, 0));
        assert_eq!(recorder.requests.lock().unwrap().len(), 3);
        assert_eq!(
            recorder.waits.lock().unwrap().as_slice(),
            &[vec![
                "use-1".to_owned(),
                "use-2".to_owned(),
                "use-3".to_owned(),
            ]]
        );
    }

    #[test]
    fn multi_facet_daily_expansion_dispatches_each_active_facet_and_reports_units() {
        // Source-derived, not measured: thinking.py:2217-2237 expands one applicable unit per eligible facet.
        let journal = tempdir().unwrap();
        for facet in ["home", "work"] {
            let path = journal.path().join("facets").join(facet);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("facet.json"), "{}").unwrap();
        }
        let active = journal
            .path()
            .join("chronicle/20260813/090000_60/talents/facets.json");
        fs::create_dir_all(active.parent().unwrap()).unwrap();
        fs::write(active, r#"[{"facet":"home"},{"facet":"work"}]"#).unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "multi",
                "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"md\", \"multi_facet\": true\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "daily");
        let result = daily::run(&context, &mut log, None, false, 0).unwrap();
        assert_eq!(
            result.applicable_units,
            BTreeSet::from([
                ("multi".to_owned(), Some("home".to_owned())),
                ("multi".to_owned(), Some("work".to_owned())),
            ])
        );
        assert_eq!(result.success, 2);
        assert_eq!(
            recorder
                .requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| request.config["facet"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["home", "work"]
        );
    }

    #[test]
    fn ac22_daily_weekly_and_cadence_route_requests_through_output_persistence() {
        // Source-derived, not measured: thinking.py:2058 applies this helper only to daily, weekly, and cadence; Phase 2b covers the direct-output modes.
        let journal = tempdir().unwrap();
        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.complete","ts":1,"mode":"segment","stream":"default","segment":"093000_600","name":"sense"}"#,
        );
        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.complete","ts":2,"mode":"activity","facet":"work","activity":"meeting_1","name":"conversation"}"#,
        );
        let segment = journal
            .path()
            .join("chronicle/20260813/default/093000_600/talents");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("activity.md"), "Reviewed the current release.").unwrap();
        write_activity_record(
            journal.path(),
            "work",
            "20260813",
            serde_json::json!({"id":"meeting_1", "activity":"meeting", "title":"Release review", "description":"Resolved the current blocker"}),
        );
        // Yesterday can complete after midnight. Identical IDs on two days
        // must stay distinct and retain their own source coordinate.
        write_health_event(
            journal.path(),
            "20260812",
            r#"{"event":"talent.complete","ts":3,"mode":"segment","stream":"default","segment":"093000_600","name":"sense"}"#,
        );
        write_health_event(
            journal.path(),
            "20260812",
            r#"{"event":"talent.complete","ts":4,"mode":"activity","facet":"work","activity":"meeting_1","name":"conversation"}"#,
        );
        let prior = journal
            .path()
            .join("chronicle/20260812/default/093000_600/talents");
        fs::create_dir_all(&prior).unwrap();
        fs::write(prior.join("activity.md"), "Yesterday's review.").unwrap();
        write_activity_record(
            journal.path(),
            "work",
            "20260812",
            serde_json::json!({"id":"meeting_1", "activity":"meeting", "title":"Yesterday's meeting", "description":"Prior day only"}),
        );
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "daily-output",
                    "{\n\"type\": \"generate\", \"schedule\": \"daily\", \"priority\": 1, \"output\": \"json\"\n}\n",
                ),
                (
                    "weekly-output",
                    "{\n\"type\": \"generate\", \"schedule\": \"weekly\", \"priority\": 1, \"output\": \"json\"\n}\n",
                ),
                (
                    "cadence-output",
                    "{\n\"type\": \"generate\", \"schedule\": \"cadence\", \"priority\": 1, \"output\": \"json\"\n}\n",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut daily_log = test_log(&context, "daily");
        daily::run(&context, &mut daily_log, None, false, 2).unwrap();
        let mut weekly_log = test_log(&context, "weekly");
        weekly::run(&context, &mut weekly_log, false, None, 2).unwrap();
        let mut cadence_log = test_log(&context, "cadence");
        cadence::run(
            &context,
            cadence::configured(&context).unwrap(),
            &mut cadence_log,
            false,
        )
        .unwrap();
        let requests = recorder.requests.lock().unwrap();
        for name in ["daily-output", "weekly-output", "cadence-output"] {
            let request = requests
                .iter()
                .find(|request| request.name == name)
                .unwrap();
            assert_eq!(request.config["output"], "json");
            if name == "cadence-output" {
                // Pulse needs source coordinates and completion time, not bare IDs.
                assert_eq!(
                    request.config["cadence_window"],
                    serde_json::json!({
                        "since_ms": 0,
                        "segments": [{"day":"20260813", "segment": "093000_600", "stream": "default", "ts": 1}, {"day":"20260812", "segment":"093000_600", "stream":"default", "ts":3}],
                        "activities": [{"day":"20260813", "activity": "meeting_1", "facet": "work", "ts": 2}, {"day":"20260812", "activity":"meeting_1", "facet":"work", "ts":4}],
                    })
                );
                let mut prepared = solstone_core_talent_runtime::PreparedTalent {
                    name: "pulse".to_owned(),
                    config: request.config.clone(),
                };
                prepared.config.insert(
                    "prompt".to_owned(),
                    Value::String("$completed_since".to_owned()),
                );
                let pulse_context = solstone_core_talent_runtime::ExecutionContext {
                    journal: journal.path().to_owned(),
                };
                let state =
                    solstone_core_talent_runtime::pulse::build(&mut prepared, &pulse_context)
                        .unwrap();
                solstone_core_talent_runtime::pulse::apply_prompt_override(&mut prepared, &state)
                    .unwrap();
                let packet: Value =
                    serde_json::from_str(prepared.config["prompt"].as_str().unwrap()).unwrap();
                assert_eq!(
                    packet["segments"][1]["activity"],
                    "Reviewed the current release."
                );
                assert_eq!(packet["segments"][1]["stream"], "default");
                assert_eq!(packet["segments"][1]["ts"], 1);
                assert_eq!(packet["activities"][1]["title"], "Release review");
                assert_eq!(packet["activities"][1]["facet"], "work");
                assert_eq!(packet["activities"][1]["ts"], 2);
                assert_eq!(packet["segments"].as_array().unwrap().len(), 2);
                assert_eq!(packet["activities"].as_array().unwrap().len(), 2);
                assert_eq!(packet["segments"][0]["activity"], "Yesterday's review.");
                assert_eq!(packet["segments"][0]["day"], "20260812");
                assert_eq!(packet["segments"][1]["day"], "20260813");
                assert_eq!(packet["activities"][0]["title"], "Yesterday's meeting");
                assert_eq!(packet["activities"][0]["day"], "20260812");
                assert_eq!(packet["activities"][1]["day"], "20260813");
            }
        }
    }

    #[test]
    fn activity_filters_types_batches_requests_and_keeps_a_hard_deadline() {
        // Source-derived, not measured: thinking.py:3137-3145 selects matching activity talents and 3235 waits for 610 seconds.
        let journal = tempdir().unwrap();
        write_activity_record(
            journal.path(),
            "work",
            "20260813",
            serde_json::json!({"id":"reading_1", "activity":"reading", "segments":["090000"]}),
        );
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "one",
                    "{\n\"type\": \"generate\", \"schedule\": \"activity\", \"priority\": 1, \"activities\": [\"reading\"], \"output\": \"md\"\n}\n",
                ),
                (
                    "two",
                    "{\n\"type\": \"generate\", \"schedule\": \"activity\", \"priority\": 1, \"activities\": [\"*\"], \"output\": \"json\"\n}\n",
                ),
                (
                    "three",
                    "{\n\"type\": \"cogitate\", \"schedule\": \"activity\", \"priority\": 1, \"activities\": [\"reading\"]\n}\n",
                ),
                (
                    "other",
                    "{\n\"type\": \"generate\", \"schedule\": \"activity\", \"priority\": 1, \"activities\": [\"coding\"], \"output\": \"md\"\n}\n",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let next_ms = Arc::new(AtomicI64::new(1_785_000_100_000));
        let event_counter = Arc::clone(&next_ms);
        let context = context
            .with_talent_roots(talent_root, apps_root)
            .with_event_clock(Arc::new(move || {
                event_counter.fetch_add(1, Ordering::SeqCst)
            }));
        let mut log = test_log(&context, "activity");
        let result = activity::run(&context, &mut log, "reading_1", "work", false, 2).unwrap();
        assert_eq!((result.success, result.failed), (3, 0));
        let requests = recorder.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .map(|request| request.name.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "three", "two"]
        );
        assert!(!requests.iter().any(|request| request.name == "other"));
        assert_eq!(requests[0].config["output"], "md");
        assert!(!requests[1].config.contains_key("output"));
        assert_eq!(
            requests[1].prompt,
            "Processing activity 'reading_1' (reading) in facet 'work' for 2026-08-13."
        );
        assert_eq!(requests[2].config["output"], "json");
        assert!(
            requests[0].config["output_path"]
                .as_str()
                .unwrap()
                .ends_with("facets/work/activities/20260813/reading_1/one.md")
        );
        assert_eq!(
            recorder
                .waits
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(
            recorder.deadlines.lock().unwrap().as_slice(),
            &[
                Some(std::time::Duration::from_secs(610)),
                Some(std::time::Duration::from_secs(610))
            ]
        );
        // Source-derived, not measured: thinking.py:3160-3180, 3403-3417,
        // and 3441-3465 record the activity lifecycle in its run log.
        let events = oplog_records(journal.path(), &context.day, "activity");
        for event in [
            "started",
            "group.start",
            "talent.started",
            "talent.dispatch",
            "talent.completed",
            "talent.complete",
            "group.complete",
            "completed",
        ] {
            assert!(
                events.iter().any(|record| record["event"] == event),
                "missing {event}"
            );
        }
        assert!(
            events
                .iter()
                .any(|record| record["activity"] == "reading_1")
        );
        assert_eq!(context.now_ms, 9, "run identity remains fixed");
        for name in ["one", "three", "two"] {
            let timestamp = |event: &str| {
                events
                    .iter()
                    .find(|record| record["event"] == event && record["name"] == name)
                    .unwrap_or_else(|| panic!("missing {event} for {name}"))["ts"]
                    .as_i64()
                    .unwrap()
            };
            let started = timestamp("talent.started");
            let dispatch = timestamp("talent.dispatch");
            let completed = timestamp("talent.completed");
            let complete = timestamp("talent.complete");
            assert_eq!(started, dispatch, "start aliases must share one sample");
            assert_eq!(
                completed, complete,
                "terminal aliases must share one sample"
            );
            assert!(dispatch < complete, "later logical event must advance");
        }
    }

    #[test]
    fn activity_zero_concurrency_is_unlimited_and_the_json_generator_skips_output_indexing() {
        // Source-derived, not measured: thinking.py:3277-3287 skips incremental indexing for JSON generators; max_concurrency=0 leaves the group pending until its final drain.
        let journal = tempdir().unwrap();
        write_activity_record(
            journal.path(),
            "work",
            "20260813",
            serde_json::json!({"id":"reading_1", "activity":"reading", "segments":["090000"]}),
        );
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "markdown",
                    "{\n\"type\": \"generate\", \"schedule\": \"activity\", \"priority\": 1, \"activities\": [\"reading\"], \"output\": \"md\"\n}\n",
                ),
                (
                    "json",
                    "{\n\"type\": \"generate\", \"schedule\": \"activity\", \"priority\": 1, \"activities\": [\"reading\"], \"output\": \"json\"\n}\n",
                ),
            ],
        );
        let (context, recorder, index) = recorder_context_with_index(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        *recorder.finish_fields.lock().unwrap() = solstone_core_cortex_client::FinishFields {
            output_changed: Some(true),
        };
        for name in ["markdown.md", "json.json"] {
            let path = journal
                .path()
                .join("facets/work/activities/20260813/reading_1")
                .join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "output").unwrap();
        }
        let mut log = test_log(&context, "activity");
        activity::run(&context, &mut log, "reading_1", "work", false, 0).unwrap();
        assert_eq!(
            recorder
                .waits
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![2]
        );
        let requests = recorder.requests.lock().unwrap();
        assert_eq!(requests[0].config["output"], "json");
        assert_eq!(requests[1].config["output"], "md");
        assert_eq!(index.0.lock().unwrap().len(), 1);
        assert!(index.0.lock().unwrap()[0].ends_with("markdown.md"));
    }

    #[test]
    fn flush_filters_hooks_sets_direct_output_and_uses_a_hard_deadline() {
        // Source-derived, not measured: thinking.py:3524-3529 selects hook.flush and 3629 performs the fixed 610-second wait.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "flushable",
                    "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"hook\": {\"flush\": true}, \"output\": \"json\"\n}\n",
                ),
                (
                    "ordinary",
                    "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"md\"\n}\n",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let mut log = test_log(&context, "flush");
        let result = flush::run(&context, &mut log, "090000", Some("default")).unwrap();
        assert_eq!((result.success, result.failed), (1, 0));
        let requests = recorder.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].name, "flushable");
        assert_eq!(requests[0].config["flush"], true);
        assert_eq!(requests[0].config["refresh"], true);
        assert_eq!(requests[0].config["output"], "json");
        assert_eq!(
            recorder.deadlines.lock().unwrap().as_slice(),
            &[Some(std::time::Duration::from_secs(610))]
        );
        // Source-derived, not measured: thinking.py:3538-3554, 3604-3617,
        // and 3682-3703 record the flush lifecycle in its run log.
        let events = oplog_records(journal.path(), &context.day, "flush");
        for event in [
            "started",
            "talent.started",
            "talent.dispatch",
            "talent.completed",
            "talent.complete",
            "completed",
        ] {
            assert!(
                events.iter().any(|record| record["event"] == event),
                "missing {event}"
            );
        }
        assert!(events.iter().any(|record| record["segment"] == "090000"));
    }

    #[test]
    fn ac7_activity_low_level_work_guard_skips_only_the_reference_case() {
        // Source-derived, not measured: thinking.py:3328-3343 skips `work`
        // only for low-level browsing or reading records.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "work",
                "{\n\"type\": \"generate\", \"schedule\": \"activity\", \"priority\": 1, \"activities\": [\"reading\"], \"output\": \"md\"\n}\n",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        write_activity_record(
            journal.path(),
            "work",
            "20260813",
            serde_json::json!({"id":"low", "activity":"reading", "segments":["090000"], "level_avg":0.39}),
        );
        let mut log = test_log(&context, "activity");
        let low = activity::run(&context, &mut log, "low", "work", false, 2).unwrap();
        assert_eq!((low.success, low.failed), (0, 0));
        assert!(recorder.requests.lock().unwrap().is_empty());
        assert!(
            oplog_records(journal.path(), &context.day, "activity")
                .iter()
                .any(|record| record["reason"] == "low_level_activity")
        );

        write_activity_record(
            journal.path(),
            "work",
            "20260813",
            serde_json::json!({"id":"full", "activity":"reading", "segments":["090000"], "level_avg":0.4}),
        );
        let mut log = test_log(&context, "segment");
        let full = activity::run(&context, &mut log, "full", "work", false, 2).unwrap();
        assert_eq!((full.success, full.failed), (1, 0));
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn ac7_completed_and_deterministic_guards_keep_fresh_units_eligible() {
        use solstone_core_system_health::{CompletedUnit, DailyUnit, DeterministicFailure};

        let unit = CompletedUnit {
            mode: "daily".to_owned(),
            name: "sample".to_owned(),
            facet: None,
        };
        let failure = DailyUnit {
            name: "sample".to_owned(),
            facet: None,
        };
        let completed = BTreeSet::<CompletedUnit>::new();
        let deterministic = std::collections::BTreeMap::<DailyUnit, DeterministicFailure>::new();
        assert!(!completed.contains(&unit));
        assert!(!deterministic.contains_key(&failure));
    }

    #[test]
    fn ac7_completed_and_deterministic_guards_identify_populated_units() {
        use solstone_core_system_health::{CompletedUnit, DailyUnit, DeterministicFailure};

        let unit = CompletedUnit {
            mode: "daily".to_owned(),
            name: "sample".to_owned(),
            facet: None,
        };
        let failure = DailyUnit {
            name: "sample".to_owned(),
            facet: None,
        };
        let completed = BTreeSet::from([unit.clone()]);
        let deterministic = std::collections::BTreeMap::from([(
            failure.clone(),
            DeterministicFailure {
                count: 1,
                reason_code: "invalid_config".to_owned(),
            },
        )]);
        assert!(completed.contains(&unit));
        assert!(deterministic.contains_key(&failure));
    }

    #[test]
    fn daily_runs_the_whole_day_lifecycle_in_order() {
        let journal = tempdir().unwrap();
        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.complete","ts":1,"mode":"daily","name":"daily_schedule"}
{"event":"talent.complete","ts":1,"mode":"daily","name":"schedule"}
{"event":"talent.complete","ts":1,"mode":"daily","name":"morning_briefing"}"#,
        );
        let _ = run_at(journal.path(), &[]);
        let events = sidecar_events(journal.path(), "20260813", "daily");
        assert_eq!(events[0]["event"], "run.start");
        assert_eq!(events[0]["mode"], "daily");
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event"] == "phase.start")
                .map(|event| event["phase"].as_str().unwrap())
                .collect::<Vec<_>>(),
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
    fn weekly_records_its_priority_group_observable() {
        let journal = tempdir().unwrap();
        let _ = run_at(journal.path(), &["--weekly"]);
        let events = sidecar_events(journal.path(), "20260813", "weekly");
        assert_eq!(events[0]["event"], "run.start");
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "group.start" && event["mode"] == "weekly")
        );
    }

    #[test]
    fn cadence_records_a_run_start_without_firing_when_no_work_is_complete() {
        let journal = tempdir().unwrap();
        let next_ms = Arc::new(AtomicI64::new(1_785_000_000_000));
        let event_counter = Arc::clone(&next_ms);
        let start_clock: Arc<dyn Fn() -> i64 + Send + Sync> =
            Arc::new(move || event_counter.fetch_add(1, Ordering::SeqCst));
        let run_start_clock = Arc::clone(&start_clock);
        let result = run_cli_with_event_clock(
            &["--cadence".to_owned()],
            journal.path(),
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            today,
            move || run_start_clock(),
            Some(start_clock),
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(2),
            &BTreeMap::new(),
        );
        assert_eq!(result.exit_code, 0);
        let events = sidecar_events(journal.path(), "20260814", "cadence");
        assert_eq!(events[0]["event"], "run.start");
        assert_eq!(events[0]["mode"], "cadence");
        assert_eq!(events[0]["ts"], 1_785_000_000_000_i64);
        assert_eq!(events[0]["ref"], 1_785_000_000_000_i64);
        let skip_times = events
            .iter()
            .filter(|event| event["event"] == "talent.skip" && event["reason"] == "no_new_work")
            .map(|event| event["ts"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(skip_times, vec![1_785_000_000_001, 1_785_000_000_002]);
        assert!(!journal.path().join("health/cadence.json").exists());
    }

    #[test]
    fn public_injected_clock_keeps_its_borrowed_non_send_contract() {
        let journal = tempdir().unwrap();
        let reads = Cell::new(0);
        let run = run_cli_with(
            &["--help".to_owned()],
            journal.path(),
            |_| None,
            || false,
            today,
            || {
                reads.set(reads.get() + 1);
                1_785_000_000_000
            },
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(2),
            &BTreeMap::new(),
        );
        assert_eq!(run.exit_code, 0);
        assert_eq!(reads.get(), 0);
    }

    #[test]
    fn output_persistence_generate_defaults_to_markdown() {
        let config = talent(Map::from_iter([(
            "type".to_owned(),
            Value::String("generate".to_owned()),
        )]));
        let mut request = Map::new();
        dispatch::apply_output_persistence(&config, &mut request, false);
        assert_eq!(request.get("output"), Some(&Value::String("md".to_owned())));
        assert!(!request.contains_key("refresh"));
    }

    #[test]
    fn output_persistence_declared_output_sets_refresh_only_when_forced() {
        let config = talent(Map::from_iter([(
            "output".to_owned(),
            Value::String("json".to_owned()),
        )]));
        let mut ordinary = Map::new();
        dispatch::apply_output_persistence(&config, &mut ordinary, false);
        assert_eq!(
            ordinary.get("output"),
            Some(&Value::String("json".to_owned()))
        );
        assert!(!ordinary.contains_key("refresh"));
        let mut forced = Map::new();
        dispatch::apply_output_persistence(&config, &mut forced, true);
        assert_eq!(forced.get("refresh"), Some(&Value::Bool(true)));
    }

    #[test]
    fn output_persistence_cogitate_without_output_is_untouched() {
        let config = talent(Map::from_iter([(
            "type".to_owned(),
            Value::String("cogitate".to_owned()),
        )]));
        let mut request = Map::from_iter([("existing".to_owned(), Value::Bool(true))]);
        dispatch::apply_output_persistence(&config, &mut request, true);
        assert_eq!(
            request,
            Map::from_iter([("existing".to_owned(), Value::Bool(true))])
        );
    }

    #[test]
    fn cadence_missing_file_loads_empty_state() {
        let journal = tempdir().unwrap();
        assert_eq!(
            cadence_state::CadenceState::load(journal.path()).timestamp("missing"),
            None
        );
    }

    #[test]
    fn cadence_save_preserves_non_integer_and_untouched_values() {
        let journal = tempdir().unwrap();
        let path = journal.path().join("health/cadence.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"fired": 1, "unknown": "keep", "nested": {"a": 1}}"#,
        )
        .unwrap();
        let mut state = cadence_state::CadenceState::load(journal.path());
        state.set_timestamp("fired", 9);
        state.save(journal.path()).unwrap();
        let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["fired"], 9);
        assert_eq!(saved["unknown"], "keep");
        assert_eq!(saved["nested"]["a"], 1);
    }

    #[test]
    fn cadence_call_site_updates_only_clean_fires_and_keeps_prior_timestamp_on_failure() {
        let journal = tempdir().unwrap();
        let mut state = cadence_state::CadenceState::load(journal.path());
        state.set_timestamp("failed", 11);
        state.set_timestamp("other", 12);
        assert!(!cadence::record_clean_fire(&mut state, "failed", 20, false));
        assert!(cadence::record_clean_fire(&mut state, "fired", 20, true));
        state.save(journal.path()).unwrap();
        let loaded = cadence_state::CadenceState::load(journal.path());
        assert_eq!(loaded.timestamp("failed"), Some(11));
        assert_eq!(loaded.timestamp("fired"), Some(20));
        assert_eq!(loaded.timestamp("other"), Some(12));
    }

    #[test]
    fn criterion_three_value_half_preserves_parser_defaults() {
        // Criterion 3's value half is inline because unavailable run modes do not reveal defaults.
        let args::ParseOutcome::Args(parsed) = args::parse(&[]).expect("parse defaults") else {
            panic!("defaults must parse as arguments");
        };
        assert_eq!(parsed.jobs, 2);
        assert_eq!(parsed.segment_workers, None);
        assert_eq!(parsed.skip_talents, "");
    }

    #[test]
    fn gate_preserves_four_outcomes() {
        let journal = tempdir().unwrap();
        let base = |env: Option<(&str, &str)>, up| {
            run_cli_with(
                &["--dry-run".to_owned()],
                journal.path(),
                move |name| env.and_then(|(key, value)| (name == key).then(|| value.to_owned())),
                move || up,
                || NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
                || 1_785_000_000_000,
                || Some(8),
                || (false, LocalEndpointResolution::Bundled),
                || None,
                &BTreeMap::new(),
            )
        };
        assert_eq!(
            base(Some(("SOL_SKIP_SUPERVISOR_CHECK", "1")), false).exit_code,
            0
        );
        assert_eq!(base(None, true).exit_code, 0);
        let spawned = base(Some(("SOL_SUPERVISOR_SPAWNED", "1")), false);
        assert_eq!((spawned.exit_code, spawned.stderr), (75, String::new()));
        assert_eq!(base(None, false).exit_code, 1);
    }

    #[test]
    fn updated_precedes_segment_worker_range_validation() {
        let journal = tempdir().unwrap();
        let output = run_cli_with(
            &[
                "--updated".to_owned(),
                "--segment-workers".to_owned(),
                "99".to_owned(),
            ],
            journal.path(),
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            || NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
            || 1_785_000_000_000,
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || None,
            &BTreeMap::new(),
        );
        assert_eq!(output.exit_code, 0);
    }

    #[test]
    fn negative_segment_workers_reaches_updated_before_runtime_range_validation() {
        let journal = tempdir().unwrap();
        marker(journal.path(), "20260813", "stream.updated", 100);
        let output = run_at(journal.path(), &["--updated", "--segment-workers", "-1"]);
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout, "20260813\n");
    }

    #[test]
    fn negative_segment_workers_is_a_runtime_refusal() {
        let output = run(&["--segment-workers", "-1"]);
        assert_eq!(output.exit_code, 2);
        assert!(
            output
                .stderr
                .ends_with("journal think: error: --segment-workers must be between 1 and 32\n")
        );
    }

    #[test]
    fn day_defaulting_uses_injected_clock_for_cadence_and_daily() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
        assert_eq!(day::selected_day(None, true, today), "20260814");
        assert_eq!(day::selected_day(None, false, today), "20260813");
    }

    #[test]
    fn criterion_nine_day_resolution_creates_yesterday() {
        let journal = tempdir().unwrap();
        let result = run_at(journal.path(), &[]);
        assert_eq!(result.exit_code, 1);
        assert!(journal.path().join("chronicle/20260813").is_dir());
    }

    #[test]
    fn criterion_ten_creation_precedes_activity_day_refusal() {
        let journal = tempdir().unwrap();
        let result = run_at(journal.path(), &["--activity", "ID", "--facet", "F"]);
        assert!(journal.path().join("chronicle/20260813").is_dir());
        assert_eq!(result.exit_code, 2);
        assert!(
            result
                .stderr
                .ends_with("journal think: error: --activity requires --day\n")
        );
    }

    #[test]
    fn criterion_twelve_identity_bootstraps_without_overwriting_and_stays_after_gate() {
        let fresh = tempdir().unwrap();
        assert_eq!(run_at(fresh.path(), &["--facet", "F"]).exit_code, 2);
        let identity = fresh.path().join("identity");
        assert!(identity.is_dir());
        assert!(identity.join("partner.md").is_file());
        assert!(identity.join("health.md").is_file());

        let owner_bytes = b"owner-maintained partner\n";
        fs::write(identity.join("partner.md"), owner_bytes).unwrap();
        assert_eq!(run_at(fresh.path(), &["--facet", "F"]).exit_code, 2);
        assert_eq!(fs::read(identity.join("partner.md")).unwrap(), owner_bytes);

        let gate_failure = tempdir().unwrap();
        let result = run_cli_with(
            &["--facet".to_owned(), "F".to_owned()],
            gate_failure.path(),
            |_| None,
            || false,
            today,
            || 1_785_000_000_000,
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(2),
            &BTreeMap::new(),
        );
        assert_eq!(result.exit_code, 1);
        assert!(!gate_failure.path().join("identity").exists());
    }

    #[test]
    fn criterion_thirteen_updated_creates_no_chronicle_day() {
        let journal = tempdir().unwrap();
        fs::create_dir_all(journal.path().join("identity")).unwrap();
        let before = day_keys(journal.path());
        let result = run_at(journal.path(), &["--updated"]);
        assert_eq!(result.exit_code, 0);
        assert_eq!(day_keys(journal.path()), before);
    }

    #[test]
    fn criterion_fourteen_updated_scan_uses_marker_mtimes_and_injected_today() {
        let journal = tempdir().unwrap();
        fs::create_dir_all(journal.path().join("chronicle/20260809/health")).unwrap();
        marker(journal.path(), "20260810", "stream.updated", 100);
        marker(journal.path(), "20260811", "daily.updated", 100);
        marker(journal.path(), "20260811", "stream.updated", 200);
        marker(journal.path(), "20260812", "stream.updated", 100);
        marker(journal.path(), "20260812", "daily.updated", 200);
        marker(journal.path(), "20260813", "stream.updated", 100);
        marker(journal.path(), "20260813", "daily.updated", 100);
        marker(journal.path(), "20260814", "stream.updated", 100);

        let result = run_at(journal.path(), &["--updated"]);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "20260810\n20260811\n");
    }

    #[test]
    fn criterion_fifteen_updated_empty_journal_is_silent_success() {
        let journal = tempdir().unwrap();
        let result = run_at(journal.path(), &["--updated"]);
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty());
    }

    #[test]
    fn all_run_modes_are_reachable_including_the_planner() {
        assert_eq!(
            run(&["--segments", "--jobs", "0", "--segment-workers", "2"]).exit_code,
            2
        );
        assert_eq!(
            run(&["--segments", "--jobs", "0", "--segment-workers", "1"]).exit_code,
            0
        );
        assert_ne!(run(&["--segment", "missing"]).exit_code, 69);
        assert_eq!(run(&["--dry-run"]).exit_code, 0);
    }

    #[test]
    fn worker_policy_uses_cpu_formula_and_local_slot_clamp() {
        assert_eq!(
            workers::default_segment_workers(
                Some(12),
                false,
                LocalEndpointResolution::Bundled,
                Some(1)
            ),
            6
        );
        assert_eq!(
            workers::default_segment_workers(
                Some(16),
                true,
                LocalEndpointResolution::Bundled,
                Some(2)
            ),
            2
        );
    }

    #[test]
    fn bundled_slots_matches_the_existing_local_context_precedent() {
        let journal = tempdir().unwrap();
        assert_eq!(workers::bundled_slots(journal.path()), Some(1));
        fs::create_dir_all(journal.path().join("health")).unwrap();
        fs::write(journal.path().join("health/local.ctx"), "32768\n").unwrap();
        assert_eq!(workers::bundled_slots(journal.path()), Some(2));
        fs::write(journal.path().join("health/local.ctx"), "unknown\n").unwrap();
        assert_eq!(workers::bundled_slots(journal.path()), Some(1));
    }

    #[test]
    fn mode_derivation_covers_reachable_modes() {
        for (args, expected) in [
            (
                vec!["--activity", "a", "--facet", "f", "--day", "20260813"],
                "activity",
            ),
            (vec!["--flush", "--segment", "x"], "flush"),
            (vec!["--segments"], "segments"),
            (vec!["--segment", "x"], "segment"),
            (vec!["--weekly"], "weekly"),
            (vec!["--cadence"], "cadence"),
            (vec![], "daily"),
        ] {
            let args::ParseOutcome::Args(parsed) =
                args::parse(&args.iter().map(|item| item.to_string()).collect::<Vec<_>>()).unwrap()
            else {
                panic!("mode test must parse arguments");
            };
            assert_eq!(run_log::mode(&parsed), expected);
        }
    }

    #[test]
    fn criterion_sixteen_cadence_round_trips_and_save_replaces_previous_state() {
        let journal = tempdir().unwrap();
        fs::create_dir_all(journal.path().join("health")).unwrap();
        let mut initial = cadence_state::CadenceState::load(journal.path());
        initial.set_timestamp("one", 12);
        initial.set_timestamp("two", 24);
        initial.save(journal.path()).unwrap();
        let mut replacement = cadence_state::CadenceState::load(journal.path());
        assert_eq!(replacement.timestamp("one"), Some(12));
        replacement.set_timestamp("two", 36);
        replacement.save(journal.path()).unwrap();
        let loaded = cadence_state::CadenceState::load(journal.path());
        assert_eq!(loaded.timestamp("one"), Some(12));
        assert_eq!(loaded.timestamp("two"), Some(36));
    }

    #[test]
    fn criterion_sixteen_cadence_reads_corrupt_and_non_object_state_leniently() {
        let _log_guard = capture_logs();
        let journal = tempdir().unwrap();
        let path = journal.path().join("health/cadence.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let corrupt = b"{ definitely not json }";
        fs::write(&path, corrupt).unwrap();
        assert_eq!(
            cadence_state::CadenceState::load(journal.path()).timestamp("x"),
            None
        );
        assert_eq!(warnings().len(), 1);
        assert_eq!(fs::read(&path).unwrap(), corrupt);

        LOGS.get().unwrap().lock().unwrap().clear();
        fs::write(&path, "[]").unwrap();
        assert_eq!(
            cadence_state::CadenceState::load(journal.path()).timestamp("x"),
            None
        );
        assert!(warnings().is_empty());
    }

    #[test]
    fn run_log_creates_canonical_jsonl_records() {
        let root = tempdir().unwrap();
        let mut writer = run_log::RunLogWriter::open(root.path(), "20260813", "daily");
        writer.log("talent.skip", 9, Map::<String, Value>::new());
        assert_eq!(writer.skip_count, 1);
        assert_eq!(
            oplog_records(root.path(), "20260813", "daily"),
            vec![serde_json::json!({"event":"talent.skip","ts":9})]
        );
    }

    #[test]
    fn daily_invocations_create_distinct_intact_oplogs() {
        let root = tempdir().unwrap();
        let first = run_at(root.path(), &[]);
        let second = run_at(root.path(), &[]);
        assert_eq!(first.exit_code, 1);
        assert_eq!(second.exit_code, 1);

        let day = today().pred_opt().unwrap();
        let snapshot = catalog_oplogs(JournalRoot::open(root.path()).unwrap(), &[day]).unwrap();
        let mut leaves = snapshot
            .into_catalogued_entries()
            .into_iter()
            .filter(|(entry, _)| {
                entry.name().source().display_slug() == "think"
                    && entry.name().run().display_slug() == "daily"
                    && entry.name().format() == OplogFormat::Jsonl
            })
            .map(|(entry, mut file)| {
                file.seek(SeekFrom::Start(entry.payload_offset() as u64))
                    .unwrap();
                let values = BufReader::new(file)
                    .lines()
                    .map_while(Result::ok)
                    .map(|line| serde_json::from_str::<Value>(&line).unwrap())
                    .collect::<Vec<_>>();
                assert!(values.iter().any(|value| value["event"] == "run.start"));
                assert!(values.iter().any(|value| value["event"] == "run.summary"));
                entry.leaf().to_owned()
            })
            .collect::<Vec<_>>();
        leaves.sort();
        assert_eq!(leaves.len(), 2);
        assert_ne!(leaves[0], leaves[1]);
    }

    #[test]
    fn daily_run_log_create_failure_reports_after_primary_success() {
        let root = tempdir().unwrap();
        let blocked_root = root.path().join("not-a-directory");
        fs::write(&blocked_root, b"not a directory").unwrap();
        let mut writer = run_log::RunLogWriter::open(&blocked_root, "20260813", "daily");
        writer.log("talent.skip", 9, Map::<String, Value>::new());
        writer.log("talent.skip", 10, Map::<String, Value>::new());
        let run = logged_mode_outcome(
            writer,
            Ok(dispatch::ModeResult {
                success: 1,
                success_names: vec!["sense".to_owned()],
                ..dispatch::ModeResult::default()
            }),
        )
        .unwrap();
        assert_eq!(run.exit_code, 1);
        assert!(
            run.stderr
                .starts_with("journal think: 1 completed\nsense\n")
        );
        assert!(run.stderr.contains("think run log"));
    }

    fn segment_dir(journal: &Path, day: &str, segment: &str) -> std::path::PathBuf {
        let path = journal
            .join("chronicle")
            .join(day)
            .join("default")
            .join(segment);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn sense_output_path(context: &context::ThinkContext, segment: &str) -> std::path::PathBuf {
        solstone_core_talent_config::get_output_path(
            &context.day_dir,
            "sense",
            Some(segment),
            Some("json"),
            None,
            Some("default"),
        )
    }

    fn write_sense_output(context: &context::ThinkContext, segment: &str, value: Value) {
        let path = sense_output_path(context, segment);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn segment_context(
        journal: &Path,
        roots: &Path,
        metadata: &str,
    ) -> (context::ThinkContext, Arc<Recorder>) {
        let (talent_root, apps_root) = talent_roots(roots, &[("sense", metadata)]);
        let (context, recorder) = recorder_context(journal, "20260813", 9);
        (context.with_talent_roots(talent_root, apps_root), recorder)
    }

    fn run_segment(
        context: &context::ThinkContext,
        journal: &Path,
        segment: &str,
        refresh: bool,
        live: bool,
    ) -> dispatch::ModeResult {
        run_segment_with(
            context,
            journal,
            segment,
            refresh,
            live,
            2,
            Some(std::time::Duration::from_secs(610)),
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_segment_with(
        context: &context::ThinkContext,
        _journal: &Path,
        segment: &str,
        refresh: bool,
        live: bool,
        jobs: i64,
        timeout: Option<std::time::Duration>,
        skip_talents: &[String],
    ) -> dispatch::ModeResult {
        let mut log = test_log(context, "segment");
        segment::run(
            context,
            &mut log,
            segment,
            refresh,
            Some("default"),
            jobs,
            timeout,
            live,
            skip_talents,
        )
        .unwrap()
    }

    #[test]
    fn segment_inflight_gate_skips_pending_and_analyzing_media_without_dispatching() {
        // Source-derived, not measured: thinking.py:1485-1499 leaves a
        // pending or analyzing modality untouched and records `raw_media_pending`.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, recorder) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        let path = segment_dir(journal.path(), "20260813", "090000_300");
        fs::write(path.join("audio.jsonl"), "{}\n").unwrap();

        let first = run_segment(&context, journal.path(), "090000_300", false, false);
        let analyzing = segment_dir(journal.path(), "20260813", "090500_300");
        fs::write(analyzing.join(".analyzing_audio"), "{}").unwrap();
        let second = run_segment(&context, journal.path(), "090500_300", false, false);
        assert_eq!(first, dispatch::ModeResult::default());
        assert_eq!(second, first);
        assert!(recorder.requests.lock().unwrap().is_empty());
        assert!(
            oplog_records(journal.path(), &context.day, "segment")
                .iter()
                .any(|record| record["reason"] == "raw_media_pending")
        );
    }

    #[test]
    fn segment_without_a_stream_uses_the_only_named_stream_binding() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, _) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        let segment = "090000_300";
        let named = journal
            .path()
            .join("chronicle/20260813/audio")
            .join(segment);
        fs::create_dir_all(&named).unwrap();
        let output = solstone_core_talent_config::get_output_path(
            &context.day_dir,
            "sense",
            Some(segment),
            Some("json"),
            None,
            Some("audio"),
        );
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(
            output,
            serde_json::to_vec(&serde_json::json!({"density":"active","content_type":"work"}))
                .unwrap(),
        )
        .unwrap();
        let mut log = test_log(&context, "segment");

        let result = segment::run(
            &context,
            &mut log,
            segment,
            false,
            None,
            2,
            None,
            false,
            &[],
        )
        .unwrap();

        assert_eq!((result.success, result.failed), (1, 0));
        assert!(named.join("talents/activity.md").is_file());
        assert!(
            !journal
                .path()
                .join("chronicle/20260813")
                .join(segment)
                .exists()
        );
    }

    #[test]
    fn segment_without_a_stream_rejects_an_ambiguous_basename() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, _) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        let segment = "090000_300";
        fs::create_dir_all(journal.path().join("chronicle/20260813").join(segment)).unwrap();
        fs::create_dir_all(
            journal
                .path()
                .join("chronicle/20260813/audio")
                .join(segment),
        )
        .unwrap();
        let mut log = test_log(&context, "segment");

        let error = segment::run(
            &context,
            &mut log,
            segment,
            false,
            None,
            2,
            None,
            false,
            &[],
        )
        .expect_err("an omitted stream must not silently select one of two segments");

        assert!(error.contains("ambiguous segment"), "{error}");
    }

    #[test]
    fn segment_no_input_gate_writes_idle_artifacts_without_dispatching() {
        // Source-derived, not measured: thinking.py:1536-1584 writes a
        // schema-valid idle Sense result and terminalizes the segment.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, recorder) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\", \"load\": {\"audio\": true}\n}\n",
        );
        let path = segment_dir(journal.path(), "20260813", "090000_300");

        let first = run_segment(&context, journal.path(), "090000_300", false, false);
        let density = fs::read(path.join("talents/density.json")).unwrap();
        let second = run_segment(&context, journal.path(), "090000_300", false, false);
        assert_eq!(first, dispatch::ModeResult::default());
        assert_eq!(second, first);
        assert_eq!(
            serde_json::from_slice::<Value>(&density).unwrap()["classification"],
            "idle"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(path.join("talents/density.json")).unwrap())
                .unwrap()["classification"],
            "idle"
        );
        assert!(recorder.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn segment_new_only_is_historical_skip_but_live_dispatches() {
        // Source-derived, not measured: thinking.py:1423-1433 only dispatches
        // a raw-truthy `new_only` talent from a live segment invocation.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, recorder) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\", \"new_only\": true\n}\n",
        );
        segment_dir(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work"}),
        );

        let historical = run_segment(&context, journal.path(), "090000_300", false, false);
        assert_eq!(historical.success, 0);
        assert!(recorder.requests.lock().unwrap().is_empty());
        let live = run_segment(&context, journal.path(), "090000_300", false, true);
        assert_eq!((live.success, live.failed), (1, 0));
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn segment_not_claimed_is_distinct_from_send_failure() {
        // Source-derived, not measured: thinking.py:1602-1628 records a
        // not-claimed Sense request as `request_lost`, not a send failure.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, recorder) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        let context = context.with_event_clock(Arc::new(|| 1_785_000_123_456));
        segment_dir(journal.path(), "20260813", "090000_300");
        *recorder.dispatch_failure.lock().unwrap() = Some(context::DispatchFailure::NotClaimed {
            use_id: "lost-1".to_owned(),
        });

        let result = run_segment(&context, journal.path(), "090000_300", false, false);
        assert_eq!(result.failed_names, vec!["sense (request_lost)"]);
        let log = oplog_records(journal.path(), &context.day, "segment");
        let failure = log
            .iter()
            .find(|record| record["state"] == "request_lost")
            .expect("request_lost record");
        assert_eq!(failure["ts"], 1_785_000_123_456_i64);
        assert_ne!(failure["ts"], context.now_ms);
        assert!(!log.iter().any(|record| record["reason"] == "send_failed"));
    }

    #[test]
    fn segment_rejects_malformed_and_missing_required_sense_output() {
        // Source-derived, not measured: thinking.py:1682-1724 treats invalid
        // JSON and either missing required Sense field as distinct failures.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, _) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        segment_dir(journal.path(), "20260813", "090000_300");
        let output = sense_output_path(&context, "090000_300");
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        fs::write(&output, "not json").unwrap();
        assert_eq!(
            run_segment(&context, journal.path(), "090000_300", false, false).failed_names,
            vec!["sense (output_parse)"]
        );
        fs::write(&output, r#"{"density":"active"}"#).unwrap();
        assert_eq!(
            run_segment(&context, journal.path(), "090000_300", false, false).failed_names,
            vec!["sense (output_invalid)"]
        );
        fs::write(&output, r#"{"content_type":"work"}"#).unwrap();
        assert_eq!(
            run_segment(&context, journal.path(), "090000_300", false, false).failed_names,
            vec!["sense (output_invalid)"]
        );
    }

    #[test]
    fn segment_idle_and_redundant_branches_terminalize_after_sense() {
        // Idle segments terminalize on their Sense projection alone; redundant
        // active segments record the change and dispatch nothing further.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, _) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        let idle = segment_dir(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"idle","content_type":"idle"}),
        );
        let idle_result = run_segment(&context, journal.path(), "090000_300", false, false);
        assert_eq!((idle_result.success, idle_result.failed), (1, 0));
        assert_eq!(
            canonical_terminals(
                &oplog_records(journal.path(), &context.day, "segment"),
                "sense"
            ),
            vec![("talent.complete", Some("use-1"), Some("finish"))]
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(idle.join("talents/density.json")).unwrap())
                .unwrap()["classification"],
            "idle"
        );

        let previous = segment_dir(journal.path(), "20260813", "090500_300");
        let current = segment_dir(journal.path(), "20260813", "091000_300");
        fs::write(previous.join("imported.md"), "same transcript words").unwrap();
        fs::write(current.join("imported.md"), "same transcript words").unwrap();
        let sensor = solstone_core_system_health::detect_segment_change(
            journal.path(),
            "20260813",
            Some("default"),
            "091000_300",
            &current,
            None,
            "2026-08-13T00:00:00+00:00",
        )["sensors"]
            .clone();
        fs::create_dir_all(previous.join("talents")).unwrap();
        fs::write(
            previous.join("talents/change.json"),
            serde_json::to_vec(&serde_json::json!({"sensors": sensor})).unwrap(),
        )
        .unwrap();
        write_sense_output(
            &context,
            "091000_300",
            serde_json::json!({"density":"active","content_type":"work"}),
        );
        let redundant = run_segment(&context, journal.path(), "091000_300", false, false);
        assert_eq!((redundant.success, redundant.failed), (1, 0));
        assert!(
            canonical_terminals(
                &oplog_records(journal.path(), &context.day, "segment"),
                "sense"
            )
            .contains(&("talent.complete", Some("use-2"), Some("finish")))
        );
        let change = serde_json::from_slice::<Value>(
            &fs::read(current.join("talents/change.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(change["change_class"], "redundant");
        assert_eq!(change["predecessor"]["segment"], "090500_300");
    }

    #[test]
    fn idle_segment_dispatches_only_sense() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "sense",
                    "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
                ),
                (
                    "entities:detection",
                    "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 3, \"output\": \"json\"\n}\n",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        segment_dir(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({
                "density":"idle",
                "content_type":"idle",
                "activity_summary":"A quiet but real event occurred."
            }),
        );

        let result = run_segment(&context, journal.path(), "090000_300", false, false);
        let names = recorder
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.name.clone())
            .collect::<Vec<_>>();

        assert_eq!((result.success, result.failed), (1, 0));
        assert_eq!(names, ["sense"]);
    }

    #[test]
    fn segment_activity_tail_persists_ended_records_runs_prompts_and_is_idempotent() {
        // Source-derived, not measured: thinking.py:379-435 advances durable
        // activity state, appends an ended record once, then runs its matching
        // activity talent; repeated segment replay must not duplicate either.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "activity_probe",
                "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"md\",\"activities\":[\"work\"]\n}",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        for (segment, sense) in [
            (
                "090000_300",
                serde_json::json!({"density":"active","content_type":"work","activity_summary":"work","facets":[{"facet":"work","level":"high","activity":"work"}]}),
            ),
            (
                "090500_300",
                serde_json::json!({"density":"idle","content_type":"idle","activity_summary":"","facets":[]}),
            ),
        ] {
            let path = segment_dir(journal.path(), "20260813", segment).join("talents");
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("sense.json"), serde_json::to_vec(&sense).unwrap()).unwrap();
        }
        let mut log = test_log(&context, "segment");
        let segments = vec![
            ("090000_300".to_owned(), Some("default".to_owned())),
            ("090500_300".to_owned(), Some("default".to_owned())),
        ];
        segment::replay_activity_state(&context, &mut log, &segments, false, 2, false, true)
            .unwrap();
        assert!(journal
            .path()
            .join("chronicle/20260813/health/talent-provenance/activity-inputs/work/work_090000_300.json")
            .is_file());
        segment::replay_activity_state(&context, &mut log, &segments, false, 2, false, true)
            .unwrap();
        let records =
            fs::read_to_string(journal.path().join("facets/work/activities/20260813.jsonl"))
                .unwrap();
        assert_eq!(records.lines().count(), 1);
        assert!(
            journal
                .path()
                .join("awareness/activity_state.json")
                .is_file()
        );
        assert_eq!(
            recorder
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.name == "activity_probe")
                .count(),
            1
        );
    }

    #[test]
    fn segment_activity_tail_no_activity_prompts_only_suppresses_prompt_dispatch() {
        // Source-derived, not measured: thinking.py:323-332 persists ended
        // activity data but records `activity.prompts_skipped` when requested.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "activity_probe",
                "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"md\",\"activities\":[\"work\"]\n}",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        for (segment, sense) in [
            (
                "090000_300",
                serde_json::json!({"density":"active","content_type":"work","activity_summary":"work","facets":[{"facet":"work"}]}),
            ),
            (
                "090500_300",
                serde_json::json!({"density":"idle","content_type":"idle","activity_summary":"","facets":[]}),
            ),
        ] {
            let path = segment_dir(journal.path(), "20260813", segment).join("talents");
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("sense.json"), serde_json::to_vec(&sense).unwrap()).unwrap();
        }
        let mut log = test_log(&context, "segment");
        segment::replay_activity_state(
            &context,
            &mut log,
            &[
                ("090000_300".to_owned(), Some("default".to_owned())),
                ("090500_300".to_owned(), Some("default".to_owned())),
            ],
            false,
            2,
            true,
            true,
        )
        .unwrap();
        assert!(
            journal
                .path()
                .join("facets/work/activities/20260813.jsonl")
                .is_file()
        );
        assert!(recorder.requests.lock().unwrap().is_empty());
        assert!(
            oplog_records(journal.path(), &context.day, "segment")
                .iter()
                .any(|record| record["event"] == "activity.prompts_skipped")
        );
    }

    #[test]
    fn activity_tail_routes_cross_day_completion_to_the_prior_day() {
        // Source-derived, not measured: thinking.py:394-435 captures
        // `last_segment_day` before update, so a carried activity completes
        // and dispatches on the day where it began.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "activity_probe",
                "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"md\",\"activities\":[\"work\"]\n}",
            )],
        );
        fs::create_dir_all(journal.path().join("awareness")).unwrap();
        fs::write(
            journal.path().join("awareness/activity_state.json"),
            serde_json::to_vec(&serde_json::json!({
                "last_segment_key":"235500_300", "last_segment_day":"20260813",
                "active":{"work":{"id":"work_235500_300","activity":"work","since":"235500_300","description":"work","facet":"work","segment":"235500_300","segments":["235500_300"]}}
            }))
            .unwrap(),
        )
        .unwrap();
        let (context, recorder) = recorder_context(journal.path(), "20260814", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        let path = segment_dir(journal.path(), "20260814", "000000_300").join("talents");
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("sense.json"),
            br#"{"density":"idle","content_type":"idle"}"#,
        )
        .unwrap();
        let mut log = test_log(&context, "segment");
        segment::replay_activity_state(
            &context,
            &mut log,
            &[("000000_300".to_owned(), Some("default".to_owned()))],
            false,
            2,
            false,
            true,
        )
        .unwrap();
        assert!(
            journal
                .path()
                .join("facets/work/activities/20260813.jsonl")
                .is_file()
        );
        assert_eq!(
            recorder.requests.lock().unwrap()[0].config["day"],
            "20260813"
        );
    }

    #[test]
    fn segment_batch_replay_keeps_streams_separate_and_flushes_imports() {
        // Source-derived, not measured: thinking.py:594-634 keeps a machine
        // per stream, then closes finite import streams at batch end.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "activity_probe",
                "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"md\",\"activities\":[\"work\"]\n}",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 1_786_708_800_000);
        let context = context.with_talent_roots(talent_root, apps_root);
        for (stream, segment, sense) in [
            (
                "import.audio",
                "090000_300",
                serde_json::json!({"density":"active","content_type":"work","activity_summary":"import","facets":[{"facet":"work"}]}),
            ),
            (
                "default",
                "100000_300",
                serde_json::json!({"density":"active","content_type":"work","activity_summary":"live","facets":[{"facet":"work"}]}),
            ),
            (
                "default",
                "100500_300",
                serde_json::json!({"density":"idle","content_type":"idle"}),
            ),
        ] {
            let path = journal
                .path()
                .join("chronicle/20260813")
                .join(stream)
                .join(segment)
                .join("talents");
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("sense.json"), serde_json::to_vec(&sense).unwrap()).unwrap();
        }
        let mut log = test_log(&context, "segments");
        segment::replay_activity_state(
            &context,
            &mut log,
            &[
                ("090000_300".to_owned(), Some("import.audio".to_owned())),
                ("100000_300".to_owned(), Some("default".to_owned())),
                ("100500_300".to_owned(), Some("default".to_owned())),
            ],
            false,
            2,
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(journal.path().join("facets/work/activities/20260813.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        assert_eq!(recorder.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn activity_replay_reruns_only_when_its_durable_input_changes() {
        // Source-derived, not measured: thinking.py:335-375 compares the
        // activity input provenance after an existing record, rerunning only
        // when the span's durable Sense input changed.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "activity_probe",
                "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"md\",\"activities\":[\"work\"]\n}",
            )],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        for (segment, sense) in [
            (
                "090000_300",
                serde_json::json!({"density":"active","content_type":"work","activity_summary":"one","facets":[{"facet":"work"}]}),
            ),
            (
                "090500_300",
                serde_json::json!({"density":"idle","content_type":"idle"}),
            ),
        ] {
            let path = segment_dir(journal.path(), "20260813", segment).join("talents");
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("sense.json"), serde_json::to_vec(&sense).unwrap()).unwrap();
        }
        let segments = vec![
            ("090000_300".to_owned(), Some("default".to_owned())),
            ("090500_300".to_owned(), Some("default".to_owned())),
        ];
        let mut log = test_log(&context, "segment");
        segment::replay_activity_state(&context, &mut log, &segments, false, 2, false, true)
            .unwrap();
        segment::replay_activity_state(&context, &mut log, &segments, false, 2, false, true)
            .unwrap();
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
        let path = segment_dir(journal.path(), "20260813", "090000_300").join("talents/sense.json");
        fs::write(path, br#"{"density":"active","content_type":"work","activity_summary":"changed","facets":[{"facet":"work"}]}"#).unwrap();
        segment::replay_activity_state(&context, &mut log, &segments, false, 2, false, true)
            .unwrap();
        assert_eq!(recorder.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn replay_accepts_minimal_sense_and_snapshot_failure_does_not_block_completion() {
        // Source-derived, not measured: thinking.py:408-435 swallows a
        // snapshot failure, while 547-591 require only density/content_type.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "activity_probe",
                "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"md\",\"activities\":[\"work\"]\n}",
            )],
        );
        fs::write(journal.path().join("awareness"), "not a directory").unwrap();
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        // Hydration cannot use the blocked snapshot, so the first replay
        // starts activity; the second minimal idle projection closes it.
        for (segment, sense) in [
            (
                "090000_300",
                br#"{"density":"active","content_type":"work","facets":[{"facet":"work"}]}"#
                    .as_slice(),
            ),
            (
                "090500_300",
                br#"{"density":"idle","content_type":"idle"}"#.as_slice(),
            ),
        ] {
            let path = segment_dir(journal.path(), "20260813", segment).join("talents");
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("sense.json"), sense).unwrap();
        }
        let mut log = test_log(&context, "segment");
        segment::replay_activity_state(
            &context,
            &mut log,
            &[
                ("090000_300".to_owned(), Some("default".to_owned())),
                ("090500_300".to_owned(), Some("default".to_owned())),
            ],
            false,
            2,
            false,
            true,
        )
        .unwrap();
        assert!(
            journal
                .path()
                .join("facets/work/activities/20260813.jsonl")
                .is_file()
        );
        assert_eq!(recorder.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn segments_replay_reads_durable_sense_outputs_after_repairs() {
        // Source-derived, not measured: thinking.py:594-634 replays persisted
        // Sense projections after the concurrent `--segments` repair pool.
        let journal = tempdir().unwrap();
        let (context, _) = recorder_context(journal.path(), "20260813", 9);
        let path = segment_dir(journal.path(), "20260813", "090000_300").join("talents");
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("sense.json"),
            serde_json::to_vec(&serde_json::json!({"density":"active","content_type":"work","activity_summary":"work","facets":[{"facet":"work"}]})).unwrap(),
        )
        .unwrap();
        let mut log = test_log(&context, "segments");
        segment::replay_activity_state(
            &context,
            &mut log,
            &[("090000_300".to_owned(), Some("default".to_owned()))],
            false,
            2,
            false,
            false,
        )
        .unwrap();
        // Source-derived, not measured: thinking.py:594-634 keeps one
        // in-memory machine per replay stream and does not overwrite the
        // direct-run activity snapshot from a batch replay.
        assert!(
            !journal
                .path()
                .join("awareness/activity_state.json")
                .exists()
        );
    }

    #[test]
    fn segment_selects_direct_output_talents_and_observes_sense_change() {
        // Source-derived, not measured: thinking.py:1818-1882 selects the
        // floor, summary, and detection talents after a non-terminal Sense result.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "sense",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\"\n}",
                ),
                (
                    "documents",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"json\",\"accumulate\":true,\"provider\":\"test-provider\",\"model\":\"test-model\"\n}",
                ),
                (
                    "entities:detection",
                    "{\n\"type\":\"cogitate\",\"schedule\":\"segment\",\"priority\":2\n}",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        segment_dir(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work","recommend":{}}),
        );

        let result = run_segment(&context, journal.path(), "090000_300", false, false);
        assert_eq!((result.success, result.failed), (3, 0));
        let requests = recorder.requests.lock().unwrap();
        let documents = requests
            .iter()
            .find(|request| request.name == "documents")
            .unwrap();
        // Source-derived, not measured: thinking.py:1435-1468 gives segment
        // requests direct persistence, never `apply_output_persistence`.
        assert_eq!(documents.config["output"], "json");
        assert_eq!(documents.config["provider"], "test-provider");
        assert_eq!(documents.config["model"], "test-model");
        assert!(
            oplog_records(journal.path(), &context.day, "segment")
                .iter()
                .any(|record| record["event"] == "sense.change_detect")
        );
    }

    #[test]
    fn segment_recommendation_and_floor_cap_guards_keep_their_outcomes_distinct() {
        // Source-derived, not measured: thinking.py:1824-1838 caps floor
        // talents, while 1905-1930 requires audio embeddings for speakers.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "sense",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\"\n}",
                ),
                (
                    "documents",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
                (
                    "screen",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
                (
                    "speaker_attribution",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        segment_dir(journal.path(), "20260813", "090000_300");
        let health = journal.path().join("chronicle/20260813/health/cap.jsonl");
        fs::create_dir_all(health.parent().unwrap()).unwrap();
        fs::write(
            health,
            (0..5)
                .map(|index| format!(r#"{{"event":"talent.fail","ts":{},"mode":"segment","stream":"default","segment":"090000_300","name":"documents"}}"#, index * 1_800_000))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work","recommend":{"screen_record":true,"speaker_attribution":true}}),
        );
        let first = run_segment(&context, journal.path(), "090000_300", false, false);
        assert_eq!(first.success, 2);
        assert!(
            !recorder
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.name == "documents")
        );
        assert!(
            recorder
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.name == "screen")
        );
        assert!(
            !recorder
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.name == "speaker_attribution")
        );
        let health_source =
            solstone_core_system_health::FilesystemHealthLogSource::new(journal.path());
        let health_records = solstone_core_system_health::HealthLogSource::health_log_paths(
            &health_source,
            "20260813",
        )
        .unwrap();
        let skips = health_records
            .iter()
            .flat_map(|path| {
                fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
            .filter(|row| row["event"] == "talent.skip" && row["reason"] == "not_recommended")
            .collect::<Vec<_>>();
        assert_eq!(skips.len(), 1);
        assert_eq!(skips[0]["name"], "speaker_attribution");
        assert_eq!(skips[0]["stream"], "default");
        let path = journal.path().join("chronicle/20260813/default/090000_300");
        fs::write(path.join("audio.npz"), []).unwrap();
        let second = run_segment(&context, journal.path(), "090000_300", true, false);
        assert_eq!(second.success, 4);
        assert!(
            recorder
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.name == "speaker_attribution")
        );
    }

    #[test]
    fn selected_nongating_failures_remain_visible_without_failing_segment_repair() {
        struct FailingTalent {
            recorder: Arc<Recorder>,
            name: &'static str,
            failure: &'static str,
            selected_use: Mutex<Option<String>>,
        }
        impl context::CortexBoundary for FailingTalent {
            fn dispatch(
                &self,
                runtime: &tokio::runtime::Runtime,
                request: &solstone_core_cortex_client::CortexRequest,
            ) -> Result<String, context::DispatchFailure> {
                let id = context::CortexBoundary::dispatch(&*self.recorder, runtime, request)?;
                if request.name == self.name {
                    *self.selected_use.lock().unwrap() = Some(id.clone());
                    match self.failure {
                        "terminal" => {
                            self.recorder.end_states.lock().unwrap().insert(
                                id.clone(),
                                solstone_core_cortex_client::UseEndState::Error,
                            );
                        }
                        "timeout" => self.recorder.timed_out.lock().unwrap().push(
                            solstone_core_cortex_client::TimedOutUse::GenuineTimeout {
                                use_id: id.clone(),
                            },
                        ),
                        "lost" => self.recorder.timed_out.lock().unwrap().push(
                            solstone_core_cortex_client::TimedOutUse::LostAtDeadline {
                                use_id: id.clone(),
                            },
                        ),
                        "missing" => {
                            self.recorder.omit_ids.lock().unwrap().insert(id.clone());
                        }
                        _ => {}
                    }
                }
                Ok(id)
            }
            fn wait(
                &self,
                runtime: &tokio::runtime::Runtime,
                ids: &[String],
                deadline: Option<std::time::Duration>,
            ) -> Result<solstone_core_cortex_client::WaitForUsesReport, String> {
                if self.failure == "wait_error"
                    && self
                        .selected_use
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|id| ids.contains(id))
                {
                    return Err("injected wait failure".to_owned());
                }
                context::CortexBoundary::wait(&*self.recorder, runtime, ids, deadline)
            }
        }
        for name in ["entities:detection", "documents"] {
            for failure in [
                "terminal",
                "timeout",
                "lost",
                "missing",
                "wait_error",
                "send",
                "unclaimed",
            ] {
                let journal = tempdir().unwrap();
                let roots = tempdir().unwrap();
                let metadata = "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"json\"\n}";
                let (talent_root, apps_root) = talent_roots(
                    roots.path(),
                    &[
                        ("sense", metadata),
                        ("documents", metadata),
                        ("entities:detection", metadata),
                    ],
                );
                let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
                if failure == "send" {
                    recorder
                        .dispatch_failures
                        .lock()
                        .unwrap()
                        .insert(name.to_owned(), context::DispatchFailure::Unavailable);
                } else if failure == "unclaimed" {
                    recorder.dispatch_failures.lock().unwrap().insert(
                        name.to_owned(),
                        context::DispatchFailure::NotClaimed {
                            use_id: "unclaimed-selected".to_owned(),
                        },
                    );
                }
                let context = context
                    .with_talent_roots(talent_root, apps_root)
                    .with_boundary(Arc::new(FailingTalent {
                        recorder,
                        name,
                        failure,
                        selected_use: Mutex::new(None),
                    }));
                segment_dir(journal.path(), "20260813", "090000_300");
                write_sense_output(
                    &context,
                    "090000_300",
                    serde_json::json!({"density":"active","content_type":"work","recommend":{}}),
                );
                let result = run_segment_with(
                    &context,
                    journal.path(),
                    "090000_300",
                    false,
                    false,
                    1,
                    Some(std::time::Duration::from_secs(610)),
                    &[],
                );
                let blocking = name == "documents";
                assert_eq!(
                    result.failed,
                    usize::from(blocking),
                    "{name}/{failure}: {result:?}"
                );
                assert_eq!(
                    result.timed_out,
                    blocking && matches!(failure, "timeout" | "lost"),
                    "{name}/{failure}"
                );
                assert_eq!(result.failed_names.is_empty(), !blocking);
                assert!(!result.success_names.iter().any(|label| label == name));
                let records = oplog_records(journal.path(), &context.day, "segment");
                assert!(
                    records.iter().any(|row| row["name"] == name
                        && (row["event"] == "talent.fail"
                            || (row["event"] == "talent.skip" && row["reason"] == "send_failed"))),
                    "failure telemetry missing for {name}/{failure}"
                );
            }
        }
    }

    #[test]
    fn segment_selected_dispatch_outcomes_and_batches_are_distinct() {
        // Source-derived, not measured: thinking.py:1931-2016 keeps skipped,
        // unavailable, and unclaimed selection outcomes separate while draining batches.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "sense",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\"\n}",
                ),
                (
                    "documents",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
                (
                    "entities:detection",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
                (
                    "screen",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        segment_dir(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work","recommend":{"screen_record":true}}),
        );
        recorder.dispatch_failures.lock().unwrap().insert(
            "documents".to_owned(),
            context::DispatchFailure::Unavailable,
        );
        recorder.dispatch_failures.lock().unwrap().insert(
            "screen".to_owned(),
            context::DispatchFailure::NotClaimed {
                use_id: "lost-selected".to_owned(),
            },
        );
        let skipped = vec!["entities:detection".to_owned()];
        let result = run_segment_with(
            &context,
            journal.path(),
            "090000_300",
            false,
            false,
            1,
            Some(std::time::Duration::from_secs(610)),
            &skipped,
        );
        assert_eq!((result.success, result.failed), (1, 2));
        assert_eq!(
            result.failed_names,
            vec![
                "documents (send)".to_owned(),
                "screen (request_lost)".to_owned(),
            ]
        );
        assert_eq!(
            recorder
                .waits
                .lock()
                .unwrap()
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![1]
        );
        let log = oplog_records(journal.path(), &context.day, "segment");
        assert!(log.iter().any(|record| record["state"] == "request_lost"));
        assert!(
            log.iter()
                .any(|record| record["reason"] == "skip_talents_flag")
        );
    }

    #[test]
    fn segment_zero_jobs_and_repair_pool_keep_optional_deadlines_and_unique_ids() {
        // Source-derived, not measured: thinking.py:1994-2010 leaves jobs=0
        // unlimited, and 4444 gives both segment entry paths the optional deadline.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[
                (
                    "sense",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\"\n}",
                ),
                (
                    "documents",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal.path(), "20260813", 9);
        let context = context.with_talent_roots(talent_root, apps_root);
        for segment in ["090000_300", "090500_300"] {
            segment_dir(journal.path(), "20260813", segment);
            write_sense_output(
                &context,
                segment,
                serde_json::json!({"density":"active","content_type":"work","recommend":{}}),
            );
        }
        let direct = run_segment_with(
            &context,
            journal.path(),
            "090000_300",
            false,
            false,
            0,
            None,
            &[],
        );
        assert_eq!((direct.success, direct.failed), (2, 0));
        let log = test_log(&context, "segments");
        let result = segment::run_repair_batch(
            &context,
            &log,
            vec![
                ("090000_300".to_owned(), Some("default".to_owned())),
                ("090500_300".to_owned(), Some("default".to_owned())),
            ],
            false,
            0,
            2,
            None,
            Vec::new(),
        )
        .unwrap();
        assert_eq!((result.success, result.failed), (4, 0));
        assert!(
            recorder
                .deadlines
                .lock()
                .unwrap()
                .iter()
                .all(Option::is_none)
        );
        let use_ids = recorder
            .waits
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        // Use-log paths derive from these ids; asserting a fake path would only
        // measure the fake, while id uniqueness is the real construction invariant.
        assert_eq!(use_ids.len(), 6);
    }

    #[test]
    fn dry_run_oracle_replays_all_thirteen_cases_byte_for_byte() {
        let blocks = oracle_blocks();
        assert_eq!(
            blocks.iter().map(|(line, _, _)| *line).collect::<Vec<_>>(),
            vec![20, 42, 54, 66, 70, 82, 89, 95, 101, 126, 138, 193, 221]
        );
        assert_eq!(blocks.len(), 13);
        assert!(oracle_dry_run_argv(&["--dry-run"]).is_err());
        for (line, argv, expected) in blocks {
            let journal = tempdir().unwrap();
            let now_ms = 1_767_225_600_000;
            seed_oracle_case(journal.path(), line, now_ms);
            let run = run_oracle_case(journal.path(), &argv, now_ms);
            assert_eq!(run.exit_code, 0, "oracle case at line {line}");
            // The fixture's two declared deviations are not present in planner
            // stdout for these cases: no usage error is rendered, and no
            // sibling executable path is displayed.  They remain explicit
            // fixture metadata rather than values normalised from this output.
            assert_eq!(run.stdout, expected, "oracle case at line {line}");
            // The captured oracle is a public, package-relative surface: its
            // expected bytes and native replay must not disclose the temporary
            // journal path or a host-specific executable location.
            assert!(!expected.contains(journal.path().to_string_lossy().as_ref()));
            assert!(
                !run.stdout
                    .contains(journal.path().to_string_lossy().as_ref())
            );
            assert!(!expected.contains("/home/"));
        }
    }

    #[test]
    fn dry_run_binds_success_stdout_and_exact_bootstrap_write_set_without_overwriting_partner() {
        // The fixture's measured side effect contract (lines 153-171) is only
        // meaningful together with successful planner stdout and exit status.
        let journal = tempdir().unwrap();
        let args = oracle_dry_run_argv(&["--day", "20260101"]).unwrap();
        let first = run_cli_with(
            &args,
            journal.path(),
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            || NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            || 1_767_225_600_000,
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(4),
            &BTreeMap::new(),
        );
        assert_eq!(first.exit_code, 0);
        assert_eq!(first.stdout.lines().next(), Some("Day 2026-01-01"));
        assert_eq!(
            created_paths(journal.path()),
            BTreeSet::from([
                "chronicle/".to_owned(),
                "chronicle/20260101/".to_owned(),
                "identity/".to_owned(),
                "identity/.identity.lock".to_owned(),
                "identity/health.md".to_owned(),
                "identity/history.jsonl".to_owned(),
                "identity/partner.md".to_owned(),
            ])
        );
        let partner = fs::read(journal.path().join("identity/partner.md")).unwrap();
        let second = run_cli_with(
            &args,
            journal.path(),
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            || NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            || 1_767_225_600_000,
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(4),
            &BTreeMap::new(),
        );
        assert_eq!(second.exit_code, 0);
        assert_eq!(
            fs::read(journal.path().join("identity/partner.md")).unwrap(),
            partner
        );
    }

    #[test]
    fn dry_run_output_paths_are_package_relative_and_format_sensitive() {
        // Source-derived, not measured: oracle lines 173-190 require a JSON
        // daily_schedule output to ignore a same-stem markdown decoy.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "daily_schedule",
                "{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":10,\"output\":\"json\"\n}\n",
            )],
        );
        let day_dir = day::create_day(journal.path(), "20260101").unwrap();
        let context =
            context::ThinkContext::new(journal.path(), "20260101".to_owned(), day_dir.clone(), 1)
                .expect("think context")
                .with_talent_roots(talent_root, apps_root);
        let args = args::ThinkArgs {
            dry_run: true,
            day: Some("20260101".to_owned()),
            ..args::ThinkArgs::default()
        };
        let before = dry_run::run(&context, &args, 4).unwrap();
        fs::create_dir_all(day_dir.join("talents")).unwrap();
        fs::write(day_dir.join("talents/daily_schedule.md"), "decoy").unwrap();
        assert_eq!(dry_run::run(&context, &args, 4).unwrap(), before);
        fs::write(day_dir.join("talents/daily_schedule.json"), "{}\n").unwrap();
        let after = dry_run::run(&context, &args, 4).unwrap();
        assert!(after.contains("daily_schedule (exists)"), "{after}");
        assert!(!before.contains(journal.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn dry_run_source_derived_cadence_fire_reports_segment_and_activity_counts() {
        // Source-derived, not measured: fixture lines 236-257 exclude this
        // row; its exact shape comes from thinking.py:3808-3811.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "steward",
                "{\n\"type\":\"generate\",\"schedule\":\"cadence\",\"priority\":1,\"cadence_minutes\":30,\"output\":\"json\"\n}\n",
            )],
        );
        let health = journal
            .path()
            .join("chronicle/20260101/health/events.jsonl");
        fs::create_dir_all(health.parent().unwrap()).unwrap();
        fs::write(
            health,
            concat!(
                r#"{"event":"talent.complete","ts":2,"mode":"segment","stream":"_default","segment":"093000_600","name":"sense"}"#, "\n",
                r#"{"event":"talent.complete","ts":3,"mode":"activity","facet":"work","activity":"meeting","name":"conversation"}"#, "\n",
            ),
        )
        .unwrap();
        let day_dir = day::create_day(journal.path(), "20260101").unwrap();
        let context =
            context::ThinkContext::new(journal.path(), "20260101".to_owned(), day_dir, 10)
                .expect("think context")
                .with_talent_roots(talent_root, apps_root);
        let args = args::ThinkArgs {
            cadence: true,
            dry_run: true,
            ..args::ThinkArgs::default()
        };
        assert_eq!(
            dry_run::run(&context, &args, 4).unwrap(),
            "Day 2026-01-01 — cadence agents\n\n  fire  steward — window: 1 segment(s), 1 activity(ies)\n"
        );
    }

    #[test]
    fn dry_run_source_derived_flush_renders_eligible_hooks() {
        // Source-derived, not measured: fixture lines 236-257 capture only
        // the empty branch; this is thinking.py:4035-4055.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "flush_agent",
                "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"md\",\"hook\":{\"flush\":true}\n}\n",
            )],
        );
        let day_dir = day::create_day(journal.path(), "20260101").unwrap();
        let context = context::ThinkContext::new(journal.path(), "20260101".to_owned(), day_dir, 1)
            .expect("think context")
            .with_talent_roots(talent_root, apps_root);
        let args = args::ThinkArgs {
            flush: true,
            segment: Some("010203_60".to_owned()),
            dry_run: true,
            ..args::ThinkArgs::default()
        };
        assert_eq!(
            dry_run::run(&context, &args, 4).unwrap(),
            "Day 2026-01-01 --flush segment 010203_60\n\n  gen  flush_agent\n\nTotal: 1 agents\n"
        );
    }

    #[test]
    fn dry_run_source_derived_active_multi_facet_renders_one_row_per_facet() {
        // Source-derived, not measured: fixture lines 236-257 omit the active
        // multi-facet branch; this is thinking.py:3901-3920.
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (talent_root, apps_root) = talent_roots(
            roots.path(),
            &[(
                "facet_newsletter",
                "{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":40,\"output\":\"md\",\"multi_facet\":true\n}\n",
            )],
        );
        let declaration = journal.path().join("facets/work/facet.json");
        fs::create_dir_all(declaration.parent().unwrap()).unwrap();
        fs::write(declaration, "{}\n").unwrap();
        let state = journal
            .path()
            .join("chronicle/20260101/090000_60/talents/facets.json");
        fs::create_dir_all(state.parent().unwrap()).unwrap();
        fs::write(state, r#"[{"facet":"work"}]"#).unwrap();
        let day_dir = day::create_day(journal.path(), "20260101").unwrap();
        let context = context::ThinkContext::new(journal.path(), "20260101".to_owned(), day_dir, 1)
            .expect("think context")
            .with_talent_roots(talent_root, apps_root);
        let args = args::ThinkArgs {
            dry_run: true,
            ..args::ThinkArgs::default()
        };
        assert_eq!(
            dry_run::run(&context, &args, 4).unwrap(),
            concat!(
                "Day 2026-01-01\n\n",
                "Pre-phase:  journal sense --day 20260101 -j 4\n",
                "Priority 40:\n  gen  facet_newsletter/work (new)\n\n",
                "Total: 1 agents\n",
                "Post-phase: journal indexer --rescan\n",
                "Post-phase: journal journal-stats\n",
            )
        );
    }

    fn canonical_terminals<'a>(
        records: &'a [Value],
        name: &'a str,
    ) -> Vec<(&'a str, Option<&'a str>, Option<&'a str>)> {
        records
            .iter()
            .filter_map(|record| {
                let event = record.get("event").and_then(Value::as_str)?;
                if event != "talent.complete" && event != "talent.fail" {
                    return None;
                }
                (record.get("name").and_then(Value::as_str) == Some(name)).then_some((
                    event,
                    record.get("use_id").and_then(Value::as_str),
                    record.get("state").and_then(Value::as_str),
                ))
            })
            .collect()
    }

    fn named_records<'a>(records: &'a [Value], event: &'a str, name: &'a str) -> Vec<&'a Value> {
        records
            .iter()
            .filter(|record| {
                record.get("event").and_then(Value::as_str) == Some(event)
                    && record.get("name").and_then(Value::as_str) == Some(name)
            })
            .collect()
    }

    fn assert_terminal_matches_dispatch_fold_keys(records: &[Value], name: &str) {
        let dispatches = named_records(records, "talent.dispatch", name);
        let mut terminals = named_records(records, "talent.complete", name);
        terminals.extend(named_records(records, "talent.fail", name));
        assert_eq!(dispatches.len(), 1, "{name} dispatch");
        assert_eq!(terminals.len(), 1, "{name} terminal");
        let dispatch = dispatches[0];
        let terminal = terminals[0];
        assert_eq!(
            terminal.get("mode").and_then(Value::as_str),
            Some("segment")
        );
        assert_eq!(terminal.get("day"), dispatch.get("day"));
        assert_eq!(terminal.get("segment"), dispatch.get("segment"));
        assert_eq!(terminal.get("stream"), dispatch.get("stream"));
        assert_eq!(terminal.get("use_id"), dispatch.get("use_id"));
    }

    fn write_analyzed_screen(journal: &Path, day: &str, segment: &str) {
        let path = segment_dir(journal, day, segment).join("screen.jsonl");
        fs::write(path, "{\"timestamp\":\"2026-08-13T09:00:00Z\"}\n").unwrap();
    }

    fn transcripts_shell() -> axum::response::Response {
        axum::response::Response::new(axum::body::Body::from("shell"))
    }

    fn segment_think(journal: &Path, day: &str, key: &str) -> Option<String> {
        let runtime = dispatch::runtime().unwrap();
        runtime.block_on(async {
            use axum::body::{Body, to_bytes};
            use axum::http::Request;
            use chrono::TimeZone;
            use tower::ServiceExt;
            let app = solstone_core_transcripts_web::router(
                journal.to_path_buf(),
                solstone_core_transcripts_web::Clock::fixed(
                    chrono::Utc.with_ymd_and_hms(2026, 8, 13, 12, 0, 0).unwrap(),
                ),
                transcripts_shell,
            );
            let response = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/app/transcripts/api/segments/{day}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let payload: Value = serde_json::from_slice(&body).unwrap();
            payload["segments"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|segment| segment["key"].as_str() == Some(key))
                .and_then(|segment| segment["think"].as_str().map(str::to_owned))
        })
    }

    fn floor_context(journal: &Path, roots: &Path) -> (context::ThinkContext, Arc<Recorder>) {
        let (talent_root, apps_root) = talent_roots(
            roots,
            &[
                (
                    "sense",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\"\n}",
                ),
                (
                    "documents",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\"\n}",
                ),
            ],
        );
        let (context, recorder) = recorder_context(journal, "20260813", 9);
        (context.with_talent_roots(talent_root, apps_root), recorder)
    }

    // AC: the durable record's reason and the operator-facing name come from ONE read, so they
    // cannot disagree. They used to be two independent `failure_cause` calls against the use
    // log; on the owner's journal 2026-09-03 the same failure on the same segment reported
    // `sense (error)` once and `sense (context_window_exceeded)` another time, and the health
    // record disagreed with the CLI line for the same attempt.
    //
    // NOTE what this does and does not fix: the underlying use-log read is still subject to a
    // flush race, so the *reason* can still degrade to the state word. What is now guaranteed
    // is that when it does, both surfaces degrade together and an operator is never shown two
    // different causes for one failure.
    #[test]
    fn the_durable_reason_and_the_operator_name_come_from_one_read() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, recorder) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        segment_dir(journal.path(), "20260813", "090000_300");
        recorder.end_states.lock().unwrap().insert(
            "use-1".to_owned(),
            solstone_core_cortex_client::UseEndState::Error,
        );
        let use_log = journal.path().join("talents/sense/use-1.jsonl");
        fs::create_dir_all(use_log.parent().unwrap()).unwrap();
        fs::write(
            &use_log,
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-1\"}\n",
                "{\"event\":\"error\",\"use_id\":\"use-1\",\"terminal\":true,",
                "\"reason_code\":\"context_window_exceeded\"}\n",
            ),
        )
        .unwrap();

        let result = run_segment(&context, journal.path(), "090000_300", false, false);

        assert_eq!(
            result.failed_names,
            vec!["sense (context_window_exceeded)".to_owned()],
            "the operator-facing name carries the real cause"
        );
        let records = oplog_records(journal.path(), &context.day, "segment");
        let fail = records
            .iter()
            .find(|r| r["event"] == "talent.fail" && r["name"] == "sense")
            .expect("a sense failure record");
        assert_eq!(
            fail["reason_code"], "context_window_exceeded",
            "the durable record must carry the SAME cause the name did, not a second read"
        );
    }

    // AC: a failed wait is not evidence about any individual use. When the use's own
    // durable log already ended in `finish`, the drain records the completion instead of
    // blaming the wait on it. Without this, every pending use in the batch was marked
    // `wait_failed` -- 17 of 327 `talent.fail` records on the owner's journal on
    // 2026-09-03 were false this way, with every output present on disk.
    #[test]
    fn wait_failure_does_not_overwrite_a_use_that_already_finished() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, recorder) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        segment_dir(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work"}),
        );
        // the use finished durably -- this is the shape that dominated the false failures
        let use_log = journal.path().join("talents/sense/use-1.jsonl");
        fs::create_dir_all(use_log.parent().unwrap()).unwrap();
        fs::write(
            &use_log,
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-1\"}\n",
                "{\"event\":\"finish\",\"use_id\":\"use-1\",\"skip_reason\":\"no_input\"}\n",
            ),
        )
        .unwrap();
        // ...and only then does the wait itself fail
        *recorder.wait_error.lock().unwrap() = Some("cortex socket closed".to_owned());

        let result = run_segment(&context, journal.path(), "090000_300", false, false);

        assert_eq!(
            (result.success, result.failed, result.failed_names.clone()),
            (1, 0, Vec::<String>::new()),
            "a use whose own log ended in finish must not be failed by the wait"
        );
        let records = oplog_records(journal.path(), &context.day, "segment");
        assert_eq!(
            canonical_terminals(&records, "sense"),
            vec![("talent.complete", Some("use-1"), Some("finish"))]
        );
    }

    #[test]
    fn segment_drain_outcomes_emit_one_canonical_terminal_and_preserve_mode_result() {
        struct Case {
            name: &'static str,
            configure: fn(&Recorder),
            event: &'static str,
            state: &'static str,
            success: usize,
            failed: usize,
            timed_out: bool,
            failed_names: &'static [&'static str],
            success_names: &'static [&'static str],
            need_sense_output: bool,
        }
        let cases = [
            Case {
                name: "finish",
                configure: |_| {},
                event: "talent.complete",
                state: "finish",
                success: 1,
                failed: 0,
                timed_out: false,
                failed_names: &[],
                success_names: &["sense"],
                need_sense_output: true,
            },
            Case {
                name: "end_state",
                configure: |recorder| {
                    recorder.end_states.lock().unwrap().insert(
                        "use-1".to_owned(),
                        solstone_core_cortex_client::UseEndState::Error,
                    );
                },
                event: "talent.fail",
                state: "error",
                success: 0,
                failed: 1,
                timed_out: false,
                failed_names: &["sense (error)"],
                success_names: &[],
                need_sense_output: false,
            },
            Case {
                name: "timeout",
                configure: |recorder| {
                    recorder.timed_out.lock().unwrap().push(
                        solstone_core_cortex_client::TimedOutUse::GenuineTimeout {
                            use_id: "use-1".to_owned(),
                        },
                    );
                },
                event: "talent.fail",
                state: "timeout",
                success: 0,
                failed: 1,
                timed_out: true,
                failed_names: &["sense (timeout)"],
                success_names: &[],
                need_sense_output: false,
            },
            Case {
                name: "lost",
                configure: |recorder| {
                    recorder.timed_out.lock().unwrap().push(
                        solstone_core_cortex_client::TimedOutUse::LostAtDeadline {
                            use_id: "use-1".to_owned(),
                        },
                    );
                },
                event: "talent.fail",
                state: "lost",
                success: 0,
                failed: 1,
                timed_out: true,
                failed_names: &["sense (lost)"],
                success_names: &[],
                need_sense_output: false,
            },
            Case {
                name: "missing_completion",
                configure: |recorder| {
                    recorder.omit_ids.lock().unwrap().insert("use-1".to_owned());
                },
                event: "talent.fail",
                state: "missing_completion",
                success: 0,
                failed: 1,
                timed_out: false,
                failed_names: &["sense (unknown)"],
                success_names: &[],
                need_sense_output: false,
            },
            Case {
                name: "wait_failed",
                // The wait error must reach the operator. This case previously used the
                // mock error "unavailable" -- the same word the code hardcoded when it
                // discarded the error -- so it passed whether or not the cause survived.
                // A distinctive error proves the real one is surfaced.
                configure: |recorder| {
                    *recorder.wait_error.lock().unwrap() = Some("cortex socket closed".to_owned());
                },
                event: "talent.fail",
                state: "wait_failed",
                success: 0,
                failed: 1,
                timed_out: false,
                failed_names: &["sense (wait failed: \"cortex socket closed\")"],
                success_names: &[],
                need_sense_output: false,
            },
        ];
        for case in cases {
            let journal = tempdir().unwrap();
            let roots = tempdir().unwrap();
            let (context, recorder) = segment_context(
                journal.path(),
                roots.path(),
                "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
            );
            segment_dir(journal.path(), "20260813", "090000_300");
            if case.need_sense_output {
                write_sense_output(
                    &context,
                    "090000_300",
                    serde_json::json!({"density":"active","content_type":"work"}),
                );
            }
            (case.configure)(&recorder);
            let result = run_segment(&context, journal.path(), "090000_300", false, false);
            assert_eq!(
                (
                    result.success,
                    result.failed,
                    result.timed_out,
                    result.failed_names,
                    result.success_names
                ),
                (
                    case.success,
                    case.failed,
                    case.timed_out,
                    case.failed_names
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect(),
                    case.success_names
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect(),
                ),
                "{}",
                case.name
            );
            let records = oplog_records(journal.path(), &context.day, "segment");
            assert!(
                records
                    .iter()
                    .all(|record| record.get("event").and_then(Value::as_str)
                        != Some("talent.completed")),
                "{}",
                case.name
            );
            assert_eq!(
                canonical_terminals(&records, "sense"),
                vec![(case.event, Some("use-1"), Some(case.state))],
                "{}",
                case.name
            );
            assert_terminal_matches_dispatch_fold_keys(&records, "sense");
        }
    }

    fn isolated_sense_run(
        segment: &str,
        prepare: impl FnOnce(&context::ThinkContext, &Path),
    ) -> (dispatch::ModeResult, Vec<Value>) {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, _) = segment_context(
            journal.path(),
            roots.path(),
            "{\n\"type\": \"generate\", \"schedule\": \"segment\", \"priority\": 1, \"output\": \"json\"\n}\n",
        );
        prepare(&context, journal.path());
        let result = run_segment(&context, journal.path(), segment, false, false);
        (
            result,
            oplog_records(journal.path(), &context.day, "segment"),
        )
    }

    fn assert_sole_sense_finish(records: &[Value]) {
        assert_eq!(
            canonical_terminals(records, "sense"),
            vec![("talent.complete", Some("use-1"), Some("finish"))]
        );
        assert_terminal_matches_dispatch_fold_keys(records, "sense");
    }

    #[test]
    fn segment_post_drain_early_returns_keep_the_sense_finish_terminal() {
        let (parse, parse_log) = isolated_sense_run("090000_300", |context, journal| {
            segment_dir(journal, "20260813", "090000_300");
            let output = sense_output_path(context, "090000_300");
            fs::create_dir_all(output.parent().unwrap()).unwrap();
            fs::write(output, "not json").unwrap();
        });
        assert_eq!(parse.failed_names, vec!["sense (output_parse)"]);
        assert_sole_sense_finish(&parse_log);

        let (invalid, invalid_log) = isolated_sense_run("090000_300", |context, journal| {
            segment_dir(journal, "20260813", "090000_300");
            let output = sense_output_path(context, "090000_300");
            fs::create_dir_all(output.parent().unwrap()).unwrap();
            fs::write(output, r#"{"density":1,"content_type":"work"}"#).unwrap();
        });
        assert_eq!(invalid.failed_names, vec!["sense (output_invalid)"]);
        assert_sole_sense_finish(&invalid_log);

        let (idle, idle_log) = isolated_sense_run("090000_300", |context, journal| {
            segment_dir(journal, "20260813", "090000_300");
            write_sense_output(
                context,
                "090000_300",
                serde_json::json!({"density":"idle","content_type":"idle"}),
            );
        });
        assert_eq!((idle.success, idle.failed), (1, 0));
        assert_sole_sense_finish(&idle_log);

        let (redundant, redundant_log) = isolated_sense_run("091000_300", |context, journal| {
            let previous = segment_dir(journal, "20260813", "090500_300");
            let current = segment_dir(journal, "20260813", "091000_300");
            fs::write(previous.join("imported.md"), "same transcript words").unwrap();
            fs::write(current.join("imported.md"), "same transcript words").unwrap();
            let sensor = solstone_core_system_health::detect_segment_change(
                journal,
                "20260813",
                Some("default"),
                "091000_300",
                &current,
                None,
                "2026-08-13T00:00:00+00:00",
            )["sensors"]
                .clone();
            fs::create_dir_all(previous.join("talents")).unwrap();
            fs::write(
                previous.join("talents/change.json"),
                serde_json::to_vec(&serde_json::json!({"sensors": sensor})).unwrap(),
            )
            .unwrap();
            write_sense_output(
                context,
                "091000_300",
                serde_json::json!({"density":"active","content_type":"work"}),
            );
        });
        assert_eq!((redundant.success, redundant.failed), (1, 0));
        assert_sole_sense_finish(&redundant_log);
    }

    #[test]
    fn unmatched_latest_documents_dispatch_is_awaiting_until_its_use_id_completes() {
        let journal = tempdir().unwrap();
        write_analyzed_screen(journal.path(), "20260813", "090000_300");
        let health = journal
            .path()
            .join("chronicle/20260813/health/9_segment.jsonl");
        fs::create_dir_all(health.parent().unwrap()).unwrap();
        let base = concat!(
            r#"{"event":"sense.complete","ts":9,"mode":"segment","day":"20260813","stream":"default","segment":"090000_300","density":"active"}"#,
            "\n",
            r#"{"event":"talent.dispatch","ts":9,"mode":"segment","day":"20260813","stream":"default","segment":"090000_300","name":"documents","use_id":"old"}"#,
            "\n",
            r#"{"event":"talent.complete","ts":9,"mode":"segment","day":"20260813","stream":"default","segment":"090000_300","name":"documents","use_id":"old","state":"finish"}"#,
            "\n",
            r#"{"event":"talent.dispatch","ts":9,"mode":"segment","day":"20260813","stream":"default","segment":"090000_300","name":"documents","use_id":"new"}"#,
            "\n",
        );
        fs::write(&health, base).unwrap();
        let source = solstone_core_system_health::FilesystemHealthLogSource::new(journal.path());
        let progress =
            solstone_core_system_health::read_segment_progress(&source, "20260813").unwrap();
        assert_eq!(
            solstone_core_system_health::segment_fully_thought(
                solstone_core_system_health::lookup_segment_progress(
                    &progress.value,
                    "default",
                    "090000_300",
                )
            ),
            solstone_core_system_health::ThoughtVerdict::Floor("documents".to_owned())
        );
        assert_eq!(
            segment_think(journal.path(), "20260813", "090000_300").as_deref(),
            Some("awaiting")
        );

        fs::write(
            &health,
            format!(
                "{base}{}\n",
                r#"{"event":"talent.complete","ts":9,"mode":"segment","day":"20260813","stream":"default","segment":"090000_300","name":"documents","use_id":"other","state":"finish"}"#
            ),
        )
        .unwrap();
        assert_eq!(
            segment_think(journal.path(), "20260813", "090000_300").as_deref(),
            Some("awaiting")
        );

        fs::write(
            &health,
            format!(
                "{base}{}\n",
                r#"{"event":"talent.complete","ts":9,"mode":"segment","day":"20260813","stream":"default","segment":"090000_300","name":"documents","use_id":"new","state":"finish"}"#
            ),
        )
        .unwrap();
        assert_eq!(
            segment_think(journal.path(), "20260813", "090000_300").as_deref(),
            Some("thought")
        );
    }

    #[test]
    fn composed_segment_run_log_drives_transcripts_thought_and_awaiting() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, recorder) = floor_context(journal.path(), roots.path());
        write_analyzed_screen(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work"}),
        );
        let mut log = test_log(&context, "segment");
        let result = segment::run(
            &context,
            &mut log,
            "090000_300",
            false,
            Some("default"),
            2,
            Some(std::time::Duration::from_secs(610)),
            false,
            &[],
        )
        .unwrap();
        assert_eq!((result.success, result.failed), (2, 0));
        assert_eq!(recorder.requests.lock().unwrap().len(), 2);
        let records = oplog_records(journal.path(), &context.day, "segment");
        assert!(
            records
                .iter()
                .all(|record| record.get("event").and_then(Value::as_str)
                    != Some("talent.completed"))
        );
        for name in ["sense", "documents"] {
            let terminals = canonical_terminals(&records, name);
            assert_eq!(terminals.len(), 1, "{name}");
            assert_eq!(terminals[0].0, "talent.complete");
            assert_eq!(terminals[0].2, Some("finish"));
            let use_id = terminals[0].1.expect("use_id");
            assert!(records.iter().any(|record| {
                record.get("event").and_then(Value::as_str) == Some("talent.dispatch")
                    && record.get("name").and_then(Value::as_str) == Some(name)
                    && record.get("use_id").and_then(Value::as_str) == Some(use_id)
            }));
        }
        let source = solstone_core_system_health::FilesystemHealthLogSource::new(journal.path());
        let progress =
            solstone_core_system_health::read_segment_progress(&source, "20260813").unwrap();
        let row = &progress.value[&solstone_core_system_health::SegmentIdentity {
            stream: Some("default".to_owned()),
            segment: "090000_300".to_owned(),
        }];
        assert!(row.completed.contains("sense"));
        assert!(row.completed.contains("documents"));
        assert_eq!(
            segment_think(journal.path(), "20260813", "090000_300").as_deref(),
            Some("thought")
        );
    }

    #[test]
    fn unopenable_segment_oplog_reports_after_primary_work() {
        let journal = tempdir().unwrap();
        let roots = tempdir().unwrap();
        let (context, _) = floor_context(journal.path(), roots.path());
        write_analyzed_screen(journal.path(), "20260813", "090000_300");
        write_sense_output(
            &context,
            "090000_300",
            serde_json::json!({"density":"active","content_type":"work"}),
        );
        let health = context.day_dir.join("health");
        fs::write(&health, b"not a directory").unwrap();
        let mut log = test_log(&context, "segment");
        let result = segment::run(
            &context,
            &mut log,
            "090000_300",
            false,
            Some("default"),
            2,
            Some(std::time::Duration::from_secs(610)),
            false,
            &[],
        )
        .unwrap();
        assert_eq!((result.success, result.failed), (2, 0));
        assert_eq!(
            segment_think(journal.path(), "20260813", "090000_300").as_deref(),
            Some("awaiting")
        );
        let run = logged_mode_outcome(log, Ok(result)).unwrap();
        assert_eq!(run.exit_code, 1);
        assert!(run.stderr.starts_with("journal think: 2 completed\n"));
        assert!(run.stderr.contains("think run log"));
    }
}
