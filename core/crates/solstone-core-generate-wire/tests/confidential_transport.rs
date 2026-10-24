// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real loopback HTTP exchange for the confidential attested transport adapter.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use serde_json::Map;
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_generate_wire::{
    EndpointResult, EndpointRuntime, test_support::confidential_generate_over_channel,
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
        timeout_s,
        json_output: false,
        json_schema: None,
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

fn generate_response_body() -> &'static str {
    r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#
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

fn paced_success() -> solstone_core_generate_wire::ConfidentialResult {
    solstone_core_generate_wire::ConfidentialResult::Generated(
        solstone_core_generate_wire::EndpointGenerated {
            text: "ok".into(),
            model: "fixture".into(),
            usage: None,
            finish_reason: "stop".into(),
            input_budget: None,
            request_budget: None,
        },
    )
}

#[test]
fn confidential_background_child() {
    use solstone_core_generate_wire::{
        ConfidentialResult, test_support::confidential_generate_attested_with_readiness,
    };
    use solstone_core_spp_attest::CpuBundle;
    use solstone_core_spp_ratls::{
        CompositeVerdict, CompositeVerificationError, CompositeVerificationInput,
        CompositeVerifier, NvattestEnsureStatus,
    };
    struct NeverVerify;
    impl CompositeVerifier for NeverVerify {
        fn verify(
            &self,
            _: CpuBundle<'_>,
            _: CompositeVerificationInput<'_>,
        ) -> Result<CompositeVerdict, CompositeVerificationError> {
            panic!("another process owns the older-work slot");
        }
    }
    let Some(root) = std::env::var_os("CONFIDENTIAL_PACING_CHILD_ROOT") else {
        return;
    };
    let result = confidential_generate_attested_with_readiness(
        &request(Some(0.05)),
        std::path::Path::new(&root),
        &endpoint(1),
        &Map::new(),
        &EndpointRuntime::default(),
        &NeverVerify,
        NvattestEnsureStatus::Unavailable,
    );
    assert!(matches!(
        result,
        ConfidentialResult::Failed(failure)
            if failure.reason_code.as_deref() == Some("local_capacity_exhausted")
    ));
    println!("background-child-held");
}

#[test]
fn older_confidential_calls_share_a_process_safe_slot_while_foreground_bypasses_it() {
    use solstone_core_generate_wire::{
        ConfidentialResult, test_support::confidential_background_call,
    };
    use std::time::Duration;
    let root = journal("pacing-slot");
    let result = confidential_background_call(
        &root,
        "4:0".parse().unwrap(),
        Duration::from_secs(5),
        |_| {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "confidential_background_child", "--nocapture"])
                .env("CONFIDENTIAL_PACING_CHILD_ROOT", &root)
                .env("SOLSTONE_WORK_PRIORITY", "5:0")
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stderr)
            );
            assert!(String::from_utf8_lossy(&child.stdout).contains("background-child-held"));
            for band in 1..=3 {
                let foreground = confidential_background_call(
                    &root,
                    format!("{band}:0").parse().unwrap(),
                    Duration::from_millis(1),
                    |_| paced_success(),
                );
                assert!(matches!(foreground, ConfidentialResult::Generated(_)));
            }
            paced_success()
        },
    );
    assert!(matches!(result, ConfidentialResult::Generated(_)));
    // Rest is not the call's: once a call finishes, the next older call runs.
    for band in 4..=6 {
        let result = confidential_background_call(
            &root,
            format!("{band}:0").parse().unwrap(),
            Duration::from_secs(5),
            |_| paced_success(),
        );
        assert!(matches!(result, ConfidentialResult::Generated(_)));
    }
    assert!(solstone_core_local::admission::confidential_background_open(&root));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_older_confidential_failure_pauses_older_work_without_holding_its_caller() {
    use solstone_core_generate_wire::{
        ConfidentialResult, test_support::confidential_background_call,
    };
    use solstone_core_local::admission::confidential_background_open;
    use std::time::Duration;
    let root = journal("pacing-failure");
    let result = confidential_background_call(
        &root,
        "6:0".parse().unwrap(),
        Duration::from_secs(1),
        |_| ConfidentialResult::AttestationFailed("gateway_unreachable"),
    );
    assert!(matches!(
        result,
        ConfidentialResult::AttestationFailed("gateway_unreachable")
    ));
    assert!(!confidential_background_open(&root));
    // The task queue honours the pause; a call it already admitted is not held.
    let result = confidential_background_call(
        &root,
        "4:0".parse().unwrap(),
        Duration::from_secs(1),
        |_| paced_success(),
    );
    assert!(matches!(result, ConfidentialResult::Generated(_)));
    std::fs::remove_dir_all(root).unwrap();
}
