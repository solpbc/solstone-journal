// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The session framing: the stdin reader, the in-flight bound, the worker
//! dispatch, and the abort path.
//!
//! This lived in the binary crate because the wire was built there first. It is
//! protocol, not dispatch, so it belongs in the library — where it can be driven
//! in-process instead of only by spawning a child.
//!
//! ⚠ Three things are injected rather than owned here, because a library must
//! not decide them: what a request responds with, how a protocol failure
//! terminates, and what a bare EOF does. The binary supplies all three.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use solstone_core_generate::{GenerateRequest, GenerateResponse};

use crate::EndpointRuntime;
use crate::refusal::protocol_reason;

/// What the fixture says this session may do.
pub struct SessionConfig {
    pub max_in_flight: usize,
    pub line_limit_bytes: usize,
    pub terminal_schema: String,
}

enum SessionInput {
    // Boxed: the request dwarfs the unit variant, and this crosses a channel.
    Request(Box<GenerateRequest>),
    Terminal,
}

/// How the loop ended. ⛔ Exit codes stay in the binary — the library reports
/// what happened and does not decide what the process returns.
pub enum SessionOutcome {
    /// The terminal record arrived and every in-flight request completed.
    Completed,
    /// The reader thread went away without sending a terminal record.
    ReaderDisconnected,
}

/// Everything the session needs from its host.
pub struct SessionHost<Respond, Fail, Abort> {
    /// Produce a response for one request.
    pub respond: Respond,
    /// A protocol failure the session cannot continue past.
    pub fail: Fail,
    /// Bare EOF: the caller disappeared.
    ///
    /// ⚠ Called from the reader thread, deliberately, so it takes effect before
    /// a worker can emit. Routing EOF through the channel instead would leave a
    /// window in which an in-flight response still reached stdout — which is
    /// exactly what the abort guarantee forbids.
    pub abort: Abort,
}

/// A closure that cannot return normally still has to be spelled with a type
/// the stable compiler accepts. `Infallible` is that type; this turns it back
/// into divergence at the call site.
fn never(value: Infallible) -> ! {
    match value {}
}

/// Read one newline-terminated line, bounded by the fixture's limit.
///
/// `Ok(None)` is EOF with nothing buffered.
pub fn read_session_line(
    reader: &mut impl BufRead,
    line_limit_bytes: usize,
) -> Result<Option<String>, String> {
    let mut line = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|error| format!("stdin I/O error: {error}"))?;
        if buffer.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            if line
                .len()
                .checked_add(newline)
                .is_none_or(|length| length > line_limit_bytes)
            {
                return Err(format!(
                    "stdin line exceeds fixture limit of {line_limit_bytes} bytes"
                ));
            }
            line.extend_from_slice(&buffer[..newline + 1]);
            reader.consume(newline + 1);
            break;
        }
        if line
            .len()
            .checked_add(buffer.len())
            .is_none_or(|length| length > line_limit_bytes)
        {
            return Err(format!(
                "stdin line exceeds fixture limit of {line_limit_bytes} bytes"
            ));
        }
        line.extend_from_slice(buffer);
        let consumed = buffer.len();
        reader.consume(consumed);
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|error| format!("stdin is not UTF-8: {error}"))
}

