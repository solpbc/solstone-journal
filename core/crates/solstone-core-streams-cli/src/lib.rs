// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process native `journal streams` list, inspect, and rebuild verb.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_segment::{
    LockOptions, MarkerTail, RepairOutcome, SUPERVISOR_MESSAGE, SupervisorRefusal, UnchangedReason,
    is_safe_stream_component, is_solstone_up, list_days, list_segments,
    list_stream_records_tolerant, repair_stream_tail_from_markers, require_solstone_with,
};

const USAGE: &str = "usage: journal streams [-h] [--rebuild] [-v] [-d] [name]";
const STREAMS_HELP_FIXTURE: &str =
    include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");

/// The observable result of a library-hosted CLI invocation.
#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Run the native streams verb using the process environment and local supervisor probe.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    run_cli_with(
        args,
        journal_path,
        |name| std::env::var(name).ok(),
        || is_solstone_up(journal_path),
    )
}

/// Run the native streams verb with injectable environment and supervisor connectivity seams.
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
    let command = match parse_arguments(args) {
        Ok(Command::Help) => return success(streams_help()),
        Ok(command) => command,
        Err(arguments) => {
            return CliRun {
                stdout: String::new(),
                stderr: format!(
                    "{USAGE}\njournal streams: error: unrecognized arguments: {arguments}\n"
                ),
                exit_code: 2,
            };
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
        Command::Action {
            name: Some(name),
            rebuild: false,
        } => inspect_stream(journal_path, &name),
        Command::Action {
            name,
            rebuild: true,
        } => rebuild_streams(journal_path, name.as_deref(), lock_options),
        Command::Action {
            name: None,
            rebuild: false,
        } => list_streams(journal_path),
        Command::Help => unreachable!("help returns before supervisor preflight"),
    }
}

#[derive(Debug)]
enum Command {
    Help,
    Action { name: Option<String>, rebuild: bool },
}

fn parse_arguments(args: &[String]) -> Result<Command, String> {
    let mut rebuild = false;
    let mut name = None;
    let mut unrecognized = Vec::new();

    for argument in args {
        match argument.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--rebuild" => rebuild = true,
            "-v" | "--verbose" | "-d" | "--debug" => {}
            _ if argument.starts_with('-') => unrecognized.push(argument.clone()),
            _ if name.is_none() => name = Some(argument.clone()),
            _ => unrecognized.push(argument.clone()),
        }
    }

    if unrecognized.is_empty() {
        Ok(Command::Action { name, rebuild })
    } else {
        Err(unrecognized.join(" "))
    }
}

fn streams_help() -> String {
    let header = "=== streams --help\n";
    let start = STREAMS_HELP_FIXTURE
        .find(header)
        .expect("streams help fixture block exists")
        + header.len();
    let rest = &STREAMS_HELP_FIXTURE[start..];
    let end = rest.find("\n=== ").unwrap_or(rest.len());
    rest[..end].to_owned()
}

fn list_streams(journal: &Path) -> CliRun {
    let listing = match list_stream_records_tolerant(journal) {
        Ok(listing) => listing,
        Err(error) => return failure("", &format!("Could not list streams: {error}\n"), 3),
    };

    if listing.records.is_empty() && listing.anomalies.is_empty() {
        return success("No streams found\n".to_owned());
    }

    let mut rows = listing
        .records
        .into_iter()
        .map(|(_, value)| stream_row(&value))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.name.cmp(&right.name));

    let mut stdout = format!(
        "{:<24} {:<12} {:<10} {:<16} {:>5}\n{}\n",
        "Name",
        "Type",
        "Last Day",
        "Last Segment",
        "Seq",
        "-".repeat(71)
    );
    for row in rows {
        stdout.push_str(&format!(
            "{:<24} {:<12} {:<10} {:<16} {:>5}\n",
            row.name, row.kind, row.last_day, row.last_segment, row.seq
        ));
    }
    let has_anomalies = !listing.anomalies.is_empty();
    for (path, reason) in listing.anomalies {
        stdout.push_str(&format!(
            "Could not read stream record {}: {reason}\n",
            path.display()
        ));
    }

    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: if has_anomalies { 3 } else { 0 },
    }
}

