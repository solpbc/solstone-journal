// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::timeout;

struct TempJournal(PathBuf);
impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-supervisor-tick-{stamp}"));
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
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn start(journal: &TempJournal, cap_seconds: Option<u64>) -> ChildGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command.env(
        "SOLSTONE_LOCAL_BINARY",
        env!("CARGO_BIN_EXE_solstone-system-test-child"),
    );
    command.env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1");
    if let Some(cap_seconds) = cap_seconds {
        command.env(
            "SOLSTONE_SUPERVISOR_TASK_CAP_SECONDS",
            cap_seconds.to_string(),
        );
    }
    ChildGuard(command.spawn().expect("supervisor starts"))
}

fn wait_for_socket(child: &mut ChildGuard, socket: &std::path::Path) {
    for _ in 0..400 {
        if socket.exists() {
            return;
        }
        if let Some(status) = child.0.try_wait().expect("supervisor status") {
            panic!("supervisor exited during boot: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("socket did not appear");
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

async fn receive_until(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    reference: &str,
    event: &str,
) -> Value {
    timeout(Duration::from_secs(8), async {
        loop {
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .await
                .expect("read Callosum frame");
            let value: Value = serde_json::from_str(&line).expect("Callosum JSON");
            if value["tract"] == "supervisor"
                && value["event"] == event
                && value["ref"] == reference
            {
                return value;
            }
        }
    })
    .await
    .expect("expected Callosum event")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac9_real_task_over_real_socket_runs_and_reports_back() {
    let journal = TempJournal::new();
    let mut child = start(&journal, None);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;
    send_request(
        &mut write,
        vec![
            env!("CARGO_BIN_EXE_solstone-system-test-child").into(),
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
            "cmd": [env!("CARGO_BIN_EXE_solstone-system-test-child"), "lines"],
            "every": "1m"
        }}))
        .expect("schedule JSON"),
    )
    .expect("write schedule");
    let mut child = start(&journal, None);
    wait_for_socket(&mut child, &journal.0.join("health/callosum.sock"));
    let scheduler = journal.0.join("health/scheduler.json");
    for _ in 0..800 {
        if let Ok(bytes) = fs::read(&scheduler)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && value["ac10"]["last_status"] == "ok"
        {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("scheduled work did not write completion state");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac13_status_projects_live_provider_and_schedule_state() {
    let journal = TempJournal::new();
    fs::write(
        journal.0.join("config/schedules.json"),
        serde_json::to_vec(&json!({"ac13": {
            "cmd": [env!("CARGO_BIN_EXE_solstone-system-test-child"), "lines"],
            "every": "1m"
        }}))
        .expect("schedule JSON"),
    )
    .expect("write schedule");
    let mut child = start(&journal, None);
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, _write) = connect(&socket).await;
    let status = timeout(Duration::from_secs(8), async {
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("status frame");
            let value: Value = serde_json::from_str(&line).expect("status JSON");
            if value["tract"] == "supervisor" && value["event"] == "status" {
                return value;
            }
        }
    })
    .await
    .expect("status broadcast");
    let names = status["services"]
        .as_array()
        .expect("service projection")
        .iter()
        .filter_map(|service| service["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"local") && names.contains(&"parakeet"));
    assert!(
        status["schedules"]
            .as_array()
            .expect("schedule projection")
            .iter()
            .any(|schedule| schedule == "ac13")
    );
    assert!(status["crashed"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ac11_capped_task_is_terminated_with_timeout_exit() {
    let journal = TempJournal::new();
    let ready = journal.0.join("task-ready");
    let mut child = start(&journal, Some(1));
    let socket = journal.0.join("health/callosum.sock");
    wait_for_socket(&mut child, &socket);
    let (mut reader, mut write) = connect(&socket).await;
    send_request(
        &mut write,
        vec![
            env!("CARGO_BIN_EXE_solstone-system-test-child").into(),
            "ready-sleep".into(),
            ready.display().to_string(),
            "10000".into(),
        ],
        "ac11-task",
    )
    .await;
    let _ = receive_until(&mut reader, "ac11-task", "started").await;
    for _ in 0..300 {
        if ready.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ready.exists(), "task process really started");
    let stopped = receive_until(&mut reader, "ac11-task", "stopped").await;
    assert_eq!(
        stopped["exit_code"],
        json!(-15),
        "deadline termination is surfaced as timeout exit"
    );
    let status = timeout(Duration::from_secs(8), async {
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("status frame");
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
    })
    .await
    .expect("timeout history projection");
    assert!(status["recent_tasks"].is_array());
}
