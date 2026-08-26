// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solstone_core_journal_io::{FlatDirectory, JournalRoot, create_or_open_flat_directory_bound};
#[cfg(target_os = "linux")]
use solstone_core_system::lifecycle::sweep_orphans;
use solstone_core_system::lifecycle::{
    Heartbeat, ShutdownDriver, ShutdownPhase, ShutdownRegime, SupervisorLifecycle, SyncCheckResult,
    SyncRescan, append_supervisor_log, compact_log_if_oversized, format_conflict_message,
    rescan_sync_read_only, sanitize_hostname, shutdown, sync_conflict_event, write_sync_heartbeat,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use solstone_core_system::lifecycle::{
    LifecycleError, OrphanSweepOutcome, is_supervisor_up, readiness_is_valid,
    recorded_supervisor_pid, wait_ready_with,
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
        let root = std::env::temp_dir().join(format!("solstone-lifecycle-{name}-{stamp}"));
        fs::create_dir_all(&root).expect("journal");
        Self { root }
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_for(path: &std::path::Path) {
    for _ in 0..200 {
        if fs::read(path).is_ok_and(|body| !body.is_empty()) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("fixture was not ready");
}

#[cfg(target_os = "linux")]
fn spawn_orphan(journal: &std::path::Path, ready: &std::path::Path, holder_mode: &str) {
    let status = Command::new(FIXTURE)
        .args([
            "orphan-sweep-spawner",
            journal.to_str().expect("utf8 journal"),
            ready.to_str().expect("utf8 ready path"),
            holder_mode,
        ])
        .status()
        .expect("spawn orphan fixture");
    assert!(status.success());
    wait_for(ready);
}

#[cfg(target_os = "linux")]
fn wait_for_orphan(pid: u32) {
    for _ in 0..200 {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if stat
            .rfind(')')
            .and_then(|end| stat[end + 1..].split_whitespace().nth(1))
            == Some("1")
        {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("fixture did not become an orphan");
}

#[cfg(target_os = "linux")]
fn process_is_live(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rfind(')')
        .and_then(|end| stat[end + 1..].split_whitespace().next())
        != Some("Z")
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_secs_f64()
}

fn heartbeat(journal: &std::path::Path, machine_id: &str) -> Heartbeat {
    Heartbeat {
        schema: 1,
        machine_id: machine_id.to_owned(),
        hostname: "foreign-host".to_owned(),
        pid: 2,
        wall_time: "now".to_owned(),
        solstone_version: "1".to_owned(),
        interval_seconds: 15,
        journal_path: journal.display().to_string(),
    }
}

fn bound_sync(journal: &std::path::Path) -> FlatDirectory {
    let root = JournalRoot::open(journal).expect("open journal root");
    let health = create_or_open_flat_directory_bound(
        &root,
        OsStr::new("health"),
        0o700,
        root.canonical_path(),
    )
    .expect("bind health");
    create_or_open_flat_directory_bound(
        &health,
        OsStr::new("sync"),
        0o700,
        &root.canonical_path().join("health"),
    )
    .expect("bind sync")
}

fn rescan_sync(
    journal: &std::path::Path,
    self_filename: &str,
    self_machine_id: &str,
    previous: Option<&solstone_core_system::lifecycle::SyncSnapshot>,
    now: f64,
) -> Result<SyncCheckResult, solstone_core_system::lifecycle::SyncScanFailure> {
    match rescan_sync_read_only(journal, self_filename, self_machine_id, previous, now)? {
        SyncRescan::Absent => panic!("sync directory should be present for this fixture"),
        SyncRescan::Complete(result) => Ok(result),
    }
}

fn write_fixture_heartbeat(journal: &std::path::Path, filename: &str, body: &[u8]) {
    let sync = journal.join("health/sync");
    fs::create_dir_all(&sync).expect("sync directory");
    fs::write(sync.join(filename), body).expect("fixture heartbeat");
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ac6_ac7_singleton_admission_is_real_process_safe() {
    let bed = Bed::new("singleton");
    let ready = bed.root.join("first-ready");
    let mut first = Command::new(FIXTURE)
        .args([
            "hold-supervisor-lock",
            bed.root.to_str().expect("utf8"),
            ready.to_str().expect("utf8"),
        ])
        .spawn()
        .expect("first fixture");
    wait_for(&ready);
    let result = bed.root.join("second-result");
    let second = Command::new(FIXTURE)
        .args([
            "try-supervisor-lock",
            bed.root.to_str().expect("utf8"),
            result.to_str().expect("utf8"),
        ])
        .status()
        .expect("second fixture");
    assert!(second.success());
    assert_eq!(
        fs::read_to_string(&result).expect("result"),
        "already-running"
    );
    first.kill().expect("kill holder");
    first.wait().expect("reap holder");
    let lifecycle = SupervisorLifecycle::boot(&bed.root).expect("lock released after process exit");
    assert_eq!(lifecycle.journal(), bed.root);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ac7_single_process_lease_lives_with_supervisor_value() {
    let bed = Bed::new("lease-lifetime");
    let lifecycle = SupervisorLifecycle::boot(&bed.root).expect("first boot");
    assert!(matches!(
        SupervisorLifecycle::boot(&bed.root),
        Err(LifecycleError::AlreadyRunning)
    ));
    drop(lifecycle);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ac5_boot_conflict_does_not_write_identity_and_shutdown_retains_only_conflicts() {
    let conflicting = Bed::new("boot-conflict");
    write_fixture_heartbeat(
        &conflicting.root,
        "foreign.check",
        &serde_json::to_vec(&heartbeat(&conflicting.root, "foreign")).expect("json"),
    );
    let Err(conflict) = SupervisorLifecycle::boot(&conflicting.root) else {
        panic!("expected sync conflict");
    };
    let LifecycleError::SyncConflict(result) = conflict else {
        panic!("expected sync conflict");
    };
    assert!(format_conflict_message(&result).contains("Refusing to start"));
    assert_eq!(
        sync_conflict_event(&result).expect("tick event").hostname,
        "foreign-host"
    );
    assert!(recorded_supervisor_pid(&conflicting.root).is_none());

    struct Driver;
    impl ShutdownDriver for Driver {
        fn reap_managed(
            &mut self,
            _: Duration,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
        fn drain_tasks(
            &mut self,
            _: Duration,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
        fn stop_children(
            &mut self,
            _: Option<Duration>,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
        fn join_bus(
            &mut self,
            _: Duration,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
    }
    let ordinary = Bed::new("ordinary-shutdown");
    let lifecycle = SupervisorLifecycle::boot(&ordinary.root).expect("boot");
    assert!(matches!(
        lifecycle.last_orphan_sweep(),
        OrphanSweepOutcome::Completed(report) if report.targeted == 0
    ));
    let heartbeat_path = fs::read_dir(ordinary.root.join("health/sync"))
        .expect("sync")
        .next()
        .expect("heartbeat")
        .expect("entry")
        .path();
    let outcome = lifecycle.shutdown(&mut Driver, ShutdownRegime::Standard, false);
    assert!(matches!(
        outcome.self_heartbeat,
        solstone_core_system::lifecycle::ArtifactClearOutcome::Cleared
    ));
    assert!(!heartbeat_path.exists());

    let conflict = Bed::new("conflict-shutdown");
    let lifecycle = SupervisorLifecycle::boot(&conflict.root).expect("boot");
    let heartbeat_path = fs::read_dir(conflict.root.join("health/sync"))
        .expect("sync")
        .next()
        .expect("heartbeat")
        .expect("entry")
        .path();
    let outcome = lifecycle.shutdown(&mut Driver, ShutdownRegime::Standard, true);
    assert!(matches!(
        outcome.self_heartbeat,
        solstone_core_system::lifecycle::ArtifactClearOutcome::Skipped
    ));
    assert!(heartbeat_path.exists());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ac8_ac17_identity_and_readiness_are_pid_bound() {
    let bed = Bed::new("readiness");
    let lifecycle = SupervisorLifecycle::boot(&bed.root).expect("boot");
    let mut extra = serde_json::Map::new();
    extra.insert("pid".to_owned(), serde_json::json!(0));
    extra.insert("ready_at".to_owned(), serde_json::json!(0));
    extra.insert("stage".to_owned(), serde_json::json!("ready"));
    lifecycle.signal_ready(123.0, extra).expect("marker");
    assert!(is_supervisor_up(&bed.root));
    assert!(readiness_is_valid(&bed.root));
    let marker: serde_json::Value = serde_json::from_slice(
        &fs::read(bed.root.join("health/supervisor.ready")).expect("marker bytes"),
    )
    .expect("marker json");
    assert_eq!(marker["pid"], std::process::id());
    assert_eq!(marker["ready_at"], 123.0);
    assert_eq!(marker["stage"], "ready");
    fs::write(bed.root.join("health/supervisor.pid"), "1").expect("tamper pid");
    assert!(!readiness_is_valid(&bed.root));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ac10_ac11_ac16_identity_tolerance_and_readiness_shape_rules() {
    let bed = Bed::new("identity-rules");
    let lifecycle = SupervisorLifecycle::boot(&bed.root).expect("boot");
    lifecycle
        .signal_ready(1.0, serde_json::Map::new())
        .expect("ready");
    let start_path = bed.root.join("health/supervisor.start_time");
    let actual: f64 = fs::read_to_string(&start_path)
        .expect("start")
        .parse()
        .expect("number");
    fs::write(&start_path, (actual + 1.49).to_string()).expect("under tolerance");
    assert!(is_supervisor_up(&bed.root));
    assert!(readiness_is_valid(&bed.root));
    fs::write(&start_path, (actual + 1.51).to_string()).expect("over tolerance");
    assert!(!is_supervisor_up(&bed.root));
    assert!(!readiness_is_valid(&bed.root));
    fs::write(&start_path, actual.to_string()).expect("restore identity");
    let ready_path = bed.root.join("health/supervisor.ready");
    let mut marker: serde_json::Value =
        serde_json::from_slice(&fs::read(&ready_path).expect("ready")).expect("json");
    marker["start_time"] = serde_json::json!(0.0);
    fs::write(&ready_path, serde_json::to_vec(&marker).expect("json")).expect("wrong marker start");
    assert!(readiness_is_valid(&bed.root));
    fs::remove_file(bed.root.join("health/supervisor.pid")).expect("remove pid");
    assert!(!is_supervisor_up(&bed.root));
    fs::write(bed.root.join("health/supervisor.pid"), "nope").expect("bad pid");
    assert!(!is_supervisor_up(&bed.root));
    fs::write(bed.root.join("health/supervisor.pid"), "99999999").expect("dead pid");
    assert!(!is_supervisor_up(&bed.root));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ac17_wait_ready_uses_injected_clock_and_poll() {
    let bed = Bed::new("wait-ready");
    let lifecycle = SupervisorLifecycle::boot(&bed.root).expect("boot");
    let ticks = std::cell::Cell::new(0_u64);
    let marker = wait_ready_with(
        &bed.root,
        Duration::from_secs(2),
        || Duration::from_secs(ticks.get()),
        || {
            lifecycle
                .signal_ready(1.0, serde_json::Map::new())
                .expect("ready");
            ticks.set(1);
        },
    );
    assert!(marker.is_some());
    let timeout = wait_ready_with(
        &Bed::new("wait-timeout").root,
        Duration::from_secs(1),
        || Duration::from_secs(ticks.get()),
        || ticks.set(ticks.get() + 1),
    );
    assert!(timeout.is_none());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn ac3_ac8_stale_pid_identity_is_rejected() {
    let bed = Bed::new("stale-pid");
    let lifecycle = SupervisorLifecycle::boot(&bed.root).expect("boot");
    lifecycle
        .signal_ready(1.0, serde_json::Map::new())
        .expect("ready");
    let start_path = bed.root.join("health/supervisor.start_time");
    let actual: f64 = fs::read_to_string(&start_path)
        .expect("start")
        .parse()
        .expect("number");
    fs::write(
        start_path,
        (actual + solstone_core_system::lifecycle::START_TIME_TOLERANCE_SECONDS + 0.1).to_string(),
    )
    .expect("stale identity");
    assert!(!is_supervisor_up(&bed.root));
    assert!(!readiness_is_valid(&bed.root));
}

#[cfg(target_os = "linux")]
#[test]
fn ac28_orphan_sweep_matches_journal_before_signalling() {
    let first = Bed::new("orphan-first");
    let second = Bed::new("orphan-second");
    let first_ready = first.root.join("first.pid");
    let second_ready = second.root.join("second.pid");
    for (journal, ready) in [(&first.root, &first_ready), (&second.root, &second_ready)] {
        let status = Command::new(FIXTURE)
            .args([
                "orphan-sweep-spawner",
                journal.to_str().expect("utf8"),
                ready.to_str().expect("utf8"),
            ])
            .status()
            .expect("spawn orphan fixture");
        assert!(status.success());
        wait_for(ready);
    }
    let first_pid: u32 = fs::read_to_string(&first_ready)
        .expect("first pid")
        .parse()
        .expect("numeric pid");
    let second_pid: u32 = fs::read_to_string(&second_ready)
        .expect("second pid")
        .parse()
        .expect("numeric pid");
    wait_for_orphan(first_pid);
    wait_for_orphan(second_pid);
    let outcome = sweep_orphans(&first.root, Duration::from_millis(20));
    let OrphanSweepOutcome::Completed(report) = outcome else {
        panic!("linux sweep must run");
    };
    assert_eq!(report.targeted, 1);
    for _ in 0..200 {
        if !process_is_live(first_pid) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!process_is_live(first_pid));
    assert!(process_is_live(second_pid));
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(second_pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ac30_orphan_sweep_reports_reaped_survivor_and_unresolvable_candidates() {
    let bed = Bed::new("orphan-report");
    let normal_ready = bed.root.join("normal.pid");
    let resistant_ready = bed.root.join("resistant.pid");
    let unresolved_ready = bed.root.join("unresolved.pid");
    let missing_journal = bed.root.join("missing-journal");
    for (journal, ready, mode) in [
        (&bed.root, &normal_ready, "orphan-sweep-holder"),
        (
            &bed.root,
            &resistant_ready,
            "orphan-sweep-holder-resists-term",
        ),
        (&missing_journal, &unresolved_ready, "orphan-sweep-holder"),
    ] {
        let status = Command::new(FIXTURE)
            .args([
                "orphan-sweep-spawner",
                journal.to_str().expect("utf8"),
                ready.to_str().expect("utf8"),
                mode,
            ])
            .status()
            .expect("spawn orphan fixture");
        assert!(status.success());
        wait_for(ready);
    }
    let normal_pid: u32 = fs::read_to_string(&normal_ready)
        .expect("normal pid")
        .parse()
        .expect("pid");
    let resistant_pid: u32 = fs::read_to_string(&resistant_ready)
        .expect("resistant pid")
        .parse()
        .expect("pid");
    let unresolved_pid: u32 = fs::read_to_string(&unresolved_ready)
        .expect("unresolved pid")
        .parse()
        .expect("pid");
    wait_for_orphan(normal_pid);
    wait_for_orphan(resistant_pid);
    wait_for_orphan(unresolved_pid);
    let OrphanSweepOutcome::Completed(report) = sweep_orphans(&bed.root, Duration::from_millis(20))
    else {
        panic!("linux sweep must run");
    };
    assert_eq!(report.targeted, 2);
    assert_eq!(report.reaped + report.survivors, 2);
    assert!(report.survivors >= 1);
    assert!(report.skipped_unresolvable >= 1);
    for pid in [normal_pid, resistant_pid, unresolved_pid] {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn ac28_orphan_sweep_matches_name_parent_and_uid() {
    let bed = Bed::new("orphan-macos");
    let ready = bed.root.join("orphan.pid");
    spawn_orphan(&bed.root, &ready, "orphan-sweep-holder");
    let pid: u32 = fs::read_to_string(&ready)
        .expect("pid")
        .parse()
        .expect("numeric pid");
    wait_for_orphan(pid);
    let OrphanSweepOutcome::Completed(report) = sweep_orphans(&bed.root, Duration::from_millis(20))
    else {
        panic!("macOS sweep must run");
    };
    assert_eq!(report.targeted, 1);
    for _ in 0..200 {
        if !process_is_live(pid) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert!(!process_is_live(pid));
}

#[test]
fn ac18_ac25_sync_preserves_foreign_writer_rules() {
    let bed = Bed::new("sync");
    let sync = bed.root.join("health/sync");
    fs::create_dir_all(&sync).expect("sync directory");
    let heartbeat = heartbeat(&bed.root, "foreign");
    write_fixture_heartbeat(
        &bed.root,
        "foreign.check",
        &serde_json::to_vec(&heartbeat).expect("heartbeat json"),
    );
    let now = now_seconds();
    let first = rescan_sync(&bed.root, "self.check", "self", None, now).expect("check");
    assert!(first.is_boot_conflict());
    assert!(!first.is_tick_conflict(None));
    assert!(first.is_tick_conflict(Some(&first.snapshot)));
    write_fixture_heartbeat(
        &bed.root,
        "same-machine.check",
        &serde_json::to_vec(&Heartbeat {
            machine_id: "self".to_owned(),
            ..heartbeat.clone()
        })
        .expect("self heartbeat json"),
    );
    fs::write(sync.join("broken.check"), b"{").expect("broken heartbeat");
    let malformed = rescan_sync(&bed.root, "self.check", "self", Some(&first.snapshot), now)
        .expect("check malformed");
    assert!(
        malformed
            .peer_observations
            .iter()
            .any(|peer| peer.heartbeat.is_none())
    );
    assert_eq!(sanitize_hostname("My Host!"), "My-Host");
    assert!(
        malformed
            .peer_observations
            .iter()
            .all(|peer| { peer.source_filename != OsStr::new("same-machine.check") })
    );
    write_fixture_heartbeat(
        &bed.root,
        "self.check",
        &serde_json::to_vec(&heartbeat).expect("filename guard json"),
    );
    let guarded = rescan_sync(&bed.root, "self.check", "", None, now).expect("guarded");
    assert!(
        guarded
            .peer_observations
            .iter()
            .all(|peer| { peer.source_filename != OsStr::new("self.check") })
    );
    let message = format_conflict_message(&first);
    let event = sync_conflict_event(&first).expect("tick event");
    assert!(message.contains("Refusing to start"));
    assert_eq!(event.machine_id_prefix, "foreign");
    assert_eq!(event.hostname, "foreign-host");
}

#[test]
fn heartbeat_filename_rejects_path_traversal() {
    let bed = Bed::new("heartbeat-path");
    let escaped = bed.root.join("escape.check");
    let sync = bound_sync(&bed.root);
    assert!(matches!(
        write_sync_heartbeat(&sync, "../escape.check", b"heartbeat"),
        Err(solstone_core_system::lifecycle::HeartbeatWriteError::InvalidFilename)
    ));
    assert!(!escaped.exists());
}

#[test]
fn ac19_ac20_stale_heartbeats_are_history_unless_changed_on_tick() {
    let bed = Bed::new("stale-sync");
    let body = serde_json::to_vec(&heartbeat(&bed.root, "foreign")).expect("json");
    write_fixture_heartbeat(&bed.root, "foreign.check", &body);
    let current =
        rescan_sync(&bed.root, "self.check", "self", None, now_seconds()).expect("current");
    let stale_now = now_seconds() + 61.0;
    let stale = rescan_sync(
        &bed.root,
        "self.check",
        "self",
        Some(&current.snapshot),
        stale_now,
    )
    .expect("stale");
    assert_eq!(stale.peer_observations.len(), 1);
    assert!(!stale.peer_observations[0].is_live);
    assert!(!stale.is_boot_conflict());
    let mut changed_snapshot = current.snapshot.clone();
    changed_snapshot
        .files
        .get_mut(OsStr::new("foreign.check"))
        .expect("file")
        .bytes = b"different".to_vec();
    let tick = rescan_sync(
        &bed.root,
        "self.check",
        "self",
        Some(&changed_snapshot),
        stale_now,
    )
    .expect("changed tick");
    assert!(tick.peer_observations[0].is_live);
    let boot = rescan_sync(&bed.root, "self.check", "self", None, stale_now).expect("boot");
    assert!(!boot.peer_observations[0].is_live);
}

#[test]
fn ac34_shutdown_order_is_explicit() {
    struct Driver(Vec<Option<Duration>>);
    impl ShutdownDriver for Driver {
        fn reap_managed(
            &mut self,
            cap: Duration,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            self.0.push(Some(cap));
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
        fn drain_tasks(
            &mut self,
            cap: Duration,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            self.0.push(Some(cap));
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
        fn stop_children(
            &mut self,
            cap: Option<Duration>,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            self.0.push(cap);
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
        fn join_bus(
            &mut self,
            cap: Duration,
        ) -> solstone_core_system::lifecycle::ShutdownDisposition {
            self.0.push(Some(cap));
            solstone_core_system::lifecycle::ShutdownDisposition::Orderly
        }
    }
    let mut app_driver = Driver(Vec::new());
    let report = shutdown(&mut app_driver, ShutdownRegime::AppSupervised);
    let reap = report
        .phases
        .iter()
        .position(|phase| *phase == ShutdownPhase::ReapManagedCompleted)
        .expect("reap complete");
    let drain = report
        .phases
        .iter()
        .position(|phase| *phase == ShutdownPhase::DrainTasksStarted)
        .expect("drain start");
    assert!(reap < drain);
    assert_eq!(
        app_driver.0,
        vec![
            Some(Duration::from_secs(3)),
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(2)),
        ]
    );
    let mut standard_driver = Driver(Vec::new());
    let standard = shutdown(&mut standard_driver, ShutdownRegime::Standard);
    assert_eq!(standard.phases.len(), 8);
    assert_eq!(
        standard_driver.0,
        vec![
            Some(Duration::from_secs(3)),
            Some(Duration::from_secs(10)),
            None,
            Some(Duration::from_secs(5)),
        ]
    );
}

#[test]
fn ac36_ac37_log_compaction_and_rotation_are_bounded() {
    let bed = Bed::new("logs");
    let log = bed.root.join("health/supervisor.log");
    fs::create_dir_all(log.parent().expect("parent")).expect("health");
    fs::write(&log, b"old\nkeep-one\nkeep-two\n").expect("log");
    compact_log_if_oversized(&log, 16).expect("compact");
    assert!(
        fs::read_to_string(&log)
            .expect("compact log")
            .contains("keep")
    );
    append_supervisor_log(&log, b"new\n", 1, 2).expect("rotate");
    assert!(log.with_extension("1").exists());
    assert_eq!(fs::read_to_string(&log).expect("new log"), "new\n");
}

#[test]
fn ac37_failed_log_compaction_preserves_original_bytes() {
    let bed = Bed::new("compact-failure");
    let log = bed.root.join("health/supervisor.log");
    fs::create_dir_all(log.parent().expect("parent")).expect("health");
    let original = b"old\nkeep-one\nkeep-two\n";
    fs::write(&log, original).expect("log");
    fs::create_dir(log.with_file_name("supervisor.log.compact")).expect("block compact sibling");
    assert!(compact_log_if_oversized(&log, 16).is_err());
    assert_eq!(fs::read(&log).expect("original"), original);
}
