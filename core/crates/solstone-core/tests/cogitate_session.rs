// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_cogitate::{COGITATE_RUNTIME_PREAMBLE, cogitate_sol_tool_hint};

const BOUND: Duration = Duration::from_secs(3);
const ENDPOINT_ENV: &str = "SOLSTONE_COGITATE_ENDPOINT_URL_OVERRIDE";
const API_KEY_ENV: &str = "SOLSTONE_COGITATE_API_KEY_OVERRIDE";
const BASE_URL_ENV: &str = "SOLSTONE_GENERATE_BASE_URL_OVERRIDE";

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

    fn with_byo_endpoint(label: &str) -> Self {
        let journal = Self::new(label);
        fs::create_dir_all(journal.0.join("config")).expect("create config directory");
        fs::write(
            journal.0.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"http://configured","served_model_id":"configured"}}}"#,
        )
        .expect("write local endpoint config");
        journal
    }

    fn write_config(&self, config: Value) {
        fs::create_dir_all(self.0.join("config")).expect("create config directory");
        fs::write(self.0.join("config/journal.json"), config.to_string())
            .expect("write journal config");
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
                let _ = read_request(&mut stream);
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

fn read_request(stream: &mut TcpStream) -> String {
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
    String::from_utf8(bytes).expect("request is UTF-8")
}

fn request(journal: &TempJournal, dry_run: bool) -> Value {
    json!({
        "schema": "solstone-cogitate-request-v2",
        "access_tier": "normal",
        "outbound_approval": null,
        "diagnostic": false,
        "talent_instruction": "Be concise.",
        "sol_tool_name": "solstone",
        "read_scope": [],
        "output_path": null,
        "schedule": "daily",
        "max_turns": 4,
        "context_window": 4096,
        "timeout_ms": 30_000,
        "read_call_budget": 5,
        "model": "fixture-model",
        "correlation_id": "session-corr",
        "initial_prompt": "Do the task.",
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

fn google_final_response() -> Value {
    json!({
        "modelVersion": "google-stub-model",
        "candidates": [{
            "content": {"parts": [{"functionCall": {
                "id": "final-1",
                "name": "emit_final",
                "args": {"content": "done"}
            }}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {
            "promptTokenCount": 2,
            "candidatesTokenCount": 3,
            "totalTokenCount": 5
        }
    })
}

fn anthropic_final_response() -> Value {
    json!({
        "model": "anthropic-stub-model",
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 2, "output_tokens": 3},
        "content": [{
            "type": "tool_use",
            "id": "final-1",
            "name": "emit_final",
            "input": {"content": "done"}
        }]
    })
}

fn openai_final_response() -> Value {
    json!({
        "model": "openai-stub-model",
        "status": "completed",
        "usage": {"input_tokens": 2, "output_tokens": 3},
        "output": [{
            "type": "function_call",
            "call_id": "final-1",
            "name": "emit_final",
            "arguments": "{\"content\":\"done\"}"
        }]
    })
}

fn spawn_one_shot(input: &str, endpoint: Option<&str>) -> Child {
    spawn_one_shot_with_base_url(input, endpoint, None)
}

fn spawn_one_shot_with_base_url(
    input: &str,
    endpoint: Option<&str>,
    base_url: Option<&str>,
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["cogitate", "--one-shot"])
        .env_remove(ENDPOINT_ENV)
        .env_remove(API_KEY_ENV)
        .env_remove(BASE_URL_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(endpoint) = endpoint {
        command
            .env(ENDPOINT_ENV, endpoint)
            .env(API_KEY_ENV, "test-cogitate-credential");
    }
    if let Some(base_url) = base_url {
        command.env(BASE_URL_ENV, base_url);
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

struct CapturingStub {
    url: String,
    request: mpsc::Receiver<String>,
    handle: thread::JoinHandle<()>,
}

impl CapturingStub {
    fn start(body: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("stub bind");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let (sender, request) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("stub accept");
            sender
                .send(read_request(&mut stream))
                .expect("send request");
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("stub write");
        });
        Self {
            url,
            request,
            handle,
        }
    }

    fn join(self) -> String {
        let request = self
            .request
            .recv_timeout(BOUND)
            .expect("stub receives request");
        self.handle.join().expect("stub joins");
        request
    }
}

fn parsed_lines(stdout: &[u8]) -> Vec<Value> {
    let text = std::str::from_utf8(stdout).expect("stdout is UTF-8");
    assert!(text.ends_with('\n'), "stdout must end in an NDJSON newline");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("stdout line is JSON"))
        .collect()
}

fn contract_exit_code(name: &str) -> i32 {
    serde_json::from_str::<Value>(solstone_core_cogitate_wire::contract_source())
        .expect("cogitate wire contract is JSON")["exit_codes"][name]
        .as_i64()
        .expect("cogitate exit code is an integer") as i32
}

#[test]
fn one_shot_streams_valid_terminal_ndjson() {
    let journal = TempJournal::with_byo_endpoint("one-shot");
    let stub = Stub::start(vec![(final_response(), Duration::ZERO)]);
    let child = spawn_one_shot(&request(&journal, false).to_string(), Some(&stub.url));
    let output = child.wait_with_output().expect("wait cogitate core");
    stub.join();
    assert_eq!(
        output.status.code(),
        Some(contract_exit_code("success")),
        "stderr: {:?}",
        output.stderr
    );
    let lines = parsed_lines(&output.stdout);
    assert!(
        lines
            .iter()
            .any(|line| { matches!(line["event"].as_str(), Some("finish") | Some("error")) })
    );
    assert_eq!(lines.last().expect("terminal line")["terminal"], true);
}

#[test]
fn one_shot_sends_live_composed_system_instruction_to_provider() {
    let journal = TempJournal::with_byo_endpoint("live-composition");
    let stub = CapturingStub::start(final_response());
    let mut request = request(&journal, false);
    request["talent_instruction"] = json!("TALENT RULES");
    let child = spawn_one_shot(&request.to_string(), Some(&stub.url));
    let output = child.wait_with_output().expect("wait cogitate core");
    let request = stub.join();
    assert_eq!(output.status.code(), Some(contract_exit_code("success")));

    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| serde_json::from_str::<Value>(body).expect("request body is JSON"))
        .expect("HTTP request has a body");
    let expected = live_system_instruction_from_oracle_vector("prompt_with_system_instruction");
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": expected})
    );
}

fn live_system_instruction_from_oracle_vector(id: &str) -> String {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/cogitate_oracle.json"))
            .expect("cogitate oracle fixture parses");
    let vector = fixture["prompt_assembly"]
        .as_array()
        .expect("prompt assembly vectors")
        .iter()
        .find(|vector| vector["id"] == id)
        .expect("named prompt assembly vector");
    let instruction = &vector["expect"]["system_instruction"];
    let separator = instruction["separator"]
        .as_str()
        .expect("system instruction separator");

    instruction["parts"]
        .as_array()
        .expect("system instruction parts")
        .iter()
        .map(|part| {
            part["text"].as_str().map(str::to_owned).unwrap_or_else(|| {
                match part["role"].as_str().expect("part role") {
                    "runtime_preamble" => {
                        COGITATE_RUNTIME_PREAMBLE.trim_end_matches('\n').to_owned()
                    }
                    "diagnostic_preamble" => solstone_core_cogitate::COGITATE_DIAGNOSTIC_PREAMBLE
                        .trim_end_matches('\n')
                        .to_owned(),
                    "sol_tool_hint" => cogitate_sol_tool_hint(
                        vector["sol_tool_name"]
                            .as_str()
                            .expect("sol tool name for hint"),
                    ),
                    role => panic!("unsupported fixture preamble role {role}"),
                }
            })
        })
        .collect::<Vec<_>>()
        .join(separator)
}

#[test]
fn one_shot_streams_before_process_exit() {
    let journal = TempJournal::with_byo_endpoint("streaming");
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
    assert_eq!(
        child.wait().expect("wait cogitate child").code(),
        Some(contract_exit_code("success"))
    );
    reader.join().expect("stdout reader joins");
    stub.join();
}

#[test]
fn two_turn_run_accumulates_nondefault_terminal_usage() {
    let journal = TempJournal::with_byo_endpoint("two-turn-usage");
    let stub = Stub::start(vec![
        (read_file_response(), Duration::ZERO),
        (final_response(), Duration::ZERO),
    ]);
    let child = spawn_one_shot(&request(&journal, false).to_string(), Some(&stub.url));
    let output = child.wait_with_output().expect("wait cogitate core");
    stub.join();
    assert_eq!(output.status.code(), Some(contract_exit_code("success")));
    let lines = parsed_lines(&output.stdout);
    let usage = &lines.last().expect("terminal event")["usage"];
    assert_eq!(usage["requests"], 2);
    assert_eq!(usage["input_tokens"], 4);
    assert_eq!(usage["output_tokens"], 6);
    assert_ne!(
        usage,
        &json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cached_tokens": 0,
            "cache_creation_tokens": 0,
            "reasoning_tokens": 0,
            "requests": 0,
        })
    );
}

/// Before this behavior landed, this exits 78 with no stdout: one-shot cogitate constructs
/// only the endpoint-override provider and never reaches Google's dialect.
#[test]
fn one_shot_google_config_dispatches_to_google_dialect() {
    let journal = TempJournal::new("google-dispatch");
    journal.write_config(json!({
        "providers": {"active": {"provider": "google", "model": "configured-model"}},
        "env": {"GOOGLE_API_KEY": "configured-google-key"}
    }));
    let stub = CapturingStub::start(google_final_response());
    let child =
        spawn_one_shot_with_base_url(&request(&journal, false).to_string(), None, Some(&stub.url));
    let output = child.wait_with_output().expect("wait cogitate core");
    let request = stub.join();
    assert_eq!(output.status.code(), Some(contract_exit_code("success")));
    assert!(request.starts_with("POST /v1beta/models/fixture-model:generateContent "));
    assert!(
        request
            .lines()
            .any(|line| { line.eq_ignore_ascii_case("x-goog-api-key: configured-google-key") })
    );
    let lines = parsed_lines(&output.stdout);
    assert_eq!(lines.last().expect("terminal event")["event"], "finish");
    assert_eq!(
        lines.last().expect("terminal event")["usage"]["requests"],
        1
    );
}

#[test]
fn one_shot_anthropic_config_dispatches_to_anthropic_dialect() {
    let journal = TempJournal::new("anthropic-dispatch");
    journal.write_config(json!({
        "providers": {"active": {"provider": "anthropic"}},
        "env": {"ANTHROPIC_API_KEY": "configured-anthropic-key"}
    }));
    let stub = CapturingStub::start(anthropic_final_response());
    let child =
        spawn_one_shot_with_base_url(&request(&journal, false).to_string(), None, Some(&stub.url));
    let output = child.wait_with_output().expect("wait cogitate core");
    let request = stub.join();
    assert_eq!(output.status.code(), Some(contract_exit_code("success")));
    assert!(request.starts_with("POST /v1/messages "));
    assert!(
        request
            .lines()
            .any(|line| { line.eq_ignore_ascii_case("x-api-key: configured-anthropic-key") })
    );
    let lines = parsed_lines(&output.stdout);
    assert_eq!(lines.last().expect("terminal event")["event"], "finish");
    assert_eq!(
        lines.last().expect("terminal event")["usage"]["requests"],
        1
    );
}

#[test]
fn one_shot_openai_config_dispatches_to_openai_dialect() {
    let journal = TempJournal::new("openai-dispatch");
    journal.write_config(json!({
        "providers": {"active": {"provider": "openai"}},
        "env": {"OPENAI_API_KEY": "configured-openai-key"}
    }));
    let stub = CapturingStub::start(openai_final_response());
    let child =
        spawn_one_shot_with_base_url(&request(&journal, false).to_string(), None, Some(&stub.url));
    let output = child.wait_with_output().expect("wait cogitate core");
    let request = stub.join();
    assert_eq!(output.status.code(), Some(contract_exit_code("success")));
    assert!(request.starts_with("POST /v1/responses "));
    assert!(
        request.lines().any(|line| {
            line.eq_ignore_ascii_case("authorization: bearer configured-openai-key")
        })
    );
    let lines = parsed_lines(&output.stdout);
    assert_eq!(lines.last().expect("terminal event")["event"], "finish");
    assert_eq!(
        lines.last().expect("terminal event")["usage"]["requests"],
        1
    );
}

#[test]
fn dry_run_needs_no_endpoint_configuration() {
    let journal = TempJournal::new("dry-run");
    let child = spawn_one_shot(&request(&journal, true).to_string(), None);
    let output = child.wait_with_output().expect("wait dry run");
    assert_eq!(
        output.status.code(),
        Some(contract_exit_code("success")),
        "stderr: {:?}",
        output.stderr
    );
    let lines = parsed_lines(&output.stdout);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "dry_run");
}

#[test]
fn malformed_one_shot_stays_off_stdout() {
    let child = spawn_one_shot("{", None);
    let output = child.wait_with_output().expect("wait malformed request");
    assert_eq!(
        output.status.code(),
        Some(contract_exit_code("malformed_request"))
    );
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

/// Before this behavior landed, this exits 78 with no stdout because one-shot cogitate always
/// constructs the endpoint-override provider before consulting journal config.
#[test]
fn no_engine_streams_a_terminal_error_instead_of_exiting_silently() {
    let journal = TempJournal::new("missing-endpoint");
    let child = spawn_one_shot(&request(&journal, false).to_string(), None);
    let output = child.wait_with_output().expect("wait missing endpoint");
    assert_eq!(output.status.code(), Some(contract_exit_code("success")));
    let lines = parsed_lines(&output.stdout);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["event"], "error");
    assert_eq!(lines[0]["reason_code"], "no_engine_configured");
    assert_eq!(lines[0]["terminal"], true);
    assert_eq!(lines[0]["usage"]["requests"], 0);
}

fn assert_preflight_error(journal: &TempJournal, expected_reason_code: &str) {
    let child = spawn_one_shot(&request(journal, false).to_string(), None);
    let output = child.wait_with_output().expect("wait preflight failure");
    assert_eq!(output.status.code(), Some(contract_exit_code("success")));
    let lines = parsed_lines(&output.stdout);
    assert_eq!(lines.len(), 1);
    let event = &lines[0];
    assert_eq!(event["event"], "error");
    assert_eq!(event["terminal"], true);
    assert_eq!(event["reason_code"], expected_reason_code);
    assert_eq!(
        event["usage"],
        json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "cached_tokens": 0,
            "cache_creation_tokens": 0,
            "reasoning_tokens": 0,
            "requests": 0,
        })
    );
}

