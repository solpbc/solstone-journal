// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use solstone_core_system::process::{
    CAP_TERMINATION_TIMEOUT, DRAIN_JOIN_TIMEOUT, EXIT_TEMPFAIL, KILL_REAP_GRACE, ManagedProcess,
    OutputStream, ProcessEvent, ProcessEventSink, RestartPolicy, SERVICE_SHUTDOWN_TIMEOUT,
    SpawnError, SpawnOptions, TASK_QUEUE_SHUTDOWN_TIMEOUT, TerminationError, TerminationOutcome,
    describe_exit, exit_status_for_code,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_solstone-system-test-child");

struct Bed {
    root: PathBuf,
}

impl Bed {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-system-{name}-{stamp}"));
        fs::create_dir_all(&root).expect("temporary journal");
        Self { root }
    }

    fn spawn(&self, reference: &str, args: &[&str]) -> ManagedProcess {
        let mut cmd = vec![FIXTURE.to_owned()];
        cmd.extend(args.iter().map(|value| (*value).to_owned()));
        ManagedProcess::spawn(
            cmd,
            SpawnOptions {
                journal_root: self.root.clone(),
                reference: reference.to_owned(),
                day: Some("20260807".to_owned()),
                sink: None,
                environment: Default::default(),
            },
        )
        .expect("spawn fixture")
    }

    fn spawn_exact(&self, reference: &str, args: &[&str]) -> ManagedProcess {
        self.spawn_exact_with_environment(reference, args, BTreeMap::new())
    }

    fn spawn_exact_with_environment(
        &self,
        reference: &str,
        args: &[&str],
        environment: BTreeMap<OsString, OsString>,
    ) -> ManagedProcess {
        let mut cmd = vec![FIXTURE.to_owned()];
        cmd.extend(args.iter().map(|value| (*value).to_owned()));
        ManagedProcess::spawn_exact(
            cmd,
            SpawnOptions {
                journal_root: self.root.clone(),
                reference: reference.to_owned(),
                day: Some("20260807".to_owned()),
                sink: None,
                environment,
            },
        )
        .expect("spawn exact fixture")
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_for_ready(path: &std::path::Path) {
    for _ in 0..200 {
        if fs::read_to_string(path)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("fixture did not signal readiness");
}

fn process_is_gone(pid: u32) -> bool {
    let pid = i32::try_from(pid).expect("fixture pid fits i32");
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

#[derive(Default)]
struct Collector(Mutex<Vec<ProcessEvent>>);

impl ProcessEventSink for Collector {
    fn emit(&self, event: ProcessEvent) {
        self.0.lock().expect("collector lock").push(event);
    }
}

#[test]
fn ac13_escaped_setsid_descendant_is_reaped_by_its_snapshotted_pid() {
    let bed = Bed::new("escaped");
    let ready = bed.root.join("escaped-ready");
    let mut process = bed.spawn(
        "escaped",
        &["setsid-grandchild", ready.to_str().expect("utf8")],
    );
    wait_for_ready(&ready);
    let grandchild_pid: u32 = fs::read_to_string(&ready)
        .expect("read escaped grandchild pid")
        .trim()
        .parse()
        .expect("escaped grandchild published its pid");
    assert!(
        !process_is_gone(grandchild_pid),
        "fixture precondition: the escaped grandchild must be alive before termination"
    );

    let result = process.terminate(Duration::from_secs(1));
    assert!(
        matches!(
            result,
            Ok(TerminationOutcome::Graceful { .. })
                | Ok(TerminationOutcome::EscalatedAndReaped { .. })
        ),
        "termination result: {result:?}"
    );

    // The load-bearing half. A group-signal-only implementation never snapshots
    // descendants, so its survivor set is empty, so it also returns Graceful --
    // with this setsid'd grandchild still running. Asserting the outcome variant
    // alone does not discriminate between the two implementations; asserting the
    // pid does.
    assert!(
        process_is_gone(grandchild_pid),
        "escaped setsid descendant {grandchild_pid} survived termination"
    );
    process.cleanup();
}

#[test]
fn ac24_drop_reaps_snapshotted_setsid_descendant() {
    let bed = Bed::new("drop-escaped");
    let ready = bed.root.join("escaped-ready");
    let process = bed.spawn(
        "drop-escaped",
        &["setsid-grandchild", ready.to_str().expect("utf8")],
    );
    wait_for_ready(&ready);
    let grandchild_pid: u32 = fs::read_to_string(&ready)
        .expect("read escaped grandchild pid")
        .trim()
        .parse()
        .expect("escaped grandchild published its pid");
    assert!(
        !process_is_gone(grandchild_pid),
        "fixture precondition: the escaped grandchild must be alive before drop"
    );

    drop(process);
    assert!(
        process_is_gone(grandchild_pid),
        "escaped setsid descendant {grandchild_pid} survived Drop"
    );
}

#[test]
fn ac12_spawned_child_is_the_leader_of_its_own_process_group() {
    let bed = Bed::new("process-group");
    let mut process = bed.spawn("process-group", &["sleep"]);
    assert_eq!(
        process.pgid().expect("child process group"),
        process.pid() as i32
    );
    let _ = process.terminate(Duration::from_secs(1));
    process.cleanup();
}

#[test]
fn ac14_all_named_graceful_windows_allow_graceful_termination() {
    let bed = Bed::new("graceful-windows");
    for (reference, timeout) in [
        ("cap-window", CAP_TERMINATION_TIMEOUT),
        ("queue-shutdown-window", TASK_QUEUE_SHUTDOWN_TIMEOUT),
        ("service-window", SERVICE_SHUTDOWN_TIMEOUT),
    ] {
        let mut process = bed.spawn(reference, &["sleep"]);
        assert!(matches!(
            process.terminate(timeout),
            Ok(TerminationOutcome::Graceful { .. })
        ));
        process.cleanup();
    }
}

#[test]
fn ac14_escalated_descendant_is_reaped_with_the_named_kill_grace() {
    let bed = Bed::new("escalated-descendant");
    let ready = bed.root.join("resistant-ready");
    let mut process = bed.spawn(
        "escalated-descendant",
        &["term-resistant-descendant", ready.to_str().expect("utf8")],
    );
    wait_for_ready(&ready);
    let pid = process.pid();
    assert_eq!(KILL_REAP_GRACE, Duration::from_millis(500));
    assert!(matches!(
        process.terminate(Duration::from_millis(50)),
        Ok(TerminationOutcome::EscalatedAndReaped { .. })
    ));
    assert!(process_is_gone(pid));
    process.cleanup();
}

#[test]
fn ac14_parent_grace_timeout_is_distinct_from_proven_escalation() {
    let bed = Bed::new("timeout");
    let ready = bed.root.join("blocked-ready");
    let mut process = bed.spawn(
        "timeout",
        &["block-term-sleep", ready.to_str().expect("utf8")],
    );
    wait_for_ready(&ready);
    let pid = process.pid();
    assert!(matches!(
        process.terminate(Duration::ZERO),
        Err(TerminationError::ParentGraceTimeout)
    ));
    assert!(process_is_gone(pid));
    process.cleanup();
}

#[test]
fn ac18_daily_writer_creates_one_canonical_operational_log_without_symlinks() {
    let bed = Bed::new("links");
    let mut process = bed.spawn("links", &["lines"]);
    assert_eq!(process.wait().expect("fixture exits"), 0);
    process.cleanup();
    let day = bed.root.join("chronicle/20260807/health");
    let leaves = fs::read_dir(&day)
        .expect("day health directory")
        .map(|entry| entry.expect("health entry"))
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("oplog--"))
        .collect::<Vec<_>>();
    assert_eq!(leaves.len(), 1);
    let leaf = &leaves[0];
    assert!(leaf.file_name().to_string_lossy().starts_with("oplog--"));
    assert!(leaf.path().is_file());
    assert!(
        !fs::symlink_metadata(leaf.path())
            .expect("canonical leaf metadata")
            .file_type()
            .is_symlink()
    );
    assert!(
        fs::read_dir(&day)
            .expect("day health directory")
            .all(|entry| !entry
                .expect("health entry")
                .file_type()
                .expect("file type")
                .is_symlink())
    );
    assert!(!bed.root.join("health").exists());
}

#[test]
fn ac16_exit_descriptions_and_catchup_status_are_exact() {
    assert_eq!(describe_exit(0), "exit 0");
    assert_eq!(describe_exit(-15), "exit -15 / SIGTERM");
    let usr1 = -(nix::sys::signal::Signal::SIGUSR1 as i32);
    assert_eq!(describe_exit(usr1), format!("exit {usr1} / SIGUSR1"));
    assert_eq!(describe_exit(-999), "exit -999 / signal 999");
    assert_eq!(exit_status_for_code(0), "ok");
    assert_eq!(exit_status_for_code(66), "empty");
    assert_eq!(exit_status_for_code(1), "error");

    // Tempfail is a RESTART delay, never a status label. The catchup outcome
    // ledger's consumer branches on these exact strings and knows only
    // {ok, empty, error, timeout}, so minting a "tempfail" label here would
    // change that ledger silently. Named against the constant, not the integer.
    assert_eq!(exit_status_for_code(EXIT_TEMPFAIL), "error");
}

#[test]
fn ac16_managed_process_preserves_signal_exit_codes() {
    let bed = Bed::new("signal-exit");
    let mut process = bed.spawn("signal-exit", &["sleep"]);
    let pid = i32::try_from(process.pid()).expect("fixture pid fits i32");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    )
    .expect("signal fixture");

    let exit_code = loop {
        if let Some(exit_code) = process.poll().expect("poll fixture") {
            break exit_code;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(exit_code, -15);
    assert_eq!(describe_exit(exit_code), "exit -15 / SIGTERM");
    process.cleanup();
}

#[test]
fn ac17_restart_policy_clamps_and_tempfail_does_not_consume_attempt() {
    use solstone_core_system::process::{STRUGGLING_THRESHOLD, TEMPFAIL_DELAY};

    let mut policy = RestartPolicy::default();
    for expected in 1..=10 {
        assert_eq!(
            policy.decide_after_exit(75, Duration::ZERO),
            TEMPFAIL_DELAY,
            "tempfail {expected} must keep the 15s delay"
        );
        assert_eq!(policy.attempts(), 0, "tempfail must not consume attempts");
        assert_eq!(policy.unsuccessful_starts(), expected);
    }
    assert_eq!(policy.attempts(), 0);
    assert!(policy.unsuccessful_starts() > STRUGGLING_THRESHOLD);

    let mut schedule = RestartPolicy::default();
    assert_eq!(
        schedule.decide_after_exit(1, Duration::ZERO),
        Duration::ZERO
    );
    assert_eq!(
        schedule.decide_after_exit(1, Duration::ZERO),
        Duration::from_secs(1)
    );
    assert_eq!(
        schedule.decide_after_exit(1, Duration::ZERO),
        Duration::from_secs(5)
    );
    assert_eq!(
        schedule.decide_after_exit(1, Duration::ZERO),
        Duration::from_secs(5)
    );
    assert_eq!(
        schedule.decide_after_exit(1, Duration::from_secs(60)),
        Duration::ZERO
    );

    let mut reset = RestartPolicy::default();
    for _ in 0..(STRUGGLING_THRESHOLD - 1) {
        reset.decide_after_exit(1, Duration::ZERO);
    }
    assert_eq!(reset.unsuccessful_starts(), STRUGGLING_THRESHOLD - 1);
    assert_eq!(
        reset.decide_after_exit(1, Duration::from_secs(60)),
        Duration::ZERO
    );
    assert_eq!(reset.unsuccessful_starts(), 0);
    assert_eq!(
        reset.decide_after_exit(1, Duration::ZERO),
        Duration::from_secs(1)
    );
    assert_eq!(reset.unsuccessful_starts(), 1);
}

#[test]
fn ac20_spawn_line_and_exit_events_are_emitted_by_a_caller_owned_sink() {
    let bed = Bed::new("events");
    let collector = Arc::new(Collector::default());
    let mut process = ManagedProcess::spawn(
        vec![FIXTURE.to_owned(), "lines".to_owned()],
        SpawnOptions {
            journal_root: bed.root.clone(),
            reference: "events".to_owned(),
            day: Some("20260807".to_owned()),
            sink: Some(collector.clone()),
            environment: Default::default(),
        },
    )
    .expect("spawn fixture");
    let _ = process.wait().expect("fixture exits");
    process.cleanup();
    let events = collector.0.lock().expect("collector lock");
    assert!(matches!(events.first(), Some(ProcessEvent::Spawned { .. })));
    assert!(events.iter().any(|event| matches!(event, ProcessEvent::Line { stream: OutputStream::Stdout, line, .. } if line == "stdout-line")));
    assert!(events.iter().any(|event| matches!(event, ProcessEvent::Line { stream: OutputStream::Stderr, line, .. } if line == "stderr-line")));
    assert!(matches!(events.last(), Some(ProcessEvent::Exited { .. })));
}

#[test]
fn ac20_absent_sink_still_drains_child_output_and_writes_operational_log() {
    let bed = Bed::new("no-sink");
    let mut process = bed.spawn("no-sink", &["lines"]);
    let _ = process.wait().expect("fixture exits");
    let path = process.log_path();
    process.cleanup();
    let content = fs::read_to_string(path).expect("operational log");
    assert!(content.contains("stdout-line"));
    assert!(content.contains("stderr-line"));
}

#[test]
fn ac22_independent_managed_processes_run_concurrently_without_cross_talk() {
    let bed = Bed::new("concurrent");
    let mut first = bed.spawn("one", &["sleep"]);
    let mut second = bed.spawn("two", &["sleep"]);
    assert_ne!(first.pid(), second.pid());
    assert!(matches!(
        first.terminate(Duration::from_secs(1)),
        Ok(_) | Err(TerminationError::ParentGraceTimeout)
    ));
    assert_eq!(second.poll().expect("second child liveness"), None);
    assert!(matches!(
        second.terminate(Duration::from_secs(1)),
        Ok(_) | Err(TerminationError::ParentGraceTimeout)
    ));
    first.cleanup();
    second.cleanup();
}

#[test]
fn ac23_drop_terminates_a_live_child() {
    let bed = Bed::new("drop-live");
    let process = bed.spawn("drop-live", &["sleep"]);
    let pid = process.pid();
    assert!(
        !process_is_gone(pid),
        "fixture precondition: child is alive"
    );
    let started = Instant::now();
    drop(process);
    assert!(
        started.elapsed() < SERVICE_SHUTDOWN_TIMEOUT + DRAIN_JOIN_TIMEOUT + Duration::from_secs(1),
        "Drop must return within the named terminate+drain windows"
    );
    assert!(process_is_gone(pid), "live child survived Drop");
}

#[test]
fn ac25_drain_join_timeout_is_the_named_two_second_backstop() {
    assert_eq!(DRAIN_JOIN_TIMEOUT, Duration::from_secs(2));
}

#[test]
fn ac26_drop_after_reap_does_not_attempt_termination() {
    let bed = Bed::new("drop-reaped");
    let mut process = bed.spawn("drop-reaped", &["lines"]);
    let _ = process.wait().expect("fixture exits");
    process.cleanup();
    let started = Instant::now();
    drop(process);
    // Proxy: terminate() of a live child waits up to SERVICE_SHUTDOWN_TIMEOUT;
    // Drop after wait() returns immediately because try_wait already shows reaped.
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "Drop of an already-reaped child must not pay the terminate window"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ac27_linux_sigkill_of_spawner_kills_direct_child() {
    let bed = Bed::new("host-death-managed");
    let ready = bed.root.join("host-death-ready");
    let mut spawner = std::process::Command::new(FIXTURE)
        .args([
            "host-death-managed",
            ready.to_str().expect("utf8"),
            bed.root.to_str().expect("utf8"),
        ])
        .spawn()
        .expect("spawn host-death-managed fixture");
    wait_for_ready(&ready);
    let grandchild_pid: u32 = fs::read_to_string(&ready)
        .expect("read host-death child pid")
        .trim()
        .parse()
        .expect("host-death child published its pid");
    assert!(
        !process_is_gone(grandchild_pid),
        "fixture precondition: direct child is alive before SIGKILL"
    );
    let spawner_pid = spawner.id();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(spawner_pid).expect("spawner pid fits i32")),
        nix::sys::signal::Signal::SIGKILL,
    )
    .expect("sigkill host-death spawner");
    for _ in 0..200 {
        if process_is_gone(grandchild_pid) && process_is_gone(spawner_pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        process_is_gone(grandchild_pid),
        "direct child {grandchild_pid} survived SIGKILL of its spawner"
    );
    let _ = spawner.wait();
}

#[test]
fn ac28_exact_managed_process_terminates_without_process_group_fallback() {
    let bed = Bed::new("exact");
    let mut process = bed.spawn_exact("exact", &["sleep"]);
    assert!(matches!(
        process.terminate_exact(Duration::from_secs(1)),
        Ok(TerminationOutcome::Graceful { .. })
            | Ok(TerminationOutcome::EscalatedAndReaped { .. })
            | Err(TerminationError::ParentGraceTimeout)
    ));
    process.cleanup();
}

#[test]
fn ac29_exact_spawn_preserves_an_immediate_child_exit_code() {
    let bed = Bed::new("exact-immediate-exit");
    let mut environment = BTreeMap::new();
    environment.insert(
        OsString::from("SOLSTONE_TEST_EXACT_SPAWN_INSPECT_DELAY_MS"),
        OsString::from("100"),
    );
    let mut process =
        bed.spawn_exact_with_environment("exact-immediate-exit", &["always-exit"], environment);
    let exit_code = loop {
        if let Some(exit_code) = process.poll().expect("poll immediate exit") {
            break exit_code;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(exit_code, 1);
    assert!(matches!(
        process.terminate_exact(Duration::ZERO),
        Ok(TerminationOutcome::Graceful { exit_code: Some(1) })
    ));
    process.cleanup();
}

#[test]
fn ac30_exact_spawn_unverifiable_live_child_is_reaped_without_group_cleanup() {
    let bed = Bed::new("exact-unverifiable");
    let descendant_path = bed.root.join("exact-unverifiable-descendant.pid");
    let mut environment = BTreeMap::new();
    environment.insert(
        OsString::from("SOLSTONE_TEST_EXACT_SPAWN_FORCE_UNVERIFIABLE"),
        OsString::from("1"),
    );
    environment.insert(
        OsString::from("SOLSTONE_TEST_EXACT_SPAWN_INSPECT_DELAY_MS"),
        OsString::from("100"),
    );
    let result = ManagedProcess::spawn_exact(
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "sleep 30 & child=$!; printf '%s\\n' \"$child\" > \"$1\"; wait \"$child\"".to_owned(),
            "sh".to_owned(),
            descendant_path.display().to_string(),
        ],
        SpawnOptions {
            journal_root: bed.root.clone(),
            reference: "exact-unverifiable".to_owned(),
            day: Some("20260807".to_owned()),
            sink: None,
            environment,
        },
    );
    let Err(error) = result else {
        panic!("live unobservable child must refuse exact spawn");
    };
    let SpawnError::ExactInstanceUnavailable { pid } = error else {
        panic!("unexpected spawn error: {error}");
    };
    wait_for_ready(&descendant_path);
    let descendant: u32 = fs::read_to_string(&descendant_path)
        .expect("descendant pid")
        .trim()
        .parse()
        .expect("numeric descendant pid");
    assert!(
        process_is_gone(pid),
        "the refused exact child must be directly reaped before Drop can use legacy cleanup"
    );
    assert!(
        !process_is_gone(descendant),
        "a process-group fallback would have killed the shell descendant"
    );
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(descendant).expect("descendant pid fits i32")),
        nix::sys::signal::Signal::SIGKILL,
    );
    for _ in 0..100 {
        if process_is_gone(descendant) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("test cleanup did not reap shell descendant {descendant}");
}
