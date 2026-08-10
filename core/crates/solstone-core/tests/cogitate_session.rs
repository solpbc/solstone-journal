// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const BOUND: Duration = Duration::from_secs(3);
const ENDPOINT_ENV: &str = "SOLSTONE_COGITATE_ENDPOINT_URL_OVERRIDE";
const API_KEY_ENV: &str = "SOLSTONE_COGITATE_API_KEY_OVERRIDE";

struct TempJournal(PathBuf);

impl TempJournal {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-core-cogitate-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos(),
        ));
        fs::create_dir_all(&path).expect("create journal root");
        Self(path)
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Stub {
    url: String,
    handle: thread::JoinHandle<()>,
}

impl Stub {
    fn start(responses: Vec<(Value, Duration)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("stub bind");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let handle = thread::spawn(move || {
            for (body, delay) in responses {
                let (mut stream, _) = listener.accept().expect("stub accept");
                read_request(&mut stream);
                thread::sleep(delay);
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).expect("stub write");
            }
        });
        Self { url, handle }
    }

    fn join(self) {
        self.handle.join().expect("stub joins");
    }
}

fn read_request(stream: &mut TcpStream) {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("stub read");
        assert!(read > 0, "request closed before headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = std::str::from_utf8(&bytes[..header_end]).expect("headers are UTF-8");
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .expect("request content length");
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("stub body read");
        assert!(read > 0, "request closed before body");
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn request(journal: &TempJournal, dry_run: bool) -> Value {
    json!({
        "schema": "solstone-cogitate-request-v1",
        "access_tier": "normal",
        "outbound_approval": null,
        "expects_emit_final": true,
        "max_turns": 4,
        "cost_cap_usd": 1.0,
        "context_window": 4096,
        "timeout_ms": 30_000,
        "read_call_budget": 5,
        "model": "fixture-model",
        "correlation_id": "session-corr",
        "initial_prompt": "Do the task.",
        "system_instruction": "Be concise.",
        "journal_root": journal.0,
        "dry_run": dry_run
    })
}

fn final_response() -> Value {
    json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": "final-1",
                    "type": "function",
                    "function": {"name": "emit_final", "arguments": "{\"content\":\"done\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
    })
}

fn read_file_response() -> Value {
    json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": "read-1",
                    "type": "function",
                    "function": {"name": "read_file", "arguments": "{\"path\":\"missing.txt\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
    })
}

fn spawn_one_shot(input: &str, endpoint: Option<&str>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["cogitate", "--one-shot"])
        .env_remove(ENDPOINT_ENV)
        .env_remove(API_KEY_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(endpoint) = endpoint {
        command
            .env(ENDPOINT_ENV, endpoint)
            .env(API_KEY_ENV, "test-cogitate-credential");
    }
    let mut child = command.spawn().expect("spawn cogitate core");
    child
        .stdin
        .take()
        .expect("cogitate stdin")
        .write_all(input.as_bytes())
        .expect("write cogitate request");
    child
}

fn parsed_lines(stdout: &[u8]) -> Vec<Value> {
    let text = std::str::from_utf8(stdout).expect("stdout is UTF-8");
    assert!(text.ends_with('\n'), "stdout must end in an NDJSON newline");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("stdout line is JSON"))
        .collect()
}

#[test]
fn one_shot_streams_valid_terminal_ndjson() {
    let journal = TempJournal::new("one-shot");
    let stub = Stub::start(vec![(final_response(), Duration::ZERO)]);
    let child = spawn_one_shot(&request(&journal, false).to_string(), Some(&stub.url));
    let output = child.wait_with_output().expect("wait cogitate core");
    stub.join();
    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
    let lines = parsed_lines(&output.stdout);
    assert!(
        lines
            .iter()
            .any(|line| { matches!(line["event"].as_str(), Some("finish") | Some("error")) })
    );
    assert_eq!(lines.last().expect("terminal line")["terminal"], true);
}

#[test]
fn one_shot_streams_before_process_exit() {
    let journal = TempJournal::new("streaming");
    let stub = Stub::start(vec![
        (read_file_response(), Duration::ZERO),
        (final_response(), Duration::from_millis(300)),
    ]);
    let mut child = spawn_one_shot(&request(&journal, false).to_string(), Some(&stub.url));
    let stdout = child.stdout.take().expect("cogitate stdout");
    let (sender, receiver) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        let mut first = String::new();
        stdout
            .read_line(&mut first)
            .expect("read first NDJSON line");
        sender.send(first).expect("send first NDJSON line");
        let mut rest = String::new();
        stdout
            .read_to_string(&mut rest)
            .expect("drain remaining stdout");
    });
    let first = receiver
        .recv_timeout(BOUND)
        .expect("first NDJSON line before timeout");
    assert!(serde_json::from_str::<Value>(&first).is_ok());
    assert!(
        child.try_wait().expect("poll cogitate child").is_none(),
        "child exited before the observed first stream line"
    );
    assert_eq!(child.wait().expect("wait cogitate child").code(), Some(0));
    reader.join().expect("stdout reader joins");
    stub.join();
}

#[test]
fn dry_run_needs_no_endpoint_configuration() {
    let journal = TempJournal::new("dry-run");
    let child = spawn_one_shot(&request(&journal, true).to_string(), None);
    let output = child.wait_with_output().expect("wait dry run");
    assert_eq!(output.status.code(), Some(0), "stderr: {:?}", output.stderr);
    let lines = parsed_lines(&output.stdout);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "dry_run");
}

#[test]
fn malformed_one_shot_stays_off_stdout() {
    let child = spawn_one_shot("{", None);
    let output = child.wait_with_output().expect("wait malformed request");
    assert_eq!(output.status.code(), Some(65));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}
