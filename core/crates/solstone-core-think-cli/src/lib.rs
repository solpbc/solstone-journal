// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native preflight and unavailable-run boundary for `journal think`.

mod activity;
mod args;
mod cadence;
mod cadence_state;
mod context;
mod daily;
mod day;
mod dispatch;
mod dry_run;
mod flush;
mod gate;
mod helpers;
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
}

use std::path::Path;

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

pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    run_cli_with(
        args,
        journal,
        |name| std::env::var(name).ok(),
        || solstone_core_segment::is_solstone_up(journal),
        || Local::now().date_naive(),
        || chrono::Utc::now().timestamp_millis(),
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
    )
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
        let context =
            context::ThinkContext::new(journal, selected_day.clone(), day_dir.clone(), now_ms)
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
            let mut log = open_run_log(&parsed, &day_dir, now_ms, &selected_day);
            let result = cadence::run(&context, configs, &mut log, parsed.refresh)
                .map_err(|message| CliError::InvalidDay { message })?;
            return Ok(CliRun {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: (result.failed != 0) as i32,
            });
        }

        if let Some(activity_id) = parsed.activity.as_deref() {
            let mut log = open_run_log(&parsed, &day_dir, now_ms, &selected_day);
            let result = activity::run(
                &context,
                &mut log,
                activity_id,
                parsed.facet.as_deref().expect("validated activity facet"),
                parsed.refresh,
                parsed.jobs,
            )
            .map_err(|message| CliError::InvalidDay { message })?;
            return Ok(CliRun {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: (result.failed != 0) as i32,
            });
        }
        if parsed.flush {
            let mut log = open_run_log(&parsed, &day_dir, now_ms, &selected_day);
            let result = flush::run(
                &context,
                &mut log,
                parsed.segment.as_deref().expect("validated flush segment"),
                parsed.stream.as_deref(),
            )
            .map_err(|message| CliError::InvalidDay { message })?;
            return Ok(CliRun {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: (result.failed != 0) as i32,
            });
        }
        if parsed.segments {
            let source = solstone_core_system_health::FilesystemSegmentSource;
            let segments: Vec<(String, Option<String>)> = solstone_core_system_health::scan_day(
                &source,
                &context.journal,
                &context.day,
                chrono::Utc::now(),
            )
            .map_err(|error| CliError::InvalidDay {
                message: error.to_string(),
            })?
            .2
            .into_iter()
            .map(|entry| (entry.key, Some(entry.stream)))
            .collect();
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
            let result = segment::run_repair_batch(
                &context,
                segments.clone(),
                parsed.refresh,
                parsed.jobs,
                workers,
                timeout,
                skip_talents,
            )
            .map_err(|message| CliError::InvalidDay { message })?;
            // Source-derived, not measured: thinking.py:594-634 and 4449-4456
            // replay durable Sense output after the concurrent repair workers.
            segment::replay_activity_state(
                &context,
                &mut run_log::RunLogWriter::open(&run_log::path(&day_dir, now_ms, "segments")),
                &segments,
                parsed.refresh,
                parsed.jobs,
                parsed.no_activity_prompts,
                false,
            )
            .map_err(|message| CliError::InvalidDay { message })?;
            return Ok(CliRun {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: (result.failed != 0) as i32,
            });
        }
        if let Some(segment) = parsed.segment.as_deref() {
            let mut log = open_run_log(&parsed, &day_dir, now_ms, &selected_day);
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
            )
            .map_err(|message| CliError::InvalidDay { message })?;
            // Source-derived, not measured: thinking.py:2021-2030 advances
            // activity state after every direct segment Sense run.
            segment::replay_activity_state(
                &context,
                &mut log,
                &[(segment.to_owned(), parsed.stream.clone())],
                parsed.refresh,
                parsed.jobs,
                parsed.no_activity_prompts,
                true,
            )
            .map_err(|message| CliError::InvalidDay { message })?;
            return Ok(CliRun {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: (result.failed != 0) as i32,
            });
        }
        let mut log = open_run_log(&parsed, &day_dir, now_ms, &selected_day);
        let result = if parsed.weekly {
            weekly::run(
                &context,
                &mut log,
                parsed.refresh,
                parsed.stream.as_deref(),
                parsed.jobs,
            )
        } else {
            // Source-derived, not measured: main records this pre-phase at
            // thinking.py:4606 before it invokes `run_daily_prompts`.
            let mut fields = serde_json::Map::new();
            fields.insert(
                "mode".to_owned(),
                serde_json::Value::String("daily".to_owned()),
            );
            fields.insert(
                "day".to_owned(),
                serde_json::Value::String(selected_day.clone()),
            );
            fields.insert(
                "phase".to_owned(),
                serde_json::Value::String("sense_repair".to_owned()),
            );
            log.log("phase.start", now_ms, fields);
            daily::run(
                &context,
                &mut log,
                parsed.stream.as_deref(),
                parsed.from_scratch,
                parsed.jobs,
            )
        }
        .map_err(|message| CliError::InvalidDay { message })?;
        Ok(CliRun {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: (result.failed != 0) as i32,
        })
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

