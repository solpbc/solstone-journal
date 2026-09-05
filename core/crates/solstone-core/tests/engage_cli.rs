// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output, Stdio};
use std::thread;

use serde_json::Value;
use tempfile::TempDir;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");

fn run(journal: &TempDir, args: &[&str], prompt: &[u8]) -> Output {
    let mut child = Command::new(BINARY)
        .args(args)
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("HOME", journal.path().join("home"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn solstone-core");
    child.stdin.take().unwrap().write_all(prompt).unwrap();
    child.wait_with_output().unwrap()
}

fn bind(journal: &TempDir) -> UnixListener {
    let health = journal.path().join("health");
    fs::create_dir_all(&health).unwrap();
    UnixListener::bind(health.join("callosum.sock")).unwrap()
}

fn claim_one(
    journal: &TempDir,
    listener: UnixListener,
    terminal: Option<&'static str>,
) -> thread::JoinHandle<Value> {
    let journal = journal.path().to_path_buf();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        let use_id = request["use_id"].as_str().unwrap();
        let path = journal
            .join("talents")
            .join(request["name"].as_str().unwrap())
            .join(format!("{use_id}_active.jsonl"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut body = format!("{request}\n");
        if let Some(terminal) = terminal {
            let mut value: Value = serde_json::from_str(terminal).unwrap();
            value["use_id"] = Value::String(use_id.to_string());
            body.push_str(&format!("{value}\n"));
        }
        fs::write(path, body).unwrap();
        request
    })
}

#[test]
fn engage_parser_owns_usage_and_empty_prompt_error() {
    let journal = TempDir::new().unwrap();
    let invalid = run(&journal, &["engage", "--nonsense"], b"prompt");
    assert_eq!(invalid.status.code(), Some(2));
    assert!(
        invalid
            .stderr
            .starts_with(b"usage: journal engage [-h] [--wait]")
    );

    let help = run(&journal, &["engage", "--help"], b"");
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(help.stdout.starts_with(b"usage: journal engage"));

    let empty = run(&journal, &["engage", "partner"], b" \n\t");
    assert_eq!(empty.status.code(), Some(1));
    assert_eq!(
        empty.stderr,
        b"Error: no prompt provided on stdin.\n".as_slice()
    );
    assert!(!journal.path().join("talents").exists());

    let help_as_value = run(&journal, &["engage", "partner", "--facet", "--help"], b"");
    assert_eq!(help_as_value.status.code(), Some(1));
    assert_eq!(
        help_as_value.stderr,
        b"Error: no prompt provided on stdin.\n".as_slice()
    );
}

#[test]
fn engage_claims_flat_request_and_prints_use_id() {
    let journal = TempDir::new().unwrap();
    let server = claim_one(&journal, bind(&journal), None);
    let output = run(
        &journal,
        &["engage", "--facet", "work", "partner", "--day=20260404"],
        b"  review this  \n",
    );
    let request = server.join().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        request["use_id"].as_str().unwrap()
    );
    assert_eq!(request["tract"], "cortex");
    assert_eq!(request["event"], "request");
    assert_eq!(request["prompt"], "review this");
    assert_eq!(request["name"], "partner");
    assert_eq!(request["facet"], "work");
    assert_eq!(request["day"], "20260404");
}

#[test]
fn engage_wait_recovers_finish_result_from_durable_log() {
    let journal = TempDir::new().unwrap();
    let server = claim_one(
        &journal,
        bind(&journal),
        Some("{\"event\":\"finish\",\"result\":\"finished work\"}"),
    );
    let output = run(&journal, &["engage", "partner", "--wait"], b"do the thing");
    let _request = server.join().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"finished work\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn engage_wait_reports_durable_error_state() {
    let journal = TempDir::new().unwrap();
    let server = claim_one(&journal, bind(&journal), Some("{\"event\":\"error\"}"));
    let output = run(&journal, &["engage", "--wait", "partner"], b"do the thing");
    let _request = server.join().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"Error: agent ended with state: error\n");
}
