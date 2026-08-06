// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::io::{self, BufRead, BufWriter, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use serde_json::json;
use solstone_core_generate::{
    GenerateRequest, contract, decode_session_request_line, decode_session_terminal_line,
};

const MODE_ENV: &str = "SOLSTONE_GENERATE_SESSION_STUB_MODE";
const PID_PATH_ENV: &str = "SOLSTONE_GENERATE_SESSION_STUB_PID_PATH";
const STATS_PATH_ENV: &str = "SOLSTONE_GENERATE_SESSION_STUB_STATS_PATH";

fn main() {
    let declared_max_in_flight = session_bound();
    write_pid();
    let mode = env::var(MODE_ENV).unwrap_or_else(|_| "immediate".to_owned());
    let stdin = io::stdin();
    let mut stdout = BufWriter::new(io::stdout().lock());
    let mut pending = Vec::new();
    let mut observed_max = 0;
    let mut requests_seen = 0;
    let mut stderr_written = false;

    for line in stdin.lock().lines() {
        let line = line.expect("stub stdin is readable");
        if decode_session_terminal_line(&line).is_ok() {
            if mode == "bound" || mode == "out_of_order" {
                respond_pending(&mut stdout, &mut pending);
            }
            write_stats(declared_max_in_flight, observed_max);
            return;
        }
        let request = decode_session_request_line(&line).expect("stub request is valid");
        requests_seen += 1;

        match mode.as_str() {
            "immediate" => write_generated(&mut stdout, request_id(&request), request_id(&request)),
            "out_of_order" => {
                pending.push(request);
                observed_max = observed_max.max(pending.len());
                if pending.len() == 2 {
                    let second = pending.pop().expect("two pending requests");
                    let first = pending.pop().expect("two pending requests");
                    write_generated(&mut stdout, request_id(&second), "second");
                    write_generated(&mut stdout, request_id(&first), "first");
                }
            }
            "bound" => {
                pending.push(request);
                observed_max = observed_max.max(pending.len());
                if pending.len() == declared_max_in_flight {
                    respond_pending(&mut stdout, &mut pending);
                }
            }
            "refuse_then_generate" => {
                if requests_seen == 1 {
                    write_refused(&mut stdout, request_id(&request), "first refusal", None);
                } else {
                    write_generated(&mut stdout, request_id(&request), "second response");
                }
            }
            "hold" => pending.push(request),
            "stray_idle" => {
                stdout.write_all(b"not a generate record\n").unwrap();
                stdout.flush().unwrap();
                idle();
            }
            "unknown_id_idle" => {
                write_generated(&mut stdout, "unknown-request", "wrong response");
                idle();
            }
            "retired_id_idle" => {
                write_generated(&mut stdout, request_id(&request), "first response");
                write_generated(&mut stdout, request_id(&request), "duplicate response");
                idle();
            }
            "exit" => std::process::exit(0),
            "unknown_reason" => write_refused(
                &mut stdout,
                request_id(&request),
                "future refusal",
                Some("future_reason_code"),
            ),
            "stderr_noise" => {
                if !stderr_written {
                    io::stderr().write_all(&vec![b'x'; 300 * 1024]).unwrap();
                    io::stderr().flush().unwrap();
                    stderr_written = true;
                }
                write_generated(&mut stdout, request_id(&request), request_id(&request));
            }
            unknown => panic!("unsupported session stub mode {unknown}"),
        }
    }
}

fn session_bound() -> usize {
    let args = env::args().collect::<Vec<_>>();
    assert_eq!(args.get(1).map(String::as_str), Some("--session"));
    assert_eq!(args.get(2).map(String::as_str), Some("--max-in-flight"));
    args.get(3)
        .expect("session concurrency argument")
        .parse()
        .expect("session concurrency is an integer")
}

fn request_id(request: &GenerateRequest) -> &str {
    request.id.as_deref().expect("session request has an id")
}

fn write_generated(stdout: &mut BufWriter<impl Write>, id: &str, text: &str) {
    let schemas = &contract()["schema_identifiers"];
    writeln!(
        stdout,
        "{}",
        json!({
            "schema": schemas["response"],
            "id": id,
            "outcome": "generated",
            "text": text,
            "model": "stub-model",
            "usage": {},
            "finish_reason": "stop",
            "thinking": null,
            "schema_validation": null,
            "input_budget": null,
            "request_budget": null,
            "inference": null,
        })
    )
    .unwrap();
    stdout.flush().unwrap();
}

fn write_refused(
    stdout: &mut BufWriter<impl Write>,
    id: &str,
    detail: &str,
    reason_code: Option<&str>,
) {
    let schemas = &contract()["schema_identifiers"];
    writeln!(
        stdout,
        "{}",
        json!({
            "schema": schemas["response"],
            "id": id,
            "outcome": "refused",
            "reason": "provider-response-invalid",
            "reason_code": reason_code,
            "retryable": true,
            "blocking": false,
            "reset_at_ms": null,
            "provider": "stub",
            "detail": detail,
        })
    )
    .unwrap();
    stdout.flush().unwrap();
}

fn respond_pending(stdout: &mut BufWriter<impl Write>, pending: &mut Vec<GenerateRequest>) {
    for request in std::mem::take(pending) {
        write_generated(stdout, request_id(&request), request_id(&request));
    }
}

fn write_pid() {
    if let Some(path) = env::var_os(PID_PATH_ENV) {
        fs::write(PathBuf::from(path), std::process::id().to_string()).unwrap();
    }
}

fn write_stats(declared_max_in_flight: usize, observed_max: usize) {
    if let Some(path) = env::var_os(STATS_PATH_ENV) {
        fs::write(
            PathBuf::from(path),
            json!({
                "declared_max_in_flight": declared_max_in_flight,
                "observed_max_in_flight": observed_max,
            })
            .to_string(),
        )
        .unwrap();
    }
}

fn idle() -> ! {
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
