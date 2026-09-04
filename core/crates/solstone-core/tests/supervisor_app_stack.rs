// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use solstone_core_system::process::describe_exit;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

#[path = "support/installation_binding.rs"]
mod installation_binding;
#[path = "support/speakers_analyze_stub.rs"]
mod speakers_analyze_stub;
#[path = "support/supervisor_guard.rs"]
mod supervisor_guard;

use supervisor_guard::SupervisorGuard;

#[allow(dead_code)]
#[path = "support/await_outcome.rs"]
mod await_outcome;

use await_outcome::{PollState, WaitOutcome, WaitPolarity, await_outcome};

const SERVICES: [&str; 4] = ["convey", "sense", "cortex", "spl"];
// Status is emitted every five seconds and is deliberately suppressed while a
// process observation is indeterminate. Four intervals keep this a hang
// ceiling without treating two consecutive transient observations as failure.
const SUPERVISOR_EVENT_HANG_CEILING: Duration = Duration::from_secs(20);
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

impl SupervisorGuard {
    fn running(&mut self) -> bool {
        self.try_wait().expect("supervisor status").is_none()
    }
}

fn start(journal: &TempJournal, args: &[&str], convey_argv: Option<String>) -> SupervisorGuard {
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let home = installation_binding::admit_for(&journal.0);
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
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture)
        .env("HOME", home);
    speakers_analyze_stub::apply(&mut command);
    if let Some(argv) = convey_argv {
        command.env("SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV", argv);
    }
    SupervisorGuard::new(command.spawn().expect("supervisor starts"))
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
    // macOS scheduling can stretch a single 100ms sleep beyond await_outcome's
    // 1.1x dilation threshold under the full CI workload. A two-second
    // observation window strengthens the negative proof while amortizing that
    // fixed scheduling delay.
    #[cfg(target_os = "macos")]
    let (interval, iterations) = (Duration::from_millis(100), 20);
    #[cfg(not(target_os = "macos"))]
    let (interval, iterations) = (Duration::from_millis(5), 100);
    let outcome = await_outcome(
        WaitPolarity::Negative,
        interval,
        iterations,
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
    assert!(
        !journal.marker(service).exists(),
        "unexpected {service} fixture marker at the observation boundary"
    );
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
    fixture_child_pids(parent_pid, argument).next().is_some()
}

