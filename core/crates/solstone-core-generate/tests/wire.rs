// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod support;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, OneShotClient, RefusalReason, contract,
    encode_one_shot_request,
};

struct Journal {
    path: PathBuf,
}

impl Journal {
    fn no_engine() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-generate-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(path.join("config")).unwrap();
        fs::write(
            path.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"none"}}}"#,
        )
        .unwrap();
        Self { path }
    }

    fn bundled_local(port: u16) -> Self {
        let journal = Self::no_engine();
        fs::write(
            journal.path.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"local"}}}"#,
        )
        .unwrap();
        fs::create_dir_all(journal.path.join("health")).unwrap();
        fs::write(journal.path.join("health/local.port"), port.to_string()).unwrap();
        journal
    }

    fn byo_unreachable() -> Self {
        let journal = Self::no_engine();
        fs::write(
            journal.path.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"http://127.0.0.1:1","served_model_id":"stub"}}}"#,
        )
        .unwrap();
        journal
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn request() -> GenerateRequest {
    GenerateRequest {
        id: Some("wire-test".to_owned()),
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: "OK".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 16,
        thinking_budget: None,
        timeout_s: Some(3.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

struct LocalStub {
    port: u16,
    worker: thread::JoinHandle<()>,
}

#[derive(Clone, Copy)]
struct Completion {
    text: &'static str,
    finish_reason: &'static str,
}

impl LocalStub {
    fn start() -> Self {
        Self::with_completion(Completion {
            text: "OK",
            finish_reason: "stop",
        })
    }

    fn with_completion(completion: Completion) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            for stream in listener.incoming().take(12) {
                let completed = handle_local_request(stream.unwrap(), completion);
                if completed {
                    return;
                }
            }
        });
        Self { port, worker }
    }

    fn finish(self) {
        self.worker.join().unwrap();
    }
}

