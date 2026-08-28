// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use solstone_core::supervisor::{
    InstallationBindingRefusal, ShutdownCause, SupervisorBootRefusal, SupervisorHostOutcome,
    SupervisorSignal,
    receipt::{
        HostedSupervisorReceiptReadError, read_hosted_supervisor_receipt,
        write_hosted_supervisor_receipt,
    },
};
use solstone_core_system::direct_door::{
    DirectDoorOutcome, DirectDoorPublishResult, publish_direct_door,
};
use solstone_core_system::lifecycle::{ParentAdmissionFailure, ParentLossLedger};
use solstone_core_system::process::{
    InstanceCensus, InstanceVerdict, ProcessInstanceSource, SystemProcessInstanceSource,
};

#[path = "support/hostile_binary.rs"]
mod hostile_binary;

use super::{supervisor_guard::SupervisorGuard, temporary_root::temporary_root};
use hostile_binary::{copied_binary, hostile_binary};

struct TempJournal(PathBuf);

static NEXT_TEMP_JOURNAL: AtomicU64 = AtomicU64::new(0);

impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = NEXT_TEMP_JOURNAL.fetch_add(1, Ordering::Relaxed);
        let root = temporary_root().join(format!(
            "solstone-core-supervisor-{}-{stamp}-{sequence}",
            std::process::id()
        ));
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

const RECOVERY_HEADER: &str = "this installation couldn't be verified.";
const RECOVERY_SETUP: &str =
    "run `journal setup` to check it. if setup finishes successfully, try again.";
const TRUNCATION_MARKER: &str = "…[truncated]";

fn supervisor_output(binary: &Path, journal: &OsStr, home: Option<&Path>) -> Output {
    let mut command = Command::new(binary);
    command
        .args(["supervisor", "--journal"])
        .arg(journal)
        .stdin(Stdio::null())
        .env_remove("SOLSTONE_JOURNAL");
    if let Some(home) = home {
        command.env("HOME", home);
    } else {
        command.env_remove("HOME");
    }
    command.output().expect("supervisor process runs")
}

fn recovery_details(stderr: &[u8]) -> String {
    let rendered = std::str::from_utf8(stderr).expect("recovery stderr is UTF-8");
    assert!(
        rendered.ends_with('\n'),
        "recovery stderr has one final newline"
    );
    let lines: Vec<_> = rendered
        .strip_suffix('\n')
        .expect("final newline")
        .split('\n')
        .collect();
    assert_eq!(lines.len(), 3, "recovery has exactly three lines");
    assert_eq!(lines[0], RECOVERY_HEADER);
    assert_eq!(lines[1], RECOVERY_SETUP);
    lines[2]
        .strip_prefix("details: ")
        .expect("recovery has details line")
        .to_owned()
}

fn start(journal: &TempJournal) -> SupervisorGuard {
    start_with_convey_argv(journal, None)
}

