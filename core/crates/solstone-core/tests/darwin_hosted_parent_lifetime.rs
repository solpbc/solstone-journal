// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(target_os = "macos")]

// This needs its own integration-test binary: real parent death, kqueue
// delivery, libproc census, and process-global signal state cannot safely
// share an in-process harness with the ordinary supervisor tests.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, getpgid, getpgrp};
use solstone_core_system::lifecycle::{
    ParentLossLedger, ParentLossReaderOutcome, ParentLossTerminalDisposition,
    ParentLossUnresolvedReason, read_parent_loss_outcome,
};
use solstone_core_system::process::{
    InspectResult, InstanceCensus, InstanceVerdict, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource,
};

#[path = "support/installation_binding.rs"]
mod installation_binding;

const DEADLINE: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DarwinFixtureMode {
    Cooperative,
    HostileLateSpawner,
}

impl DarwinFixtureMode {
    const fn environment_value(self) -> &'static str {
        match self {
            Self::Cooperative => "cooperative",
            Self::HostileLateSpawner => "hostile-late-spawner",
        }
    }
}

struct TempJournal {
    directory: tempfile::TempDir,
}

impl TempJournal {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("solstone-darwin-parent-lifetime-")
            .tempdir_in("/var/tmp")
            .expect("temporary journal");
        fs::create_dir_all(directory.path().join("config")).expect("journal config directory");
        fs::create_dir_all(directory.path().join("health")).expect("journal health directory");
        fs::write(
            directory.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("journal config");
        Self { directory }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn health(&self, name: &str) -> PathBuf {
        self.path().join("health").join(name)
    }
}

struct ControlWitness {
    label: &'static str,
    child: Child,
    instance: ProcessInstance,
    port: Option<u16>,
}

impl ControlWitness {
    fn assert_intact(&self) {
        assert_same_live(self.instance, self.label);
        if let Some(port) = self.port {
            assert!(
                port_is_bound(port),
                "{} control port was released",
                self.label
            );
        }
    }
}

impl Drop for ControlWitness {
    fn drop(&mut self) {
        // All no-signal assertions run before Drop. This is only test cleanup.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture_binary() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-hosted-supervisor-fixture")
}

fn system_test_child() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-system-test-child")
}

fn reserve_ephemeral_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral loopback port");
    listener.local_addr().expect("loopback address").port()
}

fn port_is_bound(port: u16) -> bool {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            drop(listener);
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => true,
        Err(error) => panic!("could not inspect loopback port {port}: {error}"),
    }
}

fn wait_until(mut check: impl FnMut() -> bool, label: &str) {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {label}");
}

fn wait_until_before(deadline: Instant, mut check: impl FnMut() -> bool, label: &str) {
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {label}");
}

fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn pid_file_is_ready(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| contents.trim().parse::<u32>().ok())
        .is_some()
}

fn current_instance(pid: u32, label: &str) -> ProcessInstance {
    match SystemProcessInstanceSource.inspect(pid) {
        InspectResult::Present { instance, .. } => instance,
        observation => panic!("{label} identity was not live: {observation:?}"),
    }
}

fn assert_same_live(instance: ProcessInstance, label: &str) {
    assert!(
        matches!(
            SystemProcessInstanceSource.observe(&instance),
            InstanceVerdict::SameLive { .. }
        ),
        "{label} was signalled, exited, or replaced"
    );
}

fn instance_is_gone(instance: ProcessInstance) -> bool {
    matches!(
        SystemProcessInstanceSource.observe(&instance),
        InstanceVerdict::NotSameOrExited
    )
}

fn assert_tree_census_complete(root: ProcessInstance, label: &str) {
    match SystemProcessInstanceSource.census_tree(root.pid, None) {
        InstanceCensus::Complete(_) => {}
        InstanceCensus::Incomplete(rows) => panic!(
            "{label} process tree was not completely observable before parent loss: {rows:?}"
        ),
    }
}

fn same_pid_with_different_birth(instance: ProcessInstance) -> ProcessInstance {
    let mut wire = serde_json::to_value(instance).expect("serialize process instance");
    let birth = wire
        .get_mut("birth")
        .and_then(serde_json::Value::as_object_mut)
        .expect("serialized process birth");
    let epoch_micros = birth
        .get("epoch_micros")
        .and_then(serde_json::Value::as_i64)
        .expect("macOS birth token");
    birth.insert(
        "epoch_micros".to_owned(),
        serde_json::Value::from(epoch_micros.saturating_add(1)),
    );
    serde_json::from_value(wire).expect("different-birth process instance")
}

