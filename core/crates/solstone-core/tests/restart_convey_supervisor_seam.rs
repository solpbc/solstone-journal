// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[path = "support/installation_binding.rs"]
mod installation_binding;

struct Journal(PathBuf);
impl Journal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("solstone-restart-seam-{stamp}"));
        fs::create_dir_all(path.join("config")).expect("config directory");
        fs::write(
            path.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("config writes");
        Self(path)
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Supervisor(Child);
impl Drop for Supervisor {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            signal_process_group(&self.0, nix::sys::signal::Signal::SIGTERM);
            for _ in 0..1_000 {
                if self.0.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            signal_process_group(&self.0, nix::sys::signal::Signal::SIGKILL);
        }
        let _ = self.0.wait();
    }
}

fn signal_process_group(child: &Child, signal: nix::sys::signal::Signal) {
    let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-(child.id() as i32)), signal);
}

fn start(journal: &Journal, convey_argv: Option<String>) -> Supervisor {
    let home = installation_binding::admit_for(&journal.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env(
            "SOLSTONE_LOCAL_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("HOME", home)
        .env(
            "SOLSTONE_SUPERVISOR_APP_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .process_group(0);
    if let Some(convey_argv) = convey_argv {
        command.env("SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV", convey_argv);
    }
    let child = command.spawn().expect("supervisor starts");
    Supervisor(child)
}

fn wait_for_path(path: &std::path::Path, message: &str) {
    for _ in 0..1600 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{message}: {}", path.display());
}

async fn connect_after_boot(
    journal: &Journal,
) -> (
    BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let socket = journal.0.join("health/callosum.sock");
    let ready = journal.0.join("health/supervisor.ready");
    wait_for_path(&socket, "Callosum socket did not appear");
    wait_for_path(&ready, "supervisor did not become ready");
    let stream = UnixStream::connect(socket)
        .await
        .expect("Callosum connects");
    let (read, write) = stream.into_split();
    (BufReader::new(read), write)
}

async fn send_restart(write: &mut tokio::net::unix::OwnedWriteHalf, restart_id: &str) {
    let request = serde_json::to_vec(&json!({
        "tract": "supervisor", "event": "restart", "service": "convey", "restart_id": restart_id,
    }))
    .expect("request JSON");
    write.write_all(&request).await.expect("request writes");
    write.write_all(b"\n").await.expect("request frame");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_id_flows_through_the_real_handler_and_process_sink() {
    let journal = Journal::new();
    let _supervisor = start(&journal, None);
    let marker = journal.0.join("health/fixture-convey.marker");
    wait_for_path(&marker, "fixture Convey did not start");
    let (mut reader, mut write) = connect_after_boot(&journal).await;
    let restart_id = "seam-restart-id";
    send_restart(&mut write, restart_id).await;

    let (saw_restarting, saw_stopped, saw_started) =
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut saw_restarting = false;
            let mut saw_stopped = false;
            let mut saw_started = false;
            while !(saw_restarting && saw_stopped && saw_started) {
                let mut line = String::new();
                reader.read_line(&mut line).await.expect("event reads");
                let event: Value = serde_json::from_str(&line).expect("event JSON");
                if event["restart_id"] != restart_id {
                    continue;
                }
                match (event["tract"].as_str(), event["event"].as_str()) {
                    (Some("supervisor"), Some("restarting")) => saw_restarting = true,
                    (Some("supervisor"), Some("stopped")) => saw_stopped = true,
                    (Some("supervisor"), Some("started")) => saw_started = true,
                    _ => {}
                }
            }
            (saw_restarting, saw_stopped, saw_started)
        })
        .await
        .expect("correlated process lifecycle arrives");
    assert!(saw_restarting, "handler emits correlated restarting");
    assert!(saw_stopped, "old app process emits correlated stopped");
    assert!(
        saw_started,
        "replacement app process emits correlated started"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_id_survives_the_first_start_and_the_following_crash_restart() {
    let journal = Journal::new();
    let health = journal.0.join("health");
    let ready_marker = health.join("crash-once.ready");
    let state = health.join("crash-once.state");
    let port = health.join("convey.port");
    let fixture_argv = format!(
        "ready-sleep-crash-once {} {} {}",
        ready_marker.display(),
        state.display(),
        port.display(),
    );
    let _supervisor = start(&journal, Some(fixture_argv));
    wait_for_path(&ready_marker, "crash-once Convey did not become ready");
    let (mut reader, mut write) = connect_after_boot(&journal).await;
    let restart_id = "seam-crash-restart-id";
    send_restart(&mut write, restart_id).await;

    let (stopped, started) = tokio::time::timeout(Duration::from_secs(8), async {
        let mut stopped = 0_u8;
        let mut started = 0_u8;
        while stopped < 2 || started < 2 {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("event reads");
            let event: Value = serde_json::from_str(&line).expect("event JSON");
            if event["restart_id"] != restart_id {
                continue;
            }
            match (event["tract"].as_str(), event["event"].as_str()) {
                (Some("supervisor"), Some("stopped")) => stopped += 1,
                (Some("supervisor"), Some("started")) => started += 1,
                _ => {}
            }
        }
        (stopped, started)
    })
    .await
    .expect("correlated crash and replacement lifecycle arrives");
    assert_eq!(stopped, 2, "first stop and crash stop remain correlated");
    assert_eq!(
        started, 2,
        "first and crash-replacement starts remain correlated"
    );
    assert_eq!(fs::read_to_string(state).expect("fixture state"), "crashed");
}
