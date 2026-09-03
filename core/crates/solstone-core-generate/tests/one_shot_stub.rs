// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient, STDERR_LIMIT,
    STDOUT_LIMIT,
};

mod support;

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
fn copy_payload_file(fixture: &Fixture, relative: &str) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let core = manifest_dir
        .ancestors()
        .nth(2)
        .expect("core directory")
        .to_path_buf();
    let source = core.join("payload").join(relative);
    let destination = fixture.path.join("share").join(relative);
    fs::create_dir_all(destination.parent().expect("payload parent"))
        .expect("payload parent creates");
    fs::copy(&source, &destination).unwrap_or_else(|error| {
        panic!(
            "copy fixture payload {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
}

#[cfg(unix)]
fn install_talent_worker_wrapper(fixture: &Fixture) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    for relative in [
        "solstone/talent/journal/contract/bundle.json",
        "solstone/think/contract/layout.json",
        "solstone/think/templates/segment_preamble.md",
        "solstone/apps/timeline/talent/segment_summary.md",
        "solstone/apps/timeline/talent/segment_summary.schema.json",
    ] {
        copy_payload_file(fixture, relative);
    }

    let bin = fixture.path.join("bin");
    fs::create_dir_all(&bin).expect("worker binary directory creates");
    let real = bin.join("solstone-core-real");
    fs::copy(support::core_binary(), &real).expect("real core binary copies");
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700))
        .expect("real core binary becomes executable");

    let wrapper = bin.join("solstone-core");
    fs::write(
        &wrapper,
        format!(
            r##"#!/bin/sh
if [ "$1" = "generate" ] && [ "$2" = "--one-shot" ]; then
    cat > "$SOLSTONE_JOURNAL/generate-request.json"
    case "$(cat "$SOLSTONE_JOURNAL/config/journal.json")" in
        *'"provider":"local"'*)
            printf '%s\n' '{{"schema":"solstone-generate-response-v2","id":null,"outcome":"generated","text":"{{\"title\":\"Local\",\"description\":\"Local provider fixture.\"}}","model":"local/fixture","usage":{{}},"finish_reason":"stop","thinking":null,"schema_validation":{{"valid":true}},"input_budget":null,"request_budget":null,"inference":{{"reason_code":"manifest-missing","runtime_reason_code":"manifest-missing"}}}}'
            ;;
        *)
            printf '%s\n' '{{"schema":"solstone-generate-response-v2","id":null,"outcome":"generated","text":"{{\"title\":\"BYO\",\"description\":\"Google provider fixture.\"}}","model":"google/gemini-fixture","usage":{{"input_tokens":1}},"finish_reason":"stop","thinking":null,"schema_validation":{{"valid":true}},"input_budget":null,"request_budget":null,"inference":{{"provider":"google"}}}}'
            ;;
    esac
    exit 0
fi
exec '{}' "$@"
"##,
            real.display()
        ),
    )
    .expect("worker wrapper writes");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700))
        .expect("worker wrapper becomes executable");
    wrapper
}

#[cfg(unix)]
fn write_timeline_worker_journal(root: &std::path::Path, provider: &str, model: &str) {
    let segment = root.join("chronicle/20260401/080000_300");
    fs::create_dir_all(segment.join("talents")).expect("activity parent creates");
    fs::write(
        segment.join("talents/activity.md"),
        "Completed fixture work.\n",
    )
    .expect("activity writes");
    fs::create_dir_all(root.join("config")).expect("journal config parent creates");
    fs::write(
        root.join("config/journal.json"),
        serde_json::to_vec(&json!({"providers":{"active":{"provider":provider,"model":model}}}))
            .expect("journal config serializes"),
    )
    .expect("journal config writes");
}

