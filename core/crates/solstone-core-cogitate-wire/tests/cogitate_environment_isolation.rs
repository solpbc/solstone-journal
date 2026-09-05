// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Process-boundary validation for cogitate environment isolation.

use std::collections::BTreeMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use socket2::{Domain, Protocol, Socket, Type};
use solstone_core_cogitate_runtime::ConverseProvider;
use solstone_core_cogitate_wire::{
    COGITATE_API_KEY_OVERRIDE_ENV, COGITATE_ENDPOINT_URL_OVERRIDE_ENV, CogitateRequest,
    DispatchConverseProvider, EndpointOverrides, REQUEST_SCHEMA,
};
use solstone_core_generate_wire::{ConverseFailure, ConverseMessage, resolve_lane};

const CHILD_CASE_ENV: &str = "SOLSTONE_COGITATE_ENV_CASE";
const PROVIDER_BASE_ENV: &str = "SOLSTONE_GENERATE_BASE_URL_OVERRIDE";
const RECEIPT_PREFIX: &str = "SOLSTONE-COGITATE-ENV-RECEIPT-V1\t";
const CHILD_TIMEOUT: Duration = Duration::from_secs(3);

struct ReapingChild(Option<Child>);

impl ReapingChild {
    fn spawn(command: &mut Command) -> Self {
        Self(Some(command.spawn().expect("environment child starts")))
    }

    fn output(mut self) -> Output {
        let deadline = Instant::now() + CHILD_TIMEOUT;
        loop {
            let child = self.0.as_mut().expect("child remains owned");
            match child.try_wait().expect("child status can be polled") {
                Some(_) => {
                    return self
                        .0
                        .take()
                        .expect("child remains owned")
                        .wait_with_output()
                        .expect("child output can be collected");
                }
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                None => {
                    let mut child = self.0.take().expect("child remains owned");
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("environment child exceeded {CHILD_TIMEOUT:?}");
                }
            }
        }
    }
}

impl Drop for ReapingChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn request() -> CogitateRequest {
    CogitateRequest::from_value(&json!({
        "schema": REQUEST_SCHEMA,
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
        "timeout_ms": 250,
        "read_call_budget": 5,
        "model": "fixture-model",
        "correlation_id": "corr-env",
        "initial_prompt": "Do the task.",
        "journal_root": "/var/tmp/solstone-cogitate-wire-env-test"
    }))
    .expect("fixture request is valid")
}

fn google_failure() -> ConverseFailure {
    let config = json!({"providers": {"active": {"provider": "google"}}})
        .as_object()
        .expect("config object")
        .clone();
    let (_, lane) = resolve_lane(&config);
    let mut provider = DispatchConverseProvider::from_lane(
        &request(),
        config,
        lane,
        EndpointOverrides::from_values(None, None),
    )
    .expect("google provider constructs");
    provider
        .converse(
            "request-model",
            None,
            &[ConverseMessage::User {
                text: "hello".to_owned(),
            }],
            &[],
            Duration::from_millis(200),
        )
        .expect_err("ambient provider key must not authorize transport")
}

#[test]
fn environment_child() {
    let Ok(case) = std::env::var(CHILD_CASE_ENV) else {
        return;
    };
    let receipt = match case.as_str() {
        "present" => {
            let overrides = EndpointOverrides::from_process();
            json!({
                "case": case,
                "endpoint": overrides.endpoint_url(),
                "api_key": overrides.api_key(),
            })
        }
        "absent" => {
            let overrides = EndpointOverrides::from_process();
            json!({
                "case": case,
                "endpoint": overrides.endpoint_url(),
                "api_key": overrides.api_key(),
            })
        }
        "ambient-provider" => {
            let failure = google_failure();
            json!({"case": case, "reason_code": failure.reason_code})
        }
        unexpected => panic!("unrecognized child case {unexpected:?}"),
    };
    println!("{RECEIPT_PREFIX}{receipt}");
}

fn environment_snapshot() -> BTreeMap<std::ffi::OsString, std::ffi::OsString> {
    std::env::vars_os().collect()
}

fn run_case(case: &str, configure: impl FnOnce(&mut Command)) -> Value {
    let before = environment_snapshot();
    let mut command = Command::new(std::env::current_exe().expect("test executable path"));
    command
        .arg("--exact")
        .arg("environment_child")
        .arg("--nocapture")
        .env_clear()
        .env(CHILD_CASE_ENV, case)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure(&mut command);
    let Output {
        status,
        stdout,
        stderr,
    } = ReapingChild::spawn(&mut command).output();
    assert_child_success(status, &stdout, &stderr);
    let stdout = String::from_utf8(stdout).expect("child stdout is UTF-8");
    let receipts = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(RECEIPT_PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(
        receipts.len(),
        1,
        "stdout must contain exactly one receipt: {stdout:?}"
    );
    let receipt: Value = serde_json::from_str(receipts[0]).expect("receipt is JSON");
    assert_eq!(
        receipt["case"], case,
        "receipt is bound to the requested case"
    );
    assert_eq!(
        environment_snapshot(),
        before,
        "parent environment is unchanged"
    );
    receipt
}

fn assert_child_success(status: ExitStatus, stdout: &[u8], stderr: &[u8]) {
    assert!(
        status.success(),
        "environment child failed: status={status}; stdout={}; stderr={}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
}

#[test]
fn endpoint_override_environment_reads_present_values() {
    let receipt = run_case("present", |command| {
        command
            .env(COGITATE_ENDPOINT_URL_OVERRIDE_ENV, " http://endpoint ")
            .env(COGITATE_API_KEY_OVERRIDE_ENV, " credential ");
    });
    assert_eq!(receipt["endpoint"], "http://endpoint");
    assert_eq!(receipt["api_key"], "credential");
}

#[test]
fn endpoint_override_environment_reads_absent_values() {
    let receipt = run_case("absent", |_| {});
    assert!(receipt["endpoint"].is_null());
    assert!(receipt["api_key"].is_null());
}

fn held_bound_not_listening() -> io::Result<(Socket, u16)> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())?;
    let port = socket
        .local_addr()?
        .as_socket()
        .expect("IPv4 socket address")
        .port();
    Ok((socket, port))
}

#[test]
fn ambient_provider_key_does_not_bypass_missing_journal_credential() {
    let (_held, port) = held_bound_not_listening().expect("held refusal socket");
    let receipt = run_case("ambient-provider", |command| {
        command
            .env("GOOGLE_API_KEY", "poison-ambient-provider-key")
            .env(PROVIDER_BASE_ENV, format!("http://127.0.0.1:{port}"));
    });
    assert_eq!(receipt["reason_code"], "provider_key_missing");
}