struct StreamRow {
    name: String,
    kind: String,
    last_day: String,
    last_segment: String,
    seq: String,
}

fn stream_row(value: &Value) -> StreamRow {
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    };
    StreamRow {
        name: string("name").unwrap_or_else(|| "?".to_owned()),
        kind: string("type")
            .or_else(|| string("kind"))
            .unwrap_or_else(|| "?".to_owned()),
        last_day: string("last_day").unwrap_or_else(|| "?".to_owned()),
        last_segment: string("last_segment").unwrap_or_else(|| "?".to_owned()),
        seq: match value.get("seq") {
            None => "0".to_owned(),
            Some(sequence) => sequence
                .as_u64()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".to_owned()),
        },
    }
}

fn inspect_stream(journal: &Path, name: &str) -> CliRun {
    if !is_safe_stream_component(name) {
        return CliRun {
            stdout: format!("Stream not found: {name}\n"),
            stderr: String::new(),
            exit_code: 1,
        };
    }
    let path = journal.join("streams").join(format!("{name}.json"));
    let bytes = match fs::read(&path) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => {
            return CliRun {
                stdout: format!("Stream not found: {name}\n"),
                stderr: String::new(),
                exit_code: 1,
            };
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return CliRun {
                stdout: format!("Stream not found: {name}\n"),
                stderr: String::new(),
                exit_code: 1,
            };
        }
        Err(_) => {
            return failure(
                "",
                &format!(
                    "Could not read stream {}: malformed record\n",
                    path.display()
                ),
                3,
            );
        }
    };

    if serde_json::from_slice::<Value>(&bytes).is_err() {
        return failure(
            "",
            &format!(
                "Could not read stream {}: malformed record\n",
                path.display()
            ),
            3,
        );
    }

    let mut stdout = String::from_utf8(bytes).expect("valid JSON is UTF-8");
    if !stdout.ends_with('\n') {
        stdout.push('\n');
    }
    success(stdout)
}

#[derive(Debug)]
struct MarkerCandidate {
    path: PathBuf,
    day: String,
    segment: String,
}

#[derive(Debug)]
struct CollectedTail {
    day: String,
    segment: String,
    seq: u64,
}

#[derive(Debug)]
enum MarkerAnomaly {
    Unreadable(PathBuf),
    BadSeq(PathBuf),
}

fn rebuild_streams(journal: &Path, filter: Option<&str>, lock_options: LockOptions) -> CliRun {
    let markers = match marker_candidates(journal) {
        Ok(markers) => markers,
        Err(error) => return failure("", &format!("Could not list segments: {error}\n"), 3),
    };
    let mut tails = BTreeMap::<String, CollectedTail>::new();
    let mut anomalies = Vec::new();
    let mut scanned = 0_u64;

    for marker in markers {
        let value = match fs::read(&marker.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        {
            Some(Value::Object(object)) => Value::Object(object),
            _ => {
                anomalies.push(MarkerAnomaly::Unreadable(marker.path));
                continue;
            }
        };
        let Some(stream) = value
            .get("stream")
            .and_then(Value::as_str)
            .filter(|stream| !stream.is_empty())
        else {
            continue;
        };
        let sequence = match value.get("seq") {
            None => 0,
            Some(value) => match value.as_u64() {
                Some(sequence) => sequence,
                None => {
                    anomalies.push(MarkerAnomaly::BadSeq(marker.path));
                    continue;
                }
            },
        };
        if filter.is_some_and(|wanted| wanted != stream) {
            continue;
        }
        scanned += 1;
        let candidate = CollectedTail {
            day: marker.day,
            segment: marker.segment,
            seq: sequence,
        };
        match tails.get(stream) {
            Some(existing) if existing.seq >= candidate.seq => {}
            _ => {
                tails.insert(stream.to_owned(), candidate);
            }
        }
    }

    let mut stdout = String::new();
    let mut has_anomaly = !anomalies.is_empty();
    if tails.is_empty() {
        stdout.push_str(&format!("No streams found ({scanned} segments scanned)\n"));
    } else {
        stdout.push_str(&format!(
            "Rebuilt {} stream(s) from {scanned} segments:\n",
            tails.len()
        ));
        for (stream, tail) in &tails {
            let marker_tail = MarkerTail {
                last_day: &tail.day,
                last_segment: &tail.segment,
                max_seq: tail.seq,
            };
            let outcome =
                repair_stream_tail_from_markers(journal, stream, &marker_tail, lock_options);
            let (line, anomalous) = repair_report_line(stream, outcome);
            has_anomaly |= anomalous;
            stdout.push_str(&line);
        }
    }
    for anomaly in anomalies {
        match anomaly {
            MarkerAnomaly::Unreadable(path) => stdout.push_str(&format!(
                "unreadable marker {}: could not read marker\n",
                path.display()
            )),
            MarkerAnomaly::BadSeq(path) => stdout.push_str(&format!(
                "unreadable marker {}: invalid sequence\n",
                path.display()
            )),
        }
    }
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: if has_anomaly { 3 } else { 0 },
    }
}

