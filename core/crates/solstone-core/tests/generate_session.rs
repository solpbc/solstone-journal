// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, SessionClient, SessionCompletion,
    decode_session_response_line, encode_session_request_line, encode_session_terminal_line,
};

const BOUND: Duration = Duration::from_secs(3);
const LARGE_TEXT_BYTES: usize = 1024 * 1024;

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "solstone-core-generate-session-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos(),
    ))
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct Journal {
    path: PathBuf,
}

impl Journal {
    fn bundled_local(confidential: bool) -> Self {
        let path = temp_path("journal");
        fs::create_dir_all(path.join("config")).expect("create config directory");
        let config = if confidential {
            json!({"providers":{"active":{"provider":"local"}},"services":{"confidential":{}}})
        } else {
            json!({"providers":{"active":{"provider":"local"}}})
        };
        fs::write(path.join("config/journal.json"), config.to_string()).expect("write config");
        Self { path }
    }

    fn set_port(&self, port: u16) {
        fs::create_dir_all(self.path.join("health")).expect("create health directory");
        fs::write(self.path.join("health/local.port"), port.to_string()).expect("write local port");
    }

    fn set_no_engine(&self) {
        fs::write(
            self.path.join("config/journal.json"),
            json!({"providers":{"active":{"provider":"none"}}}).to_string(),
        )
        .expect("disable generate engine");
    }

    fn set_corrupt_config(&self) {
        fs::write(self.path.join("config/journal.json"), b"{").expect("corrupt generate config");
    }

    fn token_lines(&self) -> usize {
        self.token_log_lines().len()
    }

    fn token_log_lines(&self) -> Vec<String> {
        fs::read_dir(self.path.join("tokens"))
            .map(|entries| {
                entries
                    .map(|entry| {
                        fs::read_to_string(entry.expect("token entry").path()).expect("read token")
                    })
                    .flat_map(|text| text.lines().map(str::to_owned).collect::<Vec<_>>())
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct StubState {
    release_path: PathBuf,
    observed_path: PathBuf,
    hold: bool,
    stopping: AtomicBool,
    active: AtomicUsize,
    maximum: AtomicUsize,
    gates: Mutex<StubGates>,
    release: Condvar,
}

#[derive(Default)]
struct StubGates {
    released_all: bool,
    released_ids: BTreeSet<String>,
    observed_ids: BTreeSet<String>,
}

struct LocalStub {
    port: u16,
    state: Arc<StubState>,
    worker: Option<thread::JoinHandle<()>>,
}

impl LocalStub {
    fn start(hold: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local stub");
        listener.set_nonblocking(true).expect("set nonblocking");
        let port = listener.local_addr().expect("stub address").port();
        let state = Arc::new(StubState {
            release_path: temp_path("release"),
            observed_path: temp_path("observed"),
            hold,
            stopping: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            gates: Mutex::new(StubGates::default()),
            release: Condvar::new(),
        });
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            while !worker_state.stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("set local stub stream blocking");
                        let state = Arc::clone(&worker_state);
                        thread::spawn(move || handle_local_request(stream, state));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept local stub request: {error}"),
                }
            }
        });
        Self {
            port,
            state,
            worker: Some(worker),
        }
    }

    fn wait_for_observed(&self) {
        wait_for_path(&self.state.observed_path);
    }

    fn wait_for_observed_ids(&self, expected: &[&str]) {
        let expected = expected
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<BTreeSet<_>>();
        let deadline = Instant::now() + BOUND;
        let mut gates = self.state.gates.lock().expect("stub gates");
        while !expected.is_subset(&gates.observed_ids) {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for named local requests");
            let waited = self
                .state
                .release
                .wait_timeout(gates, remaining)
                .expect("wait for named local requests");
            gates = waited.0;
            assert!(
                !waited.1.timed_out() || expected.is_subset(&gates.observed_ids),
                "timed out waiting for local requests {expected:?}; observed {:?}",
                gates.observed_ids
            );
        }
    }

    fn release(&self) {
        fs::write(&self.state.release_path, b"release").expect("write release trigger");
        self.state.gates.lock().expect("stub gates").released_all = true;
        self.state.release.notify_all();
    }

    fn release_request(&self, id: &str) {
        self.state
            .gates
            .lock()
            .expect("stub gates")
            .released_ids
            .insert(id.to_owned());
        self.state.release.notify_all();
    }