#[test]
fn bundled_local_selects_the_cogitate_converse_arm_and_reports_server_state() {
    let journal = TempJournal::new("bundled-local");
    journal.write_config(json!({"providers": {"active": {"provider": "local"}}}));
    assert_preflight_error(&journal, "local_model_not_ready");
}

#[test]
fn unknown_provider_streams_a_named_error() {
    let journal = TempJournal::new("unknown-provider");
    journal.write_config(json!({"providers": {"active": {"provider": "unknown"}}}));
    assert_preflight_error(&journal, "unimplemented_lane");
}

#[test]
fn dead_stdout_pipe_uses_contract_exit_code() {
    let journal = TempJournal::with_byo_endpoint("dead-stdout");
    let stub = Stub::start(vec![(read_file_response(), Duration::ZERO)]);
    let mut child = spawn_one_shot(&request(&journal, false).to_string(), Some(&stub.url));
    let mut stdout = BufReader::new(child.stdout.take().expect("cogitate stdout"));
    let mut first = String::new();
    stdout
        .read_line(&mut first)
        .expect("read first NDJSON line");
    assert!(serde_json::from_str::<Value>(&first).is_ok());
    assert!(child.try_wait().expect("poll cogitate child").is_none());
    drop(stdout);

    let deadline = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll cogitate child") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "cogitate did not observe closed stdout"
        );
        thread::sleep(Duration::from_millis(10));
    };
    stub.join();
    assert_eq!(status.code(), Some(contract_exit_code("dead_stdout_pipe")));
}

