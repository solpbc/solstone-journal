// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
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

    fn enable_thinking(&self) {
        fs::write(
            self.0.join("config/journal.json"),
            br#"{"providers":{"active":{"provider":"local"}}}"#,
        )
        .expect("thinking config");
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
                env!("CARGO_BIN_EXE_solstone-system-test-child"),
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
    command.env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1");
    command.env(
        "SOLSTONE_SUPERVISOR_APP_BINARY",
        env!("CARGO_BIN_EXE_solstone-system-test-child"),
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
    ChildGuard(command.spawn().expect("supervisor starts"))
}

fn wait_for_socket(child: &mut ChildGuard, socket: &std::path::Path) {
    let ready = socket
        .parent()
        .expect("Callosum socket health directory")
        .join("supervisor.ready");
    for _ in 0..1600 {
        if socket.exists() && ready.exists() {
            return;
        }
        if let Some(status) = child.0.try_wait().expect("supervisor status") {
            panic!("supervisor exited during boot: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("supervisor did not become ready");
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

async fn receive_started_command(
    reader: &mut tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    command: &[&str],
) -> Value {
    // The budget is deliberately generous because the journal fixture reaches
    // the test child through PATH and a shell exec; a tight budget here would
    // report a slow fork as a supervisor defect. This applies to every caller.
    timeout(Duration::from_secs(20), async {
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("started frame");
            let value: Value = serde_json::from_str(&line).expect("Callosum JSON");
            if value["tract"] == "supervisor"
                && value["event"] == "started"
                && value["cmd"] == json!(command)
            {
                return value;
            }
        }
    })
    .await
    .expect("handler task start")
}

async fn wait_for_path(path: &Path) {
    timeout(Duration::from_secs(8), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected path");
}

async fn wait_for_logged_message(path: &Path, message: &Value) {
    timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(contents) = fs::read_to_string(path)
                && let Some(line) = contents.lines().next()
                && let Ok(mut logged) = serde_json::from_str::<Value>(line)
                && logged["ts"].is_number()
            {
                logged.as_object_mut().expect("event object").remove("ts");
                if &logged == message {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("expected logged event");
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
    // Spawning the child and having it write its marker is scaffolding, not the
    // behaviour under test, so this budget is deliberately generous: the old
    // 3 s window failed intermittently on an idle machine — reproduced once in
    // ~10 runs, on the first run after a cold build — while passing 6/6 in
    // isolation. A tight budget here reports a slow fork as a supervisor defect.
    for _ in 0..3_000 {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_message_submits_live_segment_think_over_socket() {
    let journal = TempJournal::new();
    journal.enable_thinking();
    let _stub_marker = journal.install_journal_stub();
    let mut child = start(&journal, None);
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
    let mut child = start(&journal, None);
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
    let mut child = start(&journal, None);
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
    let mut child = start(&journal, None);
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
    let mut child = start(&journal, None);
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
    let mut child = start(&journal, None);
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
    let restarting = timeout(Duration::from_secs(8), async {
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("restart frame");
            let value: Value = serde_json::from_str(&line).expect("restart JSON");
            if value["tract"] == "supervisor" && value["event"] == "restarting" {
                return value;
            }
        }
    })
    .await
    .expect("restart notification");
    assert_eq!(restarting["service"], "convey");
    assert_eq!(restarting["pid"], previous_pid);

    timeout(Duration::from_secs(8), async {
        loop {
            let current = fs::read_to_string(&marker).expect("convey marker");
            let pid = current
                .trim()
                .rsplit(':')
                .next()
                .expect("convey pid")
                .parse::<u32>()
                .expect("numeric convey pid");
            if pid != previous_pid {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("replacement convey process");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn segment_events_log_appends_existing_stream_segment() {
    let journal = TempJournal::new();
    let segment = journal.segment_dir("20260102", Some("camera"), "120000_60");
    fs::create_dir_all(&segment).expect("segment directory");
    let mut child = start(&journal, None);
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
    let mut child = start(&journal, None);
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
    assert!(child.0.try_wait().expect("supervisor status").is_none());
}