    fn maximum_depth(&self) -> usize {
        self.state.maximum.load(Ordering::Acquire)
    }

    fn finish(mut self) {
        self.stop();
        self.worker
            .take()
            .expect("stub worker")
            .join()
            .expect("join stub");
    }

    fn stop(&self) {
        self.state.stopping.store(true, Ordering::Release);
        self.state.gates.lock().expect("stub gates").released_all = true;
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
        for path in [&self.state.release_path, &self.state.observed_path] {
            let _ = fs::remove_file(path);
        }
    }
}

fn handle_local_request(mut stream: TcpStream, state: Arc<StubState>) {
    let request = read_http_request(&mut stream);
    let request_line = request.lines().next().expect("HTTP request line");
    let body = if request_line.starts_with("GET /health ") {
        r#"{"loaded_model":"local"}"#.to_owned()
    } else if request_line.starts_with("GET /props ") {
        r#"{"n_ctx":16384,"total_slots":16}"#.to_owned()
    } else if request_line.starts_with("POST /tokenize ") {
        r#"{"tokens":[1]}"#.to_owned()
    } else if request_line.starts_with("POST /v1/chat/completions/input_tokens ") {
        r#"{"object":"response.input_tokens","input_tokens":1}"#.to_owned()
    } else if request_line.starts_with("POST /v1/chat/completions ") {
        let content = chat_request_content(&request);
        let active = state.active.fetch_add(1, Ordering::AcqRel) + 1;
        state.maximum.fetch_max(active, Ordering::AcqRel);
        fs::write(&state.observed_path, b"observed").expect("write observed trigger");
        let mut gates = state.gates.lock().expect("stub gates");
        gates.observed_ids.insert(content.clone());
        state.release.notify_all();
        if state.hold {
            while !gates.released_all
                && !gates.released_ids.contains(&content)
                && !state.stopping.load(Ordering::Acquire)
            {
                if state.release_path.exists() {
                    gates.released_all = true;
                    break;
                }
                let waited = state
                    .release
                    .wait_timeout(gates, Duration::from_millis(10))
                    .expect("wait for release");
                gates = waited.0;
            }
            if state.stopping.load(Ordering::Acquire) {
                state.active.fetch_sub(1, Ordering::AcqRel);
                return;
            }
        }
        drop(gates);
        state.active.fetch_sub(1, Ordering::AcqRel);
        chat_response_body(&content)
    } else {
        panic!("unexpected local stub request: {request_line}");
    };
    write_http_response(&mut stream, &body);
}

fn chat_request_content(request: &str) -> String {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("chat request has HTTP body");
    let body: Value = serde_json::from_str(body).expect("chat request body is JSON");
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .map(str::to_owned)
        .expect("chat request has string user content")
}

fn stub_text(content: &str) -> String {
    if content.len() > 1024 {
        "stub-large-content".to_owned()
    } else {
        format!("stub-{content}")
    }
}

fn stub_usage(content: &str) -> Value {
    let prompt_tokens = content.bytes().map(u64::from).sum::<u64>() % 10_000 + 1;
    json!({
        "prompt_tokens": prompt_tokens,
        "completion_tokens": prompt_tokens + 1,
        "total_tokens": prompt_tokens * 2 + 1,
    })
}

fn generated_usage(content: &str) -> Value {
    let usage = stub_usage(content);
    json!({
        "input_tokens": usage["prompt_tokens"],
        "output_tokens": usage["completion_tokens"],
        "total_tokens": usage["total_tokens"],
    })
}