fn spawn_reader<Fail, Abort>(
    mut reader: impl BufRead + Send + 'static,
    input: mpsc::Sender<SessionInput>,
    terminal_schema: String,
    line_limit_bytes: usize,
    aborting: Arc<AtomicBool>,
    fail: Fail,
    abort: Abort,
) where
    Fail: Fn(Option<String>, &'static str, String) -> Infallible + Send + 'static,
    Abort: Fn() -> Infallible + Send + 'static,
{
    thread::spawn(move || {
        loop {
            let line = match read_session_line(&mut reader, line_limit_bytes) {
                Ok(Some(line)) => line,
                Ok(None) => {
                    aborting.store(true, Ordering::Release);
                    never(abort());
                }
                Err(detail) => never(fail(None, protocol_reason("malformed_request"), detail)),
            };
            let value = match serde_json::from_str::<Value>(line.trim_end()) {
                Ok(Value::Object(value)) => value,
                Ok(_) => never(fail(
                    None,
                    protocol_reason("malformed_request"),
                    "request must be a JSON object".to_owned(),
                )),
                Err(_) => never(fail(
                    None,
                    protocol_reason("malformed_request"),
                    "stdin is not valid JSON".to_owned(),
                )),
            };
            if value.get("schema").and_then(Value::as_str) == Some(terminal_schema.as_str()) {
                if let Err(detail) = solstone_core_generate::decode_session_terminal_line(&line) {
                    never(fail(None, protocol_reason("malformed_request"), detail));
                }
                let _ = input.send(SessionInput::Terminal);
                return;
            }
            let request = match solstone_core_generate::decode_session_request_line(&line) {
                Ok(request) => request,
                Err(detail) => never(fail(None, protocol_reason("malformed_request"), detail)),
            };
            if input
                .send(SessionInput::Request(Box::new(request)))
                .is_err()
            {
                return;
            }
        }
    });
}

#[allow(clippy::type_complexity)]
fn spawn_worker<Respond, Fail>(
    request: GenerateRequest,
    out: Arc<Mutex<Box<dyn Write + Send>>>,
    aborting: Arc<AtomicBool>,
    endpoint_runtime: Arc<EndpointRuntime>,
    respond: Arc<Respond>,
    fail: Arc<Fail>,
) -> thread::JoinHandle<()>
where
    Respond: Fn(&GenerateRequest, &EndpointRuntime) -> Result<GenerateResponse, String>
        + Send
        + Sync
        + 'static,
    Fail: Fn(Option<String>, &'static str, String) -> Infallible + Send + Sync + 'static,
{
    thread::spawn(move || {
        let request_id = request.id.clone();
        let response = match respond(&request, &endpoint_runtime) {
            Ok(response) => response,
            Err(detail) => never(fail(
                request_id,
                protocol_reason("internal_failure"),
                detail,
            )),
        };
        if aborting.load(Ordering::Acquire) {
            return;
        }
        let line = match solstone_core_generate::encode_session_response_line(&response) {
            Ok(line) => line,
            Err(detail) => never(fail(
                request_id,
                protocol_reason("internal_failure"),
                detail,
            )),
        };
        if aborting.load(Ordering::Acquire) {
            return;
        }
        let mut out = out.lock().expect("session stdout lock poisoned");
        if aborting.load(Ordering::Acquire) {
            return;
        }
        let result = out
            .write_all(line.as_bytes())
            .and_then(|()| out.flush())
            .map_err(|error| format!("stdout I/O error: {error}"));
        if let Err(detail) = result {
            fail(request_id, protocol_reason("internal_failure"), detail);
        }
    })
}

fn reap(workers: &mut Vec<thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let _ = workers.remove(index).join();
        } else {
            index += 1;
        }
    }
}

