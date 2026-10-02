// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-core-chatgpt-plan-{name}-{stamp}"))
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("test path has parent")).expect("create parent");
    fs::write(path, contents).expect("write test file");
}

fn configured_journal(name: &str) -> PathBuf {
    let root = temp_path(name);
    fs::create_dir_all(&root).expect("create root");
    let config = json!({
        "providers": {
            "active": {
                "provider": "chatgpt",
                "model": "gpt-test"
            }
        }
    });
    write(
        &root,
        "config/journal.json",
        &serde_json::to_vec(&config).expect("encode config"),
    );
    write(&root, "health/brain-fingerprint.key", &[7_u8; 32]);
    solstone_core_thinking::chatgpt::write_expired_test_credential(&root)
        .expect("write expired test credential");
    root
}

const SENTINEL: &str = "\x1fsolstone-journal-brain-owner-v1";

#[test]
fn acceptance_14_owner_refresh_lifecycle() {
    let journal = configured_journal("acceptance-14");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().expect("local addr").port();
    let base_url = format!("http://127.0.0.1:{port}");

    let grant_count = Arc::new(AtomicUsize::new(0));
    let grant_count_clone = grant_count.clone();

    let _server = thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 4096];
            let n = match stream.read(&mut buf) {
                Ok(n) if n > 0 => n,
                _ => continue,
            };
            let req_str = String::from_utf8_lossy(&buf[..n]);

            if req_str.contains("/oauth/token") {
                let count = grant_count_clone.fetch_add(1, Ordering::SeqCst);
                let rotated_token = format!("rotated-refresh-{count}");
                let body = json!({
                    "access_token": format!("bearer-token-{count}"),
                    "refresh_token": rotated_token,
                    "expires_in": 3600,
                    "scope": "openid profile email chatgpt.tokens.use.direct"
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            } else if req_str.contains("/oauth/revoke") {
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
            } else if req_str.contains("/v1/responses") {
                let body = r#"{"error":{"message":"unauthorized","code":"invalid_token"}}"#;
                let resp = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        }
    });

    let _output = Command::new(bin())
        .args([SENTINEL, "brain", "refresh", "--json"])
        .env("SOLSTONE_JOURNAL", &journal)
        .env("SOLSTONE_GENERATE_BASE_URL_OVERRIDE", &base_url)
        .env("SOLSTONE_CHATGPT_AUTH_BASE_URL_OVERRIDE", &base_url)
        .env("SOLSTONE_CHATGPT_API_BASE_URL_OVERRIDE", &base_url)
        .output()
        .expect("run journal brain refresh");

    let brain_path = journal.join("health/brain.json");
    assert!(brain_path.exists(), "brain record exists");
    let brain_bytes = fs::read(&brain_path).expect("read brain.json");
    let brain: Value = serde_json::from_slice(&brain_bytes).expect("parse brain.json");

    assert_eq!(brain["aggregate_state"], "blocked");
    assert_eq!(brain["reason_code"], "chatgpt_sign_in_required");
    assert_ne!(brain["aggregate_state"], "checking");

    // lease is not held
    let lease_path = journal.join("health/brain-refresh.lease");
    assert!(
        !lease_path.exists()
            || fs::read_to_string(&lease_path)
                .unwrap_or_default()
                .is_empty()
    );

    // Sign out with forget: true
    let transport = solstone_core_chatgpt_auth::UreqTransport;
    let _ = solstone_core_thinking::chatgpt::sign_out(&journal, &transport, true);

    // Run refresh again
    let _output2 = Command::new(bin())
        .args([SENTINEL, "brain", "refresh", "--json"])
        .env("SOLSTONE_JOURNAL", &journal)
        .env("SOLSTONE_GENERATE_BASE_URL_OVERRIDE", &base_url)
        .env("SOLSTONE_CHATGPT_AUTH_BASE_URL_OVERRIDE", &base_url)
        .env("SOLSTONE_CHATGPT_API_BASE_URL_OVERRIDE", &base_url)
        .output()
        .expect("run journal brain refresh 2");

    let brain_bytes2 = fs::read(&brain_path).expect("read brain.json 2");
    let brain2: Value = serde_json::from_slice(&brain_bytes2).expect("parse brain.json 2");
    assert_eq!(brain2["reason_code"], "chatgpt_sign_in_required");
    assert_eq!(brain2["evidence"]["generate"]["status"], "not_attempted");

    let _ = fs::remove_dir_all(journal);
}

