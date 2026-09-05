// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, GeneratedResponse, RefusalReason,
    SessionClient, SessionCompletion, encode_session_request_line,
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

    fn external_local(port: u16, served_model_id: &str) -> Self {
        let journal = Self::at_temp_path();
        journal.write_config(&format!(
            r#"{{"providers":{{"active":{{"provider":"local"}},"local":{{"endpoint_url":"http://127.0.0.1:{port}","served_model_id":"{served_model_id}","served_context_window":4096}}}}}}"#
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
    requests: AtomicUsize,
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
            requests: AtomicUsize::new(0),
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

    fn requests(&self) -> usize {
        self.state.requests.load(Ordering::Acquire)
    }

    fn inferences(&self) -> usize {
        self.state.inferences.load(Ordering::Acquire)
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
    stream.set_nonblocking(false).unwrap();
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
    state.requests.fetch_add(1, Ordering::Release);
    let body = if request_line.starts_with("GET /health ") {
        r#"{"loaded_model":"local"}"#.to_owned()
    } else if request_line.starts_with("GET /props ") {
        r#"{"n_ctx":16384,"total_slots":16}"#.to_owned()
    } else if request_line.starts_with("POST /tokenize ") {
        r#"{"tokens":[1]}"#.to_owned()
    } else if request_line.starts_with("POST /v1/chat/completions/input_tokens ") {
        r#"{"object":"response.input_tokens","input_tokens":1}"#.to_owned()
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
        .with_env("SOLSTONE_JOURNAL", journal.path.as_os_str().to_owned())
        .spawn(max_in_flight)
        .unwrap()
}

fn explicit_client(
    explicit_journal: &Journal,
    inherited_journal: &Journal,
    max_in_flight: usize,
) -> SessionClient {
    SessionClient::at_path(support::core_binary())
        .with_prefix_arguments(support::prefix())
        .with_session_journal(&explicit_journal.path)
        .with_env(
            "SOLSTONE_JOURNAL",
            inherited_journal.path.as_os_str().to_owned(),
        )
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

fn assert_unconsulted_journal(journal: &Journal, stub: &LocalStub, config: &[u8]) {
    assert_eq!(stub.requests(), 0, "the inherited endpoint was contacted");
    assert_eq!(stub.inferences(), 0, "the inherited endpoint inferred");
    assert!(
        !journal.path.join("tokens").exists(),
        "the inherited journal recorded token usage"
    );
    assert!(
        !journal.path.join("health").exists(),
        "the inherited journal materialized health state"
    );
    assert_eq!(
        fs::read(journal.path.join("config/journal.json")).unwrap(),
        config,
        "the inherited journal config changed"
    );
}

fn token_log_lines(journal: &Journal) -> Vec<String> {
    let entries = fs::read_dir(journal.path.join("tokens"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1, "exactly one daily token log is expected");
    let name = entries[0]
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap();
    assert_eq!(name.len(), 14);
    assert!(name[..8].bytes().all(|byte| byte.is_ascii_digit()));
    assert_eq!(&name[8..], ".jsonl");
    fs::read_to_string(&entries[0])
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
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
fn criterion_20_explicit_session_journal_is_used_for_the_lifetime_of_the_child() {
    let explicit_stub = LocalStub::normal();
    let inherited_stub = LocalStub::normal();
    let explicit = Journal::external_local(explicit_stub.port, "explicit-model");
    let inherited = Journal::external_local(inherited_stub.port, "inherited-model");
    let inherited_config = fs::read(inherited.path.join("config/journal.json")).unwrap();
    let client = explicit_client(&explicit, &inherited, 2);

    let mut before = request("explicit-before", "before".to_owned());
    before.max_output_tokens = 512;
    client.submit(before).unwrap();
    let response = generated(&client);
    assert_eq!(response.id.as_deref(), Some("explicit-before"));
    assert_eq!(response.model, "explicit-model");
    assert!(response.request_budget.is_some());

    explicit.set_no_engine();
    client
        .submit(request("explicit-after", "after".to_owned()))
        .unwrap();
    let GenerateResponse::Refused(refusal) = next(&client) else {
        panic!("the second request must reread the explicit journal config")
    };
    assert_eq!(refusal.reason, RefusalReason::NoEngineConfigured);
    client.close().unwrap();

    assert_eq!(explicit_stub.inferences(), 1);
    assert_eq!(token_log_lines(&explicit).len(), 1);
    assert_unconsulted_journal(&inherited, &inherited_stub, &inherited_config);
    explicit_stub.finish();
    inherited_stub.finish();
}

#[test]
fn criterion_21_explicit_missing_journal_does_not_fall_back_to_inherited_journal() {
    let inherited_stub = LocalStub::normal();
    let explicit = Journal::at_temp_path();
    let inherited = Journal::external_local(inherited_stub.port, "inherited-model");
    let inherited_config = fs::read(inherited.path.join("config/journal.json")).unwrap();
    let client = explicit_client(&explicit, &inherited, 1);

    client
        .submit(request("missing", "missing".to_owned()))
        .unwrap();
    let GenerateResponse::Refused(refusal) = next(&client) else {
        panic!("a missing explicit journal config must not fall back")
    };
    assert_eq!(refusal.reason, RefusalReason::NoEngineConfigured);
    client.close().unwrap();

    assert!(
        !explicit.path.join("config/journal.json").exists(),
        "reading a missing explicit config must not materialize it"
    );
    assert_unconsulted_journal(&inherited, &inherited_stub, &inherited_config);
    inherited_stub.finish();
}

#[test]
fn criterion_22_explicit_no_engine_does_not_fall_back_to_inherited_journal() {
    let inherited_stub = LocalStub::normal();
    let explicit = Journal::at_temp_path();
    explicit.set_no_engine();
    let inherited = Journal::external_local(inherited_stub.port, "inherited-model");
    let inherited_config = fs::read(inherited.path.join("config/journal.json")).unwrap();
    let client = explicit_client(&explicit, &inherited, 2);

    for id in ["first", "second"] {
        client.submit(request(id, id.to_owned())).unwrap();
        let GenerateResponse::Refused(refusal) = next(&client) else {
            panic!("the explicit no-engine config must refuse")
        };
        assert_eq!(refusal.reason, RefusalReason::NoEngineConfigured);
    }
    client.close().unwrap();

    assert_unconsulted_journal(&inherited, &inherited_stub, &inherited_config);
    inherited_stub.finish();
}

#[test]
fn criterion_23_unreadable_explicit_config_is_an_internal_failure() {
    let inherited_stub = LocalStub::normal();
    let explicit = Journal::at_temp_path();
    explicit.write_config("{");
    let inherited = Journal::external_local(inherited_stub.port, "inherited-model");
    let inherited_config = fs::read(inherited.path.join("config/journal.json")).unwrap();
    let session = &solstone_core_generate::contract()["framing"]["session"];
    let selector = session["selector"].as_str().unwrap();
    let concurrency_flag = session["concurrency"]["flag"].as_str().unwrap();
    let journal_flag = session["journal"]["flag"].as_str().unwrap();
    let request =
        encode_session_request_line(&request("unreadable", "unreadable".to_owned())).unwrap();
    // ⚠ The child must fail on the request, not on end-of-input. `wait_with_output`
    // drops the pipe as it starts waiting, so a plain write-then-wait races the
    // unreadable-config failure against a bare EOF and the child can win by exiting
    // 0 on clean shutdown. Hold stdin open until the child has exited so the only
    // thing it can be reacting to is the request itself. Both pipes are drained on
    // threads because the child may write either one before it exits.
    let mut child = support::generate_command()
        .arg(selector)
        .arg(concurrency_flag)
        .arg("1")
        .arg(journal_flag)
        .arg(&explicit.path)
        .env("SOLSTONE_JOURNAL", &inherited.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(request.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut stdout_pipe = child.stdout.take().unwrap();
    let mut stderr_pipe = child.stderr.take().unwrap();
    let stdout_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout_pipe.read_to_end(&mut buffer).unwrap();
        buffer
    });
    let stderr_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        stderr_pipe.read_to_end(&mut buffer).unwrap();
        buffer
    });

    let status = child.wait().unwrap();
    drop(stdin);
    let stdout = stdout_reader.join().unwrap();
    let stderr = stderr_reader.join().unwrap();

    assert_eq!(status.code(), Some(70));
    assert!(stdout.is_empty());
    let lines = std::str::from_utf8(&stderr)
        .unwrap()
        .lines()
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let error = serde_json::from_str::<serde_json::Value>(lines[0]).unwrap();
    assert_eq!(error["reason"], "internal-failure");
    assert_unconsulted_journal(&inherited, &inherited_stub, &inherited_config);
    inherited_stub.finish();
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
    let external_journal = Journal::external_local(external_stub.port, "stub");
    let external = client(&external_journal, 1);
    let mut requested = request("external", "external".to_owned());
    requested.max_output_tokens = 512;
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
