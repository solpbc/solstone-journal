// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process implementation of the native `journal segment` command.

use solstone_core_cli_copy as copy;
use std::path::Path;

use solstone_core_segment::{
    LockOptions, SupervisorRefusal, is_solstone_up, require_solstone_with,
};

mod index;
mod location;
mod r#move;
mod read;

use location::SegmentLocation;
use r#move::{NativeOperations, SegmentOperations, build_plan, execute_plan, render_plan};
use read::{checks_in, day_segments, inspect_output, list_output, render_checks, split_path};

#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    run_cli_with(
        args,
        journal_path,
        |name| std::env::var(name).ok(),
        || is_solstone_up(journal_path),
    )
}

pub fn run_cli_with<E, C>(
    args: &[String],
    journal_path: &Path,
    lookup_env: E,
    connectivity: C,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    run_cli_with_lock_options(
        args,
        journal_path,
        lookup_env,
        connectivity,
        LockOptions::default(),
    )
}

fn run_cli_with_lock_options<E, C>(
    args: &[String],
    journal_path: &Path,
    lookup_env: E,
    connectivity: C,
    lock_options: LockOptions,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    run_cli_with_operations(
        args,
        journal_path,
        lookup_env,
        connectivity,
        lock_options,
        &NativeOperations,
    )
}

fn run_cli_with_operations<E, C>(
    args: &[String],
    journal_path: &Path,
    lookup_env: E,
    connectivity: C,
    lock_options: LockOptions,
    operations: &dyn SegmentOperations,
) -> CliRun
where
    E: Fn(&str) -> Option<String>,
    C: FnOnce() -> bool,
{
    let command = match parse(args) {
        Ok(Command::Help(name)) => return success(help(name)),
        Ok(command) => command,
        Err(error) => {
            let locale = output_locale(args.iter().any(|arg| arg == "--json"));
            return failure("", &format!("{}\n", error.render(locale)), 2);
        }
    };
    let locale = output_locale(matches!(
        command,
        Command::List { json: true, .. }
            | Command::Inspect { json: true, .. }
            | Command::Verify { json: true, .. }
    ));
    match require_solstone_with(lookup_env, connectivity) {
        Ok(()) => {}
        Err(SupervisorRefusal::SpawnedUnavailable) => return failure("", "", 75),
        Err(SupervisorRefusal::Unavailable) => {
            return failure(
                "",
                &format!(
                    "{}\n",
                    copy::render_for(locale, "journal.cli.streams.supervisor", &[])
                ),
                1,
            );
        }
    }
    match command {
        Command::NoSubcommand => CliRun {
            stdout: help("segment --help"),
            stderr: String::new(),
            exit_code: 1,
        },
        Command::List { day, stream, json } => {
            match list_output(journal_path, &day, stream.as_deref(), json) {
                Ok(stdout) => success(stdout),
                Err(stderr) => failure("", &stderr, 1),
            }
        }
        Command::Inspect { path, json } => inspect(journal_path, &path, json),
        Command::Verify { path, day, json } => {
            verify(journal_path, path.as_deref(), day.as_deref(), json)
        }
        Command::Move {
            path,
            to_day,
            to_time,
            dry_run,
            verbose,
        } => move_segment(
            journal_path,
            &path,
            &to_day,
            to_time.as_deref(),
            dry_run,
            verbose,
            lock_options,
            operations,
        ),
        Command::Help(_) => unreachable!("help returns before supervisor preflight"),
    }
}

#[derive(Debug)]
enum Command {
    Help(&'static str),
    NoSubcommand,
    List {
        day: String,
        stream: Option<String>,
        json: bool,
    },
    Inspect {
        path: String,
        json: bool,
    },
    Verify {
        path: Option<String>,
        day: Option<String>,
        json: bool,
    },
    Move {
        path: String,
        to_day: String,
        to_time: Option<String>,
        dry_run: bool,
        verbose: bool,
    },
}

pub(crate) fn output_locale(json: bool) -> &'static str {
    if json {
        copy::ENGLISH
    } else {
        copy::cli_locale()
    }
}

enum ParseError {
    Arguments(String),
    Required(&'static str),
}
impl ParseError {
    fn render(&self, locale: &str) -> String {
        match self {
            Self::Arguments(arguments) => copy::display_for(
                locale,
                "journal.cli.segment.argument.unknown",
                [("arguments", &arguments)],
            ),
            Self::Required(input) => copy::display_for(
                locale,
                "journal.cli.segment.argument.missing",
                [("input", &input)],
            ),
        }
    }
}

fn parse(args: &[String]) -> Result<Command, ParseError> {
    let verbose = args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-v" | "--verbose"));
    let args = args
        .iter()
        .filter(|arg| !matches!(arg.as_str(), "-v" | "--verbose" | "-d" | "--debug"))
        .cloned()
        .collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Command::Help(match args.first().map(String::as_str) {
            Some("list") => "segment list --help",
            Some("inspect") => "segment inspect --help",
            Some("verify") => "segment verify --help",
            Some("move") => "segment move --help",
            _ => "segment --help",
        }));
    }
    let Some((verb, rest)) = args.split_first() else {
        return Ok(Command::NoSubcommand);
    };
    let mut command = match verb.as_str() {
        "list" => parse_list(rest),
        "inspect" => parse_inspect(rest),
        "verify" => parse_verify(rest),
        "move" => parse_move(rest),
        _ => Err(ParseError::Arguments(args.join(" "))),
    }?;
    if let Command::Move {
        verbose: command_verbose,
        ..
    } = &mut command
    {
        *command_verbose |= verbose;
    }
    Ok(command)
}

fn parse_list(args: &[String]) -> Result<Command, ParseError> {
    let mut day = None;
    let mut stream = None;
    let mut json = false;
    let mut bad = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "-v" | "--verbose" => {}
            "--stream" => {
                index += 1;
                if let Some(value) = args.get(index) {
                    stream = Some(value.clone());
                } else {
                    bad.push("--stream".to_owned());
                }
            }
            value if value.starts_with('-') => bad.push(value.to_owned()),
            value if day.is_none() => day = Some(value.to_owned()),
            value => bad.push(value.to_owned()),
        }
        index += 1;
    }
    match (day, bad.is_empty()) {
        (Some(day), true) => Ok(Command::List { day, stream, json }),
        (_, false) => Err(ParseError::Arguments(bad.join(" "))),
        (None, true) => Err(ParseError::Required("day")),
    }
}

fn parse_inspect(args: &[String]) -> Result<Command, ParseError> {
    let mut path = None;
    let mut json = false;
    let mut bad = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "-v" | "--verbose" => {}
            value if value.starts_with('-') => bad.push(value.to_owned()),
            value if path.is_none() => path = Some(value.to_owned()),
            value => bad.push(value.to_owned()),
        }
    }
    match (path, bad.is_empty()) {
        (Some(path), true) => Ok(Command::Inspect { path, json }),
        (_, false) => Err(ParseError::Arguments(bad.join(" "))),
        (None, true) => Err(ParseError::Required("path")),
    }
}