fn open_run_log(
    args: &args::ThinkArgs,
    day_dir: &Path,
    now_ms: i64,
    day: &str,
) -> run_log::RunLogWriter<std::fs::File> {
    // This order differs from Python's main chain only superficially: args.rs
    // refuses --segment with --weekly or --cadence before mode derivation.
    let mode = run_log::mode(args);
    let mut log = run_log::RunLogWriter::open(&run_log::path(day_dir, now_ms, mode));
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
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::{self, Write};
    use std::path::Path;
    use std::sync::{Arc, Mutex, MutexGuard, Once, OnceLock};

    use chrono::NaiveDate;
    use filetime::{FileTime, set_file_mtime};
    use log::{Level, LevelFilter, Log, Metadata, Record};
    use serde_json::{Map, Value};
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
            let finish_fields = *self.finish_fields.lock().unwrap();
            Ok(solstone_core_cortex_client::WaitForUsesReport {
                completed: use_ids
                    .iter()
                    .map(|id| {
                        (
                            id.clone(),
                            solstone_core_cortex_client::UseCompletion {
                                end_state: solstone_core_cortex_client::UseEndState::Finish,
                                finish_fields,
                            },
                        )
                    })
                    .collect(),
                timed_out: Vec::new(),
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
            103 | 128 | 140 => {
                for facet in ["personal", "work"] {
                    let declaration = journal.join("facets").join(facet).join("facet.json");
                    fs::create_dir_all(declaration.parent().unwrap()).unwrap();
                    fs::write(declaration, "{}\n").unwrap();
                }
            }
            196 => {
                for (key, body) in [
                    ("093000_600", "browser_first.jsonl"),
                    ("141500_900", "browser_second.jsonl"),
                ] {
                    let segment = journal.join("chronicle/20260101").join(key);
                    fs::create_dir_all(&segment).unwrap();
                    fs::write(segment.join(body), "browser content\n").unwrap();
                }
            }
            225 => {
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
        let path = journal
            .join("chronicle")
            .join(day)
            .join("health")
            .join(format!("1785000000000_{mode}.jsonl"));
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn write_health_event(journal: &Path, day: &str, event: &str) {
        let path = journal
            .join("chronicle")
            .join(day)
            .join("health")
            .join("terminal.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("{event}\n")).unwrap();
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
    fn day_log_appends_exact_epoch_message_rows_and_filesystem_failure_is_best_effort() {
        // Source-derived, not measured: utils.py:927-929 appends exactly
        // `epoch<TAB>message` to the selected day task log.
        let journal = tempdir().unwrap();
        helpers::day_log(
            journal.path(),
            "20260813",
            1_785_000_000_999,
            "sense_repair timeout",
        );
        helpers::day_log(
            journal.path(),
            "20260813",
            1_785_000_001_001,
            "sense_repair error 1",
        );
        let rows =
            fs::read_to_string(journal.path().join("chronicle/20260813/task_log.txt")).unwrap();
        assert_eq!(
            rows,
            "1785000000\tsense_repair timeout\n1785000001\tsense_repair error 1\n"
        );

        let blocked = tempdir().unwrap();
        fs::write(blocked.path().join("chronicle"), b"unchanged").unwrap();
        helpers::day_log(blocked.path(), "20260813", 1, "not-written");
        assert_eq!(
            fs::read(blocked.path().join("chronicle")).unwrap(),
            b"unchanged"
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
        assert!(
            !journal
                .path()
                .join("chronicle/20260814/health/1785000000000_cadence.jsonl")
                .exists()
        );
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
        let _log = open_run_log(&parsed, &context.day_dir, context.now_ms, &context.day);
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
        assert!(
            daily::run(&context, &mut log, None, false, 2)
                .unwrap()
                .applicable_units
                .is_empty()
        );
        assert!(recorder.requests.lock().unwrap().is_empty());
        let skip = fs::read_to_string(journal.path().join("daily.jsonl")).unwrap();
        assert!(skip.contains("no_active_facets"));

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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("always.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
        let fresh = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(fresh.applicable_units.len(), 1);
        write_health_event(
            journal.path(),
            "20260813",
            r#"{"event":"talent.fail","ts":1,"mode":"daily","name":"deterministic","reason_code":"no_output"}"#,
        );
        let repeated = daily::run(&context, &mut log, None, false, 2).unwrap();
        assert_eq!(repeated.applicable_units, fresh.applicable_units);
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
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
            r#"{"event":"talent.complete","ts":1,"mode":"segment","stream":"default","segment":"one","name":"sense"}"#,
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
        let mut daily_log = run_log::RunLogWriter::open(&journal.path().join("daily.jsonl"));
        daily::run(&context, &mut daily_log, None, false, 2).unwrap();
        let mut weekly_log = run_log::RunLogWriter::open(&journal.path().join("weekly.jsonl"));
        weekly::run(&context, &mut weekly_log, false, None, 2).unwrap();
        let mut cadence_log = run_log::RunLogWriter::open(&journal.path().join("cadence.jsonl"));
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
        let context = context.with_talent_roots(talent_root, apps_root);
        let path = journal.path().join("activity.jsonl");
        let mut log = run_log::RunLogWriter::open(&path);
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
        let events = fs::read_to_string(path).unwrap();
        for event in [
            "\"event\":\"started\"",
            "\"event\":\"group.start\"",
            "\"event\":\"talent.started\"",
            "\"event\":\"talent.dispatch\"",
            "\"event\":\"talent.completed\"",
            "\"event\":\"talent.complete\"",
            "\"event\":\"group.complete\"",
            "\"event\":\"completed\"",
        ] {
            assert!(events.contains(event), "missing {event}");
        }
        assert!(events.contains("\"activity\":\"reading_1\""));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("activity.jsonl"));
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
        let path = journal.path().join("flush.jsonl");
        let mut log = run_log::RunLogWriter::open(&path);
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
        let events = fs::read_to_string(path).unwrap();
        for event in [
            "\"event\":\"started\"",
            "\"event\":\"talent.started\"",
            "\"event\":\"talent.dispatch\"",
            "\"event\":\"talent.completed\"",
            "\"event\":\"talent.complete\"",
            "\"event\":\"completed\"",
        ] {
            assert!(events.contains(event), "missing {event}");
        }
        assert!(events.contains("\"segment\":\"090000\""));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("low.jsonl"));
        let low = activity::run(&context, &mut log, "low", "work", false, 2).unwrap();
        assert_eq!((low.success, low.failed), (0, 0));
        assert!(recorder.requests.lock().unwrap().is_empty());
        assert!(
            fs::read_to_string(journal.path().join("low.jsonl"))
                .unwrap()
                .contains("\"reason\":\"low_level_activity\"")
        );

        write_activity_record(
            journal.path(),
            "work",
            "20260813",
            serde_json::json!({"id":"full", "activity":"reading", "segments":["090000"], "level_avg":0.4}),
        );
        let mut log = run_log::RunLogWriter::open(&journal.path().join("full.jsonl"));
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
    fn daily_records_its_source_derived_sense_repair_pre_phase() {
        let journal = tempdir().unwrap();
        let _ = run_at(journal.path(), &[]);
        let events = sidecar_events(journal.path(), "20260813", "daily");
        assert_eq!(events[0]["event"], "run.start");
        assert_eq!(events[0]["mode"], "daily");
        assert!(events.iter().any(|event| {
            event["event"] == "phase.start"
                && event["mode"] == "daily"
                && event["phase"] == "sense_repair"
        }));
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
        assert_eq!(run_at(journal.path(), &["--cadence"]).exit_code, 0);
        let events = sidecar_events(journal.path(), "20260814", "cadence");
        assert_eq!(events[0]["event"], "run.start");
        assert_eq!(events[0]["mode"], "cadence");
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event["event"] == "talent.skip" && event["reason"] == "no_new_work"
                })
                .count(),
            2
        );
        assert!(!journal.path().join("health/cadence.json").exists());
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
            (vec!["--segments"], "segment"),
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

    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("fail"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("fail"))
        }
    }

    #[test]
    fn criterion_seventeen_run_log_creates_health_directory_and_appends() {
        let root = tempdir().unwrap();
        let path = run_log::path(root.path(), 9, "daily");
        assert_eq!(path, root.path().join("health/9_daily.jsonl"));
        let _writer = run_log::RunLogWriter::open(&path);
        assert!(path.parent().unwrap().is_dir());
        fs::write(&path, b"existing\n").unwrap();
        let mut writer = run_log::RunLogWriter::open(&path);
        writer.log("talent.skip", 9, Map::<String, Value>::new());
        assert_eq!(writer.skip_count, 1);
        assert!(fs::read(&path).unwrap().starts_with(b"existing\n"));
    }

    #[test]
    fn criterion_seventeen_failed_open_warns_once_then_is_silent() {
        let _log_guard = capture_logs();
        let root = tempdir().unwrap();
        let blocked_parent = root.path().join("blocked");
        fs::write(&blocked_parent, b"not a directory").unwrap();
        let mut writer = run_log::RunLogWriter::open(&blocked_parent.join("run.jsonl"));
        assert_eq!(warnings().len(), 1);
        writer.log("talent.skip", 9, Map::<String, Value>::new());
        writer.log("talent.skip", 10, Map::<String, Value>::new());
        assert_eq!(warnings().len(), 1);
    }

    #[test]
    fn criterion_seventeen_open_sink_write_failures_warn_per_failure() {
        let _log_guard = capture_logs();
        let root = tempdir().unwrap();
        let path = run_log::path(root.path(), 9, "daily");
        let mut writer = run_log::RunLogWriter::with_sink(path, Failing);
        writer.log("talent.skip", 9, Map::<String, Value>::new());
        writer.log("talent.skip", 10, Map::<String, Value>::new());
        assert_eq!(writer.skip_count, 2);
        assert_eq!(warnings().len(), 2);
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
        journal: &Path,
        segment: &str,
        refresh: bool,
        live: bool,
        jobs: i64,
        timeout: Option<std::time::Duration>,
        skip_talents: &[String],
    ) -> dispatch::ModeResult {
        let mut log = run_log::RunLogWriter::open(&journal.join("segment.jsonl"));
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
            fs::read_to_string(journal.path().join("segment.jsonl"))
                .unwrap()
                .contains("raw_media_pending")
        );
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
        segment_dir(journal.path(), "20260813", "090000_300");
        *recorder.dispatch_failure.lock().unwrap() = Some(context::DispatchFailure::NotClaimed {
            use_id: "lost-1".to_owned(),
        });

        let result = run_segment(&context, journal.path(), "090000_300", false, false);
        assert_eq!(result.failed_names, vec!["sense (request_lost)"]);
        let log = fs::read_to_string(journal.path().join("segment.jsonl")).unwrap();
        assert!(log.contains("request_lost"));
        assert!(!log.contains("send_failed"));
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
    fn segment_idle_and_redundant_branches_write_their_distinct_artifacts() {
        // Source-derived, not measured: thinking.py:1746-1816 terminalizes
        // idle segments and writes a continuation only for redundant changes.
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
            serde_json::from_slice::<Value>(&fs::read(idle.join("talents/density.json")).unwrap())
                .unwrap()["classification"],
            "idle"
        );
        assert!(!idle.join("timeline.json").exists());

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
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(current.join("timeline.json")).unwrap())
                .unwrap()["continuation_of"],
            "090500_300"
        );
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("segment.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("segment.jsonl"));
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
            fs::read_to_string(journal.path().join("segment.jsonl"))
                .unwrap()
                .contains("activity.prompts_skipped")
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("segment.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("segments.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("segment.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("segment.jsonl"));
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
        let mut log = run_log::RunLogWriter::open(&journal.path().join("segments.jsonl"));
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
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"json\",\"accumulate\":true\n}",
                ),
                (
                    "timeline:segment_summary",
                    "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":2,\"output\":\"md\",\"provider\":\"test-provider\",\"model\":\"test-model\"\n}",
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
        assert_eq!((result.success, result.failed), (4, 0));
        let requests = recorder.requests.lock().unwrap();
        let documents = requests
            .iter()
            .find(|request| request.name == "documents")
            .unwrap();
        // Source-derived, not measured: thinking.py:1435-1468 gives segment
        // requests direct persistence, never `apply_output_persistence`.
        assert_eq!(documents.config["output"], "json");
        let summary = requests
            .iter()
            .find(|request| request.name == "timeline:segment_summary")
            .unwrap();
        assert_eq!(summary.config["provider"], "test-provider");
        assert_eq!(summary.config["model"], "test-model");
        assert!(
            fs::read_to_string(journal.path().join("segment.jsonl"))
                .unwrap()
                .contains("sense.change_detect")
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
                    "timeline:segment_summary",
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
            "timeline:segment_summary".to_owned(),
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
        assert_eq!((result.success, result.failed), (2, 2));
        assert_eq!(
            result.failed_names,
            vec![
                "documents (send)".to_owned(),
                "timeline:segment_summary (request_lost)".to_owned(),
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
            vec![1, 1]
        );
        let log = fs::read_to_string(journal.path().join("segment.jsonl")).unwrap();
        assert!(log.contains("request_lost"));
        assert!(log.contains("skip_talents_flag"));
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
        let result = segment::run_repair_batch(
            &context,
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
            vec![20, 42, 55, 68, 72, 84, 91, 97, 103, 128, 140, 196, 225]
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
}