fn handle_local_request(mut stream: TcpStream, completion: Completion) -> bool {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).unwrap();
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        // HTTP field names are case-insensitive (RFC 9110) and the client sends
        // them lower-cased. Matching `Content-Length: ` exactly read every POST
        // body as zero-length, so this returned with the body still unread and
        // the close that followed became an RST rather than a FIN -- which can
        // discard the response already in flight. The caller then saw a
        // truncated provider response and refused with ProviderResponseInvalid
        // instead of the reason under test, in roughly one full-target run in
        // three, on any of the bundled-local cases.
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
    let head = String::from_utf8_lossy(&request);
    let request_line = head.lines().next().unwrap_or_default();
    let (body, completed) = if request_line.starts_with("GET /health ") {
        (r#"{"loaded_model":"local"}"#.to_owned(), false)
    } else if request_line.starts_with("GET /props ") {
        (r#"{"n_ctx":16384,"total_slots":1}"#.to_owned(), false)
    } else if request_line.starts_with("POST /tokenize ") {
        (r#"{"tokens":[1]}"#.to_owned(), false)
    } else if request_line.starts_with("POST /v1/chat/completions/input_tokens ") {
        (
            r#"{"object":"response.input_tokens","input_tokens":1}"#.to_owned(),
            false,
        )
    } else if request_line.starts_with("POST /v1/chat/completions ") {
        (
            format!(
                r#"{{"choices":[{{"message":{{"content":{}}},"finish_reason":{}}}],"usage":{{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}}}"#,
                serde_json::to_string(completion.text).unwrap(),
                serde_json::to_string(completion.finish_reason).unwrap(),
            ),
            true,
        )
    } else {
        ("{}".to_owned(), false)
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
    completed
}

fn spawn_v2(journal: &Journal, request: &GenerateRequest) -> std::process::Output {
    support::generate_command()
        .arg("--one-shot")
        .env("SOLSTONE_JOURNAL", &journal.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(encode_one_shot_request(request).unwrap().as_bytes())?;
            child.wait_with_output()
        })
        .unwrap()
}

fn spawn_raw_v2(journal: &Journal, input: &str) -> std::process::Output {
    support::generate_command()
        .arg("--one-shot")
        .env("SOLSTONE_JOURNAL", &journal.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .unwrap()
}

fn assert_malformed_request(output: std::process::Output) {
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["schema"], contract()["schema_identifiers"]["error"]);
    assert_eq!(error["reason"], "malformed-request");
}

fn bundled_refusal(
    completion: Completion,
    configure: impl FnOnce(&mut GenerateRequest),
    reason: RefusalReason,
    reason_code: Option<&str>,
) {
    let stub = LocalStub::with_completion(completion);
    let journal = Journal::bundled_local(stub.port);
    let mut generated_request = request();
    configure(&mut generated_request);
    let output = spawn_v2(&journal, &generated_request);
    stub.finish();
    assert_eq!(output.status.code(), Some(0));
    assert!(matches!(output.status.code(), Some(0 | 64 | 70)));
    assert_ne!(output.status.code(), Some(69));
    let response = solstone_core_generate::decode_one_shot_response(
        std::str::from_utf8(&output.stdout).unwrap(),
    )
    .unwrap();
    let GenerateResponse::Refused(refusal) = response else {
        panic!("expected refused response")
    };
    assert_eq!(refusal.reason, reason);
    assert_eq!(
        refusal.reason_code.as_ref().map(|code| code.as_wire()),
        reason_code
    );
}

#[test]
fn one_shot_client_round_trips_no_engine_refusal() {
    let journal = Journal::no_engine();
    let response = OneShotClient::at_path(support::core_binary())
        .with_prefix_arguments(support::prefix())
        .with_env("SOLSTONE_JOURNAL", journal.path.to_string_lossy())
        .execute(&request())
        .unwrap();
    let GenerateResponse::Refused(refusal) = response else {
        panic!("expected refusal")
    };
    assert_eq!(refusal.reason, RefusalReason::NoEngineConfigured);
    assert_eq!(refusal.provider.as_deref(), Some("none"));
}

#[test]
fn v2_request_has_no_provider_or_model_field() {
    let value: serde_json::Value =
        serde_json::from_str(&encode_one_shot_request(&request()).unwrap()).unwrap();
    assert!(value.get("provider").is_none());
    assert!(value.get("model").is_none());
}

#[test]
fn real_wire_rejects_unknown_v2_request_field_on_stderr() {
    let journal = Journal::no_engine();
    let mut value: serde_json::Value =
        serde_json::from_str(&encode_one_shot_request(&request()).unwrap()).unwrap();
    value["unknown"] = serde_json::json!(true);
    assert_malformed_request(spawn_raw_v2(&journal, &value.to_string()));
}

#[test]
fn real_wire_rejects_other_malformed_v2_request_shapes() {
    let journal = Journal::no_engine();
    let valid: serde_json::Value =
        serde_json::from_str(&encode_one_shot_request(&request()).unwrap()).unwrap();
    let mut contents_string = valid.clone();
    contents_string["contents"] = serde_json::json!("wrong");
    let mut context_number = valid.clone();
    context_number["context"] = serde_json::json!(3);
    let mut text_number = valid;
    text_number["contents"][0]["text"] = serde_json::json!(3);
    for malformed in [
        serde_json::json!({}),
        contents_string,
        context_number,
        text_number,
    ] {
        assert_malformed_request(spawn_raw_v2(&journal, &malformed.to_string()));
    }
}

#[test]
fn real_wire_rejects_retired_v1_request_schema() {
    let journal = Journal::no_engine();
    let mut value: serde_json::Value =
        serde_json::from_str(&encode_one_shot_request(&request()).unwrap()).unwrap();
    value["schema"] = serde_json::json!("solstone-generate-request-v1");
    assert_malformed_request(spawn_raw_v2(&journal, &value.to_string()));
}

#[test]
fn real_wire_rejects_unparseable_stdin() {
    let journal = Journal::no_engine();
    assert_malformed_request(spawn_raw_v2(&journal, "{"));
}

#[test]
fn real_wire_contract_matches_compiled_fixture_bytes() {
    let output = support::generate_command()
        .arg("--contract")
        .output()
        .unwrap();
    assert!(output.status.success());
    let expected = serde_json::to_vec_pretty(contract()).unwrap();
    assert_eq!(output.stdout, [expected, b"\n".to_vec()].concat());
}

#[test]
fn bundled_local_round_trip_generates_and_logs_one_usage_record() {
    let stub = LocalStub::start();
    let journal = Journal::bundled_local(stub.port);
    let output = spawn_v2(&journal, &request());
    stub.finish();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let response = solstone_core_generate::decode_one_shot_response(
        std::str::from_utf8(&output.stdout).unwrap(),
    )
    .unwrap();
    let GenerateResponse::Generated(generated) = response else {
        panic!("expected generated response: {response:?}")
    };
    assert_eq!(generated.text, "OK");
    let token_lines = fs::read_dir(journal.path.join("tokens"))
        .unwrap()
        .map(|entry| {
            fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .count()
        })
        .sum::<usize>();
    assert_eq!(token_lines, 1);
}

#[test]
fn bundled_local_schema_validation_log_stays_off_stdout() {
    let stub = LocalStub::with_completion(Completion {
        text: "{}",
        finish_reason: "stop",
    });
    let journal = Journal::bundled_local(stub.port);
    let mut generated_request = request();
    generated_request.json_schema = Some(serde_json::json!({
        "type": "object",
        "required": ["answer"],
    }));
    let output = spawn_v2(&journal, &generated_request);
    stub.finish();
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("schema_validation:"));
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let response = solstone_core_generate::decode_one_shot_response(
        std::str::from_utf8(&output.stdout).unwrap(),
    )
    .unwrap();
    let GenerateResponse::Generated(generated) = response else {
        panic!("expected generated response")
    };
    assert!(generated.schema_validation.is_some());
}

#[test]
fn bundled_local_incomplete_json_refuses() {
    bundled_refusal(
        Completion {
            text: "{}",
            finish_reason: "length",
        },
        |request| request.json_output = true,
        RefusalReason::IncompleteJson,
        Some("incomplete_json_length"),
    );
}

#[test]
fn bundled_local_invalid_provider_response_refuses() {
    bundled_refusal(
        Completion {
            text: "",
            finish_reason: "stop",
        },
        |_| {},
        RefusalReason::ProviderResponseInvalid,
        Some("provider_response_invalid"),
    );
}

/// A truncated **plain-text** completion is NOT a refusal, and that is worth pinning rather
/// than assuming.
///
/// The strict validator maps a truncating finish reason to an error only when `json_output` is
/// set, so a truncated plain-text answer crosses the boundary as `generated` with its
/// `finish_reason` intact. The evidence reaches the caller; the classification does not.
/// `incomplete-text` is therefore a declared refusal reason this boundary does not emit — the
/// fan-out re-derives it for itself after the call.
///
/// ⛔ Do not "fix" this by making it refuse. It is the deliberate behaviour of the
/// result-returning entry point, which exists so a caller decides for itself, and changing it
/// here would change what every one-shot consumer records.
#[test]
fn bundled_local_truncated_text_generates_and_carries_its_finish_reason() {
    let stub = LocalStub::with_completion(Completion {
        text: "a partial answ",
        finish_reason: "length",
    });
    let journal = Journal::bundled_local(stub.port);
    let mut truncating = request();
    truncating.json_output = false;
    let output = spawn_v2(&journal, &truncating);
    stub.finish();

    assert_eq!(output.status.code(), Some(0));
    let response = solstone_core_generate::decode_one_shot_response(
        std::str::from_utf8(&output.stdout).unwrap(),
    )
    .unwrap();
    let GenerateResponse::Generated(generated) = response else {
        panic!("a truncated plain-text completion is reported as generated, not refused")
    };
    // ⚠ Normalised on the way through: the endpoint said "length" and the caller sees
    // "max_tokens". A consumer matching the raw provider spelling would miss the truncation
    // entirely, which is exactly why this is pinned rather than assumed.
    assert_eq!(
        generated.finish_reason, "max_tokens",
        "the caller's only signal that the answer was cut off must survive the boundary"
    );
}

#[test]
fn bundled_local_non_responsive_output_refuses() {
    bundled_refusal(
        Completion {
            text: "I cannot help with that request.",
            finish_reason: "stop",
        },
        |_| {},
        RefusalReason::NonResponsiveOutput,
        Some("non_responsive"),
    );
}

#[test]
fn byo_unreachable_refuses_with_diagnostics_and_one_stdout_record() {
    let journal = Journal::byo_unreachable();
    let output = support::generate_command()
        .arg("--one-shot")
        .env("SOLSTONE_JOURNAL", &journal.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(encode_one_shot_request(&request()).unwrap().as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(matches!(output.status.code(), Some(0 | 64 | 70)));
    assert_ne!(output.status.code(), Some(69));
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
    let response = solstone_core_generate::decode_one_shot_response(
        std::str::from_utf8(&output.stdout).unwrap(),
    )
    .unwrap();
    assert!(matches!(response, GenerateResponse::Refused(_)));
}
