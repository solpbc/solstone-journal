// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::time::timeout;

#[allow(dead_code)]
#[path = "support/await_outcome.rs"]
mod await_outcome;

use await_outcome::{PollState, WaitOutcome, WaitPolarity, await_outcome};

const SERVICES: [&str; 4] = ["convey", "sense", "cortex", "spl"];
// Convey's initial Spawned arrives about 1ms after the Callosum socket becomes
// connectable; four of five late-connect runs instead latched its ~6s restart.
// FixtureConveyReadinessProbe::is_ready delegates to ready_sleep_marker_path,
// so the default fixture marker already proves Convey precedes the remaining
// apps, and that fact remains covered by the marker assertion.
const REMAINING_APP_START_REFS: [&str; 3] = [
    "supervisor-app-sense",
    "supervisor-app-cortex",
    "supervisor-app-spl",
];

struct TempJournal(PathBuf);

impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-supervisor-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("journal config");
        Self(root)
    }

    fn marker(&self, service: &str) -> PathBuf {
        self.0
            .join("health")
            .join(format!("fixture-{service}.marker"))
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

fn panic_for_wait(context: &str, outcome: WaitOutcome) {
    match outcome {
        WaitOutcome::Passed(_) => {}
        WaitOutcome::Failed { reason, metrics } => {
            panic!("{context}: {reason}; {}", metrics.describe());
        }
        WaitOutcome::Inconclusive(metrics) => {
            panic!("{context}: {}", metrics.describe());
        }
    }
}

fn panic_or_log_termination(outcome: WaitOutcome) {
    if matches!(outcome, WaitOutcome::Passed(_)) {
        return;
    }
    let message = match &outcome {
        WaitOutcome::Failed { reason, metrics } => {
            format!(
                "supervisor did not exit after SIGTERM: {reason}; {}",
                metrics.describe()
            )
        }
        WaitOutcome::Inconclusive(metrics) => format!(
            "supervisor shutdown wait was inconclusive after SIGTERM: {}",
            metrics.describe()
        ),
        WaitOutcome::Passed(_) => unreachable!(),
    };
    if std::thread::panicking() {
        eprintln!("suppressed termination failure while unwinding: {message}");
    } else {
        panic!("{message}");
    }
}

impl ChildGuard {
    fn running(&mut self) -> bool {
        self.0.try_wait().expect("supervisor status").is_none()
    }

    fn terminate(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGTERM,
            )
            .expect("signal supervisor");
            let outcome = await_outcome(
                WaitPolarity::Positive,
                Duration::from_millis(5),
                2_000,
                Instant::now,
                || match self.0.try_wait() {
                    Ok(Some(_)) => PollState::Held,
                    Ok(None) => PollState::Pending,
                    Err(error) => PollState::HardFail(format!("supervisor status: {error}")),
                },
                thread::sleep,
            );
            if !matches!(outcome, WaitOutcome::Passed(_)) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
            panic_or_log_termination(outcome);
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
        let _ = self.0.wait();
    }
}

fn start(journal: &TempJournal, args: &[&str], convey_argv: Option<String>) -> ChildGuard {
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("SOLSTONE_LOCAL_BINARY", fixture)
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture);
    if let Some(argv) = convey_argv {
        command.env("SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV", argv);
    }
    ChildGuard(command.spawn().expect("supervisor starts"))
}

