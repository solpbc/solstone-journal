// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

#[path = "support/supervisor_guard.rs"]
mod supervisor_guard;

use supervisor_guard::SupervisorGuard;

#[allow(dead_code)]
#[path = "support/await_outcome.rs"]
mod await_outcome;

use await_outcome::{PollState, WaitOutcome, WaitPolarity, await_outcome, await_outcome_async};

struct TempJournal(PathBuf);
impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-supervisor-shutdown-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("journal config");
        Self(root)
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

fn start(journal: &TempJournal) -> SupervisorGuard {
    SupervisorGuard::new(
        Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(["supervisor", "--journal"])
            .arg(&journal.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env(
                "SOLSTONE_LOCAL_BINARY",
                env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
            )
            .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
            .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
            .env(
                "SOLSTONE_SUPERVISOR_APP_BINARY",
                env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
            )
            .spawn()
            .expect("supervisor starts"),
    )
}
fn wait_for(path: &std::path::Path, child: &mut SupervisorGuard) {
    // The full macOS suite can delay fixture-supervisor startup while native
    // test processes are exiting. Keep a five-second readiness proof with a
    // lower polling rate so that delay does not become inconclusive evidence.
    #[cfg(target_os = "macos")]
    let (interval, iterations) = (Duration::from_millis(100), 50);
    #[cfg(not(target_os = "macos"))]
    let (interval, iterations) = (Duration::from_millis(5), 500);
    let outcome = await_outcome(
        WaitPolarity::Positive,
        interval,
        iterations,
        Instant::now,
        || {
            if path.exists() {
                PollState::Held
            } else {
                match child.try_wait() {
                    Ok(Some(status)) => PollState::HardFail(format!("supervisor exited: {status}")),
                    Ok(None) => PollState::Pending,
                    Err(error) => PollState::HardFail(format!("supervisor status: {error}")),
                }
            }
        },
        thread::sleep,
    );
    panic_for_wait(&format!("{} did not appear", path.display()), outcome);
}

async fn receive_started(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    reference: &str,
) -> i32 {
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).await.expect("event line");
        assert!(
            bytes > 0,
            "the connection closed before supervisor started event for {reference}"
        );
        let value: Value = serde_json::from_str(&line).expect("event JSON");
        if value["tract"] == "supervisor"
            && value["event"] == "started"
            && value["ref"] == reference
        {
            return value["pid"].as_i64().expect("process event pid") as i32;
        }
    }
}