fn chat_response_body(content: &str) -> String {
    if content == "json-truncated" {
        return json!({
            "model": "stub-json-truncated",
            "choices": [{"message": {"content": "I cannot complete that request."}, "finish_reason": "length"}],
            "usage": stub_usage(content),
        })
        .to_string();
    }
    json!({
        "model": format!("stub-{content}"),
        "choices": [{"message": {"content": stub_text(content)}, "finish_reason": "stop"}],
        "usage": stub_usage(content),
    })
    .to_string()
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut chunk).expect("read HTTP request");
        assert_ne!(read, 0, "unexpected EOF before HTTP headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).expect("HTTP headers are UTF-8");
    let request_line = headers.lines().next().expect("HTTP request line");
    // HTTP field names are case-insensitive (RFC 9110). This matched only the
    // lower-cased spelling the current client happens to send.
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim())
        })
        .map(|value| value.parse::<usize>().expect("Content-Length integer"));
    if request_line.starts_with("POST ") {
        assert!(
            content_length.is_some(),
            "POST request is missing Content-Length"
        );
    }
    let content_length = content_length.unwrap_or(0);
    while bytes.len() < header_end + 4 + content_length {
        let read = stream.read(&mut chunk).expect("read HTTP body");
        assert_ne!(read, 0, "unexpected EOF in HTTP body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(bytes).expect("HTTP request is UTF-8")
}

fn write_http_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write HTTP response");
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + BOUND;
    while fs::metadata(path).map_or(true, |metadata| metadata.len() == 0) {
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn request(id: &str) -> GenerateRequest {
    request_with_text(id, id.to_owned())
}

fn request_with_text(id: &str, text: String) -> GenerateRequest {
    GenerateRequest {
        id: Some(id.to_owned()),
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text { text }],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 16,
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

fn request_with_attempt(id: &str, attempt_index: u64) -> GenerateRequest {
    let mut request = request(id);
    request.attempt_index = attempt_index;
    request
}

struct RawSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: ChildStderr,
}

fn raw_session(journal: &Journal, max_in_flight: usize) -> RawSession {
    let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args([
            "generate",
            "--session",
            "--max-in-flight",
            &max_in_flight.to_string(),
        ])
        .env("SOLSTONE_JOURNAL", &journal.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn session core");
    RawSession {
        stdin: child.stdin.take().expect("session stdin"),
        stdout: BufReader::new(child.stdout.take().expect("session stdout")),
        stderr: child.stderr.take().expect("session stderr"),
        child,
    }
}

fn write_request(stdin: &mut ChildStdin, request: &GenerateRequest) {
    stdin
        .write_all(
            encode_session_request_line(request)
                .expect("encode request")
                .as_bytes(),
        )
        .expect("write session request");
    stdin.flush().expect("flush session request");
}

fn write_terminal(stdin: &mut ChildStdin) {
    stdin
        .write_all(
            encode_session_terminal_line(solstone_core_generate::SessionTerminal)
                .expect("encode terminal")
                .as_bytes(),
        )
        .expect("write terminal");
    stdin.flush().expect("flush terminal");
}

fn read_response(stdout: &mut BufReader<ChildStdout>) -> GenerateResponse {
    let mut line = String::new();
    let read = stdout.read_line(&mut line).expect("read session response");
    assert_ne!(read, 0, "unexpected EOF before session response");
    assert!(line.ends_with('\n'), "partial session response: {line:?}");
    decode_session_response_line(&line).expect("decode session response")
}

fn generated(response: GenerateResponse) -> solstone_core_generate::GeneratedResponse {
    let GenerateResponse::Generated(response) = response else {
        panic!("expected generated response");
    };
    *response
}

fn assert_one_protocol_error(stderr: &mut ChildStderr, reason: &str) {
    let mut text = String::new();
    stderr
        .read_to_string(&mut text)
        .expect("read session protocol stderr");
    assert!(
        text.ends_with('\n'),
        "protocol error is not a record: {text:?}"
    );
    assert_eq!(
        text.lines().count(),
        1,
        "expected exactly one protocol error record: {text:?}"
    );
    let error: Value = serde_json::from_str(&text).expect("protocol error JSON");
    assert_eq!(
        error["schema"],
        solstone_core_generate::contract()["schema_identifiers"]["error"]
    );
    assert_eq!(error["reason"], reason);
}

fn assert_empty_stdout(stdout: &mut BufReader<ChildStdout>) {
    let mut bytes = Vec::new();
    stdout
        .read_to_end(&mut bytes)
        .expect("read session stdout after exit");
    assert!(bytes.is_empty(), "unexpected stdout: {bytes:?}");
}

fn wait_for_exit(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + BOUND;
    loop {
        if let Some(status) = child.try_wait().expect("poll session child") {
            return status;
        }
        assert!(Instant::now() < deadline, "session child did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_process_exit(pid: u32) {
    let deadline = Instant::now() + BOUND;
    while Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        assert!(
            Instant::now() < deadline,
            "core generate session child {pid} remained alive after its caller died"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_token_count_stable(journal: &Journal, expected: usize) {
    let deadline = Instant::now() + BOUND;
    while Instant::now() < deadline {
        assert_eq!(
            journal.token_lines(),
            expected,
            "a cancelled session wrote a usage record"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn direct_output(args: &[&str], input: &[u8]) -> std::process::Output {
    let journal = Journal::bundled_local(false);
    let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(args)
        .env("SOLSTONE_JOURNAL", &journal.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn malformed session child");
    child
        .stdin
        .as_mut()
        .expect("malformed session stdin")
        .write_all(input)
        .expect("write malformed session input");
    child
        .wait_with_output()
        .expect("wait malformed session child")
}

#[test]
fn session_client_prefix_constructor_runs_core_subcommand() {
    let stub = LocalStub::start(false);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let client = SessionClient::at_path(env!("CARGO_BIN_EXE_solstone-core"))
        .with_prefix_arguments(["generate".into()])
        .with_env("SOLSTONE_JOURNAL", journal.path.as_os_str())
        .spawn(1)
        .expect("start core session client");
    client
        .submit(request("prefix"))
        .expect("submit prefix request");
    client.close().expect("close prefix session");
    let SessionCompletion::Response(GenerateResponse::Generated(response)) =
        client.recv_timeout(BOUND).expect("receive prefix response")
    else {
        panic!("expected generated prefix response");
    };
    assert_eq!(response.id.as_deref(), Some("prefix"));
    stub.finish();
}

#[test]
fn criterion_8_killing_session_owner_aborts_wire_without_usage() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut helper = Command::new(env!(
        "CARGO_BIN_EXE_solstone-core-generate-session-kill-helper"
    ))
    .env("SOLSTONE_CORE", env!("CARGO_BIN_EXE_solstone-core"))
    .env("SOLSTONE_JOURNAL", &journal.path)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .expect("spawn session-owner helper");
    let mut pid_line = String::new();
    BufReader::new(helper.stdout.take().expect("helper stdout"))
        .read_line(&mut pid_line)
        .expect("read core session PID");
    let wire_pid = pid_line
        .trim()
        .parse::<u32>()
        .expect("core session PID is an integer");
    stub.wait_for_observed();
    let token_lines = journal.token_lines();

    helper.kill().expect("kill session owner helper");
    helper.wait().expect("wait for session owner helper");
    wait_for_process_exit(wire_pid);
    assert_token_count_stable(&journal, token_lines);
    stub.finish();
}

#[test]
fn session_terminal_drains_simultaneous_completions_into_whole_usage_lines() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(true);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 4);
    for index in 0..4 {
        write_request(&mut session.stdin, &request(&format!("request-{index}")));
    }
    stub.wait_for_observed_ids(&["request-0", "request-1", "request-2", "request-3"]);
    assert_eq!(stub.maximum_depth(), 4);
    stub.release();
    write_terminal(&mut session.stdin);
    drop(session.stdin);
    for _ in 0..4 {
        assert!(matches!(
            read_response(&mut session.stdout),
            GenerateResponse::Generated(_)
        ));
    }
    assert!(wait_for_exit(&mut session.child).success());
    let mut stderr = String::new();
    session
        .stderr
        .read_to_string(&mut stderr)
        .expect("read session stderr");
    assert!(stderr.is_empty(), "session stderr: {stderr}");
    let token_lines = journal.token_log_lines();
    assert_eq!(token_lines.len(), 4);
    for line in token_lines {
        serde_json::from_str::<Value>(&line).expect("whole token log JSON line");
    }
    stub.finish();
}

#[test]
fn session_accepts_two_held_requests_and_preserves_response_ids() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 2);
    write_request(&mut session.stdin, &request("alpha"));
    write_request(&mut session.stdin, &request("beta"));
    stub.wait_for_observed_ids(&["alpha", "beta"]);
    assert_eq!(stub.maximum_depth(), 2);

    stub.release();
    write_terminal(&mut session.stdin);
    drop(session.stdin);
    let responses = [
        generated(read_response(&mut session.stdout)),
        generated(read_response(&mut session.stdout)),
    ];
    let received = responses
        .iter()
        .map(|response| response.id.clone().expect("response id"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        received,
        BTreeSet::from(["alpha".to_owned(), "beta".to_owned()])
    );
    for response in responses {
        let id = response.id.expect("response id");
        assert_eq!(response.text, stub_text(&id));
    }
    assert!(wait_for_exit(&mut session.child).success());
    stub.finish();
}

#[test]
fn session_delivers_out_of_order_completions_by_request_id() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 2);
    write_request(&mut session.stdin, &request("first"));
    write_request(&mut session.stdin, &request("second"));
    stub.wait_for_observed_ids(&["first", "second"]);

    stub.release_request("second");
    let second = generated(read_response(&mut session.stdout));
    assert_eq!(second.id.as_deref(), Some("second"));
    assert_eq!(second.text, stub_text("second"));

    stub.release_request("first");
    let first = generated(read_response(&mut session.stdout));
    assert_eq!(first.id.as_deref(), Some("first"));
    assert_eq!(first.text, stub_text("first"));
    write_terminal(&mut session.stdin);
    drop(session.stdin);
    assert!(wait_for_exit(&mut session.child).success());
    stub.finish();
}

#[test]
fn session_max_in_flight_bounds_held_local_requests() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 1);
    write_request(&mut session.stdin, &request("first"));
    write_request(&mut session.stdin, &request("second"));
    stub.wait_for_observed();
    assert_eq!(stub.maximum_depth(), 1);
    stub.release();
    write_terminal(&mut session.stdin);
    drop(session.stdin);
    let _ = read_response(&mut session.stdout);
    let _ = read_response(&mut session.stdout);
    assert!(wait_for_exit(&mut session.child).success());
    stub.finish();
}

#[test]
fn session_rechecks_config_for_each_request() {
    let stub = LocalStub::start(false);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 1);
    write_request(&mut session.stdin, &request("before"));
    assert!(matches!(
        read_response(&mut session.stdout),
        GenerateResponse::Generated(_)
    ));
    journal.set_no_engine();
    write_request(&mut session.stdin, &request("after"));
    write_terminal(&mut session.stdin);
    drop(session.stdin);
    let GenerateResponse::Refused(refusal) = read_response(&mut session.stdout) else {
        panic!("expected later request to be refused after config change");
    };
    assert_eq!(
        refusal.reason,
        solstone_core_generate::RefusalReason::NoEngineConfigured
    );
    assert!(wait_for_exit(&mut session.child).success());
    stub.finish();
}

#[test]
fn malformed_line_aborts_two_held_requests_without_output_or_usage() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(true);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 2);
    write_request(&mut session.stdin, &request("first"));
    write_request(&mut session.stdin, &request("second"));
    stub.wait_for_observed_ids(&["first", "second"]);
    assert_eq!(stub.maximum_depth(), 2);
    session
        .stdin
        .write_all(b"not-json\n")
        .expect("write malformed session record");
    session
        .stdin
        .flush()
        .expect("flush malformed session record");

    assert_eq!(wait_for_exit(&mut session.child).code(), Some(64));
    assert_empty_stdout(&mut session.stdout);
    assert_one_protocol_error(&mut session.stderr, "malformed-request");
    assert_eq!(journal.token_lines(), 0);
    stub.finish();
}

