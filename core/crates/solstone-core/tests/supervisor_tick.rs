// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_local::install::{archive, manifest, pins};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[path = "support/supervisor_guard.rs"]
mod supervisor_guard;

use supervisor_guard::SupervisorGuard;

#[allow(dead_code)]
#[path = "support/await_outcome.rs"]
mod await_outcome;

use await_outcome::{
    PollState, WaitMetrics, WaitOutcome, WaitPolarity, await_outcome, await_outcome_async,
};

struct TempJournal(PathBuf);

fn temporary_root() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/var/tmp")
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::temp_dir()
    }
}

impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = temporary_root().join(format!("solstone-core-supervisor-tick-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("journal config");
        Self(root)
    }

    fn enable_thinking(&self) {
        fs::write(
            self.0.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"local"}}}"#,
        )
        .expect("thinking config");
    }

    fn install_local_fixture_artifact(&self) {
        fs::write(
            self.0.join("config/journal.json"),
            br#"{"setup":{"completed_at":1},"transcribe":{"backend":"parakeet-cpp","parakeet-cpp":{"device":"cpu"}}}"#,
        )
        .expect("fixture journal config");
        let cache = pins::cache_root(&self.0);
        let runtime = cache.join("bin/aarch64-apple-darwin/b10068");
        let model = cache.join("models/local__qwen3.5-4b");
        fs::create_dir_all(&runtime).expect("runtime directory");
        fs::create_dir_all(&model).expect("model directory");
        fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").expect("runtime");
        archive::make_executable(&runtime.join("llama-server")).expect("executable runtime");
        fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").expect("model");
        fs::write(model.join("mmproj-F16.gguf"), b"projector").expect("projector");
        let runtime_manifest = manifest::build_manifest(
            "local",
            "llama-server-vulkan",
            "test",
            json!({"pin_identity":pins::vulkan_identity("aarch64-apple-darwin").unwrap()}),
            manifest::runtime_inventory(&runtime, &[]).unwrap(),
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(
            &manifest::artifact_manifest_path(&runtime),
            &runtime_manifest,
        )
        .unwrap();
        let model_manifest = manifest::build_manifest(
            "local",
            "local-model",
            "test",
            json!({"pin_identity":pins::model_identity("local/qwen3.5-4b").unwrap()}),
            manifest::inventory_for_tree(&model, "model").unwrap(),
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(&manifest::artifact_manifest_path(&model), &model_manifest)
            .unwrap();
    }

    fn write_local_port(&self, port: u16) {
        fs::create_dir_all(self.0.join("health")).expect("health directory");
        fs::write(self.0.join("health/local.port"), port.to_string()).expect("local port");
    }

    fn install_journal_stub(&self) -> PathBuf {
        let bin = self.0.join("test-bin");
        fs::create_dir_all(&bin).expect("journal stub directory");
        let marker = self.0.join("journal-stub-ran");
        let path = bin.join("journal");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nexec {} ready-sleep {} 30000\n",
                env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
                marker.display(),
            ),
        )
        .expect("journal stub");
        let mut permissions = fs::metadata(&path)
            .expect("journal stub metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("journal stub executable");
        marker
    }

    fn segment_dir(&self, day: &str, stream: Option<&str>, segment: &str) -> PathBuf {
        let day = self.0.join("chronicle").join(day);
        stream.map_or_else(
            || day.join(segment),
            |stream| day.join(stream).join(segment),
        )
    }
}
impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn panic_for_wait(context: &str, outcome: WaitOutcome) {
    match outcome {
        WaitOutcome::Passed(_) => {}
        WaitOutcome::Failed { reason, metrics } => {
            panic!("{context}: {reason}; {}", metrics.describe());
        }
        WaitOutcome::Inconclusive(metrics) => {
            panic!(
                "SUPERVISOR_RACE_INCONCLUSIVE {context}: {}",
                metrics.describe()
            );
        }
    }
}