#[test]
fn acceptance_19_concurrent_and_refresh_recovery() {
    // Subtest 1: 6 concurrent processes, expired credential, responses 200
    let journal1 = configured_journal("acceptance-19-1");
    let listener1 = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port1 = listener1.local_addr().expect("local addr").port();
    let base_url1 = format!("http://127.0.0.1:{port1}");

    let grants1 = Arc::new(AtomicUsize::new(0));
    let grants1_clone = grants1.clone();

    let _server1 = thread::spawn(move || {
        for stream in listener1.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 4096];
            let n = match stream.read(&mut buf) {
                Ok(n) if n > 0 => n,
                _ => continue,
            };
            let req_str = String::from_utf8_lossy(&buf[..n]);

            if req_str.contains("/oauth/token") {
                grants1_clone.fetch_add(1, Ordering::SeqCst);
                let body = json!({
                    "access_token": "plan-token-live",
                    "refresh_token": "rotated-refresh-token",
                    "expires_in": 3600,
                    "scope": "openid profile email chatgpt.tokens.use.direct"
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            } else if req_str.contains("/v1/responses") {
                // Responses 200 SSE has no Content-Type
                let sse_body = "data: {\"type\":\"response.output_item.done\",\"item\":{\"content\":[{\"type\":\"output_text\",\"text\":\"plan says hello\"}]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    sse_body.len(),
                    sse_body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        }
    });

    let request_payload = json!({
        "schema": "solstone-generate-request-v2",
        "id": "test-req",
        "attempt_index": 0,
        "context": "test",
        "contents": [{"type": "text", "text": "hello"}],
        "json_output": false,
        "json_schema": {"type": "object", "properties": {"msg": {"type": "string"}}, "additionalProperties": false},
        "temperature": 0.3,
        "max_output_tokens": 100,
        "enforce_responsiveness": true,
        "exclusive_admission": false
    });

    let mut handles = Vec::new();
    for _ in 0..6 {
        let j = journal1.clone();
        let u = base_url1.clone();
        let req = request_payload.clone();
        handles.push(thread::spawn(move || {
            let mut child = Command::new(bin())
                .args(["generate", "--one-shot"])
                .env("SOLSTONE_JOURNAL", &j)
                .env("SOLSTONE_GENERATE_BASE_URL_OVERRIDE", &u)
                .env("SOLSTONE_CHATGPT_AUTH_BASE_URL_OVERRIDE", &u)
                .env("SOLSTONE_CHATGPT_API_BASE_URL_OVERRIDE", &u)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn generate");

            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&req).unwrap())
                .unwrap();

            let out = child.wait_with_output().expect("wait generate");
            assert_eq!(out.status.code(), Some(0));
            let resp: Value = serde_json::from_slice(&out.stdout).expect("parse stdout");
            assert_eq!(resp["text"], "plan says hello");
        }));
    }

    for h in handles {
        h.join().expect("thread join");
    }

    let g1 = grants1.load(Ordering::SeqCst);
    println!("Acceptance 19 part 1 token grant count: {g1}");
    assert_eq!(g1, 1, "Token stub saw grant count {g1}, expected 1");

    let _ = fs::remove_dir_all(journal1);

    // Subtest 2: 401 on T1, holds responses until 6 arrive, then 401s, then 200s T2 with sample
    let journal2 = configured_journal("acceptance-19-2");
    let listener2 = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port2 = listener2.local_addr().expect("local addr").port();
    let base_url2 = format!("http://127.0.0.1:{port2}");

    let grants2 = Arc::new(AtomicUsize::new(0));
    let grants2_clone = grants2.clone();
    let second_refresh_token = Arc::new(Mutex::new(String::new()));
    let second_refresh_token_clone = second_refresh_token.clone();

    let t1_requests = Arc::new(Mutex::new(Vec::new()));
    let t1_requests_clone = t1_requests.clone();

    let _server2 = thread::spawn(move || {
        for stream in listener2.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut buf = [0u8; 4096];
            let n = match stream.read(&mut buf) {
                Ok(n) if n > 0 => n,
                _ => continue,
            };
            let req_str = String::from_utf8_lossy(&buf[..n]);

            if req_str.contains("/oauth/token") {
                let count = grants2_clone.fetch_add(1, Ordering::SeqCst);
                if count == 1 {
                    // check refresh token form parameter
                    if let Some(pos) = req_str.find("refresh_token=") {
                        let form_part = &req_str[pos + "refresh_token=".len()..];
                        let end = form_part.find('&').unwrap_or(form_part.len());
                        *second_refresh_token_clone.lock().unwrap() =
                            form_part[..end].trim().to_string();
                    }
                }
                let body = json!({
                    "access_token": format!("bearer-t{}", count + 1),
                    "refresh_token": "rotated-refresh-token-2",
                    "expires_in": 3600,
                    "scope": "openid profile email chatgpt.tokens.use.direct"
                })
                .to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            } else if req_str.contains("bearer-t1") {
                let mut reqs = t1_requests_clone.lock().unwrap();
                reqs.push(stream);
                if reqs.len() == 6 {
                    for mut s in reqs.drain(..) {
                        let body = r#"{"error":{"message":"unauthorized","code":"invalid_token"}}"#;
                        let resp = format!(
                            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = s.write_all(resp.as_bytes());
                    }
                }
            } else if req_str.contains("bearer-t2") {
                let sse_body = "data: {\"type\":\"response.output_item.done\",\"item\":{\"content\":[{\"type\":\"output_text\",\"text\":\"plan says hello\"}]}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-test\"}}\n\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    sse_body.len(),
                    sse_body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        }
    });

    let mut handles2 = Vec::new();
    for _ in 0..6 {
        let j = journal2.clone();
        let u = base_url2.clone();
        let req = request_payload.clone();
        handles2.push(thread::spawn(move || {
            let mut child = Command::new(bin())
                .args(["generate", "--one-shot"])
                .env("SOLSTONE_JOURNAL", &j)
                .env("SOLSTONE_GENERATE_BASE_URL_OVERRIDE", &u)
                .env("SOLSTONE_CHATGPT_AUTH_BASE_URL_OVERRIDE", &u)
                .env("SOLSTONE_CHATGPT_API_BASE_URL_OVERRIDE", &u)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn generate 2");

            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&req).unwrap())
                .unwrap();

            let out = child.wait_with_output().expect("wait generate 2");
            assert_eq!(out.status.code(), Some(0));
            let resp: Value = serde_json::from_slice(&out.stdout).expect("parse stdout 2");
            assert_eq!(resp["text"], "plan says hello");
        }));
    }

    for h in handles2 {
        h.join().expect("thread join 2");
    }

    let g2 = grants2.load(Ordering::SeqCst);
    println!("Acceptance 19 part 2 token grant count: {g2}");
    assert_eq!(g2, 2, "Token stub saw grant count {g2}, expected 2");

    let _ = fs::remove_dir_all(journal2);
}