fn assert_outer_controls_intact(controls: &[&ControlWitness]) {
    for control in controls {
        control.assert_intact();
    }
    let caller = u32::try_from(std::process::id()).expect("caller PID");
    assert_same_live(
        current_instance(caller, "outer test caller"),
        "outer test caller",
    );
    match kill(Pid::from_raw(1), None) {
        Ok(()) | Err(Errno::EPERM) => {}
        Err(error) => panic!("PID 1 was not live: {error}"),
    }
    let caller_pid = Pid::from_raw(i32::try_from(caller).expect("caller PID fits i32"));
    assert_eq!(
        getpgid(Some(caller_pid)).expect("caller process group"),
        getpgrp(),
        "the caller process group changed"
    );
}

fn spawn_control(
    label: &'static str,
    mode: &str,
    arguments: &[String],
    ready: PathBuf,
    port: Option<u16>,
) -> ControlWitness {
    let child = Command::new(fixture_binary())
        .arg(mode)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("spawn {label}: {error}"));
    wait_until(|| pid_file_is_ready(&ready), label);
    assert_eq!(read_pid(&ready), child.id(), "{label} readiness PID");
    ControlWitness {
        label,
        instance: current_instance(child.id(), label),
        child,
        port,
    }
}

fn start_hosted_supervisor(
    journal: &TempJournal,
    convey_port: u16,
    mode: DarwinFixtureMode,
) -> (Child, PathBuf) {
    let supervisor_pid = journal.health("darwin-hosted-supervisor.pid");
    let outcome = journal.health("darwin-hosted-supervisor.outcome");
    let convey_ready = journal.health("darwin-convey.ready");
    let convey_argv = format!("ready-sleep {} {convey_port}", convey_ready.display());
    let launcher = Command::new(fixture_binary())
        .arg("launcher")
        .arg(journal.path())
        .arg(&supervisor_pid)
        .arg(outcome)
        .arg("darwin-parent-lifetime")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("HOME", installation_binding::admit_for(journal.path()))
        .env("SOLSTONE_JOURNAL", journal.path())
        .env("SOLSTONE_LOCAL_BINARY", system_test_child())
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture_binary())
        .env("SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV", convey_argv)
        .env("SOLSTONE_DARWIN_PARENT_LIFETIME_FIXTURE", "1")
        .env(
            "SOLSTONE_DARWIN_PARENT_LIFETIME_MODE",
            mode.environment_value(),
        )
        .spawn()
        .expect("hosted supervisor launcher");
    (launcher, supervisor_pid)
}

fn kill_supervisor_exact(supervisor: ProcessInstance) {
    assert_same_live(supervisor, "hosted supervisor before SIGKILL");
    // This is intentionally a direct PID signal; this harness never calls
    // killpg or otherwise targets a process group.
    kill(
        Pid::from_raw(i32::try_from(supervisor.pid).expect("supervisor PID fits i32")),
        Signal::SIGKILL,
    )
    .expect("SIGKILL the exact hosted supervisor PID");
}

fn assert_sealed_ledger_contains(
    ledger: &ParentLossLedger,
    generation: u64,
    identity: ProcessInstance,
    label: &str,
) {
    let entries: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(ledger.sealed_ledger_path(generation)).expect("sealed parent-loss ledger"),
    )
    .expect("sealed parent-loss ledger JSON");
    let instance = serde_json::to_value(identity).expect("process identity JSON");
    assert!(
        entries.iter().any(|entry| {
            entry
                .get("identity")
                .and_then(|identity| identity.get("instance"))
                == Some(&instance)
        }),
        "sealed ledger must include the production-boundary Cortex descendant {label}"
    );
}