fn start(journal: &TempJournal, cap_seconds: Option<u64>, extra_args: &[&str]) -> SupervisorGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .args(extra_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command.env(
        "SOLSTONE_LOCAL_BINARY",
        env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
    );
    command.env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1");
    command.env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1");
    command.env(
        "SOLSTONE_SUPERVISOR_APP_BINARY",
        env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
    );
    let journal_stub_dir = journal.0.join("test-bin");
    if journal_stub_dir.is_dir() {
        let path = std::env::var_os("PATH").into_iter().collect::<Vec<_>>();
        let path = std::env::join_paths(
            std::iter::once(journal_stub_dir).chain(path.iter().flat_map(std::env::split_paths)),
        )
        .expect("journal stub PATH");
        command.env("PATH", path);
    }
    if let Some(cap_seconds) = cap_seconds {
        command.env(
            "SOLSTONE_SUPERVISOR_TASK_CAP_SECONDS",
            cap_seconds.to_string(),
        );
    }
    SupervisorGuard::new(command.spawn().expect("supervisor starts"))
}

fn wait_for_socket(child: &mut SupervisorGuard, socket: &std::path::Path) {
    let ready = socket
        .parent()
        .expect("Callosum socket health directory")
        .join("supervisor.ready");
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(5),
        1_600,
        Instant::now,
        || {
            if socket.exists() && ready.exists() {
                PollState::Held
            } else {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        PollState::HardFail(format!("supervisor exited during boot: {status}"))
                    }
                    Ok(None) => PollState::Pending,
                    Err(error) => PollState::HardFail(format!("supervisor status: {error}")),
                }
            }
        },
        thread::sleep,
    );
    panic_for_wait("supervisor did not become ready", outcome);
}

async fn connect(
    socket: &std::path::Path,
) -> (
    tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let stream = UnixStream::connect(socket).await.expect("connect Callosum");
    let (read, write) = stream.into_split();
    (tokio::io::BufReader::new(read), write)
}

async fn send_request(
    write: &mut tokio::net::unix::OwnedWriteHalf,
    cmd: Vec<String>,
    reference: &str,
) {
    let line = serde_json::to_vec(&json!({
        "tract": "supervisor", "event": "request", "cmd": cmd, "ref": reference,
    }))
    .expect("request JSON");
    write.write_all(&line).await.expect("write request");
    write.write_all(b"\n").await.expect("frame request");
}

async fn send_message(write: &mut tokio::net::unix::OwnedWriteHalf, message: Value) {
    let line = serde_json::to_vec(&message).expect("message JSON");
    write.write_all(&line).await.expect("write message");
    write.write_all(b"\n").await.expect("frame message");
}

async fn receive_until(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    reference: &str,
    event: &str,
) -> Value {
    await_bounded_read(
        "expected Callosum event",
        Duration::from_millis(10),
        800,
        async {
            loop {
                let mut line = String::new();
                let bytes = reader
                    .read_line(&mut line)
                    .await
                    .expect("read Callosum frame");
                assert!(
                    bytes > 0,
                    "the connection closed before supervisor {event} event for {reference}"
                );
                let value: Value = serde_json::from_str(&line).expect("Callosum JSON");
                if value["tract"] == "supervisor"
                    && value["event"] == event
                    && value["ref"] == reference
                {
                    return value;
                }
            }
        },
    )
    .await
}

async fn receive_started_command(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    command: &[&str],
) -> Value {
    // The budget is deliberately generous because the journal fixture reaches
    // the test child through PATH and a shell exec; a tight budget here would
    // report a slow fork as a supervisor defect. This applies to every caller.
    await_bounded_read(
        "handler task start",
        Duration::from_millis(10),
        2_000,
        async {
            loop {
                let mut line = String::new();
                let bytes = reader.read_line(&mut line).await.expect("started frame");
                assert!(
                    bytes > 0,
                    "the connection closed before supervisor started frame"
                );
                let value: Value = serde_json::from_str(&line).expect("Callosum JSON");
                if value["tract"] == "supervisor"
                    && value["event"] == "started"
                    && value["cmd"] == json!(command)
                {
                    return value;
                }
            }
        },
    )
    .await
}

