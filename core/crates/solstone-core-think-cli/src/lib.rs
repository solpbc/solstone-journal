// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native preflight and unavailable-run boundary for `journal think`.

mod args;
#[allow(
    dead_code,
    reason = "Wave 1 exposes cadence state before native run modes are enabled."
)]
mod cadence;
mod day;
mod gate;
#[allow(
    dead_code,
    reason = "Wave 1 exposes the run-log writer seam before native run modes are enabled."
)]
mod run_log;
mod workers;

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
    Unavailable,
}

pub fn run_cli(args: &[String], journal: &Path) -> CliRun {
    run_cli_with(
        args,
        journal,
        |name| std::env::var(name).ok(),
        || solstone_core_segment::is_solstone_up(journal),
        || Local::now().date_naive(),
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
        || None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_cli_with<E, C, N, P, R, B>(
    raw_args: &[String],
    journal: &Path,
    lookup_env: E,
    connectivity: C,
    clock: N,
    cpu_count: P,
    endpoint: R,
    bundled_slots: B,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
    N: Fn() -> NaiveDate,
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
        day::create_day(journal, &selected_day)
            .map_err(|message| CliError::InvalidDay { message })?;
        let (uses_local, endpoint) = endpoint();
        validate(&parsed, cpu_count(), uses_local, endpoint, bundled_slots())?;

        // `EXIT_UNAVAILABLE` already means this route is unavailable in this build;
        // this message, rather than the code, identifies the unavailable think run.
        // Intentional divergence: run-mode-bound inputs, including --dry-run, do not
        // execute the retained Python run and exit 69 rather than succeeding.
        Err(CliError::Unavailable)
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
        Err(CliError::Unavailable) => CliRun {
            stdout: String::new(),
            stderr: "journal think: native run mode is unavailable in this build\n".to_owned(),
            exit_code: 69,
        },
    }
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
        let workers = args.segment_workers.unwrap_or_else(|| {
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
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::{self, Write};
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, Once, OnceLock};

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
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || Some(2),
        )
    }

    fn run(args: &[&str]) -> CliRun {
        let journal = tempdir().expect("journal");
        run_at(journal.path(), args)
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
                &[],
                journal.path(),
                move |name| env.and_then(|(key, value)| (name == key).then(|| value.to_owned())),
                move || up,
                || NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
                || Some(8),
                || (false, LocalEndpointResolution::Bundled),
                || None,
            )
        };
        assert_eq!(
            base(Some(("SOL_SKIP_SUPERVISOR_CHECK", "1")), false).exit_code,
            69
        );
        assert_eq!(base(None, true).exit_code, 69);
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
            || Some(8),
            || (false, LocalEndpointResolution::Bundled),
            || None,
        );
        assert_eq!(output.exit_code, 0);
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
        assert_eq!(result.exit_code, 69);
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
    fn worker_refusal_uses_pinned_counts() {
        assert_eq!(
            run(&["--segments", "--jobs", "0", "--segment-workers", "2"]).exit_code,
            2
        );
        assert_eq!(
            run(&["--segments", "--jobs", "0", "--segment-workers", "1"]).exit_code,
            69
        );
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
    fn mode_derivation_covers_reachable_modes() {
        for (args, expected) in [
            (
                vec!["--activity", "a", "--facet", "f", "--day", "20260813"],
                "activity",
            ),
            (vec!["--flush", "--segment", "x"], "flush"),
            (vec!["--segments"], "segment"),
            (vec!["--weekly"], "weekly"),
            (vec![], "daily"),
        ] {
            let args::ParseOutcome::Args(parsed) =
                args::parse(&args.iter().map(|item| item.to_string()).collect::<Vec<_>>()).unwrap()
            else {
                panic!("mode test must parse arguments");
            };
            assert_eq!(run_log::mode(&parsed), expected);
        }
        // Cadence's writer is unreachable in this wave.
    }

    #[test]
    fn criterion_sixteen_cadence_round_trips_and_save_replaces_previous_state() {
        let journal = tempdir().unwrap();
        fs::create_dir_all(journal.path().join("health")).unwrap();
        let initial = BTreeMap::from([("one".to_owned(), 12), ("two".to_owned(), 24)]);
        cadence::save(journal.path(), &initial).unwrap();
        assert_eq!(cadence::load(journal.path()), initial);
        let replacement = BTreeMap::from([("two".to_owned(), 36)]);
        cadence::save(journal.path(), &replacement).unwrap();
        assert_eq!(cadence::load(journal.path()), replacement);
        assert!(
            !fs::read_to_string(journal.path().join("health/cadence.json"))
                .unwrap()
                .contains("one")
        );
    }

    #[test]
    fn criterion_sixteen_cadence_reads_corrupt_and_non_object_state_leniently() {
        let _log_guard = capture_logs();
        let journal = tempdir().unwrap();
        let path = journal.path().join("health/cadence.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let corrupt = b"{ definitely not json }";
        fs::write(&path, corrupt).unwrap();
        assert!(cadence::load(journal.path()).is_empty());
        assert_eq!(warnings().len(), 1);
        assert_eq!(fs::read(&path).unwrap(), corrupt);

        LOGS.get().unwrap().lock().unwrap().clear();
        fs::write(&path, "[]").unwrap();
        assert!(cadence::load(journal.path()).is_empty());
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
}