fn fixture_child_pids(parent_pid: u32, argument: &str) -> impl Iterator<Item = u32> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,command="])
        .output()
        .expect("list processes");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let ppid = fields.next()?.parse::<u32>().ok()?;
            let command = fields.collect::<Vec<_>>().join(" ");
            (ppid == parent_pid && command.contains(argument)).then_some(pid)
        })
        .collect::<Vec<_>>()
        .into_iter()
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
    match tokio::time::timeout(SUPERVISOR_EVENT_HANG_CEILING, &mut collector).await {
        Ok(outcome) => outcome.expect("app start collector"),
        Err(_) => {
            collector.abort();
            panic!("timed out waiting for remaining app start events");
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

/// When the supervisor's own speakers-analyze generation acquisition fails,
/// the supervisor refuses before any lifecycle admission artifact exists and
/// exits 78 — distinct from the ordinary `EXIT_TEMPFAIL=75` mapping every
/// other named boot refusal uses.
#[test]
fn speakers_analyze_generation_failure_refuses_before_any_lifecycle_artifact() {
    let journal = TempJournal::new();
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let home = installation_binding::admit_for(&journal.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("SOLSTONE_LOCAL_BINARY", fixture)
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture)
        .env("HOME", home)
        // Deliberately no speakers_analyze_stub::apply(...) here: the point
        // is to force the supervisor's own acquisition attempt to fail. Do
        // not rely on the ambient absence of a real speakers-analyze helper
        // binary next to `solstone-core` in the build tree to produce that
        // failure — a prior build step (or a shared/reused target
        // directory) can leave that binary sitting right there, which makes
        // acquisition succeed instead, and the supervisor then boots its
        // full app stack for real and never exits, hanging this test's
        // blocking `output()` call indefinitely. Point the helper-binary
        // override at a path that is guaranteed not to exist so the
        // negative case is hermetic regardless of build-tree state. Every
        // other fixture env var above is still set so
        // `preflight_journal_binary` short-circuits
        // (app_fixture_binary().is_some()) and the boot actually reaches the
        // speakers-analyze gate instead of failing at an unrelated earlier
        // step.
        .env(
            "SOLSTONE_SPEAKERS_ANALYZE_BINARY",
            journal.0.join("nonexistent-speakers-analyze-helper"),
        );
    let output = command.output().expect("supervisor runs to completion");
    assert_eq!(
        output.status.code(),
        Some(78),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for path in [
        "health/supervisor.ready",
        "health/supervisor.pid",
        "health/callosum.sock",
        "health/direct-door.json",
    ] {
        assert!(
            !journal.0.join(path).exists(),
            "unexpected artifact: {path}"
        );
    }
    assert!(
        !journal.0.join("health/parent-loss").exists(),
        "unexpected parent-loss lifecycle artifacts"
    );
}

/// The supervisor acquires its own speakers-analyze generation before
/// spawning any app process, then Sense's spawn environment carries that
/// generation's real inherited descriptor. This spawns the real
/// speakers-analyze-generation-holder binary as Sense's process and proves,
/// via its own `enter_speakers_analyze_generation` call, that it genuinely
/// borrowed the supervisor's generation rather than merely inheriting
/// environment strings.
#[test]
fn sense_fixture_borrows_the_supervisors_speakers_analyze_generation() {
    let journal = TempJournal::new();
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let holder = env!("CARGO_BIN_EXE_solstone-core-speakers-analyze-generation-holder");
    let home = installation_binding::admit_for(&journal.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("SOLSTONE_LOCAL_BINARY", fixture)
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture)
        .env("SOLSTONE_SUPERVISOR_SENSE_GENERATION_HOLDER", holder)
        .env("HOME", home);
    speakers_analyze_stub::apply(&mut command);
    let _child = SupervisorGuard::new(command.spawn().expect("supervisor starts"));

    wait_for_markers(&journal, &["sense", "cortex"]);
    let marker: Value = serde_json::from_slice(
        &fs::read(journal.marker("sense")).expect("read sense generation-holder marker"),
    )
    .expect("marker is JSON");
    assert_eq!(marker["ok"], true, "holder failed to enter: {marker}");
    assert_eq!(
        marker["borrowed"], true,
        "holder must borrow an inherited generation, not acquire its own: {marker}"
    );
    let install_generation_id = marker["install_generation_id"]
        .as_str()
        .expect("install_generation_id present");

    let record: Value = serde_json::from_slice(
        &fs::read(
            journal
                .0
                .join("health/speakers-analyze/install-generation.json"),
        )
        .expect("read supervisor's generation record"),
    )
    .expect("generation record is JSON");
    assert_eq!(
        record["id"], install_generation_id,
        "the borrowed generation id must match the supervisor's own acquired generation"
    );

    // AC20/AC22 exclusion: Cortex talent workers cannot reach transcription
    // (denied by the cogitate CLI allowlist), so Cortex's spawn environment
    // must never carry the speakers-analyze generation, unlike Sense's above.
    let cortex_marker =
        fs::read_to_string(journal.marker("cortex")).expect("read Cortex fixture marker");
    let has_generation_field = cortex_marker
        .trim()
        .split(':')
        .nth(2)
        .expect("marker has a generation-presence field");
    assert_eq!(
        has_generation_field, "0",
        "Cortex must not receive speakers-analyze generation env vars: {cortex_marker}"
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
    child
        .shutdown_and_wait(Duration::from_secs(10))
        .expect("supervisor exits after SIGTERM");
    // Graceful shutdown returns as soon as the SUPERVISOR exits, which is not the
    // same instant its children are gone: they still have to take SIGTERM,
    // exit, and be reparented to init before `kill(pid, None)` answers ESRCH.
    // Asserting on the instant after `shutdown_and_wait()` therefore raced,
    // and failed 4 runs in 5. Polling does not weaken the invariant — a
    // supervisor that never terminates its children still fails, just after a
    // bounded wait instead of immediately.
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
        // Fast fixture timing caps the restart backoff, but a restart can still
        // follow the remaining supervised stack's admission work under a loaded
        // full-CI shard.
        400,
        Instant::now,
        || {
            if fixture_process_running(child.id(), &state_path.display().to_string()) {
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

fn backoff_path(journal: &TempJournal, service: &str) -> PathBuf {
    journal.0.join("health").join(format!("{service}.backoff"))
}

fn port_path(journal: &TempJournal) -> PathBuf {
    journal.0.join("health/convey.port")
}

fn wait_for_backoff_record(journal: &TempJournal, service: &str) -> Value {
    let path = backoff_path(journal, service);
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(50),
        500,
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
    panic_for_wait(
        &format!("{} did not appear after repeated exits", path.display()),
        outcome,
    );
    serde_json::from_slice(&fs::read(&path).expect("read backoff record")).expect("backoff JSON")
}

fn start_with_app_binary(
    journal: &TempJournal,
    args: &[&str],
    convey_argv: Option<String>,
    app_binary: &str,
) -> SupervisorGuard {
    let home = installation_binding::admit_for(&journal.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env(
            "SOLSTONE_LOCAL_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", app_binary)
        .env("HOME", home);
    speakers_analyze_stub::apply(&mut command);
    if let Some(argv) = convey_argv {
        command.env("SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV", argv);
    }
    SupervisorGuard::new(command.spawn().expect("supervisor starts"))
}

fn crashed_row<'a>(status: &'a Value, name: &str) -> Option<&'a Value> {
    status["crashed"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["name"] == name)
}

async fn connect_callosum(
    socket: &Path,
) -> (
    BufReader<tokio::net::unix::OwnedReadHalf>,
    tokio::net::unix::OwnedWriteHalf,
) {
    let stream = UnixStream::connect(socket).await.expect("connect Callosum");
    let (read, write) = stream.into_split();
    (BufReader::new(read), write)
}

async fn receive_supervisor_event(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    event: &str,
) -> Value {
    tokio::time::timeout(SUPERVISOR_EVENT_HANG_CEILING, async {
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line).await.expect("Callosum frame");
            assert!(bytes > 0, "Callosum closed before supervisor {event}");
            let value: Value = serde_json::from_str(&line).expect("Callosum JSON");
            if value["tract"] == "supervisor" && value["event"] == event {
                return value;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for supervisor {event}"))
}

fn wait_for_socket(journal: &TempJournal) {
    let socket = journal.0.join("health/callosum.sock");
    let ready = journal.0.join("health/supervisor.ready");
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(5),
        1_600,
        Instant::now,
        || {
            if socket.exists() && ready.exists() {
                PollState::Held
            } else {
                PollState::Pending
            }
        },
        thread::sleep,
    );
    panic_for_wait("supervisor did not become ready", outcome);
}

#[test]
fn always_exit_convey_reports_backoff_and_clears_stale_backoff_file() {
    let journal = TempJournal::new();
    fs::create_dir_all(journal.0.join("health")).expect("health directory");
    let leftover = backoff_path(&journal, "convey");
    fs::write(&leftover, "{\"leftover\":true}\n").expect("seed leftover backoff file");
    let mut child = start(&journal, &[], Some("always-exit".to_owned()));
    wait_for_markers(&journal, &["sense", "cortex", "spl"]);
    assert!(
        !leftover.exists()
            || serde_json::from_slice::<Value>(&fs::read(&leftover).unwrap_or_default())
                .ok()
                .is_none_or(|value| value.get("leftover").is_none()),
        "leftover backoff file must be gone once the supervisor manages convey"
    );
    assert!(
        !leftover.exists(),
        "backoff file must stay absent until the struggling threshold"
    );

    let record = wait_for_backoff_record(&journal, "convey");
    assert_eq!(record["exit_code"], 1);
    assert!(record["restart_attempts"].as_u64() >= Some(5));
    assert_eq!(record["reason"], describe_exit(1));

    wait_for_socket(&journal);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let status = runtime.block_on(async {
        let (mut reader, _write) = connect_callosum(&journal.0.join("health/callosum.sock")).await;
        receive_supervisor_event(&mut reader, "status").await
    });
    let crashed = crashed_row(&status, "convey").expect("convey crashed row");
    assert_eq!(crashed["phase"], "backoff");
    assert_eq!(crashed["reason_code"], describe_exit(1));
    assert!(crashed["restart_attempts"].as_u64() >= Some(5));
    assert!(child.running());
}

#[test]
fn never_ready_convey_is_not_crashed_and_writes_no_backoff_file() {
    let journal = TempJournal::new();
    let child = start(&journal, &[], Some("never-ready".to_owned()));
    wait_for_markers(&journal, &["sense", "cortex", "spl"]);
    assert!(!port_path(&journal).exists(), "port file must stay absent");
    assert!(
        !backoff_path(&journal, "convey").exists(),
        "alive convey must not write a backoff file"
    );
    assert!(
        fixture_process_running(child.id(), "never-ready"),
        "never-ready fixture must still be running"
    );

    wait_for_socket(&journal);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let status = runtime.block_on(async {
        let (mut reader, _write) = connect_callosum(&journal.0.join("health/callosum.sock")).await;
        receive_supervisor_event(&mut reader, "status").await
    });
    assert!(
        crashed_row(&status, "convey").is_none(),
        "never-ready convey must not appear on crashed: {status}"
    );
}

#[test]
fn spawn_failure_backoff_does_not_describe_sighup() {
    let journal = TempJournal::new();
    let missing = journal.0.join("missing-app-binary");
    let mut child = start_with_app_binary(&journal, &[], None, missing.to_str().expect("utf8"));
    let record = wait_for_backoff_record(&journal, "convey");
    assert!(
        record.get("exit_code").is_none() || record["exit_code"].is_null(),
        "spawn failure must omit exit_code: {record}"
    );
    let reason = record["reason"].as_str().unwrap_or_default();
    assert_eq!(reason, "failed to spawn process");
    assert!(!reason.contains("SIGHUP"));
    assert_ne!(reason, describe_exit(-1));
    assert!(record["restart_attempts"].as_u64() >= Some(5));

    wait_for_socket(&journal);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let status = runtime.block_on(async {
        let (mut reader, _write) = connect_callosum(&journal.0.join("health/callosum.sock")).await;
        receive_supervisor_event(&mut reader, "status").await
    });
    let crashed = crashed_row(&status, "convey").expect("convey crashed row");
    let reason_code = crashed["reason_code"].as_str().unwrap_or_default();
    assert!(!reason_code.contains("SIGHUP"), "{crashed}");
    assert_eq!(reason_code, "failed to spawn process");
    assert_eq!(crashed["phase"], "backoff");
    assert!(child.running());
}