async fn wait_for_path(path: &Path) {
    let outcome = await_outcome_async(
        WaitPolarity::Positive,
        Duration::from_millis(10),
        800,
        Instant::now,
        || {
            if path.exists() {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        tokio::time::sleep,
    )
    .await;
    panic_for_wait("expected path", outcome);
}

async fn wait_for_logged_message(path: &Path, message: &Value) {
    let outcome = await_outcome_async(
        WaitPolarity::Positive,
        Duration::from_millis(10),
        800,
        Instant::now,
        || {
            if let Ok(contents) = fs::read_to_string(path)
                && let Some(line) = contents.lines().next()
                && let Ok(mut logged) = serde_json::from_str::<Value>(line)
                && logged["ts"].is_number()
            {
                logged.as_object_mut().expect("event object").remove("ts");
                if &logged == message {
                    return PollState::Held;
                }
            }
            PollState::Pending
        },
        tokio::time::sleep,
    )
    .await;
    panic_for_wait("expected logged event", outcome);
}

async fn wait_for_runtime_phase(path: &Path, phase: &str) -> Value {
    let mut found = None;
    let outcome = await_outcome_async(
        WaitPolarity::Positive,
        Duration::from_millis(10),
        800,
        Instant::now,
        || {
            if let Ok(bytes) = fs::read(path)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && value["phase"] == phase
            {
                found = Some(value);
                return PollState::Held;
            }
            PollState::Pending
        },
        tokio::time::sleep,
    )
    .await;
    panic_for_wait("provider runtime phase", outcome);
    found.expect("runtime phase recorded when wait passed")
}

async fn receive_status(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Value {
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await.expect("status frame");
        assert!(
            bytes > 0,
            "the connection closed before supervisor status event"
        );
        let value: Value = serde_json::from_str(&line).expect("status JSON");
        if value["tract"] == "supervisor" && value["event"] == "status" {
            return value;
        }
    }
}

async fn receive_status_with_service(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    service_name: &str,
) -> Value {
    loop {
        let status = receive_status(reader).await;
        if status["services"].as_array().is_some_and(|services| {
            services
                .iter()
                .any(|service| service["name"] == service_name)
        }) {
            return status;
        }
    }
}

async fn receive_timeout_history(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Value {
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await.expect("status frame");
        assert!(
            bytes > 0,
            "the connection closed before timeout-history status event"
        );
        let value: Value = serde_json::from_str(&line).expect("status JSON");
        if value["tract"] == "supervisor"
            && value["event"] == "status"
            && value["recent_tasks"].as_array().is_some_and(|tasks| {
                tasks
                    .iter()
                    .any(|task| task["ref"] == "ac11-task" && task["exit_status"] == "timeout")
            })
        {
            return value;
        }
    }
}

async fn receive_restarting(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Value {
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await.expect("restart frame");
        assert!(
            bytes > 0,
            "the connection closed before supervisor restarting event"
        );
        let value: Value = serde_json::from_str(&line).expect("restart JSON");
        if value["tract"] == "supervisor" && value["event"] == "restarting" {
            return value;
        }
    }
}

// A dilation-aware alternative to tokio::time::timeout for waits this file
// cannot express as a synchronous poll closure (an async socket read). It
// preserves each call site's exact interval*iterations budget and reuses
// WaitMetrics::dilation() so an expiry caused by scheduler starvation under
// check-rust-race's synthetic load reports SUPERVISOR_RACE_INCONCLUSIVE instead of an
// ordinary panic, while a genuine hang (dilation below the shared 1.10x
// threshold cited in docs/design/check-rust-race-classification.md) still reports FAILED.
async fn await_bounded_read<T>(
    context: &str,
    interval: Duration,
    iterations: usize,
    operation: impl Future<Output = T>,
) -> T {
    tokio::pin!(operation);
    let requested = interval.saturating_mul(iterations as u32);
    let started = Instant::now();
    for _ in 0..iterations {
        tokio::select! {
            value = &mut operation => return value,
            _ = tokio::time::sleep(interval) => {}
        }
    }
    panic_for_wait(
        context,
        wait_outcome_from_dilation(context, requested, started.elapsed()),
    );
    unreachable!("panic_for_wait always panics for a non-Passed outcome")
}

// Mirrors support/await_outcome.rs's own DILATION_NUMERATOR/DILATION_DENOMINATOR
// (11/10, cited in docs/design/check-rust-race-classification.md #1) without touching that
// trusted, untouched file: WaitTracker::is_dilated is private and not reusable.
fn wait_outcome_from_dilation(reason: &str, requested: Duration, slept: Duration) -> WaitOutcome {
    const DILATION_THRESHOLD: f64 = 11.0 / 10.0;
    let metrics = WaitMetrics { requested, slept };
    if metrics.dilation() >= DILATION_THRESHOLD {
        WaitOutcome::Inconclusive(metrics)
    } else {
        WaitOutcome::Failed {
            reason: format!("{reason} exhausted before completion"),
            metrics,
        }
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .expect("string panic")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_until_reports_closed_callosum_connection() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (reader, write) = connect(&socket).await;
    drop(write);
    let task = tokio::spawn(async move {
        let mut reader = reader;
        let _ = receive_until(&mut reader, "eof-reference", "started").await;
    });
    let panic = task
        .await
        .expect_err("reader must panic on EOF")
        .into_panic();
    assert!(
        panic_message(panic)
            .contains("the connection closed before supervisor started event for eof-reference")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receive_started_command_reports_closed_callosum_connection() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (reader, write) = connect(&socket).await;
    drop(write);
    let task = tokio::spawn(async move {
        let mut reader = reader;
        let _ = receive_started_command(&mut reader, &["journal", "think"]).await;
    });
    let panic = task
        .await
        .expect_err("reader must panic on EOF")
        .into_panic();
    assert!(panic_message(panic).contains("the connection closed before supervisor started frame"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reader_reports_closed_callosum_connection() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (reader, write) = connect(&socket).await;
    drop(write);
    let task = tokio::spawn(async move {
        let mut reader = reader;
        let _ = await_bounded_read(
            "supervisor status event",
            Duration::from_millis(10),
            800,
            receive_status(&mut reader),
        )
        .await;
    });
    let panic = task
        .await
        .expect_err("reader must panic on EOF")
        .into_panic();
    assert!(panic_message(panic).contains("the connection closed before supervisor status event"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timeout_history_reader_reports_closed_callosum_connection() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (reader, write) = connect(&socket).await;
    drop(write);
    let task = tokio::spawn(async move {
        let mut reader = reader;
        let _ = await_bounded_read(
            "timeout-history status event",
            Duration::from_millis(10),
            800,
            receive_timeout_history(&mut reader),
        )
        .await;
    });
    let panic = task
        .await
        .expect_err("reader must panic on EOF")
        .into_panic();
    assert!(
        panic_message(panic).contains("the connection closed before timeout-history status event")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarting_reader_reports_closed_callosum_connection() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (reader, write) = connect(&socket).await;
    drop(write);
    let task = tokio::spawn(async move {
        let mut reader = reader;
        let _ = await_bounded_read(
            "restarting notification",
            Duration::from_millis(10),
            800,
            receive_restarting(&mut reader),
        )
        .await;
    });
    let panic = task
        .await
        .expect_err("reader must panic on EOF")
        .into_panic();
    assert!(
        panic_message(panic).contains("the connection closed before supervisor restarting event")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac9_real_task_over_real_socket_runs_and_reports_back() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;
    send_request(
        &mut write,
        vec![
            env!("CARGO_BIN_EXE_solstone-core-system-test-child").into(),
            "lines".into(),
        ],
        "ac9-task",
    )
    .await;
    let _ = receive_until(&mut reader, "ac9-task", "started").await;
    let stopped = receive_until(&mut reader, "ac9-task", "stopped").await;
    assert_eq!(stopped["exit_code"], json!(0));
}

#[test]
fn ac10_due_schedule_entry_runs_through_real_engine() {
    let journal = TempJournal::new();
    fs::write(
        journal.0.join("config/schedules.json"),
        serde_json::to_vec(&json!({"ac10": {
            "cmd": [env!("CARGO_BIN_EXE_solstone-core-system-test-child"), "lines"],
            "every": "1m"
        }}))
        .expect("schedule JSON"),
    )
    .expect("write schedule");
    let mut child = start(&journal, None, &["--no-daily"]);
    wait_for_socket(&mut child, &journal.0.join("health/callosum.sock"));
    let scheduler = journal.0.join("health/scheduler.json");
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(10),
        800,
        Instant::now,
        || {
            if let Ok(bytes) = fs::read(&scheduler)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && value["ac10"]["last_status"] == "ok"
            {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        thread::sleep,
    );
    panic_for_wait("scheduled work did not write completion state", outcome);
}

#[test]
fn ac3_no_schedule_skips_invalid_schedule_state_and_reaches_readiness() {
    let ordinary = TempJournal::new();
    fs::write(
        ordinary.0.join("config/schedules.json"),
        serde_json::to_vec(&json!({"ac3": {
            "cmd": [env!("CARGO_BIN_EXE_solstone-core-system-test-child"), "lines"],
            "every": "1m"
        }}))
        .expect("schedule JSON"),
    )
    .expect("write schedule");
    let ordinary_health = ordinary.0.join("health");
    fs::create_dir_all(&ordinary_health).expect("health directory");
    let ordinary_scheduler = ordinary_health.join("scheduler.json");
    fs::write(&ordinary_scheduler, b"[]").expect("invalid schedule state");

    let mut ordinary_child = start(&ordinary, None, &[]);
    let ordinary_ready = ordinary_health.join("supervisor.ready");
    let mut ordinary_exit = None;
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(5),
        1_600,
        Instant::now,
        || match ordinary_child.try_wait() {
            Ok(Some(status)) => {
                ordinary_exit = Some(status);
                PollState::Held
            }
            Ok(None) if ordinary_ready.exists() => {
                PollState::HardFail("ordinary supervisor reached readiness".to_owned())
            }
            Ok(None) => PollState::Pending,
            Err(error) => PollState::HardFail(format!("supervisor status: {error}")),
        },
        thread::sleep,
    );
    panic_for_wait(
        "ordinary supervisor did not exit for invalid schedule state",
        outcome,
    );
    assert_eq!(
        ordinary_exit.expect("ordinary supervisor exit").code(),
        Some(75)
    );
    assert!(!ordinary_ready.exists());
    assert_eq!(
        fs::read(&ordinary_scheduler).expect("ordinary schedule state"),
        b"[]"
    );

    let disabled = TempJournal::new();
    let scheduled_marker = disabled.0.join("scheduled-marker");
    fs::write(
        disabled.0.join("config/schedules.json"),
        serde_json::to_vec(&json!({"ac3": {
            "cmd": [
                env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
                "ready-sleep",
                scheduled_marker,
                "30000"
            ],
            "every": "1m"
        }}))
        .expect("schedule JSON"),
    )
    .expect("write schedule");
    let disabled_health = disabled.0.join("health");
    fs::create_dir_all(&disabled_health).expect("health directory");
    let disabled_scheduler = disabled_health.join("scheduler.json");
    fs::write(&disabled_scheduler, b"[]").expect("invalid schedule state");

    let mut disabled_child = start(&disabled, None, &["--no-daily", "--no-schedule"]);
    wait_for_socket(&mut disabled_child, &disabled_health.join("callosum.sock"));
    assert_eq!(
        fs::read(&disabled_scheduler).expect("disabled schedule state"),
        b"[]"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        assert!(
            !scheduled_marker.exists(),
            "--no-schedule ran a scheduled command"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !scheduled_marker.exists(),
        "--no-schedule ran a scheduled command"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac4_no_schedule_preserves_inbound_scheduler_name_without_state_file() {
    let journal = TempJournal::new();
    let scheduler = journal.0.join("health/scheduler.json");
    assert!(!scheduler.exists());

    let mut child = start(&journal, None, &["--no-schedule"]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    assert!(!scheduler.exists());

    let (mut reader, mut write) = connect(&socket).await;
    send_message(
        &mut write,
        json!({
            "tract": "supervisor",
            "event": "request",
            "cmd": [env!("CARGO_BIN_EXE_solstone-core-system-test-child"), "lines"],
            "ref": "ac4-task",
            "scheduler_name": "ac4-schedule"
        }),
    )
    .await;
    let _ = receive_until(&mut reader, "ac4-task", "started").await;
    let stopped = receive_until(&mut reader, "ac4-task", "stopped").await;
    assert_eq!(stopped["exit_code"], json!(0));

    let status = await_bounded_read(
        "status retaining inbound scheduler name",
        Duration::from_millis(10),
        800,
        async {
            loop {
                let status = receive_status(&mut reader).await;
                if status["recent_tasks"].as_array().is_some_and(|tasks| {
                    tasks.iter().any(|task| {
                        task["ref"] == "ac4-task" && task["scheduler_name"] == "ac4-schedule"
                    })
                }) {
                    return status;
                }
            }
        },
    )
    .await;
    assert_eq!(status["schedules"], json!([]));
    assert!(!scheduler.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac13_status_projects_live_provider_and_schedule_state() {
    let journal = TempJournal::new();
    fs::write(
        journal.0.join("config/schedules.json"),
        serde_json::to_vec(&json!({"ac13": {
            "cmd": [env!("CARGO_BIN_EXE_solstone-core-system-test-child"), "lines"],
            "every": "1m"
        }}))
        .expect("schedule JSON"),
    )
    .expect("write schedule");
    journal.install_local_fixture_artifact();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, _write) = connect(&socket).await;
    let status = await_bounded_read(
        "supervisor status event",
        Duration::from_millis(10),
        800,
        receive_status_with_service(&mut reader, "local"),
    )
    .await;
    let names = status["services"]
        .as_array()
        .expect("service projection")
        .iter()
        .filter_map(|service| service["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"local"));
    assert!(
        status["schedules"]
            .as_array()
            .expect("schedule projection")
            .iter()
            .any(|schedule| schedule["name"] == "ac13")
    );
    assert!(status["crashed"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac11_capped_task_is_terminated_with_timeout_exit() {
    let journal = TempJournal::new();
    let ready = journal.0.join("task-ready");
    let mut child = start(&journal, Some(1), &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;
    send_request(
        &mut write,
        vec![
            env!("CARGO_BIN_EXE_solstone-core-system-test-child").into(),
            "ready-sleep".into(),
            ready.display().to_string(),
            "10000".into(),
        ],
        "ac11-task",
    )
    .await;
    let _ = receive_until(&mut reader, "ac11-task", "started").await;
    // Spawning the child and having it write its marker is scaffolding, not the
    // behaviour under test, so this budget is deliberately generous: the old
    // 3 s window failed intermittently on an idle machine — reproduced once in
    // ~10 runs, on the first run after a cold build — while passing 6/6 in
    // isolation. A tight budget here reports a slow fork as a supervisor defect.
    let outcome = await_outcome_async(
        WaitPolarity::Positive,
        Duration::from_millis(10),
        3_000,
        Instant::now,
        || {
            if ready.exists() {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        tokio::time::sleep,
    )
    .await;
    panic_for_wait("task process really started", outcome);
    let stopped = receive_until(&mut reader, "ac11-task", "stopped").await;
    assert_eq!(
        stopped["exit_code"],
        json!(-15),
        "deadline termination is surfaced as timeout exit"
    );
    let status = await_bounded_read(
        "timeout-history status event",
        Duration::from_millis(10),
        800,
        receive_timeout_history(&mut reader),
    )
    .await;
    assert!(status["recent_tasks"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_message_submits_live_segment_think_over_socket() {
    let journal = TempJournal::new();
    journal.enable_thinking();
    let _stub_marker = journal.install_journal_stub();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;

    send_message(
        &mut write,
        json!({
            "tract": "observe", "event": "observed", "day": "20260102",
            "segment": "120000_60", "stream": "camera"
        }),
    )
    .await;

    let started = receive_started_command(
        &mut reader,
        &[
            "journal",
            "think",
            "-v",
            "--day",
            "20260102",
            "--segment",
            "120000_60",
            "--stream",
            "camera",
            "--live",
        ],
    )
    .await;
    assert_eq!(started["service"], "segment");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_observed_message_does_not_run_the_journal_stub() {
    let journal = TempJournal::new();
    journal.enable_thinking();
    let stub_marker = journal.install_journal_stub();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (_reader, mut write) = connect(&socket).await;

    send_message(
        &mut write,
        json!({
            "tract": "observe", "event": "observed", "day": "20260102",
            "segment": "120000_60", "batch": true
        }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(!stub_marker.exists(), "batch observation submitted a task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_recorded_submits_activity_think_over_socket() {
    let journal = TempJournal::new();
    journal.enable_thinking();
    let _stub_marker = journal.install_journal_stub();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;

    send_message(
        &mut write,
        json!({
            "tract": "activity", "event": "recorded", "id": "activity-1",
            "facet": "work", "day": "20260102"
        }),
    )
    .await;

    let started = receive_started_command(
        &mut reader,
        &[
            "journal",
            "think",
            "--activity",
            "activity-1",
            "--facet",
            "work",
            "--day",
            "20260102",
        ],
    )
    .await;
    assert_eq!(started["service"], "activity");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daily_complete_submits_heartbeat_when_pid_file_is_absent() {
    let journal = TempJournal::new();
    let _stub_marker = journal.install_journal_stub();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;

    send_message(
        &mut write,
        json!({"tract": "think", "event": "daily_complete"}),
    )
    .await;

    let started = receive_started_command(&mut reader, &["journal", "heartbeat"]).await;
    assert_eq!(started["service"], "heartbeat");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_message_forces_day_think_over_socket() {
    let journal = TempJournal::new();
    journal.enable_thinking();
    let _stub_marker = journal.install_journal_stub();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;

    send_message(
        &mut write,
        json!({"tract": "supervisor", "event": "drain", "day": "20260102"}),
    )
    .await;

    let started = receive_started_command(
        &mut reader,
        &["journal", "think", "-v", "--day", "20260102"],
    )
    .await;
    assert_eq!(started["service"], "daily");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_message_restarts_convey_fixture() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let marker = journal.0.join("health/fixture-convey.marker");
    wait_for_path(&marker).await;
    let previous_pid = fs::read_to_string(&marker)
        .expect("convey marker")
        .trim()
        .rsplit(':')
        .next()
        .expect("convey pid")
        .parse::<u32>()
        .expect("numeric convey pid");
    let (mut reader, mut write) = connect(&socket).await;

    send_message(
        &mut write,
        json!({"tract": "supervisor", "event": "restart", "service": "convey"}),
    )
    .await;
    let restarting = await_bounded_read(
        "restarting notification",
        Duration::from_millis(10),
        800,
        receive_restarting(&mut reader),
    )
    .await;
    assert_eq!(restarting["service"], "convey");
    assert_eq!(restarting["pid"], previous_pid);

    let mut replacement = None;
    let outcome = await_outcome_async(
        WaitPolarity::Positive,
        Duration::from_millis(10),
        800,
        Instant::now,
        || {
            let current = fs::read_to_string(&marker).expect("convey marker");
            let pid = current
                .trim()
                .rsplit(':')
                .next()
                .expect("convey pid")
                .parse::<u32>()
                .expect("numeric convey pid");
            if pid != previous_pid {
                replacement = Some(pid);
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        tokio::time::sleep,
    )
    .await;
    panic_for_wait("replacement convey process", outcome);
    replacement.expect("replacement PID recorded when wait passed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segment_events_log_appends_existing_stream_segment() {
    let journal = TempJournal::new();
    let segment = journal.segment_dir("20260102", Some("camera"), "120000_60");
    fs::create_dir_all(&segment).expect("segment directory");
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (_reader, mut write) = connect(&socket).await;
    let message = json!({
        "tract": "think", "event": "finished", "day": "20260102",
        "segment": "120000_60", "stream": "camera", "detail": {"count": 1}
    });

    send_message(&mut write, message.clone()).await;
    let path = segment.join("events.jsonl");
    wait_for_logged_message(&path, &message).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segment_events_log_does_not_materialize_missing_segment() {
    let journal = TempJournal::new();
    let segment = journal.segment_dir("20260102", None, "120000_60");
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (_reader, mut write) = connect(&socket).await;

    send_message(
        &mut write,
        json!({
            "tract": "activity", "event": "recorded", "day": "20260102",
            "segment": "120000_60"
        }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(!segment.exists());
    assert!(child.try_wait().expect("supervisor status").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cortex_unavailable_threshold_recycles_bundled_local_runtime() {
    let journal = TempJournal::new();
    // The supervisor's local fixture is the existing synthetic healthy-probe
    // seam; this port is data for the recycle record, not a bound TCP listener.
    journal.write_local_port(4312);
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (_reader, mut write) = connect(&socket).await;

    for use_id in ["cortex-one", "cortex-two", "cortex-three"] {
        send_message(
            &mut write,
            json!({"tract": "cortex", "event": "start", "use_id": use_id, "provider": "local"}),
        )
        .await;
    }
    for use_id in ["cortex-one", "cortex-two", "cortex-three"] {
        send_message(
            &mut write,
            json!({"tract": "cortex", "event": "error", "use_id": use_id, "reason_code": "provider_unavailable"}),
        )
        .await;
    }

    let runtime = wait_for_runtime_phase(
        &journal.0.join("health/providers/runtime/local.json"),
        "retry-requested",
    )
    .await;
    assert!(
        runtime["generation"]
            .as_u64()
            .is_some_and(|generation| generation >= 1)
    );
    assert_eq!(runtime["reason_code"], "local-wedge-provider-unavailable");
    assert_eq!(
        runtime["detail"]["use_ids"],
        json!(["cortex-one", "cortex-three", "cortex-two"])
    );
    assert_eq!(runtime["detail"]["port"], 4312);
    assert_eq!(runtime["detail"]["health_state"], "ready");
    assert!(runtime["detail"]["token_revision"].as_u64().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cortex_finish_resets_wedge_failures_before_the_threshold() {
    let journal = TempJournal::new();
    journal.write_local_port(4312);
    let mut child = start(&journal, None, &[]);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (_reader, mut write) = connect(&socket).await;

    for use_id in ["cortex-one", "cortex-two", "cortex-three"] {
        send_message(
            &mut write,
            json!({"tract": "cortex", "event": "start", "use_id": use_id, "provider": "local"}),
        )
        .await;
    }
    for use_id in ["cortex-one", "cortex-two"] {
        send_message(
            &mut write,
            json!({"tract": "cortex", "event": "error", "use_id": use_id, "reason_code": "provider_unavailable"}),
        )
        .await;
    }
    send_message(
        &mut write,
        json!({"tract": "cortex", "event": "finish", "use_id": "cortex-one"}),
    )
    .await;
    send_message(
        &mut write,
        json!({"tract": "cortex", "event": "error", "use_id": "cortex-three", "reason_code": "provider_unavailable"}),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    let path = journal.0.join("health/providers/runtime/local.json");
    let runtime = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    assert_ne!(
        runtime.as_ref().map(|value| &value["phase"]),
        Some(&json!("retry-requested")),
        "finish must clear the first two failures before the third error"
    );
}