fn marker_candidates(
    journal: &Path,
) -> Result<Vec<MarkerCandidate>, solstone_core_segment::SegmentError> {
    let mut markers = Vec::new();
    for (day, _) in list_days(journal)? {
        for segment in list_segments(journal, &day)? {
            markers.push(MarkerCandidate {
                path: segment.path().join("stream.json"),
                day: day.clone(),
                segment: segment.key().to_owned(),
            });
        }
    }
    markers.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(markers)
}

fn repair_report_line(stream: &str, outcome: RepairOutcome) -> (String, bool) {
    let path = format!("streams/{stream}.json");
    match outcome {
        RepairOutcome::Repaired | RepairOutcome::Unchanged(UnchangedReason::AlreadyCurrent) => {
            (format!("  {stream}\n"), false)
        }
        RepairOutcome::Unchanged(UnchangedReason::RecordAhead) => (
            format!("  {stream}: stream state sequence is ahead of markers; unchanged\n"),
            true,
        ),
        RepairOutcome::NoRecord => (
            format!("  {stream}: no stream record at {path}; unchanged\n"),
            true,
        ),
        RepairOutcome::Malformed => (
            format!("  {stream}: malformed stream record at {path}; unchanged\n"),
            true,
        ),
        RepairOutcome::Locked => (
            format!("  {stream}: could not lock {path}; unchanged\n"),
            true,
        ),
        RepairOutcome::WriteFailed => (
            format!("  {stream}: write failed for {path}: atomic publication failed\n"),
            true,
        ),
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
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    use serde_json::{Value, json};
    use solstone_core_segment::{LockOptions, hold_lock};
    use tempfile::TempDir;

    use super::*;

    const HELP: &str = r#"usage: journal streams [-h] [--rebuild] [-v] [-d] [name]

Inspect and manage stream identity

positional arguments:
  name           Stream name to inspect (omit to list all streams)

options:
  -h, --help     show this help message and exit
  --rebuild      Reconstruct stream state from per-segment markers
  -v, --verbose  Enable verbose output
  -d, --debug    Enable debug logging
"#;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn bypass_run(args: &[&str], root: &Path) -> CliRun {
        run_cli_with(
            &strings(args),
            root,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
        )
    }

    fn write_record(root: &Path, file_name: &str, value: Value) {
        let path = root.join("streams").join(format!("{file_name}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&value).unwrap() + "\n").unwrap();
    }

    fn record(name: &str, seq: u64, day: &str, segment: &str) -> Value {
        json!({
            "name": name,
            "type": "observer",
            "host": null,
            "platform": null,
            "created_at": 1,
            "last_day": day,
            "last_segment": segment,
            "seq": seq,
        })
    }

    fn marker(
        root: &Path,
        day: &str,
        directory: Option<&str>,
        segment: &str,
        value: Value,
    ) -> PathBuf {
        let mut path = root.join("chronicle").join(day);
        if let Some(directory) = directory {
            path.push(directory);
        }
        path.push(segment);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("audio.flac"), b"audio").unwrap();
        let marker_path = path.join("stream.json");
        fs::write(&marker_path, value.to_string()).unwrap();
        marker_path
    }

    fn marker_value(stream: &str, seq: u64) -> Value {
        json!({"stream": stream, "prev_day": null, "prev_segment": null, "seq": seq})
    }

    fn registry_json_bytes(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(root.join("streams"))
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.unwrap();
                (entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json"))
                .then(|| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        fs::read(entry.path()).unwrap(),
                    )
                })
            })
            .collect()
    }

    #[test]
    fn help_is_fixture_exact_and_argument_forms_parse_without_changing_output() {
        let temporary = TempDir::new().unwrap();
        let help = run_cli_with(&strings(&["--help"]), temporary.path(), |_| None, || false);
        assert_eq!(
            help,
            CliRun {
                stdout: HELP.to_owned(),
                stderr: String::new(),
                exit_code: 0
            }
        );
        assert_eq!(streams_help(), HELP);
        assert_eq!(
            bypass_run(&["--nonsense"], temporary.path()),
            CliRun {
                stdout: String::new(),
                stderr: format!(
                    "{USAGE}\njournal streams: error: unrecognized arguments: --nonsense\n"
                ),
                exit_code: 2,
            }
        );
        write_record(
            temporary.path(),
            "alpha",
            record("alpha", 1, "20260101", "090000_1"),
        );
        for args in [
            ["-v"].as_slice(),
            ["--verbose"].as_slice(),
            ["-d"].as_slice(),
            ["--debug"].as_slice(),
        ] {
            assert_eq!(
                bypass_run(args, temporary.path()).stdout,
                bypass_run(&[], temporary.path()).stdout
            );
        }
        assert_eq!(
            bypass_run(&["alpha", "-v"], temporary.path()).stdout,
            bypass_run(&["alpha"], temporary.path()).stdout
        );
        assert_eq!(
            bypass_run(&["--rebuild", "alpha", "-d"], temporary.path()).exit_code,
            0
        );
    }

    #[test]
    fn supervisor_refusal_branches_are_injected_and_help_short_circuits() {
        let temporary = TempDir::new().unwrap();
        assert_eq!(
            run_cli_with(&strings(&[]), temporary.path(), |_| None, || false),
            CliRun {
                stdout: String::new(),
                stderr: format!("{SUPERVISOR_MESSAGE}\n"),
                exit_code: 1
            }
        );
        assert_eq!(
            run_cli_with(
                &strings(&[]),
                temporary.path(),
                |name| (name == "SOL_SUPERVISOR_SPAWNED").then(|| "1".to_owned()),
                || false,
            ),
            CliRun {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 75
            }
        );
        assert_eq!(
            run_cli_with(&strings(&[]), temporary.path(), |_| None, || true).exit_code,
            0
        );
        assert_eq!(
            run_cli_with(&strings(&["-h"]), temporary.path(), |_| None, || false).exit_code,
            0
        );
    }

    #[test]
    fn list_ignores_lock_sidecars_and_renders_field_variants() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        write_record(
            root,
            "z-file",
            record("zeta", 123456, "20260101", "090000_1"),
        );
        write_record(
            root,
            "alpha",
            json!({"name": "alpha", "kind": "kind-only", "seq": 2}),
        );
        write_record(
            root,
            "nulls",
            json!({"name": null, "type": null, "kind": null, "last_day": null, "last_segment": null, "seq": null}),
        );
        write_record(
            root,
            "long",
            json!({"name": "a-name-that-is-longer-than-twenty-four", "type": "legacy"}),
        );
        write_record(root, "missing-name", json!({"type": "observer", "seq": 1}));
        fs::write(root.join("streams/.registry.lock"), b"").unwrap();
        fs::write(root.join("streams/alpha.json.lock"), b"").unwrap();

        let run = bypass_run(&[], root);
        assert_eq!(run.exit_code, 0);
        assert_eq!(
            run.stdout,
            "Name                     Type         Last Day   Last Segment       Seq\n\
-----------------------------------------------------------------------\n\
?                        observer     ?          ?                    1\n\
?                        ?            ?          ?                    ?\n\
a-name-that-is-longer-than-twenty-four legacy       ?          ?                    0\n\
alpha                    kind-only    ?          ?                    2\n\
zeta                     observer     20260101   090000_1         123456\n"
        );
    }

    #[test]
    fn list_empty_and_absent_directories_do_not_create_streams_and_bad_files_follow_table() {
        let absent = TempDir::new().unwrap();
        assert_eq!(
            bypass_run(&[], absent.path()),
            success("No streams found\n".to_owned())
        );
        assert!(!absent.path().join("streams").exists());

        let temporary = TempDir::new().unwrap();
        fs::create_dir_all(temporary.path().join("streams")).unwrap();
        assert_eq!(
            bypass_run(&[], temporary.path()),
            success("No streams found\n".to_owned())
        );
        write_record(
            temporary.path(),
            "good",
            record("good", 1, "20260101", "090000_1"),
        );
        let bad = temporary.path().join("streams/bad.json");
        fs::write(&bad, b"not json").unwrap();
        let run = bypass_run(&[], temporary.path());
        assert_eq!(run.exit_code, 3);
        assert!(run.stdout.starts_with("Name                     Type"));
        assert!(run.stdout.contains("good                     observer"));
        assert!(run.stdout.ends_with(&format!(
            "Could not read stream record {}: malformed record\n",
            bad.display()
        )));
    }

    #[test]
    fn inspect_preserves_raw_legacy_bytes_and_reports_missing_or_malformed_path() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("journal");
        let path = root.join("streams/legacy.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let raw = b"{\n  \"unknown\": 7, \"type\": \"legacy\", \"name\": \"legacy\"\n}";
        fs::write(&path, raw).unwrap();
        assert_eq!(
            bypass_run(&["legacy"], &root),
            success(
                "{\n  \"unknown\": 7, \"type\": \"legacy\", \"name\": \"legacy\"\n}\n".to_owned()
            )
        );
        let missing = bypass_run(&["missing"], &root);
        assert_eq!(missing.stdout, "Stream not found: missing\n");
        assert_eq!(missing.exit_code, 1);
        let escaped = temporary.path().join("escaped.json");
        fs::write(&escaped, b"{\"outside\":true}").unwrap();
        let traversal = bypass_run(&["../../escaped"], &root);
        assert_eq!(traversal.stdout, "Stream not found: ../../escaped\n");
        assert_eq!(traversal.exit_code, 1);
        assert_eq!(fs::read(&escaped).unwrap(), b"{\"outside\":true}");
        fs::write(root.join("streams/bad.json"), b"{").unwrap();
        let run = bypass_run(&["bad"], &root);
        assert_eq!(run.exit_code, 3);
        assert_eq!(
            run.stderr,
            format!(
                "Could not read stream {}: malformed record\n",
                root.join("streams/bad.json").display()
            )
        );
    }

    #[test]
    fn rebuild_healthy_is_sorted_and_idempotent_with_permanent_sidecars() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        write_record(root, "zeta", record("zeta", 1, "20260101", "080000_1"));
        write_record(root, "alpha", record("alpha", 1, "20260101", "080000_1"));
        marker(
            root,
            "20260101",
            Some("z-dir"),
            "090000_1",
            marker_value("zeta", 3),
        );
        marker(
            root,
            "20260101",
            Some("a-dir"),
            "090000_2",
            marker_value("alpha", 2),
        );
        fs::write(root.join("streams/.registry.lock"), b"").unwrap();
        fs::write(root.join("streams/zeta.json.lock"), b"").unwrap();
        let first = bypass_run(&["--rebuild"], root);
        assert_eq!(first.exit_code, 0);
        assert_eq!(first.stderr, "");
        assert_eq!(
            first.stdout,
            "Rebuilt 2 stream(s) from 2 segments:\n  alpha\n  zeta\n"
        );
        let after_first = registry_json_bytes(root);
        let second = bypass_run(&["--rebuild"], root);
        assert_eq!(second.stdout, first.stdout);
        assert_eq!(registry_json_bytes(root), after_first);
        assert!(root.join("streams/zeta.json.lock").exists());
    }

    #[test]
    fn rebuild_repairs_raw_record_without_normalizing_legacy_or_unknown_fields() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        let path = root.join("streams/legacy.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\n  \"name\": \"legacy\",\n  \"type\": \"old\",\n  \"unknown\": {\"x\": 1},\n  \"host\": null,\n  \"platform\": null,\n  \"created_at\": 1,\n  \"last_day\": \"20260101\",\n  \"last_segment\": \"080000_1\",\n  \"seq\": 1\n}\n").unwrap();
        marker(
            root,
            "20260102",
            Some("wherever"),
            "090000_2",
            marker_value("legacy", 4),
        );
        assert_eq!(bypass_run(&["--rebuild"], root).exit_code, 0);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "{\n  \"name\": \"legacy\",\n  \"type\": \"old\",\n  \"unknown\": {\n    \"x\": 1\n  },\n  \"host\": null,\n  \"platform\": null,\n  \"created_at\": 1,\n  \"last_day\": \"20260102\",\n  \"last_segment\": \"090000_2\",\n  \"seq\": 4\n}\n"
        );
    }

    #[test]
    fn rebuild_reports_missing_malformed_and_record_ahead_without_writing_them() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        marker(
            root,
            "20260101",
            Some("missing-dir"),
            "090000_1",
            marker_value("missing", 2),
        );
        fs::create_dir_all(root.join("streams")).unwrap();
        let malformed = root.join("streams/bad.json");
        fs::write(&malformed, b"{").unwrap();
        marker(
            root,
            "20260101",
            Some("bad-dir"),
            "090000_2",
            marker_value("bad", 2),
        );
        write_record(root, "ahead", record("ahead", 10, "20260101", "080000_1"));
        let ahead = root.join("streams/ahead.json");
        let original = fs::read(&ahead).unwrap();
        marker(
            root,
            "20260101",
            Some("ahead-dir"),
            "090000_3",
            marker_value("ahead", 8),
        );
        let run = bypass_run(&["--rebuild"], root);
        assert_eq!(run.exit_code, 3);
        assert!(
            run.stdout
                .contains("missing: no stream record at streams/missing.json; unchanged")
        );
        assert!(
            run.stdout
                .contains("bad: malformed stream record at streams/bad.json; unchanged")
        );
        assert!(
            run.stdout
                .contains("ahead: stream state sequence is ahead of markers; unchanged")
        );
        assert!(!root.join("streams/missing.json").exists());
        assert_eq!(fs::read(malformed).unwrap(), b"{");
        assert_eq!(fs::read(ahead).unwrap(), original);
    }

    #[test]
    fn rebuild_filter_marker_attribution_default_stream_and_zero_sequence_are_explicit() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        write_record(root, "alpha", record("alpha", 0, "20260101", "090000_1"));
        write_record(root, "beta", record("beta", 1, "20260101", "080000_1"));
        write_record(
            root,
            "default-name",
            record("default-name", 1, "20260101", "080000_1"),
        );
        marker(
            root,
            "20260101",
            Some("beta-directory"),
            "090000_2",
            marker_value("alpha", 2),
        );
        marker(
            root,
            "20260101",
            Some("elsewhere"),
            "090000_3",
            marker_value("beta", 3),
        );
        marker(
            root,
            "20260102",
            None,
            "090000_1",
            marker_value("default-name", 2),
        );
        let alpha_before = fs::read(root.join("streams/alpha.json")).unwrap();
        let run = bypass_run(&["--rebuild", "alpha"], root);
        assert_eq!(
            run.stdout,
            "Rebuilt 1 stream(s) from 1 segments:\n  alpha\n"
        );
        assert_eq!(run.exit_code, 0);
        assert_ne!(
            fs::read(root.join("streams/alpha.json")).unwrap(),
            alpha_before
        );
        assert!(
            fs::read_to_string(root.join("streams/beta.json"))
                .unwrap()
                .contains("080000_1")
        );
        assert_eq!(
            bypass_run(&["--rebuild", "default-name"], root).stdout,
            "Rebuilt 1 stream(s) from 1 segments:\n  default-name\n"
        );

        let zero = TempDir::new().unwrap();
        write_record(
            zero.path(),
            "zero",
            record("zero", 0, "20260101", "090000_1"),
        );
        marker(
            zero.path(),
            "20260101",
            Some("zero-dir"),
            "090000_1",
            marker_value("zero", 0),
        );
        let bytes = fs::read(zero.path().join("streams/zero.json")).unwrap();
        assert_eq!(
            bypass_run(&["--rebuild"], zero.path()).stdout,
            "Rebuilt 1 stream(s) from 1 segments:\n  zero\n"
        );
        assert_eq!(
            fs::read(zero.path().join("streams/zero.json")).unwrap(),
            bytes
        );
    }

    #[test]
    fn rebuild_marker_anomalies_are_reported_or_silently_skipped_and_ties_are_stable() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        write_record(root, "target", record("target", 1, "20260101", "080000_1"));
        marker(
            root,
            "20260101",
            Some("z-last-created"),
            "090000_1",
            marker_value("target", 4),
        );
        marker(
            root,
            "20260101",
            Some("a-first-created"),
            "090000_1",
            marker_value("target", 4),
        );
        let unreadable = marker(
            root,
            "20260101",
            Some("broken"),
            "090000_2",
            marker_value("target", 99),
        );
        fs::remove_file(&unreadable).unwrap();
        fs::create_dir(&unreadable).unwrap();
        let bad_seq = marker(
            root,
            "20260101",
            Some("bad"),
            "090000_3",
            json!({"stream": "target", "seq": true}),
        );
        marker(
            root,
            "20260101",
            Some("silent"),
            "090000_4",
            json!({"seq": 7}),
        );
        let first = bypass_run(&["--rebuild"], root);
        assert_eq!(first.exit_code, 3);
        assert!(first.stdout.contains(&format!(
            "unreadable marker {}: could not read marker",
            unreadable.display()
        )));
        assert!(first.stdout.contains(&format!(
            "unreadable marker {}: invalid sequence",
            bad_seq.display()
        )));
        assert!(!first.stdout.contains("silent/090000_4"));
        assert!(
            first
                .stdout
                .contains("Rebuilt 1 stream(s) from 2 segments:")
        );
        let target = fs::read_to_string(root.join("streams/target.json")).unwrap();
        assert!(target.contains("\"seq\": 4"));
        assert!(target.contains("\"last_segment\": \"090000_1\""));
        assert_eq!(bypass_run(&["--rebuild"], root).stdout, first.stdout);
    }

    #[test]
    fn rebuild_lock_and_write_failures_are_typed_anomalies() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path();
        write_record(root, "locked", record("locked", 1, "20260101", "080000_1"));
        marker(
            root,
            "20260101",
            Some("locked-dir"),
            "090000_1",
            marker_value("locked", 2),
        );
        let record_path = root.join("streams/locked.json");
        let locked_before = fs::read(&record_path).unwrap();
        let _guard = hold_lock(&record_path, LockOptions::default()).unwrap();
        let short = LockOptions {
            timeout: Duration::from_millis(20),
            ..LockOptions::default()
        };
        let locked = run_cli_with_lock_options(
            &strings(&["--rebuild"]),
            root,
            |name| (name == "SOL_SKIP_SUPERVISOR_CHECK").then(|| "1".to_owned()),
            || false,
            short,
        );
        assert_eq!(locked.exit_code, 3);
        assert!(locked.stdout.contains("could not lock streams/locked.json"));
        assert_eq!(fs::read(&record_path).unwrap(), locked_before);
        drop(_guard);

        #[cfg(unix)]
        {
            write_record(
                root,
                "unwritable",
                record("unwritable", 1, "20260101", "080000_1"),
            );
            marker(
                root,
                "20260101",
                Some("write-dir"),
                "090000_2",
                marker_value("unwritable", 2),
            );
            drop(hold_lock(root.join("streams/unwritable.json"), LockOptions::default()).unwrap());
            let streams = root.join("streams");
            let original = fs::metadata(&streams).unwrap().permissions();
            let mut read_only = original.clone();
            read_only.set_mode(0o555);
            fs::set_permissions(&streams, read_only).unwrap();
            let write_failure = bypass_run(&["--rebuild", "unwritable"], root);
            fs::set_permissions(&streams, original).unwrap();
            assert_eq!(write_failure.exit_code, 3);
            assert!(
                write_failure.stdout.contains(
                    "write failed for streams/unwritable.json: atomic publication failed"
                )
            );
        }
    }

    #[test]
    fn rebuild_empty_journal_uses_no_streams_summary() {
        let temporary = TempDir::new().unwrap();
        assert_eq!(
            bypass_run(&["--rebuild"], temporary.path()),
            success("No streams found (0 segments scanned)\n".to_owned())
        );
    }
}
