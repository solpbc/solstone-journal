// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process native `journal segment` verb, not yet wired into a binary.

use std::path::Path;

use solstone_core_segment::{
    LockOptions, SUPERVISOR_MESSAGE, SupervisorRefusal, is_solstone_up, require_solstone_with,
};

mod index;
mod location;
mod r#move;
mod read;

use location::SegmentLocation;
use r#move::{NativeOperations, SegmentOperations, build_plan, execute_plan, render_plan};
use read::{checks, day_segments, inspect_output, list_output, render_checks, split_path};

const USAGE: &str = "usage: journal segment <command> [options]";
const HELP_FIXTURE: &str =
    include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");

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
        Err(arguments) => {
            return failure(
                "",
                &format!("{USAGE}\njournal segment: error: unrecognized arguments: {arguments}\n"),
                2,
            );
        }
    };
    match require_solstone_with(lookup_env, connectivity) {
        Ok(()) => {}
        Err(SupervisorRefusal::SpawnedUnavailable) => return failure("", "", 75),
        Err(SupervisorRefusal::Unavailable) => {
            return failure("", &format!("{SUPERVISOR_MESSAGE}\n"), 1);
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

fn parse(args: &[String]) -> Result<Command, String> {
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
        _ => Err(args.join(" ")),
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

fn parse_list(args: &[String]) -> Result<Command, String> {
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
        (_, false) => Err(bad.join(" ")),
        (None, true) => Err("the following arguments are required: day".to_owned()),
    }
}

fn parse_inspect(args: &[String]) -> Result<Command, String> {
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
        (_, false) => Err(bad.join(" ")),
        (None, true) => Err("the following arguments are required: path".to_owned()),
    }
}

fn parse_verify(args: &[String]) -> Result<Command, String> {
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
        Err(bad.join(" "))
    }
}

fn parse_move(args: &[String]) -> Result<Command, String> {
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
        (_, _, false) => Err(bad.join(" ")),
        (None, _, true) => Err("the following arguments are required: path".to_owned()),
        (Some(_), None, true) => Err("the following arguments are required: --to-day".to_owned()),
    }
}

fn help(name: &str) -> String {
    let header = format!("=== {name}\n");
    let start = HELP_FIXTURE
        .find(&header)
        .expect("segment help fixture block exists")
        + header.len();
    let rest = &HELP_FIXTURE[start..];
    let end = rest.find("\n=== ").unwrap_or(rest.len());
    rest[..end].to_owned()
}

fn inspect(journal: &Path, path: &str, json: bool) -> CliRun {
    let (day, stream, segment) = match split_path(path) {
        Ok(parts) => parts,
        Err(message) => return failure("", &format!("{message}\n"), 1),
    };
    let location = match SegmentLocation::resolve(journal, day, stream, segment) {
        Ok(location) if location.path.is_dir() => location,
        _ => return failure("", &format!("Segment not found: {path}\n"), 1),
    };
    success(inspect_output(journal, &location, json))
}

fn verify(journal: &Path, path: Option<&str>, day: Option<&str>, json: bool) -> CliRun {
    if let Some(path) = path {
        let (day, stream, segment) = match split_path(path) {
            Ok(parts) => parts,
            Err(message) => return failure("", &format!("{message}\n"), 1),
        };
        let location = match SegmentLocation::resolve(journal, day, stream, segment) {
            Ok(location) => location,
            Err(_) => return failure("", &format!("Segment not found: {path}\n"), 1),
        };
        let results = checks(journal, &location);
        let passed = results.iter().filter(|check| check.passed).count();
        let stdout = if json {
            serde_json::to_string_pretty(&results.iter().map(check_json).collect::<Vec<_>>())
                .expect("checks serialize")
                + "\n"
        } else {
            format!(
                "{}\n{passed}/{} checks passed\n",
                render_checks(&results),
                results.len()
            )
        };
        return CliRun {
            stdout,
            stderr: String::new(),
            exit_code: if passed == results.len() { 0 } else { 1 },
        };
    }
    let Some(day) = day else {
        return failure("", "verify requires a segment path or --day\n", 1);
    };
    let segments = match day_segments(journal, day) {
        Ok(segments) => segments,
        Err(message) => return failure("", &message, 1),
    };
    if segments.is_empty() {
        return failure("", &format!("No segments found for {day}\n"), 1);
    }
    let mut total_passed = 0;
    let mut total_failed = 0;
    let mut all = serde_json::Map::new();
    let mut stdout = String::new();
    for location in segments {
        let results = checks(journal, &location);
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
                render_checks(&results)
            ));
        }
    }
    if json {
        stdout = serde_json::to_string_pretty(&serde_json::json!({"segments": all, "summary": {"passed": total_passed, "failed": total_failed}})).expect("verify serializes") + "\n";
    } else {
        stdout.push_str(&format!(
            "Summary: {total_passed}/{} checks passed\n",
            total_passed + total_failed
        ));
    }
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: if total_failed == 0 { 0 } else { 1 },
    }
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
    let (day, stream, segment) = match split_path(path) {
        Ok(parts) => parts,
        Err(message) => return failure("", &format!("{message}\n"), 1),
    };
    let source = match SegmentLocation::resolve(journal, day, stream, segment) {
        Ok(location) if location.path.is_dir() => location,
        _ => return failure("", &format!("Segment not found: {path}\n"), 1),
    };
    let plan = match build_plan(journal, source, to_day, to_time) {
        Ok(plan) => plan,
        Err(refusal) => return failure("", &format!("{}\n", refusal.message()), 1),
    };
    let mut stdout = render_plan(&plan);
    if dry_run {
        stdout.push_str("\n[dry run] No changes made\n");
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
    use std::{cell::RefCell, fs, path::PathBuf};

    use rusqlite::Connection;
    use serde_json::{Value, json};
    use solstone_core_indexer_store::{
        db::{StreamPruneCounts, db_path, open_index},
        scan::RescanFileStatus,
    };
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

        fn prune(&self, journal: &Path, rel: &str) -> Result<Option<StreamPruneCounts>, String> {
            self.record("prune")?;
            NativeOperations.prune(journal, rel)
        }

        fn rescan(&self, journal: &Path, file: &Path) -> Result<RescanFileStatus, String> {
            self.record("rescan")?;
            NativeOperations.rescan(journal, file)
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

        fn prune(&self, _: &Path, _: &str) -> Result<Option<StreamPruneCounts>, String> {
            unreachable!("late collision returns before post-move work")
        }

        fn rescan(&self, _: &Path, _: &Path) -> Result<RescanFileStatus, String> {
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
            "usage: journal segment <command> [options]\njournal segment: error: unrecognized arguments: bogus\n"
        );
        let no_subcommand = run_cli_with(&[], root.path(), |_| None, || true);
        assert_eq!(no_subcommand.exit_code, 1);
        assert_eq!(no_subcommand.stdout, help("segment --help"));
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

    #[cfg(unix)]
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
        let run = bypass(
            &["move", "20260304/work/090000_60", "--to-day", "20260305"],
            root.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert_eq!(index_rows(root.path(), old), 0);
        assert!(index_rows(root.path(), new) > 0);
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
            ("rescan", 5),
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
            if failure == "rescan" {
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
            if failure == "rescan" {
                assert!(run.stderr.contains("run: journal indexer --rescan"));
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
}