fn start_with_convey_argv(journal: &TempJournal, convey_argv: Option<String>) -> SupervisorGuard {
    let home = super::installation_binding::admit_for(&journal.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
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
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING", "1")
        .env("HOME", home)
        .env(
            "SOLSTONE_SUPERVISOR_APP_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        );
    if let Some(argv) = convey_argv {
        command.env("SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV", argv);
    }
    SupervisorGuard::new(command.spawn().expect("supervisor starts"))
}

fn start_paused_before_readiness(journal: &TempJournal, marker: &Path) -> SupervisorGuard {
    start_paused_before_readiness_with_args(journal, marker, &[])
}

fn start_paused_before_readiness_with_args(
    journal: &TempJournal,
    marker: &Path,
    extra_args: &[&str],
) -> SupervisorGuard {
    let home = super::installation_binding::admit_for(&journal.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
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
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING", "1")
        .env("HOME", home)
        .env(
            "SOLSTONE_SUPERVISOR_APP_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .env(
            "SOLSTONE_SUPERVISOR_HOSTED_PAUSE_BEFORE_FINAL_PARENT_CHECK",
            marker,
        );
    command.args(extra_args);
    SupervisorGuard::new(command.spawn().expect("paused supervisor starts"))
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

fn foreign_heartbeat(journal: &TempJournal) -> (PathBuf, Vec<u8>) {
    let sync = journal.0.join("health/sync");
    fs::create_dir_all(&sync).expect("sync directory");
    let path = sync.join("foreign-host.check");
    let body = format!(
            r#"{{"schema":1,"machine_id":"foreign-machine","hostname":"foreign-host","pid":4242,"wall_time":"now","solstone_version":"1","interval_seconds":15,"journal_path":"{}"}}"#,
            journal.0.display()
        )
    .into_bytes();
    fs::write(&path, &body).expect("foreign heartbeat");
    (path, body)
}

fn age_heartbeat_with_startup_cushion(path: &PathBuf) {
    let file = fs::File::open(path).expect("open foreign heartbeat");
    file.set_times(fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(45)))
        .expect("age foreign heartbeat");
}

fn wait_for_bounded_status(mut child: SupervisorGuard, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("supervisor status") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "supervisor did not settle within {timeout:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn renew_heartbeat_when_wait_marker_appears(
    journal: &TempJournal,
    heartbeat_path: PathBuf,
    heartbeat_body: Vec<u8>,
) -> thread::JoinHandle<()> {
    let sync = journal.0.join("health/sync");
    thread::spawn(move || {
        for _ in 0..1_600 {
            let marker_present = fs::read_dir(&sync).is_ok_and(|entries| {
                entries.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.starts_with("solstone-wait-v2-"))
                })
            });
            if marker_present {
                fs::write(&heartbeat_path, &heartbeat_body).expect("renew foreign heartbeat");
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("admission-wait marker did not appear");
    })
}

fn start_hosted(journal: &TempJournal) -> (std::process::Child, PathBuf, PathBuf, String) {
    let home = super::installation_binding::admit_for(&journal.0);
    start_hosted_with_home(journal, home)
}

fn start_hosted_with_home(
    journal: &TempJournal,
    home: PathBuf,
) -> (std::process::Child, PathBuf, PathBuf, String) {
    let child_pid = journal.0.join("hosted-child.pid");
    let outcome = journal.0.join("hosted.outcome");
    let nonce = next_receipt_nonce(&outcome);
    let launcher = Command::new(env!(
        "CARGO_BIN_EXE_solstone-core-hosted-supervisor-fixture"
    ))
    .args(["launcher"])
    .arg(&journal.0)
    .arg(&child_pid)
    .arg(&outcome)
    .arg(&nonce)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .env("HOME", home)
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
    .expect("hosted launcher starts");
    (launcher, child_pid, outcome, nonce)
}

fn wait_for_ready_from_launcher(journal: &TempJournal, launcher: &mut std::process::Child) {
    let ready = journal.0.join("health/supervisor.ready");
    for _ in 0..1_600 {
        if ready.exists() {
            return;
        }
        if let Some(status) = launcher.try_wait().expect("launcher status") {
            let mut stderr = String::new();
            if let Some(pipe) = launcher.stderr.as_mut() {
                pipe.read_to_string(&mut stderr).expect("launcher stderr");
            }
            panic!("hosted launcher exited before readiness: {status}; stderr: {stderr}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("hosted supervisor did not become ready");
}

enum PendingOutcome {
    Missing,
    DifferentNonce(String),
    Malformed(String),
}

fn next_receipt_nonce(outcome: &Path) -> String {
    let nonce = format!(
        "{}-{:x}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    if outcome.exists() {
        let existing = read_hosted_supervisor_receipt(outcome)
            .unwrap_or_else(|error| panic!("existing hosted receipt is unreadable: {error}"));
        assert_ne!(
            existing.nonce, nonce,
            "fresh hosted receipt nonce collided with the existing receipt"
        );
    }
    nonce
}

fn poll_for_outcome(
    path: &Path,
    expected_nonce: &str,
    attempts: usize,
) -> Result<SupervisorHostOutcome, PendingOutcome> {
    let mut pending = PendingOutcome::Missing;
    for _ in 0..attempts {
        match read_hosted_supervisor_receipt(path) {
            Ok(receipt) if receipt.nonce == expected_nonce => return Ok(receipt.outcome),
            Ok(receipt) => pending = PendingOutcome::DifferentNonce(receipt.nonce),
            Err(HostedSupervisorReceiptReadError::Missing { .. }) => {}
            Err(error) => pending = PendingOutcome::Malformed(error.to_string()),
        }
        thread::sleep(Duration::from_millis(5));
    }
    Err(pending)
}

fn wait_for_outcome(path: &Path, expected_nonce: &str) -> SupervisorHostOutcome {
    match poll_for_outcome(path, expected_nonce, 6_000) {
        Ok(outcome) => outcome,
        Err(PendingOutcome::Missing) => {
            panic!(
                "timed out waiting for hosted supervisor receipt at {}",
                path.display()
            )
        }
        Err(PendingOutcome::DifferentNonce(actual)) => panic!(
            "found hosted supervisor receipt for a different/prior run at {}: expected nonce {:?}, found {:?}",
            path.display(),
            expected_nonce,
            actual
        ),
        Err(PendingOutcome::Malformed(detail)) => {
            panic!(
                "found malformed hosted supervisor receipt at {}: {detail}",
                path.display()
            )
        }
    }
}

fn run_hosted_with_parent(journal: &TempJournal, pid: u32, ticks: u64) -> SupervisorHostOutcome {
    let outcome = journal.0.join("declared-parent.outcome");
    let nonce = next_receipt_nonce(&outcome);
    let status = Command::new(env!(
        "CARGO_BIN_EXE_solstone-core-hosted-supervisor-fixture"
    ))
    .args(["host-with-parent"])
    .arg(&journal.0)
    .arg(&outcome)
    .arg(&nonce)
    .arg(pid.to_string())
    .arg(ticks.to_string())
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .env("HOME", super::installation_binding::admit_for(&journal.0))
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
    .expect("declared-parent host runs");
    assert_eq!(status.code(), Some(75));
    let result = wait_for_outcome(&outcome, &nonce);
    assert!(!journal.0.join("health/supervisor.pid").exists());
    assert!(!journal.0.join("health/supervisor.ready").exists());
    result
}

#[test]
fn ac2_stale_receipt_cannot_be_accepted_for_a_new_nonce() {
    let journal = TempJournal::new();
    let outcome = journal.0.join("stale-hosted.outcome");
    let stale_nonce = "stale-receipt-nonce";
    write_hosted_supervisor_receipt(
        &outcome,
        stale_nonce,
        &SupervisorHostOutcome::OrderlyShutdown {
            cause: ShutdownCause::Signal(SupervisorSignal::SigTerm),
        },
    )
    .expect("write stale hosted receipt");

    let fresh_nonce = next_receipt_nonce(&outcome);
    assert_ne!(fresh_nonce, stale_nonce);
    match poll_for_outcome(&outcome, &fresh_nonce, 3) {
        Err(PendingOutcome::DifferentNonce(found)) => assert_eq!(found, stale_nonce),
        Ok(outcome) => panic!("stale receipt was accepted for fresh nonce: {outcome:?}"),
        Err(PendingOutcome::Missing) => panic!("stale receipt disappeared while polling"),
        Err(PendingOutcome::Malformed(error)) => panic!("stale receipt was malformed: {error}"),
    }
}

#[test]
fn ac4_parent_mismatch_refuses_before_lifecycle_publication() {
    let mut unrelated = Command::new(env!("CARGO_BIN_EXE_solstone-core-system-test-child"))
        .arg("never-ready")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("unrelated child");
    let unrelated_pid = unrelated.id();
    let unrelated_journal = TempJournal::new();
    let unrelated_outcome = run_hosted_with_parent(&unrelated_journal, unrelated_pid, 1);
    assert!(matches!(
        unrelated_outcome,
        SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::ParentLiveness(
                ParentAdmissionFailure::DirectParentMismatch { .. }
            )
        }
    ));
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(unrelated_pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    );
    let _ = unrelated.wait();

    let wrong_journal = TempJournal::new();
    let wrong_outcome = run_hosted_with_parent(&wrong_journal, u32::MAX, 1);
    assert!(matches!(
        wrong_outcome,
        SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::ParentLiveness(
                ParentAdmissionFailure::DirectParentMismatch { .. }
            )
        }
    ));

    let reused_journal = TempJournal::new();
    let reused_outcome = run_hosted_with_parent(&reused_journal, std::process::id(), 0);
    assert!(matches!(
        reused_outcome,
        SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::ParentLiveness(ParentAdmissionFailure::NotLiveOrReused)
        }
    ));
}

#[test]
fn ac4_parent_loss_before_readiness_aborts_the_pre_ready_lifecycle() {
    let journal = TempJournal::new();
    let home = super::installation_binding::admit_for(&journal.0);
    let marker = journal.0.join("pause-before-parent-check");
    let child_pid = journal.0.join("hosted-child.pid");
    let outcome = journal.0.join("hosted.outcome");
    let nonce = next_receipt_nonce(&outcome);
    let mut launcher = Command::new(env!(
        "CARGO_BIN_EXE_solstone-core-hosted-supervisor-fixture"
    ))
    .args(["launcher"])
    .arg(&journal.0)
    .arg(&child_pid)
    .arg(&outcome)
    .arg(&nonce)
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .env("HOME", home)
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
    .env(
        "SOLSTONE_SUPERVISOR_HOSTED_PAUSE_BEFORE_FINAL_PARENT_CHECK",
        &marker,
    )
    .spawn()
    .expect("paused hosted launcher starts");
    for _ in 0..1_600 {
        if marker.exists() {
            break;
        }
        if let Some(status) = launcher.try_wait().expect("launcher status") {
            panic!("launcher exited before pause: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        marker.exists(),
        "hosted runtime reached final parent-check barrier"
    );
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(launcher.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("kill launcher parent");
    fs::write(format!("{}.go", marker.display()), b"go\n").expect("release final parent check");
    let result = wait_for_outcome(&outcome, &nonce);
    assert!(matches!(
        result,
        SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::ParentLostBeforeReadiness(_)
        }
    ));
    assert!(!journal.0.join("health/supervisor.pid").exists());
    assert!(!journal.0.join("health/supervisor.ready").exists());
    let _ = launcher.wait();
}

#[test]
fn ac1_ac4_hosted_and_foreground_supervisors_record_the_resident_pid() {
    let foreground_journal = TempJournal::new();
    let mut foreground = start(&foreground_journal);
    wait_for_socket(
        &mut foreground,
        &foreground_journal.0.join("health/callosum.sock"),
    );
    let foreground_pid: u32 =
        fs::read_to_string(foreground_journal.0.join("health/supervisor.pid"))
            .expect("foreground pid")
            .trim()
            .parse()
            .expect("foreground numeric pid");
    assert_eq!(foreground_pid, foreground.id());

    let hosted_journal = TempJournal::new();
    let (mut launcher, child_pid_path, outcome_path, nonce) = start_hosted(&hosted_journal);
    wait_for_ready_from_launcher(&hosted_journal, &mut launcher);
    let hosted_pid: u32 = fs::read_to_string(hosted_journal.0.join("health/supervisor.pid"))
        .expect("hosted pid")
        .trim()
        .parse()
        .expect("hosted numeric pid");
    let hosted_child_pid: u32 = fs::read_to_string(&child_pid_path)
        .expect("hosted child pid")
        .trim()
        .parse()
        .expect("hosted numeric child pid");
    assert_eq!(hosted_pid, hosted_child_pid);
    assert_ne!(
        hosted_pid,
        launcher.id(),
        "identity must name the resident host, not its launcher"
    );

    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(launcher.id()).expect("launcher pid")),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("kill hosted parent");
    let outcome = wait_for_outcome(&outcome_path, &nonce);
    assert!(matches!(outcome, SupervisorHostOutcome::ParentLost { .. }));
    assert!(!hosted_journal.0.join("health/supervisor.ready").exists());
    assert!(!hosted_journal.0.join("health/supervisor.pid").exists());
    let _ = launcher.wait();
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
        if let Some(status) = child.try_wait().expect("supervisor status") {
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stderr.take() {
                pipe.read_to_string(&mut stderr)
                    .expect("read supervisor stderr");
            }
            panic!("supervisor exited during boot: {status}; stderr: {stderr}");
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
        .env("HOME", super::installation_binding::admit_for(&journal.0))
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
fn two_empty_journal_boots_never_both_reach_readiness() {
    let journal = TempJournal::new();
    let mut first = start(&journal);
    let first_pid = first.id();
    let mut second = start(&journal);
    let second_pid = second.id();
    let ready = journal.0.join("health/supervisor.ready");

    for _ in 0..1_600 {
        if ready.exists() {
            let admitted_pid: u32 = fs::read_to_string(journal.0.join("health/supervisor.pid"))
                .expect("ready supervisor pid")
                .trim()
                .parse()
                .expect("numeric ready supervisor pid");
            assert!(
                admitted_pid == first_pid || admitted_pid == second_pid,
                "only one of the two competing supervisors may publish readiness"
            );
            let loser = if admitted_pid == first_pid {
                &mut second
            } else {
                &mut first
            };
            for _ in 0..200 {
                if loser.try_wait().expect("loser status").is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            panic!("the non-ready competing supervisor remained live");
        }

        let first_exited = first.try_wait().expect("first status").is_some();
        let second_exited = second.try_wait().expect("second status").is_some();
        if first_exited && second_exited {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("competing supervisors did not settle");
}

#[test]
fn ac8_live_foreign_writer_blocks_boot_without_pid() {
    let journal = TempJournal::new();
    let (heartbeat_path, heartbeat_body) = foreign_heartbeat(&journal);
    age_heartbeat_with_startup_cushion(&heartbeat_path);
    let renewal =
        renew_heartbeat_when_wait_marker_appears(&journal, heartbeat_path, heartbeat_body);
    let stderr_path = journal.0.join("ac8-supervisor.stderr");
    let stderr_file = fs::File::create(&stderr_path).expect("supervisor stderr file");
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .env(
            "SOLSTONE_LOCAL_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("HOME", super::installation_binding::admit_for(&journal.0))
        .env(
            "SOLSTONE_SUPERVISOR_APP_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        );
    let status = wait_for_bounded_status(
        SupervisorGuard::new(command.spawn().expect("supervisor runs")),
        Duration::from_secs(30),
    );
    renewal.join().expect("foreign heartbeat renewal");
    let stderr = fs::read(stderr_path).expect("supervisor stderr");
    assert_eq!(status.code(), Some(75));
    assert!(String::from_utf8_lossy(&stderr).contains(
        "a recent heartbeat from another run is present.\nthe solstone app did not start while that heartbeat was still present.\nwait a moment, then try again."
    ));
    assert!(!journal.0.join("health/supervisor.pid").exists());
}

fn wait_for_backoff_record(
    child: &mut SupervisorGuard,
    journal: &TempJournal,
    minimum_attempts: u64,
) -> serde_json::Value {
    let backoff = journal.0.join("health/convey.backoff");
    for _ in 0..6_000 {
        if let Ok(body) = fs::read(&backoff) {
            let record: serde_json::Value = serde_json::from_slice(&body).expect("backoff JSON");
            if record["restart_attempts"].as_u64() >= Some(minimum_attempts) {
                return record;
            }
        }
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited while convey retried: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "convey.backoff did not reach {minimum_attempts} attempts: {}",
        backoff.display()
    );
}

#[test]
fn ac9_struggling_service_retries_indefinitely_without_ending_the_supervisor() {
    let prethreshold_journal = TempJournal::new();
    let prethreshold_attempts = prethreshold_journal.0.join("prethreshold-attempts");
    let prethreshold_argv = format!(
        "fail-count-then-park {} 4 1",
        prethreshold_attempts.display()
    );
    let mut prethreshold = start_with_convey_argv(&prethreshold_journal, Some(prethreshold_argv));
    for _ in 0..6_000 {
        if fs::read_to_string(&prethreshold_attempts)
            .ok()
            .is_some_and(|value| value.trim() == "5")
        {
            break;
        }
        if let Some(status) = prethreshold.try_wait().expect("supervisor status") {
            panic!("supervisor exited before the prethreshold fixture settled: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        fs::read_to_string(&prethreshold_attempts)
            .expect("prethreshold fixture attempts")
            .trim(),
        "5",
        "the fixture must settle after exactly four short-lived exits"
    );
    assert!(
        !prethreshold_journal
            .0
            .join("health/convey.backoff")
            .exists(),
        "four short-lived exits must not publish a struggling record"
    );
    let _ = prethreshold.shutdown_and_wait(Duration::from_secs(5));

    let journal = TempJournal::new();
    let mut child = start_with_convey_argv(&journal, Some("always-exit".to_owned()));
    let first = wait_for_backoff_record(&mut child, &journal, 5);
    let first_attempts = first["restart_attempts"].as_u64().expect("attempt count");
    let refreshed = wait_for_backoff_record(&mut child, &journal, first_attempts.max(10) + 1);
    assert!(
        refreshed["restart_attempts"].as_u64() > Some(first_attempts),
        "the struggling record must refresh after the threshold"
    );
    assert_eq!(refreshed["exit_code"], 1);
    assert!(
        child.try_wait().expect("supervisor status").is_none(),
        "an indefinitely retrying service must not end the supervisor"
    );
    assert!(
        journal.0.join("health/supervisor.ready").exists(),
        "the live supervisor remains observable during a managed-service backoff"
    );
}

#[test]
fn tempfail_service_retries_indefinitely_at_its_distinct_cadence() {
    let journal = TempJournal::new();
    let mut child = start_with_convey_argv(&journal, Some("always-tempfail".to_owned()));
    let record = wait_for_backoff_record(&mut child, &journal, 10);
    assert_eq!(record["exit_code"], 75);
    assert!(
        child.try_wait().expect("supervisor status").is_none(),
        "tempfail retries must not end the supervisor"
    );
    assert!(journal.0.join("health/supervisor.ready").exists());
}

#[test]
fn struggling_backoff_clears_after_a_healthy_run() {
    let journal = TempJournal::new();
    let attempts = journal.0.join("healthy-reset-attempts");
    let argv = format!(
        "fail-count-then-healthy-exit-then-park {} 10 1",
        attempts.display()
    );
    let mut child = start_with_convey_argv(&journal, Some(argv));
    let record = wait_for_backoff_record(&mut child, &journal, 10);
    assert_eq!(record["restart_attempts"], 10);

    let backoff = journal.0.join("health/convey.backoff");
    for _ in 0..15_000 {
        let settled = fs::read_to_string(&attempts)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .is_some_and(|count| count >= 12);
        if settled && !backoff.exists() {
            break;
        }
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited before a healthy run cleared backoff: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !backoff.exists(),
        "a sixty-second healthy run must clear the struggling record"
    );
    assert!(child.try_wait().expect("supervisor status").is_none());
    assert!(journal.0.join("health/supervisor.ready").exists());
}

#[test]
fn ac3_ac7_hosted_refusals_are_typed_and_leave_no_lifecycle_artifacts() {
    let unbound_journal = TempJournal::new();
    let unbound_home = super::installation_binding::admit_for(&unbound_journal.0);
    let record = super::installation_binding::admitted_record_path(&unbound_home);
    let marker = record
        .parent()
        .expect("record namespace")
        .join("adoption.marker");
    fs::remove_file(record).expect("remove admitted record");
    fs::remove_file(marker).expect("remove admitted marker");
    let (mut unbound, _, unbound_outcome, unbound_nonce) =
        start_hosted_with_home(&unbound_journal, unbound_home);
    let unbound_outcome = wait_for_outcome(&unbound_outcome, &unbound_nonce);
    let detail = match unbound_outcome {
        SupervisorHostOutcome::Refused {
            reason:
                SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(
                    detail,
                )),
        } => detail,
        outcome => panic!("expected installation binding refusal, got {outcome:?}"),
    };
    assert_eq!(detail, "saved binding: namespace record is missing");
    assert!(!unbound_journal.0.join("health/supervisor.pid").exists());
    assert!(!unbound_journal.0.join("health/supervisor.ready").exists());
    assert!(!unbound.wait().expect("unbound launcher wait").success());

    let conflict_journal = TempJournal::new();
    let (heartbeat_path, heartbeat_body) = foreign_heartbeat(&conflict_journal);
    age_heartbeat_with_startup_cushion(&heartbeat_path);
    let renewal =
        renew_heartbeat_when_wait_marker_appears(&conflict_journal, heartbeat_path, heartbeat_body);
    let (mut conflict, _, conflict_outcome, conflict_nonce) = start_hosted(&conflict_journal);
    let conflict_outcome = wait_for_outcome(&conflict_outcome, &conflict_nonce);
    renewal.join().expect("foreign heartbeat renewal");
    assert!(matches!(
        conflict_outcome,
        SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::AdmissionWaitTerminal
        }
    ));
    assert!(!conflict_journal.0.join("health/supervisor.pid").exists());
    assert!(!conflict_journal.0.join("health/supervisor.ready").exists());
    assert!(!conflict.wait().expect("conflict launcher wait").success());
}

#[test]
fn supervisor_installation_recovery_preserves_real_provider_causes() {
    let journal = TempJournal::new();

    let missing_home = supervisor_output(
        Path::new(env!("CARGO_BIN_EXE_solstone-core")),
        journal.0.as_os_str(),
        None,
    );
    assert_eq!(missing_home.status.code(), Some(75));
    assert!(missing_home.stdout.is_empty());
    assert_eq!(
        recovery_details(&missing_home.stderr),
        "home: HOME is not set"
    );

    let relative_home = supervisor_output(
        Path::new(env!("CARGO_BIN_EXE_solstone-core")),
        journal.0.as_os_str(),
        Some(Path::new("relative-home")),
    );
    assert_eq!(relative_home.status.code(), Some(75));
    assert!(relative_home.stdout.is_empty());
    assert_eq!(
        recovery_details(&relative_home.stderr),
        "owner storage: home must be absolute"
    );

    let home = super::installation_binding::admit_for(&journal.0);
    super::installation_binding::corrupt_admitted_record_checksum(&home);
    let checksum_mismatch = supervisor_output(
        Path::new(env!("CARGO_BIN_EXE_solstone-core")),
        journal.0.as_os_str(),
        Some(&home),
    );
    assert_eq!(checksum_mismatch.status.code(), Some(75));
    assert!(checksum_mismatch.stdout.is_empty());
    assert_eq!(
        checksum_mismatch.stderr,
        b"this installation couldn't be verified.\n\
run `journal setup` to check it. if setup finishes successfully, try again.\n\
details: saved binding: identity record checksum mismatch\n"
    );

    let journal_for_tokens = TempJournal::new();
    let token_home = super::installation_binding::admit_for(&journal_for_tokens.0);
    let relative_journal = supervisor_output(
        Path::new(env!("CARGO_BIN_EXE_solstone-core")),
        OsStr::new("relative-journal"),
        Some(&token_home),
    );
    assert_eq!(relative_journal.status.code(), Some(75));
    assert_eq!(
        recovery_details(&relative_journal.stderr),
        "journal token: path must be absolute and NUL-free"
    );
    let overlong_journal = OsString::from(format!("/{}", "a".repeat(4096)));
    let overlong = supervisor_output(
        Path::new(env!("CARGO_BIN_EXE_solstone-core")),
        overlong_journal.as_os_str(),
        Some(&token_home),
    );
    assert_eq!(overlong.status.code(), Some(75));
    assert_eq!(
        recovery_details(&overlong.stderr),
        "journal token: path exceeds 4096 bytes"
    );

    let different_journal = TempJournal::new();
    let mismatch = supervisor_output(
        Path::new(env!("CARGO_BIN_EXE_solstone-core")),
        different_journal.0.as_os_str(),
        Some(&token_home),
    );
    assert_eq!(mismatch.status.code(), Some(75));
    assert_eq!(
        recovery_details(&mismatch.stderr),
        "the saved installation binding is for a different journal"
    );
}

#[test]
fn supervisor_installation_recovery_sanitizes_hostile_executable_paths() {
    let temporary = tempfile::Builder::new()
        .prefix("solstone-unmarked-executable-")
        .tempdir_in("/var/tmp")
        .expect("isolated marker-miss binary root");
    let binary = copied_binary(&temporary.path().join("bin"));
    let journal = TempJournal::new();
    let home = tempfile::Builder::new()
        .prefix("solstone-marker-miss-home-")
        .tempdir_in("/var/tmp")
        .expect("absolute marker-miss home");
    let marker_miss = supervisor_output(&binary, journal.0.as_os_str(), Some(home.path()));
    assert_eq!(marker_miss.status.code(), Some(75));
    assert_eq!(
        recovery_details(&marker_miss.stderr),
        format!(
            "installation root: could not resolve installation identity root from {}",
            binary.parent().expect("binary parent").display()
        )
    );

    let cases = [
        ("newline-control", "newline-\n-\x1b"),
        ("backslash", "backslash-\\-component"),
    ];
    for (name, component) in cases {
        let (_temporary, binary) = hostile_binary(component);
        let journal = TempJournal::new();
        let home = tempfile::Builder::new()
            .prefix("solstone-hostile-home-")
            .tempdir_in("/var/tmp")
            .expect("absolute hostile home");
        let output = supervisor_output(&binary, journal.0.as_os_str(), Some(home.path()));
        assert_eq!(output.status.code(), Some(75), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        let details = recovery_details(&output.stderr);
        assert!(
            details.starts_with(
                "installation root: could not resolve installation identity root from "
            ),
            "{name}: {details}"
        );
        match name {
            "newline-control" => {
                assert!(details.contains("\\n"), "{details}");
                assert!(details.contains("\\x1b"), "{details}");
                assert!(!details.contains('\n'), "{details}");
                assert!(!details.contains('\x1b'), "{details}");
            }
            "backslash" => {
                assert!(details.contains("\\\\"), "{details}");
                assert!(!details.contains("\\\\\\\\"), "{details}");
            }
            _ => unreachable!("known hostile path fixture"),
        }
    }

    let escaped_component = "\x1b".repeat(240);
    let temporary = tempfile::Builder::new()
        .prefix("solstone-hostile-executable-")
        .tempdir_in("/var/tmp")
        .expect("isolated oversized binary root");
    let mut binary_dir = temporary.path().to_path_buf();
    // Three 240-byte components leave room for Darwin's 1024-byte PATH_MAX
    // once the temporary-root prefix and the copied executable name are added,
    // while their escaped rendering is still well beyond the 2048-character
    // diagnostic cap exercised below.
    for _ in 0..3 {
        binary_dir.push(&escaped_component);
    }
    let binary = copied_binary(&binary_dir);
    let journal = TempJournal::new();
    let home = tempfile::Builder::new()
        .prefix("solstone-hostile-home-")
        .tempdir_in("/var/tmp")
        .expect("absolute hostile home");
    let output = supervisor_output(&binary, journal.0.as_os_str(), Some(home.path()));
    assert_eq!(output.status.code(), Some(75));
    assert!(output.stdout.is_empty());
    let details = recovery_details(&output.stderr);
    assert!(details.ends_with(TRUNCATION_MARKER));
    assert_eq!(details.chars().count(), 2048);
}

#[test]
fn pre_ready_startup_failure_clears_supervisor_identity() {
    let journal = TempJournal::new();
    fs::create_dir_all(journal.0.join("health/callosum.sock"))
        .expect("make callosum path non-removable");
    let mut child = start(&journal);
    let status = child.wait().expect("supervisor returns startup refusal");
    assert_eq!(status.code(), Some(75));
    assert!(!journal.0.join("health/supervisor.pid").exists());
    assert!(!journal.0.join("health/supervisor.start_time").exists());
    assert!(!journal.0.join("health/supervisor.ready").exists());
}

#[test]
fn pre_ready_convey_wait_renews_heartbeat_before_readiness() {
    let journal = TempJournal::new();
    let parked = journal.0.join("convey-parked");
    let mut child =
        start_with_convey_argv(&journal, Some(format!("ready-park {}", parked.display())));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut first_heartbeat = None;
    loop {
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited during pre-ready renewal proof: {status}");
        }
        let heartbeat = fs::read_dir(journal.0.join("health/sync"))
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("solstone-v2-"))
            })
            .and_then(|entry| fs::read(entry.path()).ok());
        if let Some(heartbeat) = heartbeat {
            match first_heartbeat.as_ref() {
                Some(first) if first != &heartbeat => {
                    assert!(
                        !journal.0.join("health/supervisor.ready").exists(),
                        "heartbeat renewal must happen while startup is still pre-ready"
                    );
                    break;
                }
                None => first_heartbeat = Some(heartbeat),
                Some(_) => {}
            }
        }
        assert!(
            Instant::now() < deadline,
            "pre-ready Convey wait did not renew its heartbeat"
        );
        thread::sleep(Duration::from_millis(2));
    }
    child
        .shutdown_and_wait(Duration::from_secs(5))
        .expect("supervisor shuts down after renewal proof");
}

#[test]
fn final_pre_ready_sync_refuses_an_interleaved_writer_without_convey() {
    let journal = TempJournal::new();
    let pause = journal.0.join("pause-before-final-sync");
    let mut child = start_paused_before_readiness_with_args(&journal, &pause, &["--no-convey"]);
    for _ in 0..1_600 {
        if pause.exists() {
            break;
        }
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited before the final-sync pause: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(pause.exists(), "supervisor reached the final-sync pause");
    let (foreign, _) = foreign_heartbeat(&journal);
    fs::write(format!("{}.go", pause.display()), b"go\n").expect("release final-sync pause");

    let status = child.wait().expect("supervisor returns sync refusal");
    assert_eq!(status.code(), Some(75));
    assert!(foreign.exists(), "foreign heartbeat is preserved");
    assert!(!journal.0.join("health/supervisor.ready").exists());
    assert!(!journal.0.join("health/supervisor.pid").exists());
    assert!(!journal.0.join("health/supervisor.start_time").exists());
}

#[test]
fn readiness_publication_failure_clears_heartbeat_and_supervisor_identity() {
    let journal = TempJournal::new();
    fs::write(
        journal.0.join("config/schedules.json"),
        serde_json::to_vec(&serde_json::json!({"pre-ready": {
            "cmd": [env!("CARGO_BIN_EXE_solstone-core-system-test-child"), "lines"],
            "every": "1m"
        }}))
        .expect("schedule JSON"),
    )
    .expect("schedule fixture");
    fs::create_dir_all(journal.0.join("health/supervisor.ready"))
        .expect("make readiness target a directory");
    let pause = journal.0.join("pause-before-readiness");
    let mut child = start_paused_before_readiness(&journal, &pause);
    for _ in 0..1_600 {
        if pause.exists() {
            break;
        }
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited before the pre-readiness pause: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(pause.exists(), "supervisor reached the pre-readiness pause");
    let direct_door_path = journal.0.join("health/direct-door.json");
    let direct_door = serde_json::from_slice::<serde_json::Value>(
        &fs::read(&direct_door_path).expect("boot-time direct-door record"),
    )
    .expect("direct-door JSON");
    let direct_port = u16::try_from(
        direct_door["port"]
            .as_u64()
            .expect("direct-door port is numeric"),
    )
    .expect("direct-door port fits u16");
    assert_eq!(
        publish_direct_door(
            &journal.0,
            0,
            DirectDoorOutcome::Bound { port: direct_port },
        )
        .expect("simulate Convey bound publication"),
        DirectDoorPublishResult::Published
    );
    let source = SystemProcessInstanceSource;
    let coordinator = ParentLossLedger::open(&journal.0)
        .expect("open parent-loss ledger")
        .active_generation()
        .expect("read active parent-loss generation")
        .expect("coordinator active generation")
        .coordinator
        .expect("coordinator identity");
    let rows = match source.census() {
        InstanceCensus::Complete(rows) | InstanceCensus::Incomplete(rows) => rows,
    };
    let app_instances = rows
        .into_iter()
        .filter(|row| row.ppid == child.id() && row.instance != coordinator)
        .map(|row| row.instance)
        .collect::<Vec<_>>();
    assert!(
        !app_instances.is_empty(),
        "the teardown proof must observe at least one started app fixture"
    );
    let scheduler = journal.0.join("health/scheduler.json");
    let scheduled_work_completed = fs::read(&scheduler)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| value["pre-ready"]["last_status"] == "ok");
    assert!(
        !scheduled_work_completed,
        "pre-ready queue submissions must remain pending"
    );
    fs::write(format!("{}.go", pause.display()), b"go\n").expect("release readiness pause");
    let status = child.wait().expect("supervisor returns startup refusal");
    assert_eq!(status.code(), Some(75));
    assert!(!journal.0.join("health/supervisor.pid").exists());
    assert!(!journal.0.join("health/supervisor.start_time").exists());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(&direct_door_path).expect("withheld direct-door record")
        )
        .expect("withheld direct-door JSON"),
        serde_json::json!({"state": "withheld", "port": direct_port}),
        "pre-ready teardown must not leave a dead Convey door bound"
    );
    let sync_entries = fs::read_dir(journal.0.join("health/sync"))
        .expect("sync directory")
        .map(|entry| entry.expect("sync entry").file_name())
        .collect::<Vec<_>>();
    assert!(
        sync_entries.iter().all(|name| {
            !name
                .to_str()
                .is_some_and(|name| name.starts_with("solstone-v2-"))
        }),
        "a failed readiness publication must remove the retained self heartbeat"
    );
    for _ in 0..200 {
        if app_instances
            .iter()
            .all(|instance| matches!(source.observe(instance), InstanceVerdict::NotSameOrExited))
        {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("readiness refusal left an app fixture alive");
}