#[test]
fn one_internal_failure_aborts_three_request_session_without_partial_output() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(true);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 3);
    write_request(&mut session.stdin, &request("held-first"));
    write_request(&mut session.stdin, &request("held-second"));
    stub.wait_for_observed_ids(&["held-first", "held-second"]);
    journal.set_corrupt_config();
    write_request(&mut session.stdin, &request("broken-config"));

    assert_eq!(wait_for_exit(&mut session.child).code(), Some(70));
    assert_empty_stdout(&mut session.stdout);
    assert_one_protocol_error(&mut session.stderr, "internal-failure");
    assert_eq!(journal.token_lines(), 0);
    stub.finish();
}

#[test]
fn concurrent_requests_keep_their_own_usage_and_hints() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 2);
    write_request(&mut session.stdin, &request_with_attempt("plain", 0));
    write_request(&mut session.stdin, &request_with_attempt("retried", 1));
    stub.wait_for_observed_ids(&["plain", "retried"]);
    stub.release();
    write_terminal(&mut session.stdin);
    drop(session.stdin);

    let mut responses = BTreeMap::new();
    for _ in 0..2 {
        let response = generated(read_response(&mut session.stdout));
        responses.insert(response.id.clone().expect("response id"), response);
    }
    let plain = responses.remove("plain").expect("plain response");
    assert_eq!(plain.text, stub_text("plain"));
    assert_eq!(plain.usage, generated_usage("plain"));
    assert!(plain.hints_applied.is_empty());
    let retried = responses.remove("retried").expect("retried response");
    assert_eq!(retried.text, stub_text("retried"));
    assert_eq!(retried.usage, generated_usage("retried"));
    assert_eq!(retried.hints_applied, ["attempt_index"]);
    assert!(responses.is_empty());
    assert!(wait_for_exit(&mut session.child).success());
    stub.finish();
}