#[cfg(unix)]
fn launch_timeline_worker(
    program: &std::path::Path,
    journal: &std::path::Path,
) -> std::process::Child {
    let mut child = Command::new(program)
        .arg("__talent-worker")
        .env("SOLSTONE_JOURNAL", journal)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("real talent worker starts");
    child
        .stdin
        .as_mut()
        .expect("worker stdin")
        .write_all(
            br#"{"name":"timeline:segment_summary","day":"20260401","segment":"080000_300"}
"#,
        )
        .expect("worker request writes");
    child
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
    assert!(!failure.stdin_closed_early);
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
fn successful_malformed_stdout_preserves_completed_process_evidence() {
    let fixture = Fixture::new();
    let error = client_for_mode("success_malformed", &fixture)
        .execute(&request())
        .expect_err("malformed success is decode");
    let ClientError::InvalidResponse(failure) = error else {
        panic!("expected InvalidResponse, got {error:?}");
    };
    assert_eq!(failure.status.exit_code, Some(0));
    assert_eq!(failure.stdout.bytes, b"not-json\n");
    assert_eq!(failure.stderr.bytes, b"malformed diagnostic");
    assert!(!failure.stdin_closed_early);
    assert_reaped(&fixture);
}

#[test]
fn successful_oversized_stdout_preserves_cap_and_completed_process_evidence() {
    let fixture = Fixture::new();
    let error = client_for_mode("success_oversized", &fixture)
        .execute(&request())
        .expect_err("oversized success is decode");
    let ClientError::InvalidResponse(failure) = error else {
        panic!("expected InvalidResponse, got {error:?}");
    };
    assert!(
        failure.detail.contains(&STDOUT_LIMIT.to_string()),
        "decode names the cap: {}",
        failure.detail
    );
    assert_eq!(failure.status.exit_code, Some(0));
    assert_eq!(failure.stdout.bytes.len(), STDOUT_LIMIT);
    assert!(failure.stdout.truncated);
    assert_eq!(failure.stderr.bytes, b"oversized diagnostic");
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
    let ClientError::UnexpectedChild(failure) = error else {
        panic!("BrokenPipe must not become Io, got {error:?}");
    };
    assert!(
        failure.stdin_closed_early,
        "large request must exercise the write-side early-close branch"
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

#[cfg(unix)]
#[test]
fn concurrent_real_workers_keep_byo_timeline_provenance_separate_from_local_runtime_codes() {
    let fixture = Fixture::new();
    let worker = install_talent_worker_wrapper(&fixture);
    let byo_journal = fixture.path.join("byo-journal");
    let local_journal = fixture.path.join("local-journal");
    write_timeline_worker_journal(&byo_journal, "google", "google/configured-model");
    write_timeline_worker_journal(&local_journal, "local", "local/fixture");

    let local = launch_timeline_worker(&worker, &local_journal);
    let byo = launch_timeline_worker(&worker, &byo_journal);
    let local_output = local.wait_with_output().expect("local worker waits");
    let byo_output = byo.wait_with_output().expect("BYO worker waits");
    assert!(
        local_output.status.success(),
        "local worker stderr: {}",
        String::from_utf8_lossy(&local_output.stderr)
    );
    assert!(
        byo_output.status.success(),
        "BYO worker stderr: {}",
        String::from_utf8_lossy(&byo_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&byo_output.stdout).contains("\"event\":\"finish\""),
        "BYO worker did not finish: {}",
        String::from_utf8_lossy(&byo_output.stdout)
    );

    let timeline: serde_json::Value = serde_json::from_slice(
        &fs::read(byo_journal.join("chronicle/20260401/080000_300/timeline.json"))
            .expect("BYO timeline artifact reads"),
    )
    .expect("BYO timeline artifact parses");
    assert_eq!(timeline["schema_version"], 1, "timeline: {timeline}");
    assert_eq!(timeline["provenance"]["model"], "google/gemini-fixture");
    assert_ne!(
        timeline["provenance"]["model"], "google/configured-model",
        "the persisted model must come from the BYO response, not configuration"
    );
    assert_eq!(
        timeline["provenance"]["inference"],
        json!({"provider":"google"})
    );
    let generate_request: serde_json::Value = serde_json::from_slice(
        &fs::read(byo_journal.join("generate-request.json")).expect("BYO generate request reads"),
    )
    .expect("BYO generate request parses");
    assert_eq!(generate_request["thinking_budget"], 0);
    assert_eq!(generate_request["max_output_tokens"], 1024);

    let provenance = timeline["provenance"].to_string();
    assert!(
        !solstone_core_system::provider_runtime::KNOWN_REASON_CODES
            .iter()
            .any(|code| provenance.contains(code)),
        "BYO provenance must not contain a local-provider runtime reason code: {provenance}"
    );
}