/// Drive one session to completion.
///
/// ⚠ Takes its reader and writer rather than reaching for the process's own, so
/// the loop can be driven in-process by a test instead of only by spawning a
/// child.
pub fn run_session<Respond, Fail, Abort>(
    reader: impl BufRead + Send + 'static,
    out: Box<dyn Write + Send>,
    config: SessionConfig,
    host: SessionHost<Respond, Fail, Abort>,
) -> SessionOutcome
where
    Respond: Fn(&GenerateRequest, &EndpointRuntime) -> Result<GenerateResponse, String>
        + Send
        + Sync
        + 'static,
    Fail: Fn(Option<String>, &'static str, String) -> Infallible + Send + Sync + Clone + 'static,
    Abort: Fn() -> Infallible + Send + 'static,
{
    let aborting = Arc::new(AtomicBool::new(false));
    let endpoint_runtime = Arc::new(EndpointRuntime::default());
    let (input_tx, input_rx) = mpsc::channel();
    let respond = Arc::new(host.respond);
    let fail = Arc::new(host.fail);
    spawn_reader(
        reader,
        input_tx,
        config.terminal_schema,
        config.line_limit_bytes,
        Arc::clone(&aborting),
        (*fail).clone(),
        host.abort,
    );

    let out = Arc::new(Mutex::new(out));
    let mut pending: VecDeque<GenerateRequest> = VecDeque::new();
    let mut workers = Vec::new();
    let mut terminal_received = false;

    loop {
        reap(&mut workers);
        while workers.len() < config.max_in_flight {
            let Some(request) = pending.pop_front() else {
                break;
            };
            workers.push(spawn_worker(
                request,
                Arc::clone(&out),
                Arc::clone(&aborting),
                Arc::clone(&endpoint_runtime),
                Arc::clone(&respond),
                Arc::clone(&fail),
            ));
        }
        if terminal_received && pending.is_empty() && workers.is_empty() {
            return SessionOutcome::Completed;
        }
        if terminal_received {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        match input_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(SessionInput::Request(request)) => pending.push_back(*request),
            Ok(SessionInput::Terminal) => terminal_received = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return SessionOutcome::ReaderDisconnected;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A writer the test can read back.
    #[derive(Clone)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("capture lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn config() -> SessionConfig {
        let session = &solstone_core_generate::contract()["framing"]["session"];
        SessionConfig {
            max_in_flight: 2,
            line_limit_bytes: session["line_limit_bytes"].as_u64().expect("limit") as usize,
            terminal_schema: session["terminal"]["schema"]
                .as_str()
                .expect("terminal schema")
                .to_owned(),
        }
    }

    /// 🔴 The point of moving this out of the binary: the loop runs here, in
    /// this process, against a reader and a writer the test owns. Every other
    /// check on the session framing has to spawn a child and talk to it over
    /// pipes.
    #[test]
    fn the_session_loop_runs_in_process_and_answers_every_request() {
        let terminal =
            solstone_core_generate::contract()["framing"]["session"]["terminal"]["schema"]
                .as_str()
                .expect("terminal schema");
        let mut stdin = String::new();
        for id in ["a", "b"] {
            let request = GenerateRequest {
                id: Some(id.to_owned()),
                context: "test.generate".to_owned(),
                contents: vec![solstone_core_generate::ContentPart::Text {
                    text: "hello".to_owned(),
                }],
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
            };
            stdin.push_str(
                &solstone_core_generate::encode_session_request_line(&request)
                    .expect("encode session request"),
            );
        }
        stdin.push_str(&format!("{{\"schema\":\"{terminal}\"}}\n"));

        let captured = Captured(Arc::new(Mutex::new(Vec::new())));
        let sink = captured.clone();
        let outcome = run_session(
            Cursor::new(stdin.into_bytes()),
            Box::new(captured),
            config(),
            SessionHost {
                respond: |request: &GenerateRequest, _: &EndpointRuntime| {
                    Ok(GenerateResponse::Generated(Box::new(
                        solstone_core_generate::GeneratedResponse {
                            id: request.id.clone(),
                            text: "ok".to_owned(),
                            model: "stub".to_owned(),
                            usage: serde_json::json!({}),
                            finish_reason: "stop".to_owned(),
                            thinking: None,
                            schema_validation: None,
                            input_budget: None,
                            request_budget: None,
                            inference: None,
                            hints_applied: Vec::new(),
                        },
                    )))
                },
                fail: |_, _, detail: String| panic!("unexpected protocol failure: {detail}"),
                abort: || panic!("unexpected abort: the terminal record was sent"),
            },
        );

        assert!(matches!(outcome, SessionOutcome::Completed));
        let written = String::from_utf8(sink.0.lock().expect("capture lock").clone())
            .expect("session output is UTF-8");
        let ids = written
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("response line is JSON")["id"]
                    .as_str()
                    .expect("response carries its id")
                    .to_owned()
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            ids,
            ["a".to_owned(), "b".to_owned()].into_iter().collect(),
            "every submitted request must be answered exactly once"
        );
    }

    /// The line bound is the framing's, and it is read from the fixture.
    #[test]
    fn a_line_past_the_fixture_limit_is_refused_rather_than_buffered() {
        let long = format!("{}\n", "x".repeat(64));
        let error = read_session_line(&mut Cursor::new(long.into_bytes()), 16)
            .expect_err("a line past the limit must not be returned");
        assert!(error.contains("exceeds fixture limit"), "{error}");
    }
}