#[test]
fn session_uses_the_same_validation_and_logging_order_as_one_shot() {
    let stub = LocalStub::start(false);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 1);
    let mut truncated = request("json-truncated");
    truncated.json_output = true;
    write_request(&mut session.stdin, &truncated);
    write_terminal(&mut session.stdin);
    drop(session.stdin);

    let GenerateResponse::Refused(refusal) = read_response(&mut session.stdout) else {
        panic!("JSON truncation must refuse in session framing");
    };
    assert_eq!(
        refusal.reason,
        solstone_core_generate::RefusalReason::IncompleteJson
    );
    assert_eq!(
        refusal.reason_code.as_ref().map(|reason| reason.as_wire()),
        Some("incomplete_json_length")
    );
    let token: Value = serde_json::from_str(&journal.token_log_lines()[0]).expect("token JSON");
    assert_eq!(token["non_responsive_matched_signal"], "i cannot");
    assert!(wait_for_exit(&mut session.child).success());
    stub.finish();
}

#[test]
fn session_round_trips_ten_large_inline_text_payloads_with_a_bound() {
    let stub = LocalStub::start(false);
    let journal = Journal::bundled_local(false);
    journal.set_port(stub.port);
    let mut session = raw_session(&journal, 3);
    let text = "q".repeat(LARGE_TEXT_BYTES);
    for index in 0..10 {
        write_request(
            &mut session.stdin,
            &request_with_text(&format!("large-{index}"), text.clone()),
        );
    }
    write_terminal(&mut session.stdin);
    drop(session.stdin);
    let mut received = BTreeSet::new();
    for _ in 0..10 {
        let response = generated(read_response(&mut session.stdout));
        assert_eq!(response.text, stub_text(&text));
        received.insert(response.id.expect("response id"));
    }
    let expected = (0..10)
        .map(|index| format!("large-{index}"))
        .collect::<BTreeSet<_>>();
    assert_eq!(received, expected);
    assert!(wait_for_exit(&mut session.child).success());
    stub.finish();
}

