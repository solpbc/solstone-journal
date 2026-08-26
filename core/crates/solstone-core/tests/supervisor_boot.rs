// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solstone_core::supervisor::{
    InstallationBindingRefusal, ShutdownCause, SupervisorBootRefusal, SupervisorHostOutcome,
    SupervisorSignal,
    receipt::{
        HostedSupervisorReceiptReadError, read_hosted_supervisor_receipt,
        write_hosted_supervisor_receipt,
    },
};
use solstone_core_system::lifecycle::ParentAdmissionFailure;

use super::{supervisor_guard::SupervisorGuard, temporary_root::temporary_root};

struct TempJournal(PathBuf);

impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = temporary_root().join(format!("solstone-core-supervisor-{stamp}"));
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
    match poll_for_outcome(path, expected_nonce, 1_600) {
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
        .env("HOME", super::installation_binding::admit_for(&journal.0))
        .env(
            "SOLSTONE_SUPERVISOR_APP_BINARY",
            env!("CARGO_BIN_EXE_solstone-core-system-test-child"),
        )
        .status()
        .expect("supervisor runs");
    assert_eq!(status.code(), Some(1));
    assert!(!journal.0.join("health/supervisor.pid").exists());
}

#[test]
fn ac9_give_up_is_service_local_and_does_not_end_the_supervisor() {
    let journal = TempJournal::new();
    let mut child = start_with_convey_argv(&journal, Some("always-exit".to_owned()));
    let failed = journal.0.join("health/convey.failed");
    for _ in 0..6_000 {
        if failed.exists() {
            break;
        }
        if let Some(status) = child.try_wait().expect("supervisor status") {
            panic!("supervisor exited while convey retried: {status}");
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        failed.exists(),
        "give-up must publish the failed-service artifact"
    );
    assert!(
        child.try_wait().expect("supervisor status").is_none(),
        "RestartPolicy::GiveUp remains a service-level outcome, not a supervisor exit"
    );
    assert!(
        journal.0.join("health/supervisor.ready").exists(),
        "the live supervisor remains observable after a managed-service give-up"
    );
}

#[test]
fn ac3_ac7_hosted_refusals_are_typed_and_leave_no_lifecycle_artifacts() {
    let unbound_journal = TempJournal::new();
    let unbound_home = unbound_journal.0.join("unbound-home");
    fs::create_dir_all(&unbound_home).expect("unbound home");
    let (mut unbound, _, unbound_outcome, unbound_nonce) =
        start_hosted_with_home(&unbound_journal, unbound_home);
    let unbound_outcome = wait_for_outcome(&unbound_outcome, &unbound_nonce);
    assert!(matches!(
        unbound_outcome,
        SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::InstallationBinding(
                InstallationBindingRefusal::LoadFailed
            )
        }
    ));
    assert!(!unbound_journal.0.join("health/supervisor.pid").exists());
    assert!(!unbound_journal.0.join("health/supervisor.ready").exists());
    assert!(!unbound.wait().expect("unbound launcher wait").success());

    let conflict_journal = TempJournal::new();
    foreign_heartbeat(&conflict_journal);
    let (mut conflict, _, conflict_outcome, conflict_nonce) = start_hosted(&conflict_journal);
    let conflict_outcome = wait_for_outcome(&conflict_outcome, &conflict_nonce);
    assert!(matches!(
        conflict_outcome,
        SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::SyncConflict
        }
    ));
    assert!(!conflict_journal.0.join("health/supervisor.pid").exists());
    assert!(!conflict_journal.0.join("health/supervisor.ready").exists());
    assert!(!conflict.wait().expect("conflict launcher wait").success());
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
