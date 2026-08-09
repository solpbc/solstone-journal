// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, RefusalReason,
    SessionClient, SessionCompletion,
};

const RECEIVE_BOUND: Duration = Duration::from_secs(10);
const LARGE_TEXT_BYTES: usize = 1024 * 1024;

struct Journal {
    path: PathBuf,
}

impl Journal {
    fn at_temp_path() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-generate-session-real-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(path.join("config")).unwrap();
        Self { path }
    }

    fn bundled_local(port: u16) -> Self {
        let journal = Self::at_temp_path();
        journal.write_config(r#"{"providers":{"active":{"provider":"local"}}}"#);
        fs::create_dir_all(journal.path.join("health")).unwrap();
        fs::write(journal.path.join("health/local.port"), port.to_string()).unwrap();
        journal
    }

    fn external_local(port: u16) -> Self {
        let journal = Self::at_temp_path();
        journal.write_config(&format!(
            r#"{{"providers":{{"active":{{"provider":"local"}},"local":{{"endpoint_url":"http://127.0.0.1:{port}","served_model_id":"stub"}}}}}}"#
        ));
        journal
    }

    fn write_config(&self, value: &str) {
        fs::write(self.path.join("config/journal.json"), value).unwrap();
    }

    fn set_no_engine(&self) {
        self.write_config(r#"{"providers":{"active":{"provider":"none"}}}"#);
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct StubState {
    completion_text: String,
    hold_completion: bool,
    inferences: AtomicUsize,
    stopping: AtomicBool,
    released: Mutex<bool>,
    release: Condvar,
}

struct LocalStub {
    port: u16,
    state: Arc<StubState>,
    worker: Option<thread::JoinHandle<()>>,
}

impl LocalStub {
    fn normal() -> Self {
        Self::start("OK".to_owned(), false)
    }

    fn large() -> Self {
        Self::start("x".repeat(LARGE_TEXT_BYTES), false)
    }

    fn start(completion_text: String, hold_completion: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(StubState {
            completion_text,
            hold_completion,
            inferences: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            released: Mutex::new(false),
            release: Condvar::new(),
        });
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            while !worker_state.stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state = Arc::clone(&worker_state);
                        thread::spawn(move || handle_local_request(stream, state));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            port,
            state,
            worker: Some(worker),
        }
    }

    fn finish(mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }

    fn stop(&self) {
        self.state.stopping.store(true, Ordering::Release);
        *self.state.released.lock().unwrap() = true;
        self.state.release.notify_all();
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

impl Drop for LocalStub {
    fn drop(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle_local_request(mut stream: TcpStream, state: Arc<StubState>) {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let Ok(read) = stream.read(&mut chunk) else {
            return;
        };
        if read == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        // Header names are case-insensitive and the provider client lowercases
        // them. Matching `Content-Length` exactly read the length as zero, so this
        // stub answered before it had read a megabyte-scale body and then reset the
        // connection on close — which reaches the caller as a refused request, not
        // as a stub fault.
        let content_length = header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }

    let request_line = String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let body = if request_line.starts_with("GET /health ") {
        r#"{"loaded_model":"local"}"#.to_owned()
    } else if request_line.starts_with("GET /props ") {
        r#"{"n_ctx":16384,"total_slots":16}"#.to_owned()
    } else if request_line.starts_with("POST /tokenize ") {
        r#"{"tokens":[1]}"#.to_owned()
    } else if request_line.starts_with("POST /v1/chat/completions ") {
        state.inferences.fetch_add(1, Ordering::Release);
        if state.hold_completion {
            let mut released = state.released.lock().unwrap();
            while !*released && !state.stopping.load(Ordering::Acquire) {
                released = state.release.wait(released).unwrap();
            }
            if state.stopping.load(Ordering::Acquire) {
                return;
            }
        }
        format!(
            r#"{{"choices":[{{"message":{{"content":{}}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}}}"#,
            serde_json::to_string(&state.completion_text).unwrap(),
        )
    } else {
        "{}".to_owned()
    };

    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
}

fn request(id: &str, text: String) -> GenerateRequest {
    GenerateRequest {
        id: Some(id.to_owned()),
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text { text }],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 1_000_000,
        thinking_budget: None,
        timeout_s: Some(30.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn client(journal: &Journal, max_in_flight: usize) -> SessionClient {
    SessionClient::at_path(support::core_binary())
        .with_prefix_arguments(support::prefix())
        .with_env("SOLSTONE_JOURNAL", journal.path.to_string_lossy())
        .spawn(max_in_flight)
        .unwrap()
}

fn next(client: &SessionClient) -> GenerateResponse {
    let SessionCompletion::Response(response) = client.recv_timeout(RECEIVE_BOUND).unwrap() else {
        panic!("expected response completion")
    };
    response
}

fn generated(client: &SessionClient) -> Box<GeneratedResponse> {
    let GenerateResponse::Generated(response) = next(client) else {
        panic!("expected generated response")
    };
    response
}

#[test]
fn criterion_16_real_child_transfers_large_concurrent_inline_payloads() {
    let stub = LocalStub::large();
    let journal = Journal::bundled_local(stub.port);
    let client = client(&journal, 3);
    let request_text = "q".repeat(LARGE_TEXT_BYTES);
    for index in 0..10 {
        client
            .submit(request(&format!("large-{index}"), request_text.clone()))
            .unwrap();
    }
    client.close().unwrap();

    for _ in 0..10 {
        let response = generated(&client);
        assert_eq!(response.text.len(), LARGE_TEXT_BYTES);
        assert!(response.text.bytes().all(|byte| byte == b'x'));
    }
    stub.finish();
}

#[test]
fn criterion_18_real_child_rechecks_config_for_each_session_request() {
    let stub = LocalStub::normal();
    let journal = Journal::bundled_local(stub.port);
    let client = client(&journal, 2);
    client
        .submit(request("before", "before".to_owned()))
        .unwrap();
    assert_eq!(generated(&client).id.as_deref(), Some("before"));

    journal.set_no_engine();
    client.submit(request("after", "after".to_owned())).unwrap();
    let GenerateResponse::Refused(refusal) = next(&client) else {
        panic!("config change must refuse the later request")
    };
    assert_eq!(refusal.reason, RefusalReason::NoEngineConfigured);
    client.close().unwrap();
    stub.finish();
}

#[test]
fn criterion_19_real_child_reports_applied_hints() {
    let bundled_stub = LocalStub::normal();
    let bundled_journal = Journal::bundled_local(bundled_stub.port);
    let bundled = client(&bundled_journal, 2);
    let mut requested = request("bundled", "bundled".to_owned());
    requested.attempt_index = 2;
    requested.exclusive_admission = true;
    bundled.submit(requested).unwrap();
    let response = generated(&bundled);
    assert!(response.hints_applied.contains(&"attempt_index".to_owned()));
    assert!(
        response
            .hints_applied
            .contains(&"exclusive_admission".to_owned())
    );

    let mut transport = request("transport", "transport".to_owned());
    transport.transport_retries = Some(2);
    bundled.submit(transport).unwrap();
    let response = generated(&bundled);
    assert!(
        !response
            .hints_applied
            .contains(&"transport_retries".to_owned())
    );
    bundled.close().unwrap();
    bundled_stub.finish();

    let external_stub = LocalStub::normal();
    let external_journal = Journal::external_local(external_stub.port);
    let external = client(&external_journal, 1);
    let mut requested = request("external", "external".to_owned());
    requested.attempt_index = 2;
    requested.exclusive_admission = true;
    external.submit(requested).unwrap();
    let response = generated(&external);
    // The endpoint lane does not read the attempt index.
    assert!(!response.hints_applied.contains(&"attempt_index".to_owned()));
    // 🔴 It DOES read exclusive_admission, to size its admission slot, so it
    // reports it. ⚠ This assertion was inverted while the test drove the Python
    // wire: the reference decided what to report from whether the result carried
    // an inference block, which the endpoint lane never does — so it honoured the
    // hint and stayed silent about it. A hint is reported by the lane that
    // honoured it, ⛔ not by the shape of the result.
    assert!(
        response
            .hints_applied
            .contains(&"exclusive_admission".to_owned())
    );
    external.close().unwrap();
    external_stub.finish();
}
