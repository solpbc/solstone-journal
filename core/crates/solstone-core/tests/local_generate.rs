// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn root(name: &str) -> PathBuf {
    let suffix = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "solstone-core-local-generate-{name}-{stamp}-{suffix}"
    ));
    std::fs::create_dir_all(root.join("health")).expect("create health directory");
    root
}

fn http_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn path(mut stream: TcpStream) -> String {
    let mut bytes = [0_u8; 8192];
    let read = stream.read(&mut bytes).expect("read request");
    let request = String::from_utf8_lossy(&bytes[..read]);
    let path = request
        .split_whitespace()
        .nth(1)
        .expect("HTTP request path")
        .to_owned();
    let body = match path.as_str() {
        "/health" => r#"{"loaded_model":"served"}"#,
        "/props" => r#"{"n_ctx":16384,"total_slots":1}"#,
        "/tokenize" => r#"{"tokens":[1]}"#,
        "/v1/chat/completions/input_tokens" => {
            r#"{"object":"response.input_tokens","input_tokens":1}"#
        }
        "/v1/chat/completions" => {
            r#"{"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#
        }
        other => panic!("unexpected local HTTP path: {other}"),
    };
    stream
        .write_all(http_response(200, body).as_bytes())
        .expect("write response");
    path
}

fn serve() -> (u16, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let port = listener.local_addr().expect("stub address").port();
    let handle = thread::spawn(move || {
        let mut paths = Vec::new();
        for _ in 0..5 {
            let (stream, _) = listener.accept().expect("accept request");
            paths.push(path(stream));
        }
        paths
    });
    (port, handle)
}

fn input(root: &std::path::Path) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "solstone-local-generate-input-v1",
        "journal_path": root,
        "bind_address": "127.0.0.1",
        "default_model_id": "default",
        "platform": "linux",
        "contents": "hello",
        "system_instruction": null,
        "temperature": 0.2,
        "max_output_tokens": 256,
        "json_output": false,
        "json_schema": null,
        "timeout_s": 5.0,
        "exclusive_admission": false,
        "attempt_index": 0
    }))
    .expect("serialize generate input")
}

fn run(input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["local", "generate"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start solstone-core local generate");
    child
        .stdin
        .as_mut()
        .expect("generate stdin")
        .write_all(input)
        .expect("write generate input");
    child.wait_with_output().expect("wait for local generate")
}

#[test]
fn generate_binary_emits_one_success_json_record() {
    let journal = root("success");
    let (port, server) = serve();
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write local port");
    let output = run(&input(&journal));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let result: Value = serde_json::from_slice(&output.stdout).expect("single JSON stdout record");
    assert_eq!(result["outcome"], "success", "{result}");
    assert_eq!(result["text"], "hello");
    assert_eq!(server.join().expect("join stub").len(), 5);
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn generate_binary_keeps_refusal_stdout_json_only() {
    let journal = root("refusal");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let port = listener.local_addr().expect("stub address").port();
    let server = thread::spawn(move || {
        for index in 0..5 {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 8192];
            let read = stream.read(&mut request).expect("read request");
            let path = String::from_utf8_lossy(&request[..read])
                .split_whitespace()
                .nth(1)
                .expect("request path")
                .to_owned();
            let body = if index == 4 {
                "not-json"
            } else if path == "/health" {
                r#"{"loaded_model":"served"}"#
            } else if path == "/props" {
                r#"{"n_ctx":16384,"total_slots":1}"#
            } else if path == "/v1/chat/completions/input_tokens" {
                r#"{"object":"response.input_tokens","input_tokens":1}"#
            } else {
                r#"{"tokens":[1]}"#
            };
            stream
                .write_all(http_response(200, body).as_bytes())
                .expect("write response");
        }
    });
    std::fs::write(journal.join("health/local.port"), port.to_string()).expect("write local port");
    let output = run(&input(&journal));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let result: Value = serde_json::from_slice(&output.stdout).expect("single JSON stdout record");
    assert_eq!(result["outcome"], "failure");
    assert!(result["reason_code"].is_null());
    server.join().expect("join refusal stub");
    let _ = std::fs::remove_dir_all(journal);
}
