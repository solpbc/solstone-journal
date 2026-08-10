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
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
            for _ in 0..1_000 {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn start(journal: &TempJournal) -> ChildGuard {
    ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .args(["supervisor", "--journal"])
            .arg(&journal.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env(
                "SOLSTONE_LOCAL_BINARY",
                env!("CARGO_BIN_EXE_solstone-system-test-child"),
            )
            .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
            .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
            .env(
                "SOLSTONE_SUPERVISOR_APP_BINARY",
                env!("CARGO_BIN_EXE_solstone-system-test-child"),
            )
            .spawn()
            .expect("supervisor starts"),
    )
}
fn wait_for(path: &std::path::Path, child: &mut ChildGuard) {
    for _ in 0..500 {
        if path.exists() {
            return;
        }
        if let Some(status) = child.0.try_wait().expect("supervisor status") {
            panic!("supervisor exited: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{} did not appear", path.display());
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
    timeout(Duration::from_secs(8), async {
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("event line");
            let value: Value = serde_json::from_str(&line).expect("event JSON");
            if value["tract"] == "supervisor"
                && value["event"] == "started"
                && value["ref"] == reference
            {
                return value["pid"].as_i64().expect("process event pid") as i32;
            }
        }
    })
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
            env!("CARGO_BIN_EXE_solstone-system-test-child").into(),
            "ready-sleep".into(),
            ready_path.display().to_string(),
            "30000".into(),
        ],
        "ac14-task",
    )
    .await;
    for _ in 0..300 {
        if ready_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ready_path.exists(), "task child reached its ready point");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.0.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .expect("signal supervisor");
    let mut ready_removed = None;
    let mut pid_removed = None;
    let mut socket_removed = None;
    let mut status = None;
    for tick in 0..6000 {
        if ready_removed.is_none() && !ready.exists() {
            ready_removed = Some(tick);
        }
        if pid_removed.is_none() && !pid_file.exists() {
            pid_removed = Some(tick);
        }
        if socket_removed.is_none() && !socket.exists() {
            socket_removed = Some(tick);
        }
        if let Some(exited) = child.0.try_wait().expect("supervisor status") {
            status = Some(exited);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let status = status.expect("supervisor did not exit after SIGTERM");
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
    let status = child.0.wait().expect("wait sync conflict");
    assert_eq!(status.code(), Some(2));
    assert!(journal.0.join("health/supervisor.pid").exists());
    assert!(!ready.exists());
    let sync = fs::read_dir(journal.0.join("health/sync")).expect("sync directory");
    assert!(
        sync.filter_map(Result::ok)
            .any(|entry| entry.file_name() != "foreign-host.check")
    );
}