fn exercise_parent_loss_twin(mode: DarwinFixtureMode) {
    let journal = TempJournal::new();
    let owned_convey_port = reserve_ephemeral_port();
    let control_port = reserve_ephemeral_port();
    let lookalike_ready = journal.health("darwin-lookalike.pid");
    let wrong_start_ready = journal.health("darwin-wrong-start.pid");
    let missing_provenance_ready = journal.health("darwin-missing-provenance.pid");
    let reused_pid_ready = journal.health("darwin-reused-pid.pid");

    let lookalike = spawn_control(
        "unadmitted same-command lookalike",
        "darwin-lookalike",
        &[
            control_port.to_string(),
            lookalike_ready.display().to_string(),
        ],
        lookalike_ready,
        Some(control_port),
    );
    let wrong_start = spawn_control(
        "wrong-start-time lookalike",
        "darwin-control",
        &[wrong_start_ready.display().to_string()],
        wrong_start_ready,
        None,
    );
    let missing_provenance = spawn_control(
        "missing-provenance control",
        "darwin-control",
        &[missing_provenance_ready.display().to_string()],
        missing_provenance_ready,
        None,
    );
    let reused_pid = spawn_control(
        "same-PID different-birth reuse control",
        "darwin-control",
        &[reused_pid_ready.display().to_string()],
        reused_pid_ready,
        None,
    );
    let controls = [&lookalike, &wrong_start, &missing_provenance, &reused_pid];
    // Darwin cannot force the global PID allocator to reuse one chosen PID
    // deterministically. A real live control with its same-PID, different-birth
    // identity is the exact native observation a reuse would present; the
    // injected census unit test covers the allocator-independent race itself.
    assert!(matches!(
        SystemProcessInstanceSource.observe(&same_pid_with_different_birth(wrong_start.instance)),
        InstanceVerdict::NotSameOrExited
    ));
    assert!(matches!(
        SystemProcessInstanceSource.observe(&same_pid_with_different_birth(reused_pid.instance)),
        InstanceVerdict::NotSameOrExited
    ));
    assert_outer_controls_intact(&controls);

    let (mut launcher, supervisor_pid_path) =
        start_hosted_supervisor(&journal, owned_convey_port, mode);
    let convey_ready = journal.health("darwin-convey.ready");
    let sense_ready = journal.health("fixture-sense.marker");
    let cortex_ready = journal.health("fixture-cortex.marker");
    let spl_ready = journal.health("fixture-spl.marker");
    let talent_ready = journal.health("darwin-talent-worker.pid");
    let late_spawner_ready = journal.health("darwin-late-spawner.pid");
    let late_descendant_ready = journal.health("darwin-late-descendant.pid");
    wait_until(
        || {
            assert_outer_controls_intact(&controls);
            if let Some(status) = launcher.try_wait().expect("hosted launcher status") {
                panic!("hosted launcher exited before service readiness: {status}");
            }
            pid_file_is_ready(&supervisor_pid_path)
                && pid_file_is_ready(&convey_ready)
                && pid_file_is_ready(&sense_ready)
                && pid_file_is_ready(&cortex_ready)
                && pid_file_is_ready(&spl_ready)
                && pid_file_is_ready(&talent_ready)
                && (mode == DarwinFixtureMode::Cooperative
                    || pid_file_is_ready(&late_spawner_ready))
        },
        "hosted service tree readiness",
    );
    assert!(port_is_bound(owned_convey_port), "owned Convey port bound");
    assert!(
        !late_descendant_ready.exists(),
        "late descendant must not exist before parent loss"
    );

    let supervisor = current_instance(read_pid(&supervisor_pid_path), "hosted supervisor");
    let mut owned = vec![
        supervisor,
        current_instance(read_pid(&convey_ready), "owned Convey"),
        current_instance(read_pid(&sense_ready), "owned Sense"),
        current_instance(read_pid(&cortex_ready), "owned Cortex"),
        current_instance(read_pid(&spl_ready), "owned SPL"),
        current_instance(read_pid(&talent_ready), "Cortex talent worker"),
    ];
    if mode == DarwinFixtureMode::HostileLateSpawner {
        owned.push(current_instance(
            read_pid(&late_spawner_ready),
            "Cortex late-child spawner",
        ));
    }
    assert_tree_census_complete(owned[3], "Cortex");

    kill_supervisor_exact(supervisor);
    wait_until(
        || instance_is_gone(supervisor),
        "confirmed supervisor death",
    );
    let terminal_deadline = Instant::now() + DEADLINE;
    match mode {
        DarwinFixtureMode::Cooperative => {
            let mut terminal = None;
            wait_until_before(
                terminal_deadline,
                || {
                    assert_outer_controls_intact(&controls);
                    if let Ok(outcome) = read_parent_loss_outcome(journal.path()) {
                        match outcome {
                            outcome @ (ParentLossReaderOutcome::Completed { .. }
                            | ParentLossReaderOutcome::Unresolved { .. }
                            | ParentLossReaderOutcome::RetiredExpected { .. }
                            | ParentLossReaderOutcome::CancelledBeforeAdmission {
                                ..
                            }) => {
                                terminal = Some(outcome);
                                true
                            }
                            _ => false,
                        }
                    } else {
                        false
                    }
                },
                "completed coordinator terminal record",
            );
            assert!(
                matches!(terminal, Some(ParentLossReaderOutcome::Completed { .. })),
                "cooperative twin must complete, got {terminal:?}"
            );
            wait_until_before(
                terminal_deadline,
                || {
                    assert_outer_controls_intact(&controls);
                    owned.iter().copied().all(instance_is_gone)
                },
                "owned supervisor and descendant retirement",
            );
            TcpListener::bind(("127.0.0.1", owned_convey_port))
                .expect("owned Convey port released after parent loss");

            let ledger = ParentLossLedger::open(journal.path()).expect("parent-loss ledger");
            let active = ledger
                .active_generation()
                .expect("active generation")
                .expect("completed generation");
            let record = ledger
                .record(active.generation)
                .expect("terminal record")
                .expect("terminal record exists");
            let ParentLossTerminalDisposition::Completed {
                sealed_ledger_digest,
            } = record.terminal.expect("completed terminal disposition")
            else {
                panic!("cooperative twin must record Completed");
            };
            assert_eq!(
                record.sealed_ledger_digest.as_deref(),
                Some(sealed_ledger_digest.as_str())
            );
            assert_sealed_ledger_contains(&ledger, active.generation, owned[5], "talent worker");
            let successor = ledger
                .reserve_generation(current_instance(std::process::id(), "test successor"), [])
                .expect("exactly one successor generation is eligible");
            assert_eq!(successor.generation, active.generation + 1);
            assert!(
                ledger
                    .reserve_generation(
                        current_instance(std::process::id(), "second successor"),
                        []
                    )
                    .is_err()
            );
        }
        DarwinFixtureMode::HostileLateSpawner => {
            let mut terminal = None;
            wait_until_before(
                terminal_deadline,
                || {
                    assert_outer_controls_intact(&controls);
                    if let Ok(outcome @ ParentLossReaderOutcome::Unresolved { .. }) =
                        read_parent_loss_outcome(journal.path())
                    {
                        terminal = Some(outcome);
                        true
                    } else {
                        false
                    }
                },
                "deadline-bounded unresolved coordinator terminal record",
            );
            assert!(
                matches!(
                    terminal,
                    Some(ParentLossReaderOutcome::Unresolved {
                        reason: ParentLossUnresolvedReason::RetirementDeadlineExceeded {
                            deadline_seconds: 15
                        },
                        ..
                    })
                ),
                "hostile twin must become deadline-bounded unresolved, got {terminal:?}"
            );
            let ledger = ParentLossLedger::open(journal.path()).expect("parent-loss ledger");
            let active = ledger
                .active_generation()
                .expect("active generation")
                .expect("unresolved generation");
            let coordinator = active.coordinator.expect("coordinator identity");
            assert_sealed_ledger_contains(&ledger, active.generation, owned[5], "talent worker");
            assert_sealed_ledger_contains(
                &ledger,
                active.generation,
                *owned.last().expect("hostile late spawner"),
                "late spawner",
            );
            wait_until_before(
                terminal_deadline,
                || instance_is_gone(coordinator),
                "coordinator exits after durable unresolved result",
            );
            assert!(
                ledger
                    .reserve_generation(
                        current_instance(std::process::id(), "blocked successor"),
                        []
                    )
                    .is_err()
            );
            assert!(
                late_descendant_ready.exists(),
                "hostile descendant was spawned after TERM"
            );
            let late_descendant =
                current_instance(read_pid(&late_descendant_ready), "late indirect descendant");
            kill(
                Pid::from_raw(i32::try_from(late_descendant.pid).expect("late descendant PID")),
                Signal::SIGKILL,
            )
            .expect("test cleanup SIGKILL for hostile late descendant");
            wait_until(
                || instance_is_gone(late_descendant),
                "hostile descendant cleanup",
            );
            let late_spawner = *owned.last().expect("hostile late spawner");
            assert_same_live(late_spawner, "hostile late spawner before cleanup");
            kill(
                Pid::from_raw(i32::try_from(late_spawner.pid).expect("late spawner PID")),
                Signal::SIGKILL,
            )
            .expect("test cleanup SIGKILL for hostile late spawner");
            wait_until(
                || instance_is_gone(late_spawner),
                "hostile late spawner cleanup",
            );
            let talent_worker = owned[5];
            assert_same_live(talent_worker, "hostile talent worker before cleanup");
            kill(
                Pid::from_raw(i32::try_from(talent_worker.pid).expect("talent worker PID")),
                Signal::SIGKILL,
            )
            .expect("test cleanup SIGKILL for hostile talent worker");
            wait_until(
                || instance_is_gone(talent_worker),
                "hostile talent worker cleanup",
            );
        }
    }
    assert_outer_controls_intact(&controls);
    let _ = launcher.wait();
}

#[test]
fn ac5_cooperative_hosted_parent_loss_completes_the_sealed_darwin_tree() {
    exercise_parent_loss_twin(DarwinFixtureMode::Cooperative);
}

#[test]
fn ac5_hostile_reparented_descendant_becomes_deadline_unresolved() {
    exercise_parent_loss_twin(DarwinFixtureMode::HostileLateSpawner);
}
