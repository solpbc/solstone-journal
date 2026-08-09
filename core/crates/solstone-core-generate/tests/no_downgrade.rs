// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The no-downgrade invariant, asserted on the observable that matters.
//!
//! The confidential lane is one of the boundary's two egress paths. Its guarantee is that a
//! request whose attestation cannot be verified does not egress — and the sharpest observable
//! of that is not the outcome string, it is that **the content never reached the endpoint**.
//!
//! So both cells here point at the *same reachable stub* and differ only by the presence of a
//! confidential provenance block. Cell A proves the configuration genuinely reaches a model;
//! cell B proves that adding the block stops it before a single byte leaves. Without cell A,
//! a boundary that refused everything would pass cell B.

mod support;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, RefusalReason, decode_one_shot_response,
    encode_one_shot_request,
};

/// A journal rooted in a temp directory, configured for the non-bundled local lane.
///
/// `confidential` is the only difference between the two cells: with it the resolved lane is
/// confidential and the attestation guard runs; without it the same endpoint is an ordinary
/// owner-supplied one.
struct Journal {
    path: PathBuf,
}

impl Journal {
    fn byo_endpoint(port: u16, confidential: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-no-downgrade-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir_all(path.join("config")).unwrap();
        let services = if confidential {
            r#","services":{"confidential":{"endpoint_url":"https://127.0.0.1:1","service":"stub"}}"#
        } else {
            ""
        };
        fs::write(
            path.join("config/journal.json"),
            format!(
                r#"{{"providers":{{"active":{{"provider":"local"}},"local":{{"endpoint_url":"http://127.0.0.1:{port}","served_model_id":"stub"}}}}{services}}}"#
            ),
        )
        .unwrap();
        Self { path }
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A local OpenAI-compatible endpoint that counts the inference calls it is asked to serve.
///
/// The count is the evidence: an egress guard that works means this stays at zero while the
/// endpoint is fully reachable.
struct CountingStub {
    port: u16,
    inferences: Arc<AtomicUsize>,
    worker: thread::JoinHandle<()>,
}

impl CountingStub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener
            .set_nonblocking(false)
            .expect("blocking listener for the stub");
        let port = listener.local_addr().unwrap().port();
        let inferences = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&inferences);
        let worker = thread::spawn(move || {
            for stream in listener.incoming().take(16) {
                let Ok(stream) = stream else { return };
                if serve(stream, &counter) {
                    return;
                }
            }
        });
        Self {
            port,
            inferences,
            worker,
        }
    }

    fn inferences(&self) -> usize {
        self.inferences.load(Ordering::SeqCst)
    }
}

fn serve(mut stream: TcpStream, inferences: &AtomicUsize) -> bool {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let Ok(read) = stream.read(&mut chunk) else {
            return false;
        };
        if read == 0 {
            return false;
        }
        request.extend_from_slice(&chunk[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        // HTTP field names are case-insensitive (RFC 9110) and the client sends
        // them lower-cased, so an exact match read every POST body as
        // zero-length and left the body unread when this returned.
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
    let (body, terminal) = if request_line.starts_with("GET /health ") {
        (r#"{"loaded_model":"stub"}"#.to_owned(), false)
    } else if request_line.starts_with("GET /props ") {
        (r#"{"n_ctx":16384,"total_slots":1}"#.to_owned(), false)
    } else if request_line.starts_with("POST /tokenize ") {
        (r#"{"tokens":[1]}"#.to_owned(), false)
    } else if request_line.starts_with("POST /v1/chat/completions ") {
        inferences.fetch_add(1, Ordering::SeqCst);
        (
            r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#
                .to_owned(),
            true,
        )
    } else {
        ("{}".to_owned(), false)
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    terminal
}

fn request() -> GenerateRequest {
    GenerateRequest {
        id: Some("no-downgrade".to_owned()),
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: "OK".to_owned(),
        }],
        system_instruction: None,
        temperature: 0.3,
        max_output_tokens: 16,
        thinking_budget: None,
        timeout_s: Some(5.0),
        json_output: false,
        json_schema: None,
        enforce_responsiveness: true,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn spawn(journal: &Journal) -> std::process::Output {
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
                .write_all(encode_one_shot_request(&request()).unwrap().as_bytes())?;
            child.wait_with_output()
        })
        .unwrap()
}

/// Cell A — the positive control.
///
/// Without it the invariant below is satisfied by a boundary that refuses everything, which is
/// the failure mode an outcome-only assertion cannot see.
#[test]
fn owner_supplied_endpoint_generates_and_is_actually_contacted() {
    let stub = CountingStub::start();
    let journal = Journal::byo_endpoint(stub.port, false);
    let output = spawn(&journal);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response =
        decode_one_shot_response(std::str::from_utf8(&output.stdout).unwrap()).expect("response");
    assert!(
        matches!(response, GenerateResponse::Generated(_)),
        "the control cell must genuinely reach the model, else the invariant below is vacuous; got {response:?}"
    );
    assert_eq!(
        stub.inferences(),
        1,
        "the control cell must have contacted the endpoint exactly once"
    );
    let _ = stub.worker.join();
}

/// Cell B — the invariant.
///
/// Identical configuration plus a confidential provenance block whose attestation cannot be
/// verified. The endpoint is still fully reachable, so a boundary that leaked would leak here.
#[test]
fn confidential_lane_refuses_before_any_content_leaves() {
    let stub = CountingStub::start();
    let journal = Journal::byo_endpoint(stub.port, true);
    let output = spawn(&journal);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a refusal is a successful answer and exits 0; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_ne!(
        output.status.code(),
        Some(69),
        "69 belongs to the handler namespace, never to this boundary"
    );

    let response =
        decode_one_shot_response(std::str::from_utf8(&output.stdout).unwrap()).expect("response");
    let GenerateResponse::Refused(refusal) = &response else {
        panic!("the confidential lane must not produce a generated outcome; got {response:?}");
    };
    assert!(
        refusal.blocking,
        "an unverifiable confidential environment holds the owner's material; got {refusal:?}"
    );
    assert!(
        matches!(
            refusal.reason,
            RefusalReason::AttestationFailed
                | RefusalReason::AttestationStale
                | RefusalReason::AttestationNotVerified
        ),
        "the refusal must come from the attestation guard, not from some unrelated config error \
         that would leave the invariant untested; got {refusal:?}"
    );

    // The observable that matters: the endpoint was reachable and was never asked.
    assert_eq!(
        stub.inferences(),
        0,
        "content reached the endpoint on a lane whose attestation was never verified"
    );
}