fn parse_verify(args: &[String]) -> Result<Command, ParseError> {
    let mut path = None;
    let mut day = None;
    let mut json = false;
    let mut bad = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "-v" | "--verbose" => {}
            "--day" => {
                index += 1;
                if let Some(value) = args.get(index) {
                    day = Some(value.clone());
                } else {
                    bad.push("--day".to_owned());
                }
            }
            value if value.starts_with('-') => bad.push(value.to_owned()),
            value if path.is_none() => path = Some(value.to_owned()),
            value => bad.push(value.to_owned()),
        }
        index += 1;
    }
    if bad.is_empty() {
        Ok(Command::Verify { path, day, json })
    } else {
        Err(ParseError::Arguments(bad.join(" ")))
    }
}

fn parse_move(args: &[String]) -> Result<Command, ParseError> {
    let mut path = None;
    let mut day = None;
    let mut time = None;
    let mut dry_run = false;
    let mut verbose = false;
    let mut bad = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--to-day" => {
                index += 1;
                if let Some(value) = args.get(index) {
                    day = Some(value.clone());
                } else {
                    bad.push("--to-day".to_owned());
                }
            }
            "--to-time" => {
                index += 1;
                if let Some(value) = args.get(index) {
                    time = Some(value.clone());
                } else {
                    bad.push("--to-time".to_owned());
                }
            }
            "--dry-run" => dry_run = true,
            "-v" | "--verbose" => verbose = true,
            value if value.starts_with('-') => bad.push(value.to_owned()),
            value if path.is_none() => path = Some(value.to_owned()),
            value => bad.push(value.to_owned()),
        }
        index += 1;
    }
    match (path, day, bad.is_empty()) {
        (Some(path), Some(to_day), true) => Ok(Command::Move {
            path,
            to_day,
            to_time: time,
            dry_run,
            verbose,
        }),
        (_, _, false) => Err(ParseError::Arguments(bad.join(" "))),
        (None, _, true) => Err(ParseError::Required("path")),
        (Some(_), None, true) => Err(ParseError::Required("--to-day")),
    }
}

fn help(name: &str) -> String {
    let id = match name {
        "segment list --help" => "journal.cli.segment.help.list",
        "segment inspect --help" => "journal.cli.segment.help.inspect",
        "segment verify --help" => "journal.cli.segment.help.verify",
        "segment move --help" => "journal.cli.segment.help.move",
        _ => "journal.cli.segment.help.top",
    };
    format!("{}\n", copy::text(id))
}

fn inspect(journal: &Path, path: &str, json: bool) -> CliRun {
    let locale = output_locale(json);
    let (day, stream, segment) = match split_path(path, locale) {
        Ok(parts) => parts,
        Err(message) => return failure("", &format!("{message}\n"), 1),
    };
    let location = match SegmentLocation::resolve(journal, day, stream, segment) {
        Ok(location) if location.path.is_dir() => location,
        _ => {
            return failure(
                "",
                &format!(
                    "{}\n",
                    copy::display_for(locale, "journal.cli.segment.missing", [("path", &path)])
                ),
                1,
            );
        }
    };
    success(inspect_output(journal, &location, json))
}

fn verify(journal: &Path, path: Option<&str>, day: Option<&str>, json: bool) -> CliRun {
    let locale = output_locale(json);
    if let Some(path) = path {
        let (day, stream, segment) = match split_path(path, locale) {
            Ok(parts) => parts,
            Err(message) => return failure("", &format!("{message}\n"), 1),
        };
        let location = match SegmentLocation::resolve(journal, day, stream, segment) {
            Ok(location) => location,
            Err(_) => {
                return failure(
                    "",
                    &format!(
                        "{}\n",
                        copy::display_for(locale, "journal.cli.segment.missing", [("path", &path)])
                    ),
                    1,
                );
            }
        };
        let results = checks_in(journal, &location, locale);
        let passed = results.iter().filter(|check| check.passed).count();
        let stdout = if json {
            serde_json::to_string_pretty(&results.iter().map(check_json).collect::<Vec<_>>())
                .expect("checks serialize")
                + "\n"
        } else {
            format!(
                "{}\n{}\n",
                render_checks(&results, locale),
                check_summary(locale, passed, results.len(), false)
            )
        };
        return CliRun {
            stdout,
            stderr: String::new(),
            exit_code: if passed == results.len() { 0 } else { 1 },
        };
    }
    let Some(day) = day else {
        return failure(
            "",
            &format!(
                "{}\n",
                copy::render_for(locale, "journal.cli.segment.verify.input", &[])
            ),
            1,
        );
    };
    let segments = match day_segments(journal, day, locale) {
        Ok(segments) => segments,
        Err(message) => return failure("", &message, 1),
    };
    if segments.is_empty() {
        return failure(
            "",
            &format!(
                "{}\n",
                copy::display_for(locale, "journal.cli.segment.day.empty", [("day", &day)])
            ),
            1,
        );
    }
    let mut total_passed = 0;
    let mut total_failed = 0;
    let mut all = serde_json::Map::new();
    let mut stdout = String::new();
    for location in segments {
        let results = checks_in(journal, &location, locale);
        total_passed += results.iter().filter(|check| check.passed).count();
        total_failed += results.iter().filter(|check| !check.passed).count();
        if json {
            all.insert(
                location.token(),
                serde_json::Value::Array(results.iter().map(check_json).collect()),
            );
        } else {
            stdout.push_str(&format!(
                "--- {} ---\n{}\n",
                location.token(),
                render_checks(&results, locale)
            ));
        }
    }
    if json {
        stdout = serde_json::to_string_pretty(&serde_json::json!({"segments": all, "summary": {"passed": total_passed, "failed": total_failed}})).expect("verify serializes") + "\n";
    } else {
        stdout.push_str(&check_summary(
            locale,
            total_passed,
            total_passed + total_failed,
            true,
        ));
        stdout.push('\n');
    }
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: if total_failed == 0 { 0 } else { 1 },
    }
}

pub(crate) fn check_summary(locale: &str, passed: usize, total: usize, summary: bool) -> String {
    let id = if summary {
        "journal.cli.segment.verify.day.result"
    } else {
        "journal.cli.segment.verify.result"
    };
    let total = i64::try_from(total).unwrap_or(i64::MAX);
    copy::render_for(
        locale,
        id,
        &[
            ("quantity", copy::Value::Cardinal(total)),
            ("total", copy::Value::Integer(total)),
            (
                "passed",
                copy::Value::Integer(i64::try_from(passed).unwrap_or(i64::MAX)),
            ),
        ],
    )
}

fn check_json(check: &read::Check) -> serde_json::Value {
    serde_json::json!({"check": check.name, "passed": check.passed, "detail": check.detail})
}

