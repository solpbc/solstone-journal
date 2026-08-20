// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use solstone_core_journal_io::{LeaseOptions, acquire_file_lease};

const RESULT_SCHEMA: &str = "solstone.brain.refresh.result.v1";
const READY_SCHEMA: &str = "solstone.brain.refresh.ready.v1";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-core-brain-refresh-{name}-{stamp}"))
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("test path has parent")).expect("create parent");
    fs::write(path, contents).expect("write test file");
}

fn configured_journal(name: &str, provider: &str) -> PathBuf {
    let root = temp_path(name);
    fs::create_dir(&root).expect("create root");
    let config = json!({
        "env": {"ANTHROPIC_API_KEY": "sk-test"},
        "providers": {"active": {"provider": provider}},
    });
    write(
        &root,
        "config/journal.json",
        &serde_json::to_vec(&config).expect("encode config"),
    );
    if provider != "none" {
        write(&root, "health/brain-fingerprint.key", &[7_u8; 32]);
    }
    root
}

fn ready_outcome() -> Value {
    let now = Utc::now();
    let observed_at = now.to_rfc3339();
    let expires_at = (now + ChronoDuration::hours(1)).to_rfc3339();
    let component = || {
        json!({
            "status": "ok",
            "observed_at": observed_at,
            "expires_at": expires_at,
        })
    };
    json!({
        "configuration": component(),
        "lane_prerequisites": component(),
        "generate": component(),
        "cogitate": component(),
    })
}

fn probe(outcome: Value) -> Value {
    json!({
        "schema": "solstone.brain.refresh.probe.v1",
        "outcome": outcome,
    })
}

fn abandon(reason_code: &str) -> Value {
    json!({
        "schema": "solstone.brain.refresh.abandon.v1",
        "reason_code": reason_code,
    })
}

fn terminal() -> Value {
    json!({"schema": "solstone.brain.refresh.terminal.v1"})
}

fn start(root: &Path) -> Child {
    Command::new(bin())
        .args(["brain", "refresh", "--session", "--journal"])
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("solstone-core should execute")
}

fn finish(mut child: Child, records: &[Value]) -> Output {
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    for record in records {
        serde_json::to_writer(&mut stdin, record).expect("encode input record");
        stdin.write_all(b"\n").expect("write input newline");
    }
    drop(stdin);
    child.wait_with_output().expect("wait for solstone-core")
}

fn parse_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("session should emit JSON")
}

fn parse_ready_and_result(output: &Output) -> Option<Value> {
    let mut lines = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty());
    let ready: Value = serde_json::from_slice(lines.next().expect("ready line"))
        .expect("ready line should be JSON");
    assert_eq!(ready["schema"], READY_SCHEMA);
    let result = lines
        .next()
        .map(|line| serde_json::from_slice(line).expect("result line should be JSON"));
    assert!(
        lines.next().is_none(),
        "session should emit at most one result"
    );
    result
}

