// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real loopback transport facts that cannot be scripted through EndpointTransport.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_generate_wire::{
    EndpointFailure, EndpointResult, EndpointRuntime, endpoint_generate,
};
use solstone_core_local::ByoEndpoint;

static NEXT_JOURNAL: AtomicUsize = AtomicUsize::new(0);

fn journal_path() -> std::path::PathBuf {
    let suffix = NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "solstone-endpoint-transport-{}-{suffix}",
        std::process::id()
    ))
}

fn request(timeout_s: Option<f64>) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "test.generate".into(),
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

fn endpoint(base_url: &str) -> ByoEndpoint {
    ByoEndpoint {
        base_url: base_url.to_owned(),
        served_model_id: "served".into(),
        credential: None,
        parallel_slots: Some(1),
        is_confidential: false,
        is_bundled: false,
    }
}

fn served_window_config() -> Map<String, Value> {
    json!({"providers": {"local": {"served_context_window": 4096}}})
        .as_object()
        .expect("config object")
        .clone()
}

fn completion_json() -> String {
    json!({
        "choices": [{"message": {"content": "Done"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5},
    })
    .to_string()
}

fn write_http_json(stream: &mut std::net::TcpStream, body: &str) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .expect("write response header");
    stream
        .write_all(body.as_bytes())
        .expect("write response body");
}

fn read_request(stream: &mut std::net::TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let Ok(read) = stream.read(&mut buffer) else {
            return;
        };
        if read == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        let content_length = header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            return;
        }
    }
}

#[test]
fn accepted_then_closed_is_endpoint_unreachable() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("unreachable listener");
    let address = listener.local_addr().expect("unreachable address");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept unreachable client");
        drop(stream);
    });
    let journal = journal_path();
    let result = endpoint_generate(
        &request(Some(0.2)),
        &journal,
        &endpoint(&format!("http://{address}")),
        &served_window_config(),
        &EndpointRuntime::default(),
    );
    assert_eq!(
        result,
        EndpointResult::Failed(EndpointFailure {
            reason_code: Some("local_endpoint_unreachable".into()),
            detail: None,
        })
    );
    server.join().expect("join unreachable server");
    let _ = std::fs::remove_dir_all(journal);
}

#[test]
fn accepted_but_silent_is_capacity_exhausted_then_released_response_succeeds() {
    let journal = journal_path();
    let config = served_window_config();

    let silent = TcpListener::bind("127.0.0.1:0").expect("silent listener");
    let silent_addr = silent.local_addr().expect("silent address");
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let server_gate = Arc::clone(&gate);
    let silent_thread = thread::spawn(move || {
        let (mut stream, _) = silent.accept().expect("accept silent client");
        read_request(&mut stream);
        let (lock, released) = &*server_gate;
        let mut ready = lock.lock().expect("silent gate lock");
        while !*ready {
            ready = released.wait(ready).expect("silent gate wait");
        }
    });
    struct ReleaseSilent {
        gate: Arc<(Mutex<bool>, Condvar)>,
    }
    impl Drop for ReleaseSilent {
        fn drop(&mut self) {
            let (lock, released) = &*self.gate;
            *lock.lock().expect("silent gate lock") = true;
            released.notify_all();
        }
    }
    let _release = ReleaseSilent {
        gate: Arc::clone(&gate),
    };
    let timed_out = endpoint_generate(
        &request(Some(0.2)),
        &journal,
        &endpoint(&format!("http://{silent_addr}")),
        &config,
        &EndpointRuntime::default(),
    );
    assert_eq!(
        timed_out,
        EndpointResult::Failed(EndpointFailure {
            reason_code: Some("local_capacity_exhausted".into()),
            detail: None,
        })
    );
    drop(_release);
    silent_thread.join().expect("join silent server");

    let live = TcpListener::bind("127.0.0.1:0").expect("success listener");
    let live_addr = live.local_addr().expect("success address");
    let live_thread = thread::spawn(move || {
        let (mut stream, _) = live.accept().expect("accept success client");
        read_request(&mut stream);
        write_http_json(&mut stream, &completion_json());
    });
    let generated = endpoint_generate(
        &request(Some(2.0)),
        &journal,
        &endpoint(&format!("http://{live_addr}")),
        &config,
        &EndpointRuntime::default(),
    );
    assert!(
        matches!(generated, EndpointResult::Generated(_)),
        "released response must generate, got {generated:?}"
    );
    live_thread.join().expect("join success server");
    let _ = std::fs::remove_dir_all(journal);
}
