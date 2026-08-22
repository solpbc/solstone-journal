// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use solstone_core_generate::{
    ContentPart, GenerateRequest, GenerateResponse, ReasonCodeValue, SessionClient,
    SessionCompletion, SessionFailureReason, SessionLaunchReason, SessionSubmitError,
};

const RECEIVE_BOUND: Duration = Duration::from_secs(3);

fn stub_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_solstone-generate-session-stub"))
}

fn request(id: &str) -> GenerateRequest {
    GenerateRequest {
        id: Some(id.to_owned()),
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: id.to_owned(),
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

fn client(mode: &str, max_in_flight: usize) -> SessionClient {
    SessionClient::at_path(stub_path())
        .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", mode)
        .spawn(max_in_flight)
        .unwrap()
}

fn next(client: &SessionClient) -> SessionCompletion {
    client.recv_timeout(RECEIVE_BOUND).unwrap()
}

fn response(completion: SessionCompletion) -> GenerateResponse {
    let SessionCompletion::Response(response) = completion else {
        panic!("expected response completion")
    };
    response
}

fn failure(completion: SessionCompletion) -> solstone_core_generate::SessionFailure {
    let SessionCompletion::Failure(failure) = completion else {
        panic!("expected failure completion")
    };
    assert!(!failure.retryable);
    assert!(failure.blocking);
    failure
}

fn response_id_and_text(response: GenerateResponse) -> (String, String) {
    match response {
        GenerateResponse::Generated(response) => (response.id.unwrap(), response.text),
        GenerateResponse::Refused(response) => (response.id.unwrap(), response.detail),
    }
}

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "solstone-generate-session-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ))
}

fn wait_for_file(path: &PathBuf) {
    let deadline = Instant::now() + RECEIVE_BOUND;
    while fs::metadata(path).map_or(true, |metadata| metadata.len() == 0) {
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn wait_for_process_exit(pid: u32) {
    let deadline = Instant::now() + RECEIVE_BOUND;
    while Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        assert!(
            Instant::now() < deadline,
            "stub process {pid} remained alive"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(unix))]
fn wait_for_process_exit(_pid: u32) {}

#[test]
fn criterion_2_correlates_out_of_order_stub_responses() {
    let client = client("out_of_order", 2);
    client.submit(request("first")).unwrap();
    client.submit(request("second")).unwrap();
    client.close().unwrap();
    let received = [next(&client), next(&client)]
        .into_iter()
        .map(response)
        .map(response_id_and_text)
        .collect::<BTreeMap<_, _>>();
    assert_eq!(received["first"], "first");
    assert_eq!(received["second"], "second");
}

#[test]
fn criterion_3_delivers_early_response_before_later_submission() {
    let client = client("immediate", 2);
    client.submit(request("early")).unwrap();
    assert_eq!(response_id_and_text(response(next(&client))).0, "early");
    client.submit(request("later")).unwrap();
    client.close().unwrap();
    assert_eq!(response_id_and_text(response(next(&client))).0, "later");
}

#[test]
fn criterion_4_limits_child_observed_in_flight_requests() {
    let stats_path = temp_path("stats");
    let client = SessionClient::at_path(stub_path())
        .with_env("SOLSTONE_GENERATE_SESSION_STUB_MODE", "bound")
        .with_env(
            "SOLSTONE_GENERATE_SESSION_STUB_STATS_PATH",
            stats_path.as_os_str(),
        )
        .spawn(2)
        .unwrap();
    for index in 0..6 {
        client.submit(request(&format!("request-{index}"))).unwrap();
    }
    client.close().unwrap();
    for _ in 0..6 {
        let _ = response(next(&client));
    }
    wait_for_file(&stats_path);
    let stats: serde_json::Value = serde_json::from_slice(&fs::read(&stats_path).unwrap()).unwrap();
    assert_eq!(stats["declared_max_in_flight"], 2);
    assert_eq!(stats["observed_max_in_flight"], 2);
    let _ = fs::remove_file(stats_path);
}

#[test]
fn criterion_5_resubmits_after_refusal_with_new_id() {
    let client = client("refuse_then_generate", 1);
    client.submit(request("first")).unwrap();
    assert!(matches!(
        response(next(&client)),
        GenerateResponse::Refused(_)
    ));
    client.submit(request("second")).unwrap();
    client.close().unwrap();
    assert_eq!(response_id_and_text(response(next(&client))).0, "second");
}

#[test]
fn criterion_6_rejects_duplicate_outstanding_id_without_disturbing_original() {
    let client = client("hold", 1);
    client.submit(request("first")).unwrap();
    assert!(matches!(
        client.submit(request("first")),
        Err(SessionSubmitError::Correlation(
            solstone_core_generate::SessionError::DuplicateId(id)
        )) if id == "first"
    ));
    client.close().unwrap();
    assert_eq!(failure(next(&client)).id, "first");
}

#[test]
fn criterion_11_session_failures_and_launch_failures_are_fixed_safe() {
    let launch = match SessionClient::at_path("/definitely/missing/solstone-generate-wire").spawn(1)
    {
        Err(error) => error,
        Ok(_) => panic!("missing executable unexpectedly launched"),
    };
    assert!(!launch.retryable);
    assert!(launch.blocking);
    assert!(matches!(launch.reason, SessionLaunchReason::Spawn(_)));

    let client = client("exit", 2);
    client.submit(request("first")).unwrap();
    client.submit(request("second")).unwrap();
    let first = failure(next(&client));
    let second = failure(next(&client));
    assert!(matches!(first.reason, SessionFailureReason::ChildExited));
    assert!(matches!(second.reason, SessionFailureReason::ChildExited));
}

fn assert_desync(mode: &str, ids: &[&str]) {
    let client = client(mode, 2);
    let pid = client.child_id().unwrap();
    for id in ids {
        client.submit(request(id)).unwrap();
    }
    let mut failed = BTreeMap::new();
    for _ in ids {
        let failure = failure(next(&client));
        assert!(matches!(
            failure.reason,
            SessionFailureReason::Desynchronized(_)
        ));
        failed.insert(failure.id, ());
    }
    assert_eq!(failed.len(), ids.len());
    wait_for_process_exit(pid);
}

#[test]
fn criterion_12_stray_record_desynchronizes_and_criterion_13_reaps_child() {
    assert_desync("stray_idle", &["first", "second"]);
}

#[test]
fn criterion_12_unknown_id_desynchronizes_and_criterion_13_reaps_child() {
    assert_desync("unknown_id_idle", &["first", "second"]);
}

#[test]
fn criterion_12_retired_id_desynchronizes_and_criterion_13_reaps_child() {
    let client = client("retired_id_idle", 2);
    let pid = client.child_id().unwrap();
    client.submit(request("first")).unwrap();
    client.submit(request("second")).unwrap();
    assert_eq!(response_id_and_text(response(next(&client))).0, "first");
    let failure = failure(next(&client));
    assert_eq!(failure.id, "second");
    assert!(matches!(
        failure.reason,
        SessionFailureReason::Desynchronized(_)
    ));
    wait_for_process_exit(pid);
}

#[test]
fn criterion_14_child_exit_fails_all_outstanding_requests() {
    let client = client("exit", 2);
    client.submit(request("first")).unwrap();
    client.submit(request("second")).unwrap();
    let failures = [failure(next(&client)), failure(next(&client))]
        .into_iter()
        .map(|failure| {
            assert!(matches!(failure.reason, SessionFailureReason::ChildExited));
            failure.id
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(failures.len(), 2);
}

#[test]
fn criterion_15_preserves_codec_safe_unknown_reason_code_response() {
    let client = client("unknown_reason", 1);
    client.submit(request("future")).unwrap();
    let response = response(next(&client));
    let GenerateResponse::Refused(refusal) = response else {
        panic!("expected refused response")
    };
    assert!(matches!(
        refusal.reason_code,
        Some(ReasonCodeValue::Unknown(_))
    ));
    assert!(!refusal.retryable);
    assert!(refusal.blocking);
    client.close().unwrap();
}

#[test]
fn criterion_17_drains_large_stderr_while_all_requests_complete() {
    let client = client("stderr_noise", 4);
    for index in 0..4 {
        client.submit(request(&format!("request-{index}"))).unwrap();
    }
    client.close().unwrap();
    for _ in 0..4 {
        let _ = response(next(&client));
    }
}
