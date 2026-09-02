// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient, STDERR_LIMIT,
    STDOUT_LIMIT,
};

const MODE_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_MODE";
const OVERRIDES_PATH_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_OVERRIDES_PATH";
const PID_PATH_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_PID_PATH";
const ARGV_PATH_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_ARGV_PATH";
const EXPECT_ARGV_ENV: &str = "SOLSTONE_GENERATE_ONE_SHOT_STUB_EXPECT_ARGV";

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn stub_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_solstone-generate-one-shot-stub"))
}

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("/var/tmp").join(format!(
            "solstone-one-shot-stub-{}-{}-{sequence}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("fixture directory creates");
        Self { path }
    }

    fn pid_path(&self) -> PathBuf {
        self.path.join("child.pid")
    }

    fn argv_path(&self) -> PathBuf {
        self.path.join("argv.json")
    }

    fn overrides_path(&self) -> PathBuf {
        self.path.join("overrides.json")
    }

    fn read_argv(&self) -> Vec<String> {
        serde_json::from_slice(&fs::read(self.argv_path()).expect("argv record reads"))
            .expect("argv record parses")
    }

    fn read_pid(&self) -> i32 {
        fs::read_to_string(self.pid_path())
            .expect("pid record reads")
            .trim()
            .parse()
            .expect("pid record is an integer")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct SiblingStub {
    path: PathBuf,
}

impl SiblingStub {
    fn install() -> Self {
        let path = std::env::current_exe()
            .expect("test executable")
            .parent()
            .expect("test executable has a parent")
            .join("solstone-core");
        fs::copy(stub_path(), &path).expect("sibling stub copies");
        Self { path }
    }
}

impl Drop for SiblingStub {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn request() -> GenerateRequest {
    request_with_text("test")
}

fn request_with_text(text: &str) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "test.generate".to_owned(),
        contents: vec![ContentPart::Text {
            text: text.to_owned(),
        }],
        system_instruction: None,
        temperature: 0.0,
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

fn client_for_mode(mode: &str, fixture: &Fixture) -> OneShotClient {
    OneShotClient::at_path(stub_path())
        .with_env(MODE_ENV, mode)
        .with_env(PID_PATH_ENV, fixture.pid_path().to_string_lossy())
        .with_env(ARGV_PATH_ENV, fixture.argv_path().to_string_lossy())
}

fn argv_tail(arguments: &[String]) -> &[String] {
    arguments.get(1..).unwrap_or(&[])
}

#[cfg(unix)]
fn alive(pid: i32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    matches!(
        kill(Pid::from_raw(pid), None::<Signal>),
        Ok(()) | Err(Errno::EPERM)
    )
}

fn assert_reaped(fixture: &Fixture) {
    #[cfg(unix)]
    {
        let pid = fixture.read_pid();
        assert!(
            !alive(pid),
            "child pid {pid} must not be running after execute returns"
        );
    }
    #[cfg(not(unix))]
    {
        let _ = fixture;
    }
}

#[test]
fn one_shot_stub_round_trips_and_records_child_overrides() {
    let fixture = Fixture::new();
    let path = fixture.overrides_path();
    let response = OneShotClient::at_path(stub_path())
        .with_env(OVERRIDES_PATH_ENV, path.to_string_lossy())
        .with_env("SOLSTONE_GENERATE_API_KEY_OVERRIDE", "test-key")
        .with_env("SOLSTONE_GENERATE_PROVIDER_OVERRIDE", "openai")
        .with_env("SOLSTONE_GENERATE_MODEL_OVERRIDE", "test-model")
        .with_env(PID_PATH_ENV, fixture.pid_path().to_string_lossy())
        .execute(&request())
        .expect("stub response");

    let GenerateResponse::Generated(response) = response else {
        panic!("expected generated response");
    };
    assert_eq!(response.text, "one-shot-stub");
    let overrides: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("override record reads"))
            .expect("override record parses");
    assert_eq!(
        overrides,
        json!({"api_key":"test-key","provider":"openai","model":"test-model"})
    );
    assert_reaped(&fixture);
}

#[test]
fn sibling_invokes_generate_one_shot() {
    let fixture = Fixture::new();
    let _sibling = SiblingStub::install();
    let response = OneShotClient::sibling()
        .expect("sibling resolves")
        .with_env(EXPECT_ARGV_ENV, "generate,--one-shot")
        .with_env(ARGV_PATH_ENV, fixture.argv_path().to_string_lossy())
        .with_env(PID_PATH_ENV, fixture.pid_path().to_string_lossy())
        .execute(&request())
        .expect("sibling execute");
    let GenerateResponse::Generated(response) = response else {
        panic!("expected generated response");
    };
    assert_eq!(response.text, "one-shot-stub");
    assert_eq!(argv_tail(&fixture.read_argv()), ["generate", "--one-shot"]);
    assert_reaped(&fixture);
}

#[test]
fn protocol_failures_preserve_exit_status() {
    let fixture = Fixture::new();
    let error = client_for_mode("hard_failure", &fixture)
        .execute(&request())
        .expect_err("hard failure is a protocol error");
    let ClientError::Protocol(failure) = error else {
        panic!("expected protocol error, got {error:?}");
    };
    assert_eq!(failure.error.reason, "stub_failure");
    assert_eq!(failure.status.exit_code, Some(70));
    assert_reaped(&fixture);

    let fixture = Fixture::new();
    let error = client_for_mode("protocol_64", &fixture)
        .execute(&request())
        .expect_err("protocol 64 is a protocol error");
    let ClientError::Protocol(failure) = error else {
        panic!("expected protocol error, got {error:?}");
    };
    assert_eq!(failure.error.reason, "stub_failure");
    assert_eq!(failure.error.detail, "one-shot stub hard failure");
    assert_eq!(failure.status.exit_code, Some(64));
    assert_reaped(&fixture);
}

#[test]
fn nonzero_non_protocol_stderr_is_unexpected_child() {
    for (mode, stderr_prefix) in [
        ("plain_stderr", b"Usage:\n".as_slice()),
        ("empty_stderr", b"".as_slice()),
        ("invalid_utf8_stderr", &[0xff][..]),
        ("malformed_json_stderr", b"{not json".as_slice()),
    ] {
        let fixture = Fixture::new();
        let error = client_for_mode(mode, &fixture)
            .execute(&request())
            .expect_err(mode);
        let ClientError::UnexpectedChild(failure) = error else {
            panic!("{mode}: expected UnexpectedChild, got {error:?}");
        };
        if stderr_prefix.is_empty() {
            assert!(
                failure.stderr.bytes.is_empty(),
                "{mode}: stderr {:?}",
                failure.stderr
            );
        } else {
            assert!(
                failure.stderr.bytes.starts_with(stderr_prefix),
                "{mode}: stderr {:?}",
                failure.stderr
            );
        }
        assert!(!failure.stderr.truncated);
        assert_reaped(&fixture);
    }
}

#[test]
fn successful_malformed_stdout_is_decode() {
    let fixture = Fixture::new();
    let error = client_for_mode("success_malformed", &fixture)
        .execute(&request())
        .expect_err("malformed success is decode");
    assert!(
        matches!(error, ClientError::Decode(_)),
        "expected Decode, got {error:?}"
    );
    assert_reaped(&fixture);
}

#[test]
fn successful_oversized_stdout_is_decode() {
    let fixture = Fixture::new();
    let error = client_for_mode("success_oversized", &fixture)
        .execute(&request())
        .expect_err("oversized success is decode");
    let ClientError::Decode(detail) = error else {
        panic!("expected Decode, got {error:?}");
    };
    assert!(
        detail.contains(&STDOUT_LIMIT.to_string()),
        "decode names the cap: {detail}"
    );
    assert_reaped(&fixture);
}

#[test]
fn simultaneous_pressure_truncates_both_streams() {
    let fixture = Fixture::new();
    let error = client_for_mode("pressure", &fixture)
        .execute(&request())
        .expect_err("pressure is unexpected");
    let ClientError::UnexpectedChild(failure) = error else {
        panic!("expected UnexpectedChild, got {error:?}");
    };
    assert!(failure.stdout.truncated);
    assert!(failure.stderr.truncated);
    assert_eq!(failure.stdout.bytes.len(), STDOUT_LIMIT);
    assert_eq!(failure.stderr.bytes.len(), STDERR_LIMIT);
    assert_reaped(&fixture);
}

#[test]
fn early_stdin_close_on_a_large_payload_is_not_io() {
    let fixture = Fixture::new();
    let payload = "x".repeat(STDOUT_LIMIT + 1);
    let error = client_for_mode("close_stdin_early", &fixture)
        .execute(&request_with_text(&payload))
        .expect_err("early close classifies from the exit status");
    assert!(
        matches!(error, ClientError::UnexpectedChild(_)),
        "BrokenPipe must not become Io, got {error:?}"
    );
    assert_reaped(&fixture);
}

#[cfg(unix)]
#[test]
fn aborting_child_reports_a_signal_and_is_reaped() {
    let fixture = Fixture::new();
    let error = client_for_mode("abort", &fixture)
        .execute(&request())
        .expect_err("abort is unexpected");
    let ClientError::UnexpectedChild(failure) = error else {
        panic!("expected UnexpectedChild, got {error:?}");
    };
    assert!(
        failure.status.signal.is_some(),
        "abort must surface a signal"
    );
    assert!(failure.status.exit_code.is_none());
    assert_reaped(&fixture);
}

#[test]
fn prefixed_invocation_records_generate_one_shot() {
    let fixture = Fixture::new();
    OneShotClient::at_path(stub_path())
        .with_prefix_arguments(["generate".into()])
        .with_env(EXPECT_ARGV_ENV, "generate,--one-shot")
        .with_env(ARGV_PATH_ENV, fixture.argv_path().to_string_lossy())
        .with_env(PID_PATH_ENV, fixture.pid_path().to_string_lossy())
        .execute(&request())
        .expect("prefixed stub response");
    assert_eq!(argv_tail(&fixture.read_argv()), ["generate", "--one-shot"]);
}
