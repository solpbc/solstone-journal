// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use chrono::{Local, NaiveDate, Utc};
use nix::fcntl::{Flock, FlockArg};
use serde_json::Value;
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, create_oplog_at},
};

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");
const SPECIES: &str = include_str!("../../../fixtures/native-identity/species-preamble.md");
const REFERENCE_HELP: &str = include_str!("../../../fixtures/native-identity/reference-help.txt");

struct TestJournal {
    temp: tempfile::TempDir,
}

impl TestJournal {
    fn new() -> Self {
        Self {
            temp: tempfile::tempdir().expect("journal"),
        }
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn identity(&self) -> PathBuf {
        self.path().join("identity")
    }
}

fn command(journal: &TestJournal, args: &[&str]) -> Command {
    let mut command = Command::new(BINARY);
    command
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(journal: &TestJournal, args: &[&str]) -> Output {
    command(journal, args).output().expect("run identity")
}

fn run_skipped(journal: &TestJournal, args: &[&str]) -> Output {
    command(journal, args)
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .output()
        .expect("run identity")
}

fn run_input(journal: &TestJournal, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(BINARY)
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn identity");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("wait identity")
}

fn write_daily_completion(journal: &TestJournal, day: &str, ts: i64) {
    let instant = NaiveDate::parse_from_str(day, "%Y%m%d")
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc()
        .fixed_offset();
    let mut writer = create_oplog_at(
        JournalRoot::open(journal.path()).unwrap(),
        "think",
        "daily",
        OplogFormat::Jsonl,
        instant,
    )
    .unwrap();
    writeln!(
        writer,
        "{{\"event\":\"daily.completion\",\"complete\":true,\"ts\":{ts}}}"
    )
    .unwrap();
}

fn write(path: impl AsRef<Path>, text: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    fs::write(path, text).expect("write");
}

fn health_body(stamp: &str) -> String {
    format!(
        "## Status\n<!-- generated_at: {stamp} -->\nyour journal is well.\n\n## Needs your attention\n\n## Auto-repairs (last 7d)\n"
    )
}

fn start_refresh_server(journal: &TestJournal) -> thread::JoinHandle<Value> {
    let journal_path = journal.path().to_path_buf();
    let socket_path = journal_path.join("health/callosum.sock");
    fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(socket_path).expect("bind Callosum socket");
    thread::spawn(move || {
        let (request_stream, _) = listener.accept().expect("accept request sender");
        let mut request_line = String::new();
        BufReader::new(request_stream)
            .read_line(&mut request_line)
            .expect("read request");
        let request: Value = serde_json::from_str(&request_line).expect("request JSON");
        let use_id = request["use_id"].as_str().expect("use id");
        let active = journal_path
            .join("talents/steward")
            .join(format!("{use_id}_active.jsonl"));
        write(&active, &format!("{request}\n"));

        let (mut subscriber, _) = listener.accept().expect("accept outcome subscriber");
        write(
            &active,
            &format!("{request}\n{{\"event\":\"finish\",\"use_id\":\"{use_id}\"}}\n"),
        );
        fs::rename(&active, active.with_file_name(format!("{use_id}.jsonl")))
            .expect("finalize steward output");
        let stamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        fs::write(journal_path.join("identity/health.md"), health_body(&stamp)).unwrap();
        writeln!(
            subscriber,
            "{{\"tract\":\"cortex\",\"event\":\"finish\",\"use_id\":\"{use_id}\"}}"
        )
        .expect("send finish");
        request
    })
}

#[test]
fn gate_precedes_partner_write_and_help_stays_available() {
    let journal = TestJournal::new();
    let partner = journal.identity().join("partner.md");
    write(&partner, "before\n");
    write(journal.identity().join("health.md"), "health\n");

    let down = run(
        &journal,
        &["identity", "partner", "--write", "--value", "after"],
    );
    assert_eq!(down.status.code(), Some(1));
    assert_eq!(down.stdout, b"");
    assert_eq!(
        down.stderr,
        b"journal isn't running. start it with 'journal up' and retry.\n"
    );
    assert_eq!(fs::read_to_string(&partner).unwrap(), "before\n");
    assert!(!journal.identity().join("history.jsonl").exists());

    let spawned = command(
        &journal,
        &["identity", "partner", "--write", "--value", "after"],
    )
    .env("SOL_SUPERVISOR_SPAWNED", "1")
    .output()
    .expect("run spawned");
    assert_eq!(spawned.status.code(), Some(75));
    assert_eq!(spawned.stdout, b"");
    assert_eq!(spawned.stderr, b"");

    let help = run(&journal, &["identity", "partner", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert_eq!(help.stderr, b"");
    assert!(String::from_utf8_lossy(&help.stdout).starts_with("usage: journal identity partner"));

    let allowed = run_skipped(
        &journal,
        &["identity", "partner", "--write", "--value", "after"],
    );
    assert_eq!(allowed.status.code(), Some(0));
    assert_eq!(allowed.stdout, b"partner.md updated.\n");
    assert_eq!(fs::read_to_string(&partner).unwrap(), "after");
    assert_eq!(
        fs::read_to_string(journal.identity().join("history.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn hydration_is_byte_exact_and_never_seeds_identity() {
    let journal = TestJournal::new();
    let missing = run_skipped(&journal, &["identity"]);
    assert_eq!(missing.status.code(), Some(0));
    assert_eq!(
        missing.stdout,
        format!("# species\n\n{SPECIES}\n\n# partner\n\n(not present)\n").as_bytes()
    );
    assert!(!journal.identity().exists());

    write(
        journal.identity().join("partner.md"),
        "# PARTNER\n\nowner body\n",
    );
    let present = run_skipped(&journal, &["identity"]);
    assert_eq!(
        present.stdout,
        format!("# species\n\n{SPECIES}\n\n# partner\n\nowner body\n").as_bytes()
    );
}

#[test]
fn partner_and_health_reads_add_one_newline_to_seeded_files() {
    let journal = TestJournal::new();
    let partner = run_skipped(&journal, &["identity", "partner"]);
    assert_eq!(partner.status.code(), Some(0));
    let partner_bytes = fs::read(journal.identity().join("partner.md")).unwrap();
    assert!(partner_bytes.ends_with(b"\n"));
    assert_eq!(partner.stdout, [partner_bytes, b"\n".to_vec()].concat());

    let health = run_skipped(&journal, &["identity", "health"]);
    assert_eq!(health.status.code(), Some(0));
    let health_bytes = fs::read(journal.identity().join("health.md")).unwrap();
    assert!(health_bytes.ends_with(b"\n"));
    assert_eq!(health.stdout, [health_bytes, b"\n".to_vec()].concat());
}

#[test]
fn partner_precedence_and_content_rules_match_the_reference() {
    let journal = TestJournal::new();
    write(
        journal.identity().join("partner.md"),
        "# partner\n\n## H\nold\n",
    );
    let updated = run_skipped(
        &journal,
        &[
            "identity",
            "partner",
            "--write",
            "--update-section=H",
            "--value",
            "  new  ",
        ],
    );
    assert_eq!(updated.status.code(), Some(0));
    assert_eq!(updated.stdout, b"Updated ## H in partner.md.\n");
    assert_eq!(
        fs::read_to_string(journal.identity().join("partner.md")).unwrap(),
        "# partner\n\n## H\nnew\n"
    );

    let raw = run_input(&journal, &["identity", "partner", "--write"], b"  raw  \n");
    assert_eq!(raw.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(journal.identity().join("partner.md")).unwrap(),
        "  raw  \n"
    );

    let missing = run_skipped(
        &journal,
        &[
            "identity",
            "partner",
            "--update-section",
            "H",
            "--value",
            "value",
        ],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(missing.stderr, b"Error: section '## H' not found.\n");

    let empty = run_skipped(
        &journal,
        &[
            "identity",
            "partner",
            "--update-section=",
            "--write",
            "--value",
            "replace",
        ],
    );
    assert_eq!(empty.status.code(), Some(0));
    assert_eq!(empty.stdout, b"partner.md updated.\n");
}

#[test]
fn content_errors_leave_no_history_and_value_wins_over_stdin() {
    let journal = TestJournal::new();
    let partner = journal.identity().join("partner.md");
    write(&partner, "before\n");
    write(journal.identity().join("health.md"), "health\n");
    let history = journal.identity().join("history.jsonl");
    let before_stdin = fs::read(&partner).unwrap();
    let history_before_stdin = fs::read(&history).ok();
    let stdin = run_input(&journal, &["identity", "partner", "--write"], b"   \n");
    assert_eq!(stdin.status.code(), Some(1));
    assert_eq!(stdin.stderr, b"Error: no content provided.\n");
    assert_eq!(fs::read(&partner).unwrap(), before_stdin);
    assert_eq!(fs::read(&history).ok(), history_before_stdin);

    let value = run_input(
        &journal,
        &["identity", "partner", "--write", "--value=from-value"],
        b"from-stdin",
    );
    assert_eq!(value.status.code(), Some(0));
    assert_eq!(fs::read_to_string(&partner).unwrap(), "from-value");

    let before_blank_value = fs::read(&partner).unwrap();
    let history_before_blank_value = fs::read(&history).ok();
    let blank_value = run_skipped(
        &journal,
        &["identity", "partner", "--write", "--value", "  "],
    );
    assert_eq!(blank_value.status.code(), Some(1));
    assert_eq!(blank_value.stderr, b"Error: no content provided.\n");
    assert_eq!(fs::read(&partner).unwrap(), before_blank_value);
    assert_eq!(fs::read(&history).ok(), history_before_blank_value);
}

#[test]
fn briefing_has_the_settled_outputs() {
    let journal = TestJournal::new();
    let absent = run_skipped(&journal, &["identity", "briefing"]);
    assert_eq!(absent.status.code(), Some(1));
    assert_eq!(absent.stderr, b"No briefing found.\n");

    write(
        journal
            .path()
            .join("chronicle/20260101/talents/morning_briefing.json"),
        r#"{"metadata":{},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#,
    );
    let briefing = run_skipped(&journal, &["identity", "briefing", "-d20260101"]);
    assert_eq!(briefing.status.code(), Some(0));
    let expected = concat!(
        "## Your Day\n\nNothing to report.\n\n",
        "## Yesterday\n\nNothing to report.\n\n",
        "## Needs Attention\n\nNothing to report.\n\n",
        "## Forward Look\n\nNothing to report.\n\n",
        "## Reading\n\nNothing to report.\n",
    );
    assert_eq!(briefing.stdout, expected.as_bytes());

    let malformed = run(&journal, &["identity", "briefing", "--day", "bad"]);
    assert_eq!(malformed.status.code(), Some(2));
    assert_eq!(malformed.stdout, b"");
    assert_eq!(
        malformed.stderr,
        b"usage: journal identity briefing [-h] [-d DAY]\njournal identity briefing: error: invalid arguments\n"
    );
}

#[test]
fn refresh_uses_native_callosum_request_and_reports_regeneration() {
    let journal = TestJournal::new();
    let server = start_refresh_server(&journal);

    let output = run_skipped(&journal, &["identity", "health", "--refresh"]);
    let request = server.join().expect("refresh server");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with(&format!(
        "regenerated {} (generated_at: ",
        journal.identity().join("health.md").display()
    )));
    assert!(stdout.ends_with(" bytes)\n"));
    assert_eq!(request.as_object().unwrap().len(), 9);
    assert_eq!(
        request
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "day".to_owned(),
            "event".to_owned(),
            "name".to_owned(),
            "output".to_owned(),
            "prompt".to_owned(),
            "refresh".to_owned(),
            "tract".to_owned(),
            "ts".to_owned(),
            "use_id".to_owned(),
        ])
    );
    assert_eq!(request["tract"], "cortex");
    assert_eq!(request["event"], "request");
    assert!(request["ts"].is_i64() || request["ts"].is_u64());
    assert!(
        request["use_id"]
            .as_str()
            .unwrap()
            .chars()
            .all(char::is_numeric)
    );
    assert_eq!(request["prompt"], "");
    assert_eq!(request["name"], "steward");
    assert_eq!(request["day"].as_str().unwrap().len(), 8);
    assert_eq!(request["output"], "md");
    assert_eq!(request["refresh"], true);
}

#[test]
fn refresh_short_circuits_when_the_existing_health_is_fresh() {
    let journal = TestJournal::new();
    let today = Local::now().format("%Y%m%d").to_string();
    let stamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    write(journal.identity().join("health.md"), &health_body(&stamp));
    write_daily_completion(&journal, &today, 0);

    let output = run_skipped(&journal, &["identity", "health", "--refresh"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stdout,
        format!("already fresh (generated_at: {stamp})\n").as_bytes()
    );
    assert_eq!(output.stderr, b"");
}

#[test]
fn refresh_refuses_a_contended_steward_lock_without_owner_writes() {
    let journal = TestJournal::new();
    let lock_path = journal.path().join("health/.steward.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    let _lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).unwrap();

    let output = run_skipped(&journal, &["identity", "health", "--refresh"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stderr, b"Error: steward already in flight.\n");
    assert!(!journal.path().join("talents").exists());
}

#[test]
fn refresh_checks_the_steward_lock_before_freshness() {
    let journal = TestJournal::new();
    let today = Local::now().format("%Y%m%d").to_string();
    let stamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    write(journal.identity().join("health.md"), &health_body(&stamp));
    write_daily_completion(&journal, &today, 0);
    let lock_path = journal.path().join("health/.steward.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    let _lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).unwrap();

    let output = run_skipped(&journal, &["identity", "health", "--refresh"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"Error: steward already in flight.\n");
}

#[test]
fn refresh_is_gated_before_it_can_send_to_a_bound_socket() {
    let journal = TestJournal::new();
    let socket_path = journal.path().join("health/callosum.sock");
    fs::create_dir_all(socket_path.parent().unwrap()).unwrap();
    let listener = UnixListener::bind(socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();

    let output = run(&journal, &["identity", "health", "--refresh"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        output.stderr,
        b"journal isn't running. start it with 'journal up' and retry.\n"
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!journal.path().join("talents").exists());
}

#[test]
fn refresh_failure_does_not_create_chronicle_days() {
    let journal = TestJournal::new();

    let output = run_skipped(&journal, &["identity", "health", "--refresh"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stderr,
        b"Error: failed to send steward request to cortex.\n"
    );
    assert!(!journal.path().join("chronicle").exists());
}

#[test]
fn unknown_identity_subcommand_names_the_bad_choice() {
    let journal = TestJournal::new();
    let output = run(&journal, &["identity", "bogus"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        output.stderr,
        b"usage: journal identity [-h] {partner,health,briefing} ...\n\
journal identity: error: invalid choice: 'bogus'\n"
    );
}

#[test]
fn native_help_uses_the_reference_capture_as_the_token_source() {
    let journal = TestJournal::new();
    let mut native = String::new();
    for args in [
        ["identity", "--help"].as_slice(),
        ["identity", "partner", "--help"].as_slice(),
        ["identity", "health", "--help"].as_slice(),
        ["identity", "briefing", "--help"].as_slice(),
    ] {
        let output = run(&journal, args);
        assert_eq!(output.status.code(), Some(0));
        native.push_str(&String::from_utf8(output.stdout).unwrap());
    }
    assert!(!native.contains("--install-completion"));
    assert!(!native.contains("--show-completion"));
    assert!(!native.contains("YYYYMMDD/talents/morning_briefing.json"));
    assert!(native.contains("chronicle/") || !native.contains("morning_briefing.json"));

    let captured_flags = reference_flags(REFERENCE_HELP);
    for flag in captured_flags {
        if flag == "--install-completion" || flag == "--show-completion" {
            continue;
        }
        assert!(
            native.contains(&flag),
            "native help omitted captured flag {flag}"
        );
    }
    for command in reference_commands(REFERENCE_HELP) {
        assert!(
            native.contains(&command),
            "native help omitted captured command {command}"
        );
    }
    assert!(native.contains("-h, --help"));
}

#[test]
fn identity_never_invokes_a_path_python() {
    // This is the PATH-shim proof, not the sibling-interpreter proof: the journal
    // dispatcher resolves python3 beside its own executable and never reads PATH.
    // The sibling probe tests the separate sibling-interpreter path.
    let journal = TestJournal::new();
    let shim_dir = tempfile::tempdir().expect("shims");
    let marker = shim_dir.path().join("called");
    for name in ["python", "python3"] {
        let shim = shim_dir.path().join(name);
        fs::write(
            &shim,
            format!("#!/bin/sh\nprintf called > {}\nexit 97\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        shim_dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let output = command(&journal, &["identity"])
        .env("SOL_SKIP_SUPERVISOR_CHECK", "1")
        .env("PATH", path)
        .output()
        .expect("run identity");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        !marker.exists(),
        "native identity invoked a PATH Python shim"
    );
}

fn reference_flags(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .filter(|token| token.starts_with("--") || (token.len() == 2 && token.starts_with('-')))
        .map(str::to_owned)
        .collect()
}

fn reference_commands(text: &str) -> BTreeSet<String> {
    text.lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}