#[cfg(unix)]
#[test]
fn hosted_one_shot_client_fixture() {
    use solstone_core_cogitate_wire::{CogitateOneShotClient, CogitateRequest};
    use solstone_core_system::lifecycle::acknowledge_hosted_child_admission;
    let Ok(role) = std::env::var("SOLSTONE_TEST_COGITATE_ADMISSION_ROLE") else {
        return;
    };
    let journal = PathBuf::from(std::env::var_os("SOLSTONE_JOURNAL").unwrap());
    if role != "foreign" {
        acknowledge_hosted_child_admission(&journal).unwrap();
    }
    let request = CogitateRequest::from_value(&json!({
        "schema":"solstone-cogitate-request-v2", "access_tier":"normal",
        "max_turns":4, "timeout_ms":30000, "read_call_budget":5,
        "model":"fixture", "correlation_id":"admission-fixture",
        "initial_prompt":"dry run", "journal_root":journal, "dry_run":true
    }))
    .unwrap();
    let result = CogitateOneShotClient::at_path(env!("CARGO_BIN_EXE_solstone-core"))
        .with_prefix_arguments(["cogitate".into()])
        .execute(&request);
    if role == "parent" {
        let events = result
            .expect("admitted one-shot produces valid NDJSON")
            .events;
        assert!(events.iter().any(|event| event["event"] == "dry_run"));
    } else {
        assert!(result.is_err(), "inadmissible parent spawned a one-shot");
    }
}