fn assert_abandoned(output: &Output, status: i32) {
    assert_eq!(
        output.status.code(),
        Some(status),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = parse_ready_and_result(output).expect("abandonment result");
    assert_eq!(result["schema"], RESULT_SCHEMA);
    assert_eq!(result["kind"], "abandoned");
    assert_eq!(result["reason_code"], "brain_refresh_timeout");
    assert_eq!(result["component"], "generate");
}

fn assert_lease_free(root: &Path) {
    assert!(
        acquire_file_lease(
            solstone_core_brain::brain_refresh_lease_path(root),
            LeaseOptions::default(),
        )
        .expect("probe lease")
        .is_some(),
        "refresh lease should be released"
    );
}

fn direct_projection_after_finish(root: &Path, outcome: Value) -> Value {
    let permit = solstone_core_brain::begin_refresh(root, Utc::now(), None, None, false, None)
        .expect("begin refresh")
        .expect("refresh permit");
    solstone_core_brain::finish_refresh(root, permit, outcome, Utc::now(), None)
        .expect("finish refresh");
    let config = solstone_core_brain::read_journal_config(root)
        .expect("read config")
        .config
        .unwrap_or_default();
    let projection = solstone_core_brain::inspect_brain_state(root, &config, Utc::now()).projection;
    json!({
        "schema": RESULT_SCHEMA,
        "kind": "projection",
        "projection": {
            "aggregate_state": projection.aggregate_state,
            "reason_code": projection.reason_code,
            "active_lane": projection.active_lane,
            "active_provider": projection.active_provider,
            "active_model": projection.active_model,
            "fingerprint_sha256": projection.fingerprint_sha256,
            "runtime_transition_in_progress": projection.runtime_transition_in_progress,
        },
    })
}

#[test]
fn clean_terminal_finishes_and_reports_the_fresh_inspection_projection() {
    let outcome = ready_outcome();
    let session_root = configured_journal("clean-session", "anthropic");
    let direct_root = configured_journal("clean-direct", "anthropic");

    let output = finish(start(&session_root), &[probe(outcome.clone()), terminal()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        parse_ready_and_result(&output).expect("projection result"),
        direct_projection_after_finish(&direct_root, outcome)
    );

    fs::remove_dir_all(session_root).expect("cleanup session root");
    fs::remove_dir_all(direct_root).expect("cleanup direct root");
}

#[test]
fn bare_eof_abandons_without_a_report_and_releases_the_lease() {
    let root = configured_journal("bare-eof", "anthropic");
    let output = finish(start(&root), &[]);
    assert_eq!(output.status.code(), Some(69));
    assert!(parse_ready_and_result(&output).is_none());
    assert_lease_free(&root);
    let record: Value = serde_json::from_slice(
        &fs::read(solstone_core_brain::brain_state_path(&root)).expect("read brain record"),
    )
    .expect("record JSON");
    assert!(record["checking"].is_null());
    assert_eq!(record["reason_code"], "brain_refresh_timeout");
    assert_eq!(
        record["evidence"]["generate"]["reason_code"],
        "brain_refresh_timeout"
    );
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn timeout_abandons_a_live_session_without_wedging_the_lease() {
    let root = configured_journal("timeout", "anthropic");
    let mut child = Command::new(bin())
        .args(["brain", "refresh", "--session", "--journal"])
        .arg(&root)
        .env("SOLSTONE_CORE_BRAIN_SESSION_TIMEOUT_MS", "50")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("solstone-core should execute");
    // Hold stdin OPEN for the whole wait. The behavior under test is a caller
    // that hangs — alive, silent, never closing — and the child bounding itself
    // so a stuck caller cannot wedge the permit forever. Closing stdin is the
    // other signal entirely: a bare EOF means the caller is gone, and it exits 69.
    //
    // `wait_with_output` drains stdout and stderr while it waits. A poll loop
    // that does not drain them can deadlock a child on full output pipes.
    let stdin = child.stdin.take().expect("stdin should be piped");
    let output = child
        .wait_with_output()
        .expect("wait for self-bounded child");
    drop(stdin);
    assert_abandoned(&output, 0);
    assert_lease_free(&root);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn malformed_json_abandons_with_protocol_exit() {
    let root = configured_journal("malformed", "anthropic");
    let mut child = start(&root);
    let mut stdin = child.stdin.take().expect("stdin should be piped");
    stdin
        .write_all(b"{not json}\n")
        .expect("write malformed input");
    drop(stdin);
    let output = child.wait_with_output().expect("wait for child");
    assert_abandoned(&output, 76);
    assert_lease_free(&root);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn terminal_before_probe_abandons_with_protocol_exit() {
    let root = configured_journal("terminal-first", "anthropic");
    let output = finish(start(&root), &[terminal()]);
    assert_abandoned(&output, 76);
    assert_lease_free(&root);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn duplicate_probe_abandons_with_protocol_exit() {
    let root = configured_journal("duplicate-probe", "anthropic");
    let output = finish(
        start(&root),
        &[probe(ready_outcome()), probe(ready_outcome())],
    );
    assert_abandoned(&output, 76);
    assert_lease_free(&root);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn caller_abandon_records_its_reason_and_target_component() {
    let root = configured_journal("caller-abandon", "anthropic");
    let output = finish(start(&root), &[abandon("provider_unavailable"), terminal()]);
    assert_eq!(output.status.code(), Some(0));
    let result = parse_ready_and_result(&output).expect("abandonment result");
    assert_eq!(result["schema"], RESULT_SCHEMA);
    assert_eq!(result["kind"], "abandoned");
    assert_eq!(result["reason_code"], "provider_unavailable");
    assert_eq!(result["component"], "generate");
    assert_lease_free(&root);
    let record: Value = serde_json::from_slice(
        &fs::read(solstone_core_brain::brain_state_path(&root)).expect("read brain record"),
    )
    .expect("record JSON");
    assert!(record["checking"].is_null());
    assert_eq!(record["reason_code"], "provider_unavailable");
    assert_eq!(
        record["evidence"]["generate"]["reason_code"],
        "provider_unavailable"
    );
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn abandon_followed_by_another_request_is_a_protocol_violation() {
    let root = configured_journal("duplicate-abandon", "anthropic");
    let output = finish(
        start(&root),
        &[abandon("provider_unavailable"), probe(ready_outcome())],
    );
    assert_abandoned(&output, 76);
    assert_lease_free(&root);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn none_lane_exits_immediately_with_a_projection() {
    let root = configured_journal("none", "none");
    let output = finish(start(&root), &[]);
    assert_eq!(output.status.code(), Some(0));
    let result = parse_output(&output);
    assert_eq!(result["schema"], RESULT_SCHEMA);
    assert_eq!(result["kind"], "projection");
    assert_eq!(result["projection"]["active_lane"], "none");
    assert!(!solstone_core_brain::brain_refresh_lease_path(&root).exists());
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn contention_exits_immediately_without_writing_a_brain_record() {
    let root = configured_journal("contention", "anthropic");
    let _lease = acquire_file_lease(
        solstone_core_brain::brain_refresh_lease_path(&root),
        LeaseOptions::default(),
    )
    .expect("acquire holder lease")
    .expect("holder lease");
    let output = finish(start(&root), &[]);
    assert_eq!(output.status.code(), Some(0));
    let result = parse_output(&output);
    assert_eq!(result["schema"], RESULT_SCHEMA);
    assert_eq!(result["kind"], "not_started");
    assert_eq!(result["status"], "no_permit");
    assert_eq!(result["reason"], "lease_held");
    assert!(!solstone_core_brain::brain_state_path(&root).exists());
    fs::remove_dir_all(root).expect("cleanup root");
}