async fn request_and_started(socket: &std::path::Path, cmd: Vec<String>, reference: &str) -> i32 {
    let stream = UnixStream::connect(socket).await.expect("connect Callosum");
    let (read, mut write) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(read);
    let line = serde_json::to_vec(
        &json!({"tract":"supervisor","event":"request","cmd":cmd,"ref":reference}),
    )
    .expect("request JSON");
    write.write_all(&line).await.expect("request");
    write.write_all(b"\n").await.expect("frame");
    timeout(
        Duration::from_secs(8),
        receive_started(&mut reader, reference),
    )
    .await
    .expect("started event")
}
fn foreign_heartbeat(journal: &TempJournal) {
    let sync = journal.0.join("health/sync");
    fs::create_dir_all(&sync).expect("sync directory");
    fs::write(sync.join("foreign-host.check"), format!(
        r#"{{"schema":1,"machine_id":"foreign-machine","hostname":"foreign-host","pid":4242,"wall_time":"now","solstone_version":"1","interval_seconds":15,"journal_path":"{}"}}"#, journal.0.display()
    )).expect("foreign heartbeat");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac14_shutdown_clears_lifecycle_in_order_and_reaps_task_child() {
    let journal = TempJournal::new();
    let ready_path = journal.0.join("task-ready");
    let mut child = start(&journal);
    let socket = journal.0.join("health/callosum.sock");
    let ready = journal.0.join("health/supervisor.ready");
    let pid_file = journal.0.join("health/supervisor.pid");
    wait_for(&ready, &mut child);
    let task_pid = request_and_started(
        &socket,
        vec![
            env!("CARGO_BIN_EXE_solstone-core-system-test-child").into(),
            "ready-sleep".into(),
            ready_path.display().to_string(),
            "30000".into(),
        ],
        "ac14-task",
    )
    .await;
    let outcome = await_outcome_async(
        WaitPolarity::Positive,
        Duration::from_millis(10),
        300,
        Instant::now,
        || {
            if ready_path.exists() {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        tokio::time::sleep,
    )
    .await;
    panic_for_wait("task child did not reach its ready point", outcome);
    assert!(ready_path.exists(), "task child reached its ready point");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .expect("signal supervisor");
    let mut ready_removed = None;
    let mut pid_removed = None;
    let mut socket_removed = None;
    let mut status = None;
    let mut tick = 0;
    #[cfg(target_os = "macos")]
    let (shutdown_interval, shutdown_iterations) = (Duration::from_millis(100), 300);
    #[cfg(not(target_os = "macos"))]
    let (shutdown_interval, shutdown_iterations) = (Duration::from_millis(5), 6_000);
    let outcome = await_outcome_async(
        WaitPolarity::Positive,
        shutdown_interval,
        shutdown_iterations,
        Instant::now,
        || {
            let current_tick = tick;
            tick += 1;
            if ready_removed.is_none() && !ready.exists() {
                ready_removed = Some(current_tick);
            }
            if pid_removed.is_none() && !pid_file.exists() {
                pid_removed = Some(current_tick);
            }
            if socket_removed.is_none() && !socket.exists() {
                socket_removed = Some(current_tick);
            }
            match child.try_wait() {
                Ok(Some(exited)) => {
                    status = Some(exited);
                    if ready_removed.is_some() && pid_removed.is_some() && socket_removed.is_some()
                    {
                        PollState::Held
                    } else {
                        PollState::Pending
                    }
                }
                Ok(None) => PollState::Pending,
                Err(error) => PollState::HardFail(format!("supervisor status: {error}")),
            }
        },
        tokio::time::sleep,
    )
    .await;
    panic_for_wait("supervisor did not exit after SIGTERM", outcome);
    let status = status.expect("supervisor exit status");
    assert!(status.success(), "standard shutdown exits cleanly");
    assert!(ready_removed.expect("ready marker removed") <= pid_removed.expect("pid removed"));
    assert!(pid_removed.expect("pid removed") <= socket_removed.expect("socket removed"));
    assert!(!ready.exists(), "readiness clears before teardown");
    assert!(!pid_file.exists(), "normal shutdown clears identity");
    assert!(!socket.exists(), "bus is joined after child shutdown");
    assert!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(task_pid), None).is_err(),
        "task child was reaped"
    );
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
async fn request_and_started_reports_closed_callosum_connection() {
    let journal = TempJournal::new();
    let mut child = start(&journal);
    let socket = journal.0.join("health/callosum.sock");
    wait_for(&journal.0.join("health/supervisor.ready"), &mut child);
    let stream = UnixStream::connect(&socket)
        .await
        .expect("connect Callosum");
    let (read, write) = stream.into_split();
    drop(write);
    let task = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(read);
        let _ = timeout(
            Duration::from_secs(8),
            receive_started(&mut reader, "eof-reference"),
        )
        .await
        .expect("started event");
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

#[test]
fn ac15_mid_tick_sync_conflict_exits_2_keeps_own_identity_and_heartbeat() {
    let journal = TempJournal::new();
    let mut child = start(&journal);
    let ready = journal.0.join("health/supervisor.ready");
    wait_for(&ready, &mut child);
    // The first immediate sync pass needs to establish its baseline before a
    // foreign writer can be classified as a mid-tick conflict.
    thread::sleep(Duration::from_secs(2));
    foreign_heartbeat(&journal);
    let status = child.wait().expect("wait sync conflict");
    assert_eq!(status.code(), Some(2));
    assert!(journal.0.join("health/supervisor.pid").exists());
    assert!(!ready.exists());
    let sync = fs::read_dir(journal.0.join("health/sync")).expect("sync directory");
    assert!(
        sync.filter_map(Result::ok)
            .any(|entry| entry.file_name() != "foreign-host.check")
    );
}