fn wait_for_markers(journal: &TempJournal, services: &[&str]) -> BTreeMap<String, Instant> {
    let mut observed = BTreeMap::new();
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(5),
        1_600,
        Instant::now,
        || {
            for service in services {
                if !observed.contains_key(*service) && journal.marker(service).exists() {
                    observed.insert((*service).to_owned(), Instant::now());
                }
            }
            if observed.len() == services.len() {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        thread::sleep,
    );
    panic_for_wait(
        &format!("fixture markers did not appear: {observed:?}"),
        outcome,
    );
    observed
}

fn launch_order_violation(refs: &[String]) -> Option<String> {
    let mut next_expected = 0;
    for reference in refs {
        let Some(position) = REMAINING_APP_START_REFS
            .iter()
            .position(|expected| *expected == reference)
        else {
            return Some(format!("unknown app start ref {reference:?}"));
        };
        if position < next_expected {
            return Some(format!("{reference:?} arrived after a later app start ref"));
        }
        next_expected = position + 1;
    }
    None
}

async fn collect_remaining_app_start_refs(
    socket: PathBuf,
    capture_entered: oneshot::Sender<()>,
    captured: Arc<Mutex<Vec<String>>>,
) {
    let _ = capture_entered.send(());
    let stream = loop {
        match UnixStream::connect(&socket).await {
            Ok(stream) => break stream,
            Err(_) => continue,
        }
    };
    let (read, _write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line).await.expect("event line");
        assert!(bytes > 0, "Callosum disconnected before app starts");
        let value: Value = serde_json::from_str(&line).expect("event JSON");
        let Some(reference) = value["ref"].as_str() else {
            continue;
        };
        if value["tract"] != "supervisor"
            || value["event"] != "started"
            || !REMAINING_APP_START_REFS.contains(&reference)
        {
            continue;
        }
        let mut observed = captured.lock().expect("captured app starts");
        observed.push(reference.to_owned());
        if REMAINING_APP_START_REFS
            .iter()
            .all(|expected| observed.iter().any(|reference| reference == expected))
        {
            return;
        }
    }
}

fn assert_marker_absent(journal: &TempJournal, service: &str) {
    let outcome = await_outcome(
        WaitPolarity::Negative,
        Duration::from_millis(5),
        100,
        Instant::now,
        || {
            if journal.marker(service).exists() {
                PollState::HardFail(format!("unexpected {service} fixture marker"))
            } else {
                PollState::Held
            }
        },
        thread::sleep,
    );
    panic_for_wait(&format!("unexpected {service} fixture marker"), outcome);
}

fn wait_for_path(path: &Path) {
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(5),
        1_600,
        Instant::now,
        || {
            if path.exists() {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        thread::sleep,
    );
    panic_for_wait(&format!("{} did not appear", path.display()), outcome);
}

fn process_is_gone(pid: u32) -> bool {
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

fn fixture_pid(marker: &Path) -> u32 {
    fs::read_to_string(marker)
        .expect("read fixture marker")
        .trim()
        .rsplit(':')
        .next()
        .expect("fixture pid")
        .parse()
        .expect("numeric fixture pid")
}

fn fixture_process_running(parent_pid: u32, argument: &str) -> bool {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,command="])
        .output()
        .expect("list processes");
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _pid = fields.next();
        fields.next().and_then(|value| value.parse::<u32>().ok()) == Some(parent_pid)
            && fields.collect::<Vec<_>>().join(" ").contains(argument)
    })
}

#[test]
fn app_stack_writes_all_fixture_markers() {
    let journal = TempJournal::new();
    let _child = start(&journal, &[], None);
    wait_for_markers(&journal, &SERVICES);
}

