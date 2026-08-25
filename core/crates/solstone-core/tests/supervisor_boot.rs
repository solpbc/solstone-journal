// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::supervisor_guard::SupervisorGuard;

struct TempJournal(PathBuf);

impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-supervisor-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("journal config directory");
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

fn wait_for_socket(child: &mut SupervisorGuard, socket: &std::path::Path) {
    for _ in 0..1_600 {
        if UnixStream::connect(socket).is_ok() {
            return;
        }
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited during boot: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("supervisor did not bind Callosum");
}

fn foreign_heartbeat(journal: &TempJournal) {
    let sync = journal.0.join("health/sync");
    fs::create_dir_all(&sync).expect("sync directory");
    fs::write(
        sync.join("foreign-host.check"),
        format!(
            r#"{{"schema":1,"machine_id":"foreign-machine","hostname":"foreign-host","pid":4242,"wall_time":"now","solstone_version":"1","interval_seconds":15,"journal_path":"{}"}}"#,
            journal.0.display()
        ),
    )
    .expect("foreign heartbeat");
}

#[test]
fn ac6_boot_order_is_identity_then_socket_then_ready() {
    assert!(std::path::Path::new(env!("CARGO_BIN_EXE_solstone-core-system-test-child")).exists());
    let journal = TempJournal::new();
    let mut child = start(&journal);
    let socket = journal.0.join("health/callosum.sock");
    let pid = journal.0.join("health/supervisor.pid");
    let start_time = journal.0.join("health/supervisor.start_time");
    let ready = journal.0.join("health/supervisor.ready");
    let mut first_pid = None;
    let mut first_start_time = None;
    let mut first_socket = None;
    let mut first_ready = None;
    for tick in 0..1_600 {
        if first_pid.is_none() && pid.exists() {
            first_pid = Some(tick);
        }
        if first_start_time.is_none() && start_time.exists() {
            first_start_time = Some(tick);
        }
        if first_socket.is_none() && socket.exists() {
            first_socket = Some(tick);
        }
        if first_ready.is_none() && ready.exists() {
            first_ready = Some(tick);
        }
        if first_ready.is_some() {
            break;
        }
        if child.try_wait().expect("supervisor status").is_some() {
            panic!("supervisor exited during boot");
        }
        thread::sleep(Duration::from_millis(5));
    }
    let first_pid = first_pid.expect("pid appeared");
    let first_start_time = first_start_time.expect("start-time appeared");
    let first_socket = first_socket.expect("socket appeared");
    let first_ready = first_ready.expect("ready appeared");
    assert!(first_pid <= first_socket, "pid must precede socket");
    assert!(
        first_start_time <= first_socket,
        "start time must precede socket"
    );
    assert!(first_socket <= first_ready, "socket must precede ready");
}

#[test]
fn ac7_second_instance_refused_first_survives() {
    let journal = TempJournal::new();
    let mut first = start(&journal);
    wait_for_socket(&mut first, &journal.0.join("health/callosum.sock"));
    let pid = fs::read_to_string(journal.0.join("health/supervisor.pid"))
        .expect("pid")
        .trim()
        .parse::<i32>()
        .expect("numeric pid");
    let second = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
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
        .env(
            "SOLSTONE_SUPERVISOR_APP_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .status()
        .expect("second supervisor runs");
    assert_eq!(second.code(), Some(75));
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok());
    assert!(first.try_wait().expect("first status").is_none());
}

#[test]
fn ac8_live_foreign_writer_blocks_boot_without_pid() {
    let journal = TempJournal::new();
    foreign_heartbeat(&journal);
    let status = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
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
        .env(
            "SOLSTONE_SUPERVISOR_APP_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .status()
        .expect("supervisor runs");
    assert_eq!(status.code(), Some(1));
    assert!(!journal.0.join("health/supervisor.pid").exists());
}