#[allow(clippy::too_many_arguments)]
fn move_segment(
    journal: &Path,
    path: &str,
    to_day: &str,
    to_time: Option<&str>,
    dry_run: bool,
    verbose: bool,
    locks: LockOptions,
    operations: &dyn SegmentOperations,
) -> CliRun {
    let locale = copy::cli_locale();
    let (day, stream, segment) = match split_path(path, locale) {
        Ok(parts) => parts,
        Err(message) => return failure("", &format!("{message}\n"), 1),
    };
    let source = match SegmentLocation::resolve(journal, day, stream, segment) {
        Ok(location) if location.path.is_dir() => location,
        _ => {
            return failure(
                "",
                &format!(
                    "{}\n",
                    copy::display_for(locale, "journal.cli.segment.missing", [("path", &path)])
                ),
                1,
            );
        }
    };
    let plan = match build_plan(journal, source, to_day, to_time) {
        Ok(plan) => plan,
        Err(refusal) => return failure("", &format!("{}\n", refusal.message()), 1),
    };
    let mut stdout = render_plan(&plan);
    if dry_run {
        stdout.push_str(&format!(
            "\n{}\n",
            copy::render_for(locale, "journal.cli.segment.move.dry", &[])
        ));
        return success(stdout);
    }
    let execution = execute_plan(journal, &plan, verbose, locks, operations);
    stdout.push_str(&execution.stdout);
    CliRun {
        stdout,
        stderr: execution.stderr,
        exit_code: execution.exit_code,
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}
fn failure(stdout: &str, stderr: &str, exit_code: i32) -> CliRun {
    CliRun {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        exit_code,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use crate::read::checks;
    use solstone_core_segment::SUPERVISOR_MESSAGE;
    use std::{cell::RefCell, fs, path::PathBuf};

    use rusqlite::Connection;
    use serde_json::{Value, json};
    use solstone_core_indexer_store::db::{db_path, open_index};
    use solstone_core_segment::{
        Relocation, RelocationError, RelocationOutcome, RelocationRefusal, RepairOutcome,
    };
    use tempfile::TempDir;

    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }
    fn bypass(values: &[&str], root: &Path) -> CliRun {
        run_cli_with(
            &args(values),
            root,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        )
    }

    fn segment(
        root: &Path,
        day: &str,
        disk_stream: &str,
        key: &str,
        marker: Option<Value>,
    ) -> PathBuf {
        let path = if disk_stream == solstone_core_segment::DEFAULT_STREAM {
            root.join("chronicle").join(day).join(key)
        } else {
            root.join("chronicle").join(day).join(disk_stream).join(key)
        };
        fs::create_dir_all(path.join("talents")).unwrap();
        fs::write(path.join("audio.jsonl"), "{}\n").unwrap();
        fs::write(path.join("talents/audio.md"), "indexed content\n").unwrap();
        if let Some(marker) = marker {
            fs::write(
                path.join("stream.json"),
                serde_json::to_string(&marker).unwrap(),
            )
            .unwrap();
        }
        path
    }

    fn set_segment_size(path: &Path, target: u64) {
        fn size(path: &Path) -> u64 {
            fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .map(|path| {
                    if path.is_dir() {
                        size(&path)
                    } else {
                        fs::metadata(path).unwrap().len()
                    }
                })
                .sum()
        }

        let current = size(path);
        assert!(current < target);
        fs::write(
            path.join("payload.bin"),
            vec![b'x'; (target - current) as usize],
        )
        .unwrap();
    }

    fn stream_state(root: &Path, stream: &str, day: &str, key: &str, seq: u64) -> PathBuf {
        let path = root.join("streams").join(format!("{stream}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_string(&json!({
                "name": stream, "kind": "capture", "host": "desk", "platform": "linux",
                "created_at": 7, "last_day": day, "last_segment": key, "seq": seq,
                // Tail repair never rewrites identity keys; a legacy "did" stays "did".
                "did": "device-1", "source": "microphone", "unknown": {"kept": true}
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn seed_index(root: &Path, rel: &str) {
        let connection = open_index(root).unwrap();
        connection
            .execute(
                "INSERT INTO files(path, mtime) VALUES (?1, 0)",
                [format!("{rel}/talents/audio.md")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('content', ?1, '', '', 'test', '', 0, '')",
                [format!("{rel}/talents/audio.md")],
            )
            .unwrap();
    }

    fn index_rows(root: &Path, rel: &str) -> i64 {
        let connection = Connection::open(db_path(root)).unwrap();
        connection
            .query_row(
                "SELECT count(*) FROM chunks WHERE path=?1 OR path LIKE ?2",
                rusqlite::params![rel, format!("{rel}/%")],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn index_row_set(root: &Path) -> (Vec<String>, Vec<String>) {
        let connection = Connection::open(db_path(root)).unwrap();
        let chunks = connection
            .prepare("SELECT path FROM chunks ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap();
        let files = connection
            .prepare("SELECT path FROM files ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap();
        (chunks, files)
    }

    fn move_args(path: &str, day: &str) -> Vec<String> {
        args(&["move", path, "--to-day", day])
    }

    struct RecordingOperations {
        fail: Option<&'static str>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl RecordingOperations {
        fn record(&self, step: &'static str) -> Result<(), String> {
            self.calls.borrow_mut().push(step);
            if self.fail == Some(step) {
                Err(format!("forced {step} failure"))
            } else {
                Ok(())
            }
        }
    }

    impl SegmentOperations for RecordingOperations {
        /// Drive the real write door, then damage only what it reported.
        ///
        /// The door's own step failures are proven where the mutation lives.
        /// What is under test here is this crate's rendering of them, so the
        /// move really happens and the outcome is rewritten afterwards.
        fn relocate(
            &self,
            relocation: &Relocation<'_>,
        ) -> Result<RelocationOutcome, RelocationRefusal> {
            self.calls.borrow_mut().push("relocate");
            if self.fail == Some("rename") {
                return Err(RelocationRefusal::Failed(RelocationError::new(
                    "forced rename failure",
                )));
            }
            let mut outcome = NativeOperations.relocate(relocation)?;
            match self.fail {
                Some("rewrite") => {
                    outcome.events = Err(RelocationError::new("forced rewrite failure"));
                }
                Some("patch") => {
                    outcome.successor = Some(Err(RelocationError::new("forced patch failure")));
                }
                Some("tail") => outcome.tail = RepairOutcome::WriteFailed,
                _ => {}
            }
            Ok(outcome)
        }

        fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<(), String> {
            self.record("enqueue")?;
            NativeOperations.enqueue_path(journal, path)
        }

        fn touch_health(&self, journal: &Path, day: &str) -> Result<(), String> {
            self.record("health")?;
            NativeOperations.touch_health(journal, day)
        }
    }

    struct LateCollisionOperations;

    impl SegmentOperations for LateCollisionOperations {
        /// Occupy the destination after planning, then let the real door see it.
        fn relocate(
            &self,
            relocation: &Relocation<'_>,
        ) -> Result<RelocationOutcome, RelocationRefusal> {
            let destination = &relocation.destination.path;
            fs::create_dir_all(destination).map_err(|error| {
                RelocationRefusal::Failed(RelocationError::new(error.to_string()))
            })?;
            fs::write(destination.join("stream.json"), "{\"sentinel\":true}\n").map_err(
                |error| RelocationRefusal::Failed(RelocationError::new(error.to_string())),
            )?;
            NativeOperations.relocate(relocation)
        }

        fn enqueue_path(&self, _: &Path, _: &Path) -> Result<(), String> {
            unreachable!("late collision returns before post-move work")
        }

        fn touch_health(&self, _: &Path, _: &str) -> Result<(), String> {
            unreachable!("late collision returns before post-move work")
        }
    }

    fn run_with_operations(
        values: &[String],
        root: &Path,
        operations: &dyn SegmentOperations,
    ) -> CliRun {
        run_cli_with_operations(
            values,
            root,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            LockOptions::default(),
            operations,
        )
    }

    #[test]
    fn five_help_blocks_are_fixture_exact_and_bypass_supervisor() {
        let root = TempDir::new().unwrap();
        for (argv, block) in [
            (["--help"].as_slice(), "segment --help"),
            (["list", "--help"].as_slice(), "segment list --help"),
            (["inspect", "--help"].as_slice(), "segment inspect --help"),
            (["verify", "--help"].as_slice(), "segment verify --help"),
            (["move", "--help"].as_slice(), "segment move --help"),
        ] {
            let run = run_cli_with(&args(argv), root.path(), |_| None, || false);
            assert_eq!(run, success(help(block)));
        }
    }

    #[test]
    fn default_stream_list_and_inspect_use_direct_day_layout() {
        let root = TempDir::new().unwrap();
        let segment = root.path().join("chronicle/20260304/090000_60");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("audio.jsonl"), "{}\n").unwrap();
        fs::write(
            segment.join("stream.json"),
            r#"{"stream":"workstation","seq":1}"#,
        )
        .unwrap();
        assert!(
            bypass(&["list", "20260304"], root.path())
                .stdout
                .contains("_default")
        );
        assert_eq!(
            bypass(&["inspect", "20260304/_default/090000_60"], root.path()).exit_code,
            0
        );
    }

    #[test]
    fn parser_preflight_and_empty_list_behave_like_the_reference_surface() {
        let root = TempDir::new().unwrap();
        let unexpected = bypass(&["bogus"], root.path());
        assert_eq!(unexpected.exit_code, 2);
        assert_eq!(unexpected.stdout, "");
        assert_eq!(
            unexpected.stderr,
            "usage: solstone journal segment <command> [options]\nsolstone journal segment: error: unrecognized arguments: bogus\n"
        );
        let no_subcommand = run_cli_with(&[], root.path(), |_| None, || true);
        assert_eq!(no_subcommand.exit_code, 1);
        assert_eq!(no_subcommand.stdout, help("segment --help"));
        for (argv, required) in [
            (["list"].as_slice(), "day"),
            (["inspect"].as_slice(), "path"),
            (["move"].as_slice(), "path"),
            (["move", "20260304/090000_60"].as_slice(), "--to-day"),
        ] {
            let missing = bypass(argv, root.path());
            assert_eq!(missing.exit_code, 2, "argv={argv:?}");
            assert_eq!(missing.stdout, "", "argv={argv:?}");
            assert_eq!(
                missing.stderr,
                format!(
                    "usage: solstone journal segment <command> [options]\nsolstone journal segment: error: the following arguments are required: {required}\n"
                ),
                "argv={argv:?}"
            );
            assert!(
                !missing.stderr.contains("unrecognized arguments"),
                "argv={argv:?}"
            );
        }
        let unavailable = run_cli_with(
            &args(&["list", "20260304"]),
            root.path(),
            |_| None,
            || false,
        );
        assert_eq!(unavailable.exit_code, 1);
        assert_eq!(unavailable.stderr, format!("{SUPERVISOR_MESSAGE}\n"));
        assert_eq!(
            run_cli_with(&args(&["--help"]), root.path(), |_| None, || false).exit_code,
            0
        );
        assert_eq!(
            bypass(&["list", "20260304"], root.path()).stdout,
            "No segments found for 20260304\n"
        );
        assert_eq!(
            bypass(&["list", "20260304", "--stream", "none"], root.path()).stdout,
            "No segments found for 20260304\n"
        );
    }

    #[test]
    fn list_and_verify_cover_default_layout_and_index_absence_without_creating_a_db() {
        let root = TempDir::new().unwrap();
        segment(
            root.path(),
            "20260304",
            "_default",
            "090000_60",
            Some(json!({"stream":"workstation","seq":1})),
        );
        segment(
            root.path(),
            "20260304",
            "work",
            "999999_300",
            Some(json!({"stream":"work","seq":1})),
        );
        stream_state(root.path(), "workstation", "20260304", "090000_60", 1);
        stream_state(root.path(), "work", "20260304", "999999_300", 1);
        let table = bypass(&["list", "20260304"], root.path()).stdout;
        assert!(table.contains("STREAM") && table.contains("?"));
        assert!(table.find("090000_60").unwrap() < table.find("999999_300").unwrap());
        let listing = bypass(&["list", "20260304", "--json"], root.path());
        assert_eq!(listing.exit_code, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&listing.stdout).unwrap()[0]["stream"],
            "_default"
        );
        let inspected = bypass(
            &["inspect", "20260304/_default/090000_60", "--json"],
            root.path(),
        );
        assert_eq!(inspected.exit_code, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&inspected.stdout).unwrap()["index"]["available"],
            false
        );
        let verified = bypass(&["verify", "20260304/_default/090000_60"], root.path());
        assert_eq!(verified.exit_code, 0);
        assert!(verified.stdout.contains("7/7 checks passed"));
        assert!(!db_path(root.path()).exists());
        let single_json = bypass(
            &["verify", "20260304/_default/090000_60", "--json"],
            root.path(),
        );
        let by_day_json = bypass(&["verify", "--day", "20260304", "--json"], root.path());
        let single_checks: Value = serde_json::from_str(&single_json.stdout).unwrap();
        let by_day_checks: Value = serde_json::from_str(&by_day_json.stdout).unwrap();
        assert_eq!(
            by_day_checks["segments"]["20260304/_default/090000_60"],
            single_checks
        );
    }

    #[test]
    fn list_table_and_json_are_exact_for_sizes_nested_talents_and_stream_filtering() {
        let root = TempDir::new().unwrap();
        let default = segment(
            root.path(),
            "20260304",
            "_default",
            "090000_60",
            Some(json!({"stream":"default-device","seq":1})),
        );
        let alpha = segment(
            root.path(),
            "20260304",
            "alpha",
            "090000_60",
            Some(json!({"stream":"alpha","seq":1})),
        );
        let work = segment(
            root.path(),
            "20260304",
            "work",
            "999999_300",
            Some(json!({"stream":"work","seq":1})),
        );
        fs::create_dir_all(work.join("talents/nested")).unwrap();
        fs::write(work.join("talents/nested/deep.md"), "nested\n").unwrap();
        set_segment_size(&default, 500);
        set_segment_size(&alpha, 1_500);
        set_segment_size(&work, 1_500_000);

        let table = bypass(&["list", "20260304"], root.path());
        assert_eq!(table.exit_code, 0);
        assert_eq!(
            table.stdout,
            concat!(
                "STREAM               SEGMENT        TIME              DUR FILES TALENTS     SIZE\n",
                "------------------------------------------------------------------------------\n",
                "_default             090000_60      09:00:00-09:01:00   60s     4       1     500B\n",
                "alpha                090000_60      09:00:00-09:01:00   60s     4       1     1.5K\n",
                "work                 999999_300     ?                300s     5       2     1.5M\n",
            )
        );
        let listing = bypass(&["list", "20260304", "--json"], root.path());
        assert_eq!(listing.exit_code, 0);
        assert_eq!(
            listing.stdout,
            concat!(
                "[\n",
                "  {\n",
                "    \"stream\": \"_default\",\n",
                "    \"segment\": \"090000_60\",\n",
                "    \"start\": \"09:00:00\",\n",
                "    \"end\": \"09:01:00\",\n",
                "    \"duration\": 60,\n",
                "    \"files\": 4,\n",
                "    \"talents\": 1,\n",
                "    \"size\": 500\n",
                "  },\n",
                "  {\n",
                "    \"stream\": \"alpha\",\n",
                "    \"segment\": \"090000_60\",\n",
                "    \"start\": \"09:00:00\",\n",
                "    \"end\": \"09:01:00\",\n",
                "    \"duration\": 60,\n",
                "    \"files\": 4,\n",
                "    \"talents\": 1,\n",
                "    \"size\": 1500\n",
                "  },\n",
                "  {\n",
                "    \"stream\": \"work\",\n",
                "    \"segment\": \"999999_300\",\n",
                "    \"start\": null,\n",
                "    \"end\": null,\n",
                "    \"duration\": 300,\n",
                "    \"files\": 5,\n",
                "    \"talents\": 2,\n",
                "    \"size\": 1500000\n",
                "  }\n",
                "]\n",
            )
        );
        let filtered = bypass(&["list", "20260304", "--stream", "alpha"], root.path());
        assert_eq!(
            filtered.stdout,
            concat!(
                "STREAM               SEGMENT        TIME              DUR FILES TALENTS     SIZE\n",
                "------------------------------------------------------------------------------\n",
                "alpha                090000_60      09:00:00-09:01:00   60s     4       1     1.5K\n",
            )
        );
    }

    #[test]
    fn index_status_distinguishes_absent_from_unreadable() {
        let root = TempDir::new().unwrap();
        segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(json!({"stream":"work","seq":1})),
        );
        stream_state(root.path(), "work", "20260304", "090000_60", 1);
        assert!(
            bypass(&["inspect", "20260304/work/090000_60"], root.path())
                .stdout
                .contains("Index: unavailable")
        );
        assert!(!db_path(root.path()).exists());
        fs::create_dir_all(db_path(root.path()).parent().unwrap()).unwrap();
        fs::write(db_path(root.path()), "not sqlite").unwrap();
        let inspect = bypass(&["inspect", "20260304/work/090000_60"], root.path());
        assert!(inspect.stdout.contains("Index: error"));
        let verify = bypass(&["verify", "20260304/work/090000_60"], root.path());
        assert_eq!(verify.exit_code, 1);
        assert!(verify.stdout.contains("journal index error:"));
    }

    #[test]
    fn inspect_reports_missing_predecessors_tails_next_links_and_missing_markers() {
        let root = TempDir::new().unwrap();
        segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(
                json!({"stream":"other","seq":1,"prev_day":"20260303","prev_segment":"080000_60"}),
            ),
        );
        segment(
            root.path(),
            "20260304",
            "work",
            "100000_60",
            Some(
                json!({"stream":"other","seq":2,"prev_day":"20260304","prev_segment":"090000_60"}),
            ),
        );
        segment(root.path(), "20260304", "work", "110000_60", None);
        stream_state(root.path(), "other", "20260304", "100000_60", 2);
        let source = bypass(&["inspect", "20260304/work/090000_60"], root.path());
        assert!(source.stdout.contains("20260303/other/080000_60 [MISSING]"));
        assert!(source.stdout.contains("next: 20260304/other/100000_60"));
        let tail = bypass(
            &["inspect", "20260304/work/100000_60", "--json"],
            root.path(),
        );
        let tail_json: Value = serde_json::from_str(&tail.stdout).unwrap();
        assert_eq!(tail_json["chain"]["next"], "(tail)");
        assert_eq!(tail_json["path"], "20260304/work/100000_60");
        let unmarked = bypass(&["inspect", "20260304/work/110000_60"], root.path());
        assert!(unmarked.stdout.contains("Stream:  work"));
        assert!(unmarked.stdout.contains("next: (none)"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn verify_day_refuses_unrepresentable_segment_instead_of_truncating() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let root = TempDir::new().unwrap();
        segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(json!({"stream": "work", "seq": 1})),
        );
        let unreadable = root
            .path()
            .join("chronicle/20260304")
            .join(OsStr::from_bytes(b"s\xff"))
            .join("100000_60");
        fs::create_dir_all(&unreadable).unwrap();
        fs::write(unreadable.join("audio.jsonl"), "{}\n").unwrap();

        let run = bypass(&["verify", "--day", "20260304"], root.path());
        assert_eq!(run.exit_code, 1, "{}", run.stdout);
        assert!(
            run.stderr.contains("segment list refused"),
            "stderr={}",
            run.stderr
        );
        assert!(
            !run.stdout.contains("checks passed"),
            "truncated verify reported success over a subset: {}",
            run.stdout
        );
    }

    #[test]
    fn list_and_verify_refuse_named_default_with_dedicated_cause() {
        let root = TempDir::new().unwrap();
        segment(
            root.path(),
            "20260304",
            "_default",
            "080000_60",
            Some(json!({"stream": "workstation", "seq": 1})),
        );
        let named = root.path().join("chronicle/20260304/_default/090000_60");
        fs::create_dir_all(named.join("talents")).unwrap();
        fs::write(named.join("audio.jsonl"), "{}\n").unwrap();

        let json_list = bypass(&["list", "20260304", "--json"], root.path());
        assert_eq!(json_list.exit_code, 1, "{}", json_list.stdout);
        assert!(
            json_list.stderr.contains(
                "named stream directory \"_default\" cannot be spelled as a record identity"
            ),
            "stderr={}",
            json_list.stderr
        );
        assert!(
            !json_list.stderr.contains("not UTF-8 representable"),
            "stderr={}",
            json_list.stderr
        );
        assert!(json_list.stdout.is_empty(), "{}", json_list.stdout);

        let human_list = bypass(&["list", "20260304"], root.path());
        assert_eq!(human_list.exit_code, 1, "{}", human_list.stdout);
        assert!(
            human_list.stderr.contains(
                "named stream directory \"_default\" cannot be spelled as a record identity"
            ),
            "stderr={}",
            human_list.stderr
        );
        assert!(
            !human_list.stdout.contains("_default             090000_60"),
            "collapsed named default into the human table: {}",
            human_list.stdout
        );

        let filtered = bypass(
            &["list", "20260304", "--stream", "_default", "--json"],
            root.path(),
        );
        assert_eq!(filtered.exit_code, 0, "{}", filtered.stderr);
        let rows: Value = serde_json::from_str(&filtered.stdout).unwrap();
        assert_eq!(rows.as_array().map(Vec::len), Some(1));
        assert_eq!(rows[0]["stream"], "_default");
        assert_eq!(rows[0]["segment"], "080000_60");

        let verified = bypass(&["verify", "--day", "20260304"], root.path());
        assert_eq!(verified.exit_code, 1, "{}", verified.stdout);
        assert!(
            verified.stderr.contains(
                "named stream directory \"_default\" cannot be spelled as a record identity"
            ),
            "stderr={}",
            verified.stderr
        );
        assert!(
            !verified.stderr.contains("not UTF-8 representable"),
            "stderr={}",
            verified.stderr
        );
        assert!(
            !verified.stdout.contains("checks passed"),
            "truncated verify reported success over a subset: {}",
            verified.stdout
        );
    }

    #[test]
    fn verify_argument_refusals_and_failed_checks_use_exit_one() {
        let root = TempDir::new().unwrap();
        let missing_selector = bypass(&["verify"], root.path());
        assert_eq!(missing_selector.exit_code, 1);
        assert_eq!(
            missing_selector.stderr,
            "verify requires a segment path or --day\n"
        );
        let empty_day = bypass(&["verify", "--day", "20260304"], root.path());
        assert_eq!(empty_day.exit_code, 1);
        assert_eq!(empty_day.stderr, "No segments found for 20260304\n");
        segment(root.path(), "20260304", "work", "090000_60", None);
        let failed = bypass(
            &["verify", "20260304/work/090000_60", "--json"],
            root.path(),
        );
        assert_eq!(failed.exit_code, 1);
        assert!(
            !serde_json::from_str::<Value>(&failed.stdout).unwrap()[1]["passed"]
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn default_move_is_direct_preserves_identity_and_patches_a_later_successor() {
        let root = TempDir::new().unwrap();
        let source = segment(
            root.path(),
            "20260304",
            "_default",
            "090000_60",
            Some(json!({"stream":"workstation","seq":5})),
        );
        fs::write(
            source.join("events.jsonl"),
            "{\"day\":\"20260304\",\"segment\":\"090000_60\"}\nnot json\n\n",
        )
        .unwrap();
        let successor = segment(
            root.path(),
            "20260307",
            "_default",
            "100000_60",
            Some(
                json!({"stream":"workstation","seq":6,"prev_day":"20260304","prev_segment":"090000_60","unknown":"kept"}),
            ),
        );
        segment(
            root.path(),
            "20260308",
            "unrelated",
            "110000_60",
            Some(json!({"stream":"unrelated","seq":1})),
        );
        let state = stream_state(root.path(), "workstation", "20260304", "090000_60", 5);
        let identity_before: Value =
            serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
        segment(
            root.path(),
            "20260304",
            "work",
            "110000_60",
            Some(json!({"stream":"work","seq":1})),
        );
        let run = bypass(
            &[
                "move",
                "20260304/_default/090000_60",
                "--to-day",
                "20260305",
            ],
            root.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        let destination = root.path().join("chronicle/20260305/090000_60");
        assert!(destination.is_dir());
        assert!(!root.path().join("chronicle/20260305/_default").exists());
        assert!(!source.exists());
        let events = fs::read_to_string(destination.join("events.jsonl")).unwrap();
        assert!(events.contains("\"day\":\"20260305\"") && events.contains("not json\n\n"));
        let patched: Value =
            serde_json::from_str(&fs::read_to_string(successor.join("stream.json")).unwrap())
                .unwrap();
        assert_eq!(patched["prev_day"], "20260305");
        assert_eq!(patched["prev_segment"], "090000_60");
        assert_eq!(patched["unknown"], "kept");
        let repaired: Value = serde_json::from_str(&fs::read_to_string(state).unwrap()).unwrap();
        // Tail repair patches last_day/last_segment/seq in place; "did" is preserved verbatim.
        for key in [
            "did",
            "source",
            "created_at",
            "kind",
            "host",
            "platform",
            "unknown",
        ] {
            assert_eq!(
                repaired.get(key),
                identity_before.get(key),
                "{key} was not preserved"
            );
        }
        assert_eq!(repaired["last_day"], "20260305");
        assert!(
            root.path()
                .join("chronicle/20260304/health/stream.updated")
                .is_file()
        );
        assert!(
            root.path()
                .join("chronicle/20260305/health/stream.updated")
                .is_file()
        );
        assert!(
            root.path()
                .join("chronicle/20260307/health/stream.updated")
                .is_file(),
            "the durably patched successor day must be dirty"
        );
        assert!(
            !root
                .path()
                .join("chronicle/20260308/health/stream.updated")
                .exists(),
            "an untouched day must not be dirtied"
        );
        assert!(!db_path(root.path()).exists());
    }

    #[test]
    fn default_index_relation_is_pruned_but_reindex_declines_by_existing_classifier() {
        let root = TempDir::new().unwrap();
        segment(
            root.path(),
            "20260304",
            "_default",
            "090000_60",
            Some(json!({"stream":"workstation","seq":1})),
        );
        stream_state(root.path(), "workstation", "20260304", "090000_60", 1);
        let old = "20260304/090000_60";
        seed_index(root.path(), old);
        assert_eq!(index_rows(root.path(), old), 1);
        let run = bypass(
            &[
                "move",
                "20260304/_default/090000_60",
                "--to-day",
                "20260305",
            ],
            root.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        solstone_core_indexer_store::todo::drain(root.path(), Default::default()).unwrap();
        assert_eq!(index_rows(root.path(), old), 0);
        assert_eq!(
            index_rows(root.path(), "20260305/090000_60"),
            0,
            "direct default content is deliberately Declined by the shared classifier"
        );
    }

    #[test]
    fn named_move_prunes_old_rows_and_reindexes_the_destination() {
        let root = TempDir::new().unwrap();
        segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(json!({"stream":"work","seq":1})),
        );
        stream_state(root.path(), "work", "20260304", "090000_60", 1);
        let old = "20260304/work/090000_60";
        let new = "20260305/work/090000_60";
        seed_index(root.path(), old);
        let lease =
            solstone_core_segment::hold_lock(db_path(root.path()), Default::default()).unwrap();
        let run = bypass(
            &["move", "20260304/work/090000_60", "--to-day", "20260305"],
            root.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert_eq!(
            solstone_core_indexer_store::todo::inspect(root.path())
                .unwrap()
                .keys,
            [old, new]
        );
        drop(lease);
        solstone_core_indexer_store::todo::drain(root.path(), Default::default()).unwrap();
        assert_eq!(index_rows(root.path(), old), 0);
        assert!(index_rows(root.path(), new) > 0);
        let check = solstone_core_indexer_store::scan::scan_journal(root.path(), false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
    }

    #[test]
    fn move_refusals_are_inert_and_destination_collision_precedes_marker_mismatch() {
        let root = TempDir::new().unwrap();
        let source = segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(json!({"stream":"other","seq":1})),
        );
        let original = fs::read(source.join("stream.json")).unwrap();
        let successor = segment(
            root.path(),
            "20260306",
            "work",
            "100000_60",
            Some(
                json!({"stream":"other","seq":2,"prev_day":"20260304","prev_segment":"090000_60"}),
            ),
        );
        let successor_before = fs::read(successor.join("stream.json")).unwrap();
        let state = stream_state(root.path(), "other", "20260304", "090000_60", 1);
        let state_before = fs::read(&state).unwrap();
        seed_index(root.path(), "20260304/work/090000_60");
        let index_before = index_row_set(root.path());
        let assert_inert = || {
            assert!(source.is_dir());
            assert_eq!(fs::read(source.join("stream.json")).unwrap(), original);
            assert_eq!(
                fs::read(successor.join("stream.json")).unwrap(),
                successor_before
            );
            assert_eq!(fs::read(&state).unwrap(), state_before);
            assert_eq!(index_row_set(root.path()), index_before);
        };
        let cases: &[&[&str]] = &[
            &["move", "bad", "--to-day", "20260305"],
            &["move", "20260304/work/missing", "--to-day", "20260305"],
            &["move", "20260304/work/090000_60", "--to-day", "bad"],
            &[
                "move",
                "20260304/work/090000_60",
                "--to-day",
                "20260305",
                "--to-time",
                "bad",
            ],
            &["move", "20260304/work/090000_60", "--to-day", "20260304"],
            &["move", "20260304/work/090000_60", "--to-day", "20260305"],
        ];
        for case in cases {
            let run = bypass(case, root.path());
            assert_eq!(run.exit_code, 1);
            assert_inert();
        }
        let markerless = segment(root.path(), "20260304", "work", "100000_60", None);
        let no_marker = bypass(
            &["move", "20260304/work/100000_60", "--to-day", "20260305"],
            root.path(),
        );
        assert_eq!(no_marker.exit_code, 1);
        assert_eq!(no_marker.stderr, "No stream.json in source segment\n");
        assert!(markerless.is_dir());
        assert!(!markerless.join("stream.json").exists());
        assert_inert();
        let collision_destination = segment(
            root.path(),
            "20260305",
            "work",
            "090000_60",
            Some(json!({"stream":"work","seq":2})),
        );
        let collision_before = fs::read(collision_destination.join("stream.json")).unwrap();
        let collision = bypass(
            &["move", "20260304/work/090000_60", "--to-day", "20260305"],
            root.path(),
        );
        assert!(collision.stderr.contains("already exists"));
        assert!(!collision.stderr.contains("Stream mismatch"));
        assert_eq!(
            fs::read(collision_destination.join("stream.json")).unwrap(),
            collision_before
        );
        assert_inert();
    }

    #[test]
    fn dry_run_is_inert_and_ambiguous_successors_are_left_unpatched() {
        let root = TempDir::new().unwrap();
        let source = segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(json!({"stream":"work","seq":1})),
        );
        let first = segment(
            root.path(),
            "20260305",
            "work",
            "100000_60",
            Some(json!({"stream":"work","seq":2,"prev_day":"20260304","prev_segment":"090000_60"})),
        );
        let second = segment(
            root.path(),
            "20260306",
            "work",
            "110000_60",
            Some(json!({"stream":"work","seq":3,"prev_day":"20260304","prev_segment":"090000_60"})),
        );
        stream_state(root.path(), "work", "20260304", "090000_60", 1);
        seed_index(root.path(), "20260304/work/090000_60");
        let before_first = fs::read(first.join("stream.json")).unwrap();
        let before_second = fs::read(second.join("stream.json")).unwrap();
        let index_before = index_row_set(root.path());
        let dry = bypass(
            &[
                "move",
                "20260304/work/090000_60",
                "--to-day",
                "20260307",
                "--dry-run",
            ],
            root.path(),
        );
        assert_eq!(dry.exit_code, 0);
        assert!(source.is_dir());
        assert_eq!(index_row_set(root.path()), index_before);
        assert!(
            !root
                .path()
                .join("chronicle/20260304/health/stream.updated")
                .exists()
        );
        let real = bypass(
            &["move", "20260304/work/090000_60", "--to-day", "20260307"],
            root.path(),
        );
        assert_eq!(real.exit_code, 3);
        assert!(real.stderr.contains("ambiguous chain"));
        assert_eq!(fs::read(first.join("stream.json")).unwrap(), before_first);
        assert_eq!(fs::read(second.join("stream.json")).unwrap(), before_second);
    }

    #[test]
    fn injected_directory_and_post_move_failures_are_distinct_and_checks_still_run() {
        let root = TempDir::new().unwrap();
        let source = segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(json!({"stream":"work","seq":1})),
        );
        fs::write(source.join("events.jsonl"), "{\"tract\":\"capture\"}\n").unwrap();
        let source_events = fs::read(source.join("events.jsonl")).unwrap();
        let successor = segment(
            root.path(),
            "20260305",
            "work",
            "100000_60",
            Some(json!({"stream":"work","seq":2,"prev_day":"20260304","prev_segment":"090000_60"})),
        );
        let successor_before = fs::read(successor.join("stream.json")).unwrap();
        let state = stream_state(root.path(), "work", "20260304", "090000_60", 1);
        let state_before = fs::read(&state).unwrap();
        seed_index(root.path(), "20260304/work/090000_60");
        let index_before = index_row_set(root.path());
        let directory_failure = RecordingOperations {
            fail: Some("rename"),
            calls: RefCell::new(Vec::new()),
        };
        let run = run_with_operations(
            &move_args("20260304/work/090000_60", "20260305"),
            root.path(),
            &directory_failure,
        );
        assert_eq!(run.exit_code, 3);
        assert!(source.is_dir());
        assert_eq!(
            fs::read(source.join("events.jsonl")).unwrap(),
            source_events
        );
        assert_eq!(
            fs::read(successor.join("stream.json")).unwrap(),
            successor_before
        );
        assert_eq!(fs::read(state).unwrap(), state_before);
        assert_eq!(index_row_set(root.path()), index_before);
        assert_eq!(&*directory_failure.calls.borrow(), &["relocate"]);

        for (failure, step) in [
            ("rewrite", 2),
            ("patch", 3),
            ("tail", 4),
            ("enqueue", 5),
            ("health", 6),
        ] {
            let root = TempDir::new().unwrap();
            segment(
                root.path(),
                "20260304",
                "work",
                "090000_60",
                Some(json!({"stream":"work","seq":1})),
            );
            segment(
                root.path(),
                "20260305",
                "work",
                "100000_60",
                Some(
                    json!({"stream":"work","seq":2,"prev_day":"20260304","prev_segment":"090000_60"}),
                ),
            );
            stream_state(root.path(), "work", "20260304", "090000_60", 1);
            if failure == "enqueue" {
                seed_index(root.path(), "20260304/work/090000_60");
            }
            let operations = RecordingOperations {
                fail: Some(failure),
                calls: RefCell::new(Vec::new()),
            };
            let run = run_with_operations(
                &move_args("20260304/work/090000_60", "20260306"),
                root.path(),
                &operations,
            );
            assert_eq!(run.exit_code, 3, "{failure}: {}", run.stderr);
            assert!(run.stderr.contains(&format!("step {step}")));
            if failure == "enqueue" {
                assert!(run.stderr.contains("the search update could not be queued"));
            }
            assert!(run.stdout.contains("checks passed"));
            assert!(operations.calls.borrow().contains(&"health"));
        }
    }

    #[test]
    fn late_destination_collision_refuses_without_mutating_either_segment() {
        let root = TempDir::new().unwrap();
        let source = segment(
            root.path(),
            "20260304",
            "work",
            "090000_60",
            Some(json!({"stream":"work","seq":1})),
        );
        let source_before = fs::read(source.join("stream.json")).unwrap();
        let run = run_with_operations(
            &move_args("20260304/work/090000_60", "20260305"),
            root.path(),
            &LateCollisionOperations,
        );
        let destination = root.path().join("chronicle/20260305/work/090000_60");
        assert_eq!(run.exit_code, 1);
        assert_eq!(
            run.stderr,
            "Destination 20260305/work/090000_60 already exists; no changes made\n"
        );
        assert!(source.is_dir());
        assert_eq!(fs::read(source.join("stream.json")).unwrap(), source_before);
        assert_eq!(
            fs::read(destination.join("stream.json")).unwrap(),
            b"{\"sentinel\":true}\n"
        );
    }

    #[test]
    fn null_predecessor_boundary_passes_and_a_missing_link_fails() {
        use crate::read::{Check, inspect_output};
        use solstone_core_segment::{
            Kind, PairedStreamBase, StreamAllocationBase, StreamHints, advance_bound_stream,
            bind_paired_stream,
        };

        let check_passed = |chks: &[Check], name: &str| -> bool {
            chks.iter().find(|c| c.name == name).unwrap().passed
        };
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let hints = StreamHints {
            kind: Some(Kind::Observed),
            ..StreamHints::default()
        };
        let base = PairedStreamBase {
            origin: StreamAllocationBase::ClientLabel,
            input: "phone",
        };

        // Passing journal, one source:
        // Advance 20260805 / 150000_60, then 20260805 / 140000_60 (earlier clock → null predecessor, seq 2).
        let pass_temp = TempDir::new().unwrap();
        let pass_root = pass_temp.path();
        let bound1 = bind_paired_stream(
            pass_root,
            "20260805",
            "150000_60",
            &base,
            cid,
            "mic",
            &hints,
        )
        .unwrap();
        advance_bound_stream(
            &bound1.stream,
            "20260805",
            "150000_60",
            &bound1.segment,
            hints.clone(),
            cid,
            "mic",
        )
        .unwrap();

        let bound2 = bind_paired_stream(
            pass_root,
            "20260805",
            "140000_60",
            &base,
            cid,
            "mic",
            &hints,
        )
        .unwrap();
        advance_bound_stream(
            &bound2.stream,
            "20260805",
            "140000_60",
            &bound2.segment,
            hints.clone(),
            cid,
            "mic",
        )
        .unwrap();

        let loc1 =
            SegmentLocation::resolve(pass_root, "20260805", &bound1.stream, "150000_60").unwrap();
        let loc2 =
            SegmentLocation::resolve(pass_root, "20260805", &bound2.stream, "140000_60").unwrap();

        let chks1 = checks(pass_root, &loc1);
        let chks2 = checks(pass_root, &loc2);

        assert!(check_passed(&chks1, "backward chain"));
        assert!(check_passed(&chks1, "forward chain"));
        assert!(check_passed(&chks2, "backward chain"));
        assert!(check_passed(&chks2, "forward chain"));

        let inspection: serde_json::Value =
            serde_json::from_str(&inspect_output(pass_root, &loc1, true)).unwrap();
        let next_desc = inspection["chain"]["next"].as_str().unwrap();
        assert!(next_desc.contains("20260805"));
        assert!(next_desc.contains("140000_60"));
        assert_ne!(next_desc, "(none)");

        // Only the bound observed writer creates capture-time boundaries.
        let record_path = pass_root
            .join("streams")
            .join(format!("{}.json", bound1.stream));
        let record_bytes = fs::read(&record_path).unwrap();
        let record: Value = serde_json::from_slice(&record_bytes).unwrap();
        for (kind, cid_value) in [
            ("import", Value::Null),
            ("agent-memory", Value::Null),
            ("observer", Value::Null),
        ] {
            let mut unrelated = record.clone();
            unrelated["kind"] = json!(kind);
            unrelated["cid"] = cid_value;
            fs::write(&record_path, serde_json::to_vec(&unrelated).unwrap()).unwrap();
            assert!(
                !check_passed(&checks(pass_root, &loc1), "forward chain"),
                "{kind}"
            );
        }
        fs::write(&record_path, record_bytes).unwrap();

        // Failing case 1: Forward pair 120000_60 then 120100_60. Delete earlier directory. Later segment backward chain fails.
        let fail1_temp = TempDir::new().unwrap();
        let fail1_root = fail1_temp.path();
        let f1_bound1 = bind_paired_stream(
            fail1_root,
            "20260805",
            "120000_60",
            &base,
            cid,
            "mic",
            &hints,
        )
        .unwrap();
        advance_bound_stream(
            &f1_bound1.stream,
            "20260805",
            "120000_60",
            &f1_bound1.segment,
            hints.clone(),
            cid,
            "mic",
        )
        .unwrap();
        let f1_bound2 = bind_paired_stream(
            fail1_root,
            "20260805",
            "120100_60",
            &base,
            cid,
            "mic",
            &hints,
        )
        .unwrap();
        advance_bound_stream(
            &f1_bound2.stream,
            "20260805",
            "120100_60",
            &f1_bound2.segment,
            hints.clone(),
            cid,
            "mic",
        )
        .unwrap();

        // A null pointer on a forward-time pair is damage, not a boundary.
        let f1_marker_path = f1_bound2.segment.path().join("stream.json");
        let f1_marker_bytes = fs::read(&f1_marker_path).unwrap();
        let mut f1_marker: Value = serde_json::from_slice(&f1_marker_bytes).unwrap();
        f1_marker["prev_day"] = Value::Null;
        f1_marker["prev_segment"] = Value::Null;
        fs::write(&f1_marker_path, serde_json::to_vec(&f1_marker).unwrap()).unwrap();
        let f1_loc1 =
            SegmentLocation::resolve(fail1_root, "20260805", &f1_bound1.stream, "120000_60")
                .unwrap();
        assert!(!check_passed(
            &checks(fail1_root, &f1_loc1),
            "forward chain"
        ));
        fs::write(&f1_marker_path, f1_marker_bytes).unwrap();

        fs::remove_dir_all(f1_bound1.segment.path()).unwrap();
        let f1_loc2 =
            SegmentLocation::resolve(fail1_root, "20260805", &f1_bound2.stream, "120100_60")
                .unwrap();
        let f1_chks2 = checks(fail1_root, &f1_loc2);
        assert!(!check_passed(&f1_chks2, "backward chain"));

        // Failing case 2: Three advances (20260805/150000_60, 20260805/140000_60, 20260804/120000_60). Delete middle directory. First segment forward chain fails.
        let fail2_temp = TempDir::new().unwrap();
        let fail2_root = fail2_temp.path();
        let f2_b1 = bind_paired_stream(
            fail2_root,
            "20260805",
            "150000_60",
            &base,
            cid,
            "mic",
            &hints,
        )
        .unwrap();
        advance_bound_stream(
            &f2_b1.stream,
            "20260805",
            "150000_60",
            &f2_b1.segment,
            hints.clone(),
            cid,
            "mic",
        )
        .unwrap();
        let f2_b2 = bind_paired_stream(
            fail2_root,
            "20260805",
            "140000_60",
            &base,
            cid,
            "mic",
            &hints,
        )
        .unwrap();
        advance_bound_stream(
            &f2_b2.stream,
            "20260805",
            "140000_60",
            &f2_b2.segment,
            hints.clone(),
            cid,
            "mic",
        )
        .unwrap();
        let f2_b3 = bind_paired_stream(
            fail2_root,
            "20260804",
            "120000_60",
            &base,
            cid,
            "mic",
            &hints,
        )
        .unwrap();
        advance_bound_stream(
            &f2_b3.stream,
            "20260804",
            "120000_60",
            &f2_b3.segment,
            hints.clone(),
            cid,
            "mic",
        )
        .unwrap();

        fs::remove_dir_all(f2_b2.segment.path()).unwrap();
        let f2_loc1 =
            SegmentLocation::resolve(fail2_root, "20260805", &f2_b1.stream, "150000_60").unwrap();
        let f2_chks1 = checks(fail2_root, &f2_loc1);
        assert!(!check_passed(&f2_chks1, "forward chain"));
    }
}