#[cfg(unix)]
#[test]
fn one_shot_gets_its_own_admission_before_receiving_stdin() {
    use solstone_core_system::lifecycle::{
        AdmissionAcknowledgement, AdmissionIdentity, AdmissionResult, AdmissionResultState,
        HostedServiceKind, ParentLossLedger, acknowledge_parent_loss_admission,
    };
    use solstone_core_system::process::{
        InspectResult, ProcessInstanceSource, SystemProcessInstanceSource,
    };
    for role in ["parent", "sealed", "foreign"] {
        let journal = TempJournal::new("hosted-client");
        let ledger = ParentLossLedger::open(&journal.0).unwrap();
        let InspectResult::Present { instance, uid, .. } =
            SystemProcessInstanceSource.inspect(std::process::id())
        else {
            panic!("fixture identity");
        };
        let active = ledger
            .reserve_generation(instance, [HostedServiceKind::Cortex])
            .unwrap();
        ledger.initialize_record(&active).unwrap();
        ledger
            .persist_coordinator_identity(active.generation, instance)
            .unwrap();
        ledger.mark_admitting(active.generation, instance).unwrap();
        if role == "sealed" {
            ledger.seal(active.generation, instance).unwrap();
        }
        if role == "foreign" {
            acknowledge_parent_loss_admission(
                &journal.0,
                AdmissionIdentity {
                    generation: active.generation,
                    launch_id: "talent-parent".to_owned(),
                    instance,
                    uid,
                    parent_launch_id: None,
                },
            )
            .unwrap();
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cogitate_session::hosted_one_shot_client_fixture",
                "--nocapture",
            ])
            .env("SOLSTONE_TEST_COGITATE_ADMISSION_ROLE", role)
            .env("SOLSTONE_JOURNAL", &journal.0)
            .env("SOL_PARENT_LOSS_GENERATION", active.generation.to_string())
            .env("SOL_PARENT_LOSS_LAUNCH_ID", "talent-parent")
            .env_remove("SOL_PARENT_LOSS_PARENT_LAUNCH_ID")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("one-shot admission blocked before stdin");
            }
            thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let admissions = ledger.generation_path(active.generation).join("admissions");
        let children: Vec<_> = fs::read_dir(&admissions)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.file_name() != "talent-parent")
            .collect();
        if role != "parent" {
            assert!(children.is_empty());
            continue;
        }
        assert_eq!(children.len(), 1);
        let parent: AdmissionAcknowledgement = serde_json::from_slice(
            &fs::read(admissions.join("talent-parent/acknowledgement.json")).unwrap(),
        )
        .unwrap();
        let child: AdmissionAcknowledgement = serde_json::from_slice(
            &fs::read(children[0].path().join("acknowledgement.json")).unwrap(),
        )
        .unwrap();
        let result: AdmissionResult =
            serde_json::from_slice(&fs::read(children[0].path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(
            child.identity.parent_launch_id.as_deref(),
            Some("talent-parent")
        );
        assert_ne!(child.identity.instance, parent.identity.instance);
        assert_eq!(result.identity.as_ref(), Some(&child.identity));
        assert!(matches!(result.state, AdmissionResultState::Admitted));
    }
}