#[test]
fn malformed_session_records_use_protocol_errors() {
    for input in [
        b"not-json\n".as_slice(),
        b"[]\n".as_slice(),
        b"{\"schema\":\"solstone-generate-session-terminal-v2\",\"extra\":true}\n".as_slice(),
    ] {
        let output = direct_output(&["generate", "--session", "--max-in-flight", "1"], input);
        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
        let error: Value = serde_json::from_slice(&output.stderr).expect("protocol error stderr");
        assert_eq!(error["reason"], "malformed-request");
    }
}

#[test]
fn session_line_limit_is_a_protocol_error() {
    let limit = solstone_core_generate::contract()["framing"]["session"]["line_limit_bytes"]
        .as_u64()
        .expect("fixture line limit") as usize;
    let mut input = vec![b'x'; limit + 1];
    input.push(b'\n');
    let output = direct_output(&["generate", "--session", "--max-in-flight", "1"], &input);
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    let error: Value = serde_json::from_slice(&output.stderr).expect("protocol error stderr");
    assert_eq!(error["reason"], "malformed-request");
}

#[test]
fn bare_eof_exits_immediately_without_response_or_usage() {
    let stub = LocalStub::start(true);
    let journal = Journal::bundled_local(true);
    journal.set_port(stub.port);
    let stdout_path = TempFile(temp_path("stdout"));
    let stdout_file = File::create(&stdout_path.0).expect("create stdout file");
    let mut child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["generate", "--session", "--max-in-flight", "1"])
        .env("SOLSTONE_JOURNAL", &journal.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn abort session");
    let mut stdin = child.stdin.take().expect("abort session stdin");
    write_request(&mut stdin, &request("held"));
    stub.wait_for_observed();
    let token_lines = journal.token_lines();
    drop(stdin);
    assert_eq!(wait_for_exit(&mut child).code(), Some(0));
    let first_length = fs::metadata(&stdout_path.0).expect("stdout metadata").len();
    stub.release();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        fs::metadata(&stdout_path.0)
            .expect("settled stdout metadata")
            .len(),
        first_length
    );
    assert_eq!(journal.token_lines(), token_lines);
    stub.finish();
}