#[test]
fn launch_order_violation_accepts_order_and_rejects_inversion() {
    let launch_order = REMAINING_APP_START_REFS
        .iter()
        .map(|reference| (*reference).to_owned())
        .collect::<Vec<_>>();
    assert_eq!(launch_order_violation(&launch_order), None);

    let inversion = [
        "supervisor-app-sense".to_owned(),
        "supervisor-app-spl".to_owned(),
        "supervisor-app-cortex".to_owned(),
    ];
    assert!(launch_order_violation(&inversion).is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn never_ready_convey_starts_remaining_app_events_in_launch_order() {
    let journal = TempJournal::new();
    let socket = journal.0.join("health/callosum.sock");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let (capture_entered, capture_started) = oneshot::channel();
    let mut collector = tokio::spawn(collect_remaining_app_start_refs(
        socket,
        capture_entered,
        Arc::clone(&captured),
    ));
    capture_started
        .await
        .expect("collector entered connect loop");
    let _child = start(&journal, &[], Some("sleep".to_owned()));
    match timeout(Duration::from_secs(5), &mut collector).await {
        Ok(result) => result.expect("app start collector"),
        Err(_) => {
            collector.abort();
            panic!(
                "timed out collecting remaining app starts: {:?}",
                captured.lock().expect("captured app starts")
            );
        }
    }
    let captured = captured.lock().expect("captured app starts").clone();
    assert!(
        launch_order_violation(&captured).is_none(),
        "remaining app starts arrived out of order: {captured:?}"
    );
}

#[test]
fn never_ready_convey_waits_before_starting_the_remaining_stack() {
    let journal = TempJournal::new();
    let started = Instant::now();
    let mut child = start(&journal, &[], Some("sleep".to_owned()));
    let observed = wait_for_markers(&journal, &["sense", "cortex", "spl"]);
    assert!(child.running());
    assert!(
        observed["sense"].duration_since(started) >= Duration::from_millis(100),
        "sense started before the Convey readiness wait"
    );
}

#[test]
fn convey_exit_during_startup_keeps_supervisor_running() {
    let journal = TempJournal::new();
    let mut child = start(&journal, &[], Some("dies-on-startup".to_owned()));
    wait_for_markers(&journal, &["sense", "cortex", "spl"]);
    assert!(child.running());
}

#[test]
fn app_stack_opt_out_flags_suppress_only_their_service() {
    for (flag, absent, expected) in [
        ("--no-convey", "convey", &["sense", "cortex", "spl"][..]),
        ("--no-cortex", "cortex", &["convey", "sense", "spl"][..]),
        ("--no-spl", "spl", &["convey", "sense", "cortex"][..]),
    ] {
        let journal = TempJournal::new();
        let _child = start(&journal, &[flag], None);
        wait_for_markers(&journal, expected);
        assert_marker_absent(&journal, absent);
        assert!(journal.marker("sense").exists());
    }
}

#[test]
fn remote_mode_spawns_no_app_fixture_markers() {
    let journal = TempJournal::new();
    let mut child = start(&journal, &["--remote", "https://example.test"], None);
    for service in SERVICES {
        assert_marker_absent(&journal, service);
    }
    assert!(child.running());
}

#[test]
fn app_fixture_receives_supervisor_spawned_environment() {
    let journal = TempJournal::new();
    let _child = start(&journal, &[], None);
    wait_for_markers(&journal, &["convey"]);
    assert!(
        fs::read_to_string(journal.marker("convey"))
            .expect("read Convey fixture marker")
            .contains("ready:1"),
        "fixture did not receive SOL_SUPERVISOR_SPAWNED=1"
    );
}

#[test]
fn shutdown_terminates_all_app_fixture_children() {
    let journal = TempJournal::new();
    let mut child = start(&journal, &[], None);
    wait_for_markers(&journal, &SERVICES);
    let pids = SERVICES
        .iter()
        .map(|service| fixture_pid(&journal.marker(service)))
        .collect::<Vec<_>>();
    child.terminate();
    // `terminate` returns as soon as the SUPERVISOR exits, which is not the
    // same instant its children are gone: they still have to take SIGTERM,
    // exit, and be reparented to init before `kill(pid, None)` answers ESRCH.
    // Asserting on the instant after terminate() therefore raced, and failed
    // 4 runs in 5. Polling does not weaken the invariant — a supervisor that
    // never terminates its children still fails, just after a bounded wait
    // instead of immediately.
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(5),
        2_000,
        Instant::now,
        || {
            if pids.iter().copied().all(process_is_gone) {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        thread::sleep,
    );
    let survivors = pids
        .into_iter()
        .filter(|pid| !process_is_gone(*pid))
        .collect::<Vec<_>>();
    panic_for_wait(
        &format!("app fixture children survived supervisor shutdown: {survivors:?}"),
        outcome,
    );
}

#[test]
fn exited_convey_restarts_under_restart_policy() {
    let journal = TempJournal::new();
    let state_path = journal.0.join("restart-once");
    let convey_argv = format!("restart-once {}", state_path.display());
    let child = start(&journal, &[], Some(convey_argv));
    wait_for_path(&state_path);
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(5),
        200,
        Instant::now,
        || {
            if fixture_process_running(child.0.id(), &state_path.display().to_string()) {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        thread::sleep,
    );
    panic_for_wait(
        "Convey fixture did not restart after its first exit",
        outcome,
    );
}
