// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real loopback HTTP exchange for the confidential attested transport adapter.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_generate_wire::{
    ConverseMessage, ConverseToolSpec, EndpointResult, EndpointRuntime,
    test_support::{confidential_converse_over_channel, confidential_generate_over_channel},
};
use solstone_core_local::ByoEndpoint;
use solstone_core_spp_ratls::AttestedIo;

fn request(timeout_s: Option<f64>) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "test.confidential".into(),
        contents: vec![ContentPart::Text {
            text: "Hello".into(),
        }],
        system_instruction: None,
        temperature: 0.2,
        max_output_tokens: 64,
        thinking_budget: None,
        timeout_s,
        json_output: false,
        json_schema: None,
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn endpoint(port: u16) -> ByoEndpoint {
    ByoEndpoint {
        base_url: format!("http://127.0.0.1:{port}"),
        served_model_id: "served".into(),
        credential: Some("token".into()),
        parallel_slots: None,
        is_confidential: true,
        is_bundled: false,
    }
}

fn journal(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solstone-confidential-transport-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create journal");
    path
}

fn served_window_config() -> Map<String, Value> {
    json!({"providers": {"local": {"served_context_window": 2048}}})
        .as_object()
        .expect("config object")
        .clone()
}

fn converse_messages() -> Vec<ConverseMessage> {
    vec![ConverseMessage::User { text: "ask".into() }]
}

fn converse_tools() -> Vec<ConverseToolSpec> {
    vec![ConverseToolSpec {
        name: "weather".into(),
        description: "weather".into(),
        parameters: json!({"type": "object"}),
    }]
}

fn generate_response_body() -> &'static str {
    r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#
}

fn converse_response_body() -> String {
    json!({
        "choices": [{
            "message": {
                "content": "before",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": {"name": "weather", "arguments": "{\"city\":\"Denver\"}"},
                }],
            },
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5},
    })
    .to_string()
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).expect("read request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        let content_length = header
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("content-length")
                    .then(|| value.trim())
            })
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

fn write_http_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write response");
}

#[test]
fn complete_request_frames_over_a_real_loopback_channel() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let (observed_tx, observed_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let received = read_http_request(&mut stream);
        write_http_response(&mut stream, generate_response_body());
        observed_tx.send(received).expect("send observation");
    });

    let path = journal("complete-request");
    let runtime = EndpointRuntime::default();
    let endpoint = endpoint(port);
    let stream: Box<dyn AttestedIo> =
        Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect"));
    let result = confidential_generate_over_channel(
        &request(None),
        &path,
        &endpoint,
        &Map::new(),
        &runtime,
        stream,
    );
    assert!(matches!(result, EndpointResult::Generated(_)));
    let received = observed_rx.recv().expect("server observation");
    let text = String::from_utf8(received).expect("utf8");
    assert!(text.starts_with("POST /v1/chat/completions HTTP/1.1"));
    assert!(text.contains(&format!("Host: 127.0.0.1:{port}")));
    assert!(text.contains("Content-Type: application/json"));
    assert!(text.contains("Authorization: Bearer token"));
    server.join().expect("join");
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn partial_response_stall_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_http_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
            .expect("partial write");
        stream.flush().expect("flush");
        let _ = release_rx.recv();
    });

    let path = journal("partial-stall");
    let runtime = EndpointRuntime::default();
    let endpoint = endpoint(port);
    let stream: Box<dyn AttestedIo> =
        Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect"));
    let result = confidential_generate_over_channel(
        &request(Some(0.2)),
        &path,
        &endpoint,
        &Map::new(),
        &runtime,
        stream,
    );
    match result {
        EndpointResult::Failed(failure) => {
            assert_eq!(
                failure.reason_code.as_deref(),
                Some("local_capacity_exhausted")
            );
        }
        other => panic!("expected capacity failure, got {other:?}"),
    }
    drop(release_tx);
    server.join().expect("join");
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn full_success_converse_parses_a_complete_loopback_response() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let response_body = converse_response_body();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_http_request(&mut stream);
        write_http_response(&mut stream, &response_body);
    });

    let path = journal("full-success");
    let runtime = EndpointRuntime::default();
    let endpoint = endpoint(port);
    let messages = converse_messages();
    let tools = converse_tools();
    let stream: Box<dyn AttestedIo> =
        Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect"));
    let result = confidential_converse_over_channel(
        &request(None),
        &messages,
        &tools,
        &path,
        &endpoint,
        &served_window_config(),
        &runtime,
        stream,
    );
    assert!(result.is_ok());
    server.join().expect("join");
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn accepted_but_silent_times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let _ = read_http_request(&mut stream);
        let _ = release_rx.recv();
    });

    let path = journal("accepted-silent");
    let runtime = EndpointRuntime::default();
    let endpoint = endpoint(port);
    let stream: Box<dyn AttestedIo> =
        Box::new(TcpStream::connect(("127.0.0.1", port)).expect("connect"));
    let result = confidential_generate_over_channel(
        &request(Some(0.2)),
        &path,
        &endpoint,
        &Map::new(),
        &runtime,
        stream,
    );
    match result {
        EndpointResult::Failed(failure) => {
            assert_eq!(
                failure.reason_code.as_deref(),
                Some("local_capacity_exhausted")
            );
        }
        other => panic!("expected capacity failure, got {other:?}"),
    }
    drop(release_tx);
    server.join().expect("join");
    let _ = std::fs::remove_dir_all(path);
}
