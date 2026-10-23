// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

#[path = "support/installation_binding.rs"]
mod installation_binding;
#[path = "support/speakers_analyze_stub.rs"]
mod speakers_analyze_stub;
#[path = "support/supervisor_guard.rs"]
mod supervisor_guard;
use supervisor_guard::SupervisorGuard;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use solstone_core_system::lifecycle::{
    AdmissionFinding, AdmissionIdentity, AdmissionIntent, AdmissionResult, AdmissionResultState,
    HostedServiceKind, ParentLossGenerationRecord, ParentLossLedger,
    read_parent_loss_admission_spawn_identity, write_parent_loss_admission_intent,
    write_parent_loss_admission_result, write_parent_loss_admission_spawn_identity,
};
use solstone_core_system::process::{
    InspectResult, InstanceVerdict, ProcessBirth, ProcessInstance, ProcessInstanceSource,
    SignalKind, SystemProcessInstanceSource, signal_exact_instance,
};

// A killed pre-allocation supervisor can leave its 60s startup heartbeat.
// Allow that real admission quarantine plus the unchanged helper ready window.
const RESTART_READY_WAIT: Duration = Duration::from_secs(90);

static NEXT_JOURNAL_ID: AtomicU64 = AtomicU64::new(0);

struct TempJournal {
    root: PathBuf,
}

impl TempJournal {
    fn new(name: &str) -> Self {
        let seq = NEXT_JOURNAL_ID.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("solstone-closure-resume-{name}-{seq}-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config dir");
        fs::create_dir_all(root.join("health")).expect("health dir");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("journal config");
        Self { root }
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        if thread::panicking() {
            eprintln!(
                "retained failed parent-loss fixture: {}",
                self.root.display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn synth_instance(pid: u32, birth: u64) -> ProcessInstance {
    ProcessInstance {
        pid,
        birth: ProcessBirth::linux(birth, 1, 100),
    }
}

struct CloserGuard(ProcessInstance);

impl std::ops::Deref for CloserGuard {
    type Target = ProcessInstance;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for CloserGuard {
    fn drop(&mut self) {
        let source = SystemProcessInstanceSource;
        if matches!(source.observe(&self.0), InstanceVerdict::SameLive { .. }) {
            let _ = signal_exact_instance(self.0, SignalKind::Kill, &source);
            let until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < until
                && matches!(source.observe(&self.0), InstanceVerdict::SameLive { .. })
            {
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn try_capture_live_closer(supervisor_pid: u32) -> Option<CloserGuard> {
    // `ps` provides the same direct-child/argv check on Linux and macOS.
    let output = Command::new("ps")
        .args(["-Aww", "-o", "pid=,ppid=,command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let listing = String::from_utf8(output.stdout).ok()?;
    let mut matches = listing.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse::<u32>().ok()?;
        let ppid = fields.next()?.parse::<u32>().ok()?;
        (ppid == supervisor_pid && fields.any(|a| a == "__parent-loss-coordinator")).then_some(pid)
    });
    let pid = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    match SystemProcessInstanceSource.inspect(pid) {
        InspectResult::Present { instance, .. } => Some(CloserGuard(instance)),
        _ => None,
    }
}

fn capture_live_closer(supervisor_pid: u32) -> CloserGuard {
    try_capture_live_closer(supervisor_pid).unwrap_or_else(|| {
        panic!("expected exactly one live coordinator child for supervisor pid {supervisor_pid}")
    })
}

fn prove_closer_gone(closer: &ProcessInstance) {
    let source = SystemProcessInstanceSource;
    match source.observe(closer) {
        InstanceVerdict::NotSameOrExited => {}
        InstanceVerdict::SameLive { .. } => {
            let _ = signal_exact_instance(*closer, SignalKind::Kill, &source);
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut gone = false;
            while Instant::now() < deadline {
                match source.observe(closer) {
                    InstanceVerdict::NotSameOrExited => {
                        gone = true;
                        break;
                    }
                    InstanceVerdict::Unverifiable => {
                        panic!("closer {closer:?} became Unverifiable after SIGKILL")
                    }
                    InstanceVerdict::SameLive { .. } => {}
                }
                thread::sleep(Duration::from_millis(20));
            }
            if !gone {
                panic!("closer {closer:?} still live after 5s");
            }
        }
        InstanceVerdict::Unverifiable => panic!("closer {closer:?} is Unverifiable"),
    }
}

fn supervisor_command(journal: &TempJournal) -> Command {
    let core_bin = env!("CARGO_BIN_EXE_solstone-core");
    let test_child = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let home = installation_binding::admit_for(&journal.root);
    let mut cmd = Command::new(core_bin);
    cmd.args(["supervisor", "--journal"])
        .arg(&journal.root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .env("HOME", home)
        .env("SOLSTONE_LOCAL_BINARY", test_child)
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE_FAST_TIMING", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", test_child);
    speakers_analyze_stub::apply(&mut cmd);
    cmd.env_remove("SOLSTONE_JOURNAL");
    cmd
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedAdmission {
    launch_id: String,
    instance: ProcessInstance,
    service: Option<HostedServiceKind>,
    uid: Option<u32>,
    parent_launch_id: Option<String>,
    finding: AdmissionFinding,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct SealedLedgerEntry {
    launch_id: String,
    service: Option<HostedServiceKind>,
    identity: AdmissionIdentity,
}

#[test]
fn large_history_recovery_across_repeated_restarts() {
    let journal = TempJournal::new("large-history");
    let ledger = ParentLossLedger::open(&journal.root).expect("open ledger");

    let dead_sup = synth_instance(99980, 100);
    let dead_coord = synth_instance(99981, 101);

    let active = ledger
        .reserve_generation(dead_sup, [HostedServiceKind::Sense])
        .expect("reserve");
    ledger.initialize_record(&active).expect("init record");
    let active = ledger
        .persist_coordinator_identity(active.generation, dead_coord)
        .expect("persist coord");
    let active = ledger
        .mark_admitting(active.generation, dead_coord)
        .expect("mark admitting");

    let mut expected: Vec<ExpectedAdmission> = Vec::new();
    let own_uid = nix::unistd::getuid().as_raw();

    // 520 admissions to cross two 256 batches
    for i in 0..520 {
        let launch_id = format!("admit-{i:05}");
        let parent_launch_id = if i > 0 && i % 3 == 0 {
            Some(format!("admit-{:05}", i - 1))
        } else {
            None
        };
        let inst = synth_instance(10000 + i as u32, 1000 + i as u64);
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(
                active.generation,
                &launch_id,
                Some(HostedServiceKind::Sense),
                parent_launch_id.clone(),
            ),
        )
        .expect("write intent");

        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.clone(),
            instance: inst,
            uid: own_uid,
            parent_launch_id: parent_launch_id.clone(),
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &identity).expect("spawn id");
        write_parent_loss_admission_result(
            &journal.root,
            active.generation,
            &launch_id,
            &AdmissionResult {
                schema: 1,
                identity: Some(identity),
                state: AdmissionResultState::Admitted,
            },
        )
        .expect("result");

        expected.push(ExpectedAdmission {
            launch_id,
            instance: inst,
            service: Some(HostedServiceKind::Sense),
            uid: Some(own_uid),
            parent_launch_id,
            finding: AdmissionFinding::Exited,
        });
    }

    let ready_path = journal.root.join("health/supervisor.ready");

    // First supervisor start with slow timing (40ms per admission) so it cannot finish within 18s ready window
    let mut cmd1 = supervisor_command(&journal);
    cmd1.env("SOLSTONE_PARENT_LOSS_CLOSURE_SLOW_MS", "40");
    let mut sup1 = SupervisorGuard::new(cmd1.spawn().expect("start first"));
    let source = SystemProcessInstanceSource;
    let sup1_instance = match source.inspect(sup1.id()) {
        InspectResult::Present { instance, .. } => instance,
        other => panic!("sup1 inspectable: {other:?}"),
    };

    let ids_dir = ledger.closure_recovery_ids_dir(active.generation);
    let wait_deadline = Instant::now() + Duration::from_secs(18);
    let mut has_ids = false;
    let mut closer = None;
    while Instant::now() < wait_deadline {
        if !has_ids
            && let Ok(entries) = fs::read_dir(&ids_dir)
            && entries
                .flatten()
                .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        {
            has_ids = true;
            if sup1.try_wait().expect("try_wait sup1").is_none() {
                closer = Some(capture_live_closer(sup1.id()));
            }
        }
        if sup1.try_wait().expect("try_wait sup1").is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !ready_path.exists(),
        "health/supervisor.ready must be absent"
    );
    assert!(
        has_ids,
        "at least one closure-recovery/ids/*.json file must exist"
    );
    let closer =
        closer.expect("closer must have been captured while ids existed and sup1 was running");

    // Only then kill the process if it is still running.
    if sup1.try_wait().expect("try_wait sup1").is_none() {
        let _ = sup1.kill();
        let _ = sup1.wait();
    }
    assert_eq!(
        source.observe(&sup1_instance),
        InstanceVerdict::NotSameOrExited,
        "supervisor must be NotSameOrExited before the second start"
    );

    prove_closer_gone(&closer);

    // Start a second supervisor with the slow env removed.
    let mut cmd2 = supervisor_command(&journal);
    cmd2.env_remove("SOLSTONE_PARENT_LOSS_CLOSURE_SLOW_MS");
    let mut sup2 = SupervisorGuard::new(cmd2.spawn().expect("start second"));

    let wait_second = Instant::now() + RESTART_READY_WAIT;
    let mut ready = false;
    while Instant::now() < wait_second {
        if ready_path.exists() {
            ready = true;
            break;
        }
        if let Ok(Some(st)) = sup2.try_wait() {
            panic!("second supervisor exited prematurely with {st:?}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(ready, "health/supervisor.ready must appear");
    assert!(
        sup2.try_wait().expect("try_wait").is_none(),
        "second supervisor must still be running when supervisor.ready appears"
    );

    let record_path = ledger.record_path(active.generation);
    assert!(record_path.is_file(), "predecessor record.json must exist");
    let record_bytes = fs::read(&record_path).expect("read record");
    let record: ParentLossGenerationRecord =
        serde_json::from_slice(&record_bytes).expect("parse record");
    let closure = record.closure.expect("closure must exist");

    assert_eq!(closure.admissions.len(), expected.len());
    let mut actual_ids: HashSet<String> = HashSet::new();
    for (act, exp) in closure.admissions.iter().zip(expected.iter()) {
        assert_eq!(&act.launch_id, &exp.launch_id);
        assert_eq!(act.service, exp.service);
        assert_eq!(act.uid, exp.uid);
        assert_eq!(act.instance, Some(exp.instance));
        assert_eq!(act.finding, exp.finding);
        actual_ids.insert(act.launch_id.clone());
    }
    assert_eq!(actual_ids.len(), expected.len(), "no duplicate launch ids");
    for exp in &expected {
        assert!(actual_ids.contains(&exp.launch_id), "no missing launch id");
    }

    let ledger_path = ledger.sealed_ledger_path(active.generation);
    assert!(ledger_path.is_file(), "ledger.json must exist");
    let ledger_bytes = fs::read(&ledger_path).expect("read sealed ledger");
    let sealed_entries: Vec<SealedLedgerEntry> =
        serde_json::from_slice(&ledger_bytes).expect("parse sealed ledger");
    assert_eq!(sealed_entries.len(), expected.len());
    for (entry, exp) in sealed_entries.iter().zip(expected.iter()) {
        assert_eq!(&entry.launch_id, &exp.launch_id);
        assert_eq!(entry.service, exp.service);
        assert_eq!(entry.identity.generation, active.generation);
        assert_eq!(entry.identity.launch_id, exp.launch_id);
        assert_eq!(entry.identity.instance, exp.instance);
        assert_eq!(entry.identity.uid, exp.uid.expect("expected uid"));
        assert_eq!(entry.identity.parent_launch_id, exp.parent_launch_id);
    }
}

#[test]
fn kill_at_after_batch_seam_and_resume_with_new_closer() {
    let journal = TempJournal::new("kill-after-batch");
    let ledger = ParentLossLedger::open(&journal.root).expect("open ledger");

    let dead_sup = synth_instance(99970, 100);
    let dead_coord = synth_instance(99971, 101);

    let active = ledger
        .reserve_generation(dead_sup, [HostedServiceKind::Sense])
        .expect("reserve");
    ledger.initialize_record(&active).expect("init record");
    let active = ledger
        .persist_coordinator_identity(active.generation, dead_coord)
        .expect("persist coord");
    let active = ledger
        .mark_admitting(active.generation, dead_coord)
        .expect("mark admitting");

    let own_uid = nix::unistd::getuid().as_raw();

    for i in 0..300 {
        let launch_id = format!("batch-admit-{i:04}");
        let inst = synth_instance(20000 + i as u32, 2000 + i as u64);
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(
                active.generation,
                &launch_id,
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .expect("write intent");

        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.clone(),
            instance: inst,
            uid: own_uid,
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &identity).expect("spawn id");
        write_parent_loss_admission_result(
            &journal.root,
            active.generation,
            &launch_id,
            &AdmissionResult {
                schema: 1,
                identity: Some(identity),
                state: AdmissionResultState::Admitted,
            },
        )
        .expect("result");
    }

    let ids_path = ledger.closure_recovery_ids_path(active.generation, 1);
    let ready_path = journal.root.join("health/supervisor.ready");

    let mut cmd1 = supervisor_command(&journal);
    cmd1.env("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM", "after-batch");
    let mut sup1 = SupervisorGuard::new(cmd1.spawn().expect("start first"));

    let wait_ids = Instant::now() + Duration::from_secs(15);
    while Instant::now() < wait_ids {
        if ids_path.is_file() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(ids_path.is_file(), "ids/000001.json must exist");
    assert!(sup1.try_wait().expect("try_wait").is_none());
    let closer = capture_live_closer(sup1.id());

    let _ = sup1.kill();
    let _ = sup1.wait();

    prove_closer_gone(&closer);

    let active_cur = ledger.active_generation().expect("read").expect("active");
    assert_eq!(
        active_cur.generation, active.generation,
        "pointer still names abandoned generation"
    );
    assert!(
        !ready_path.exists(),
        "supervisor.ready absent while audit incomplete"
    );

    let mut cmd2 = supervisor_command(&journal);
    let _sup2 = SupervisorGuard::new(cmd2.spawn().expect("start second"));

    let wait_ready = Instant::now() + RESTART_READY_WAIT;
    while Instant::now() < wait_ready {
        if ready_path.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        ready_path.exists(),
        "second supervisor must finish and be ready"
    );

    let successor_active = ledger.active_generation().expect("read").expect("active");
    assert_ne!(
        successor_active.generation, active.generation,
        "active generation is the successor"
    );
    let successor_record = ledger
        .record(successor_active.generation)
        .expect("read successor record")
        .expect("successor record exists");
    let successor_coordinator = successor_record
        .coordinator
        .expect("successor coordinator must be Some");

    let record = ledger
        .record(active.generation)
        .expect("read")
        .expect("record exists");
    let closure = record.closure.expect("closure exists");
    assert_eq!(
        closure.closed_by, successor_coordinator,
        "closer must equal the successor coordinator"
    );
    assert_ne!(
        closure.closed_by, dead_coord,
        "closer must not equal dead_coord"
    );
    assert_ne!(
        closure.closed_by, *closer,
        "closer must not equal the closer you killed"
    );
}

#[test]
fn kill_at_during_inventory_and_after_ledger_seams() {
    // 1. during-inventory seam
    {
        let journal = TempJournal::new("kill-during-inv");
        let ledger = ParentLossLedger::open(&journal.root).expect("open ledger");

        let dead_sup = synth_instance(99960, 100);
        let dead_coord = synth_instance(99961, 101);

        let active = ledger
            .reserve_generation(dead_sup, [HostedServiceKind::Sense])
            .expect("reserve");
        ledger.initialize_record(&active).expect("init record");
        let active = ledger
            .persist_coordinator_identity(active.generation, dead_coord)
            .expect("persist coord");
        let active = ledger
            .mark_admitting(active.generation, dead_coord)
            .expect("mark admitting");

        let launch_id = "inv-test-id";
        let inst = synth_instance(30001, 300);
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(
                active.generation,
                launch_id,
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .expect("intent");
        let own_uid = nix::unistd::getuid().as_raw();
        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.to_string(),
            instance: inst,
            uid: own_uid,
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &identity).expect("spawn id");
        write_parent_loss_admission_result(
            &journal.root,
            active.generation,
            launch_id,
            &AdmissionResult {
                schema: 1,
                identity: Some(identity),
                state: AdmissionResultState::Admitted,
            },
        )
        .expect("result");

        let mut cmd_inv = supervisor_command(&journal);
        cmd_inv.env("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM", "during-inventory");
        let mut sup_inv = SupervisorGuard::new(cmd_inv.spawn().expect("start during-inventory"));

        let poll_deadline = Instant::now() + Duration::from_secs(15);
        let mut closer = None;
        while Instant::now() < poll_deadline {
            if sup_inv.try_wait().expect("try_wait").is_some() {
                break;
            }
            if let Some(c) = try_capture_live_closer(sup_inv.id()) {
                closer = Some(c);
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let closer = closer.expect("capture_live_closer must succeed while sup_inv is running");

        let _ = sup_inv.kill();
        let _ = sup_inv.wait();

        prove_closer_gone(&closer);

        let ready_path = journal.root.join("health/supervisor.ready");
        assert!(
            !ready_path.exists(),
            "supervisor.ready absent while during-inventory"
        );

        let active_cur = ledger.active_generation().expect("read").expect("active");
        assert_eq!(
            active_cur.generation, active.generation,
            "active generation pointer is unchanged"
        );

        // Resume without the seam and require supervisor.ready
        let mut cmd_resume = supervisor_command(&journal);
        let _sup_resume = SupervisorGuard::new(cmd_resume.spawn().expect("resume"));
        let wait_resume = Instant::now() + RESTART_READY_WAIT;
        while Instant::now() < wait_resume {
            if ready_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(ready_path.exists(), "supervisor.ready required on resume");
    }

    // 2. after-ledger seam
    {
        let journal = TempJournal::new("kill-after-ledger");
        let ledger = ParentLossLedger::open(&journal.root).expect("open ledger");

        let dead_sup = synth_instance(99962, 100);
        let dead_coord = synth_instance(99963, 101);

        let active = ledger
            .reserve_generation(dead_sup, [HostedServiceKind::Sense])
            .expect("reserve");
        ledger.initialize_record(&active).expect("init record");
        let active = ledger
            .persist_coordinator_identity(active.generation, dead_coord)
            .expect("persist coord");
        let active = ledger
            .mark_admitting(active.generation, dead_coord)
            .expect("mark admitting");

        let launch_id = "after-ledger-test-id";
        let inst = synth_instance(30002, 301);
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(
                active.generation,
                launch_id,
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .expect("intent");
        let own_uid = nix::unistd::getuid().as_raw();
        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.to_string(),
            instance: inst,
            uid: own_uid,
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &identity).expect("spawn id");
        write_parent_loss_admission_result(
            &journal.root,
            active.generation,
            launch_id,
            &AdmissionResult {
                schema: 1,
                identity: Some(identity),
                state: AdmissionResultState::Admitted,
            },
        )
        .expect("result");

        let mut cmd_ledger = supervisor_command(&journal);
        cmd_ledger.env("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM", "after-ledger");
        let mut sup_ledger = SupervisorGuard::new(cmd_ledger.spawn().expect("start after-ledger"));

        let ledger_path = ledger.sealed_ledger_path(active.generation);
        let wait_ledger = Instant::now() + Duration::from_secs(15);
        while Instant::now() < wait_ledger {
            if ledger_path.is_file() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            ledger_path.is_file(),
            "ledger.json exists before record write"
        );
        assert!(sup_ledger.try_wait().expect("try_wait").is_none());
        let closer = capture_live_closer(sup_ledger.id());

        let _ = sup_ledger.kill();
        let _ = sup_ledger.wait();

        prove_closer_gone(&closer);

        let record_path = ledger.record_path(active.generation);
        let record_before = ledger
            .record(active.generation)
            .expect("read")
            .expect("exists");
        assert!(
            record_before.closure.is_none(),
            "predecessor closure is still absent"
        );

        let ledger_bytes_before = fs::read(&ledger_path).expect("read before");

        // Next supervisor start finishes closure and reuses existing ledger
        let mut cmd_finish = supervisor_command(&journal);
        let _sup_finish = SupervisorGuard::new(cmd_finish.spawn().expect("start finish"));

        let ready_path = journal.root.join("health/supervisor.ready");
        let wait_finish = Instant::now() + RESTART_READY_WAIT;
        while Instant::now() < wait_finish {
            if ready_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(ready_path.exists(), "supervisor finished");

        let record_after = ledger
            .record(active.generation)
            .expect("read")
            .expect("exists");
        assert!(record_after.closure.is_some(), "predecessor closure exists");
        assert!(record_path.is_file());
        assert!(ledger_path.is_file(), "the same ledger file is still there");
        let ledger_bytes_after = fs::read(&ledger_path).expect("read after");
        assert_eq!(
            ledger_bytes_before, ledger_bytes_after,
            "the same ledger file contents are preserved"
        );
    }
}

#[test]
fn real_coordinator_shutdown_on_multi_admission_history_recovered_through_abandoned_closure() {
    let journal = TempJournal::new("coord-shutdown");
    let ledger = ParentLossLedger::open(&journal.root).expect("open ledger");

    let ready_path = journal.root.join("health/supervisor.ready");
    let mut cmd1 = supervisor_command(&journal);
    let mut sup1 = SupervisorGuard::new(cmd1.spawn().expect("start live supervisor"));

    let wait_ready = Instant::now() + RESTART_READY_WAIT;
    let mut ready = false;
    while Instant::now() < wait_ready {
        if ready_path.exists() && sup1.try_wait().expect("try_wait sup1").is_none() {
            ready = true;
            break;
        }
        if let Ok(Some(st)) = sup1.try_wait() {
            panic!("first supervisor exited prematurely with {st:?}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        ready,
        "health/supervisor.ready exists and try_wait() is None"
    );
    assert!(sup1.try_wait().expect("try_wait sup1").is_none());

    let active_gen = ledger
        .active_generation()
        .expect("read active")
        .expect("active generation exists");

    let admissions_dir = ledger
        .generation_path(active_gen.generation)
        .join("admissions");
    assert!(admissions_dir.is_dir(), "admissions dir must exist");

    let mut pre_signal_launch_ids: HashSet<String> = HashSet::new();
    let entries = fs::read_dir(&admissions_dir).expect("read admissions dir");
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let launch_id = entry.file_name().to_string_lossy().into_owned();
            let spawn = read_parent_loss_admission_spawn_identity(
                &journal.root,
                active_gen.generation,
                &launch_id,
            )
            .expect("read spawn identity");
            assert!(spawn.is_some(), "spawn identity exists for {launch_id}");
            pre_signal_launch_ids.insert(launch_id);
        }
    }
    assert!(
        !pre_signal_launch_ids.is_empty(),
        "at least one admission drop directory required"
    );

    let _ = sup1.shutdown_and_wait(Duration::from_secs(5));

    let sealed_path = ledger.sealed_ledger_path(active_gen.generation);
    let poll_deadline = Instant::now() + Duration::from_secs(15);
    let source = SystemProcessInstanceSource;
    let mut captured_coordinator: Option<CloserGuard> = None;
    let mut captured_ledger_bytes: Option<Vec<u8>> = None;

    while Instant::now() < poll_deadline {
        let record = ledger
            .record(active_gen.generation)
            .expect("read record")
            .expect("record exists");
        if record.closure.is_some() {
            panic!("record.closure became Some during polling before coordinator was killed");
        }
        if sealed_path.is_file()
            && let Some(coord) = record.coordinator
            && matches!(source.observe(&coord), InstanceVerdict::SameLive { .. })
        {
            let bytes = fs::read(&sealed_path).expect("read sealed ledger");
            captured_coordinator = Some(CloserGuard(coord));
            captured_ledger_bytes = Some(bytes);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let captured_coordinator = captured_coordinator
        .expect("must capture coordinator while sealed ledger exists, record.closure is None, and coordinator is SameLive");
    let captured_ledger_bytes = captured_ledger_bytes.expect("must capture sealed ledger bytes");

    let sealed_entries: Vec<SealedLedgerEntry> =
        serde_json::from_slice(&captured_ledger_bytes).expect("parse captured sealed ledger");
    let sealed_launch_ids: HashSet<String> = sealed_entries
        .iter()
        .map(|entry| entry.launch_id.clone())
        .collect();
    assert_eq!(
        sealed_launch_ids, pre_signal_launch_ids,
        "sealed launch-id set must equal pre-signal launch-id set"
    );

    prove_closer_gone(&captured_coordinator);

    if ready_path.exists() {
        let _ = fs::remove_file(&ready_path);
    }

    let mut cmd2 = supervisor_command(&journal);
    let mut sup2 = SupervisorGuard::new(cmd2.spawn().expect("start successor supervisor"));

    let wait_second = Instant::now() + RESTART_READY_WAIT;
    let mut ready2 = false;
    while Instant::now() < wait_second {
        if ready_path.exists() && sup2.try_wait().expect("try_wait sup2").is_none() {
            ready2 = true;
            break;
        }
        if let Ok(Some(st)) = sup2.try_wait() {
            panic!("successor supervisor exited prematurely with {st:?}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        ready2,
        "health/supervisor.ready appeared and successor try_wait() is None"
    );
    assert!(
        sup2.try_wait().expect("try_wait sup2").is_none(),
        "successor supervisor must still be running when supervisor.ready appears"
    );

    let record_after = ledger
        .record(active_gen.generation)
        .expect("read record")
        .expect("record exists");
    let closure = record_after
        .closure
        .expect("predecessor record.json closure must exist");

    let mut closure_admissions = closure.admissions.clone();
    closure_admissions.sort_by(|a, b| a.launch_id.cmp(&b.launch_id));
    let mut sorted_sealed = sealed_entries.clone();
    sorted_sealed.sort_by(|a, b| a.launch_id.cmp(&b.launch_id));

    assert_eq!(
        closure_admissions.len(),
        sorted_sealed.len(),
        "closure admissions length matches sealed entries"
    );
    for (closed, sealed) in closure_admissions.iter().zip(sorted_sealed.iter()) {
        assert_eq!(closed.launch_id, sealed.launch_id);
        assert_eq!(closed.service, sealed.service);
        assert_eq!(closed.uid, Some(sealed.identity.uid));
    }

    let reread_bytes = fs::read(&sealed_path).expect("re-read sealed ledger");
    let reread_sealed: Vec<SealedLedgerEntry> =
        serde_json::from_slice(&reread_bytes).expect("parse re-read sealed ledger");
    assert_eq!(
        reread_sealed.len(),
        sealed_entries.len(),
        "re-read sealed ledger length matches capture"
    );
    for (reread, captured) in reread_sealed.iter().zip(sealed_entries.iter()) {
        assert_eq!(reread.launch_id, captured.launch_id);
        assert_eq!(
            reread.identity.parent_launch_id,
            captured.identity.parent_launch_id
        );
    }

    for launch_id in &pre_signal_launch_ids {
        let drop_dir = admissions_dir.join(launch_id);
        assert!(
            drop_dir.is_dir(),
            "admission drop directory for {launch_id} must still be a directory"
        );
    }

    let successor_active = ledger
        .active_generation()
        .expect("read active")
        .expect("successor active generation exists");
    assert_ne!(
        successor_active.generation, active_gen.generation,
        "active generation is successor"
    );
    let successor_record = ledger
        .record(successor_active.generation)
        .expect("read successor record")
        .expect("successor record exists");
    let successor_coordinator = successor_record
        .coordinator
        .expect("successor generation record must have a coordinator");

    assert_eq!(
        closure.closed_by, successor_coordinator,
        "closure.closed_by must equal the successor generation record's coordinator"
    );
    assert_ne!(
        closure.closed_by, *captured_coordinator,
        "closure.closed_by must not be the coordinator you killed"
    );
}

type OriginalDrops = Vec<(PathBuf, Vec<u8>)>;

fn seed_one_abandoned_admission(
    journal: &TempJournal,
    ledger: &ParentLossLedger,
    launch_id: &str,
    instance: ProcessInstance,
) -> (u64, AdmissionIdentity, OriginalDrops) {
    let active = ledger
        .reserve_generation(synth_instance(99950, 500), [])
        .unwrap();
    ledger.initialize_record(&active).unwrap();
    let coordinator = synth_instance(99951, 501);
    ledger
        .persist_coordinator_identity(active.generation, coordinator)
        .unwrap();
    ledger
        .mark_admitting(active.generation, coordinator)
        .unwrap();
    write_parent_loss_admission_intent(
        &journal.root,
        &AdmissionIntent::new(active.generation, launch_id, None, None),
    )
    .unwrap();
    let identity = AdmissionIdentity {
        generation: active.generation,
        launch_id: launch_id.to_owned(),
        instance,
        uid: nix::unistd::getuid().as_raw(),
        parent_launch_id: None,
    };
    write_parent_loss_admission_spawn_identity(&journal.root, &identity).unwrap();
    write_parent_loss_admission_result(
        &journal.root,
        active.generation,
        launch_id,
        &AdmissionResult {
            schema: 1,
            identity: Some(identity.clone()),
            state: AdmissionResultState::Admitted,
        },
    )
    .unwrap();
    let dir = ledger
        .generation_path(active.generation)
        .join("admissions")
        .join(launch_id);
    let original = ["intent.json", "spawn-identity.json", "result.json"]
        .into_iter()
        .map(|name| {
            let path = dir.join(name);
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    (active.generation, identity, original)
}

fn wait_for_condition(condition: impl FnMut() -> bool, description: &str) {
    wait_for_condition_within(Duration::from_secs(30), condition, description);
}

fn wait_for_condition_within(
    timeout: Duration,
    mut condition: impl FnMut() -> bool,
    description: &str,
) {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {description}");
}

fn assert_one_complete_identity(
    ledger: &ParentLossLedger,
    identity: &AdmissionIdentity,
    original: &OriginalDrops,
    history_count: usize,
) {
    use sha2::Digest;
    let record = ledger.record(identity.generation).unwrap().unwrap();
    let closure = record.closure.as_ref().unwrap();
    assert_eq!(closure.admissions.len(), 1 + history_count);
    let root = closure
        .admissions
        .iter()
        .find(|a| a.launch_id == identity.launch_id)
        .unwrap();
    assert_eq!(root.instance, Some(identity.instance));
    assert_eq!(root.uid, Some(identity.uid));
    assert_eq!(root.service, None);
    for i in 0..history_count {
        let id = format!("history-{i:04}");
        let rows: Vec<_> = closure
            .admissions
            .iter()
            .filter(|a| a.launch_id == id)
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].finding, AdmissionFinding::SpawnFailed);
        assert_eq!(rows[0].instance, None);
        assert_eq!(rows[0].uid, None);
        assert_eq!(rows[0].service, None);
    }
    let bytes = fs::read(ledger.sealed_ledger_path(identity.generation)).unwrap();
    assert_eq!(
        record.sealed_ledger_digest,
        Some(format!("{:x}", sha2::Sha256::digest(&bytes)))
    );
    let sealed: Vec<SealedLedgerEntry> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sealed.len(), 1);
    assert_eq!(sealed[0].launch_id, identity.launch_id);
    assert_eq!(sealed[0].service, None);
    assert_eq!(sealed[0].identity, *identity);
    for (path, bytes) in original {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
}

#[test]
fn killed_closers_resume_audited_chunks_prepared_bytes_and_published_record() {
    for seam in ["during-finalization", "before-publication", "after-record"] {
        let journal = TempJournal::new(seam);
        let ledger = ParentLossLedger::open(&journal.root).unwrap();
        let (generation, identity, original) = seed_one_abandoned_admission(
            &journal,
            &ledger,
            "publication-seam",
            synth_instance(99952, 502),
        );
        let mut command = supervisor_command(&journal);
        command.env("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM", seam);
        let mut first = SupervisorGuard::new(command.spawn().unwrap());
        let phase = ledger.closure_recovery_phase_dir(generation);
        wait_for_condition(
            || match seam {
                "during-finalization" => phase.join("audited-000001.sha256").exists(),
                "before-publication" => phase.join("publication.json").exists(),
                "after-record" => ledger
                    .record(generation)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.closure.is_some()),
                _ => unreachable!(),
            },
            seam,
        );
        assert!(first.try_wait().unwrap().is_none());
        assert!(!journal.root.join("health/supervisor.ready").exists());
        let closer = capture_live_closer(first.id());
        first.kill().unwrap();
        first.wait().unwrap();
        prove_closer_gone(&closer);
        let mut next = SupervisorGuard::new(supervisor_command(&journal).spawn().unwrap());
        wait_for_condition_within(
            RESTART_READY_WAIT,
            || {
                assert!(
                    next.try_wait().unwrap().is_none(),
                    "successor exited at {seam}"
                );
                journal.root.join("health/supervisor.ready").exists()
            },
            "successor readiness after interrupted publication",
        );
        let successor = ledger.active_generation().unwrap().unwrap();
        assert!(successor.generation > generation);
        assert_ne!(successor.coordinator.unwrap(), *closer);
        assert_one_complete_identity(&ledger, &identity, &original, 0);
    }
}

#[test]
fn reparented_term_resistant_descendant_survives_closer_death_then_is_reaped_from_census() {
    retained_descendant_recovery(false);
}

#[test]
fn a_rebound_same_user_cache_cannot_replace_retained_ancestry_or_shorten_grace() {
    retained_descendant_recovery(true);
}

fn retained_descendant_recovery(rebind_cache: bool) {
    use solstone_core_system::process::InstanceCensus;
    // The canonical macOS temp path must leave room for health/callosum.sock.
    let journal = TempJournal::new("desc");
    let ledger = ParentLossLedger::open(&journal.root).unwrap();
    let fixture_ready = journal.root.join("fixture-ready");
    let fixture = env!("CARGO_BIN_EXE_solstone-core-system-test-child");
    let mut root = SupervisorGuard::new(
        Command::new("sh")
            .args([
                "-c",
                "\"$1\" block-term-count \"$2\" \"$3\" & wait",
                "retained-root",
            ])
            .arg(fixture)
            .arg(&fixture_ready)
            .arg(journal.root.join("term-count"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_remove("SOLSTONE_JOURNAL")
            .spawn()
            .unwrap(),
    );
    wait_for_condition(
        || fixture_ready.exists(),
        "TERM-resistant descendant readiness",
    );
    let source = SystemProcessInstanceSource;
    let root_instance = match source.inspect(root.id()) {
        InspectResult::Present { instance, .. } => instance,
        other => panic!("root is not inspectable: {other:?}"),
    };
    let rows = match source.census_tree(root.id(), None) {
        InstanceCensus::Complete(rows) => rows,
        other => panic!("complete fixture census required: {other:?}"),
    };
    let children: Vec<_> = rows.iter().filter(|row| row.ppid == root.id()).collect();
    assert_eq!(children.len(), 1);
    let descendant = CloserGuard(children[0].instance);
    let sentinel = SupervisorGuard::new(Command::new("sleep").arg("180").spawn().unwrap());
    let sentinel_instance = match source.inspect(sentinel.id()) {
        InspectResult::Present { instance, .. } => instance,
        other => panic!("sentinel is not inspectable: {other:?}"),
    };
    let (generation, identity, mut original) =
        seed_one_abandoned_admission(&journal, &ledger, "real-retirement-root", root_instance);
    let history_count = if rebind_cache { 128 } else { 0 };
    for i in 0..history_count {
        let launch_id = format!("history-{i:04}");
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(generation, &launch_id, None, None),
        )
        .unwrap();
        write_parent_loss_admission_result(
            &journal.root,
            generation,
            &launch_id,
            &AdmissionResult {
                schema: 1,
                identity: None,
                state: AdmissionResultState::SpawnFailed {
                    detail: "historical failed launch".into(),
                },
            },
        )
        .unwrap();
        let dir = ledger
            .generation_path(generation)
            .join("admissions")
            .join(&launch_id);
        for name in ["intent.json", "result.json"] {
            let path = dir.join(name);
            let bytes = fs::read(&path).unwrap();
            original.push((path, bytes));
        }
    }
    let mut command = supervisor_command(&journal);
    command.env("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM", "after-retirement-term");
    if rebind_cache {
        command.env("SOLSTONE_PARENT_LOSS_CLOSURE_SLOW_MS", "40");
    }
    let mut first = SupervisorGuard::new(command.spawn().unwrap());
    wait_for_condition(
        || {
            let begun = fs::read(ledger.census_obligation_path(generation))
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .is_some_and(|v| {
                    v["obligations"]["real-retirement-root"]["term_requested"] == true
                });
            begun
                && matches!(
                    source.observe(&root_instance),
                    InstanceVerdict::NotSameOrExited
                )
        },
        "durable census and root exit after TERM",
    );
    let retained_before: serde_json::Value =
        serde_json::from_slice(&fs::read(ledger.census_obligation_path(generation)).unwrap())
            .unwrap();
    let escalation_before =
        retained_before["obligations"]["real-retirement-root"]["escalate_at_unix_ms"].clone();
    let reap_before =
        retained_before["obligations"]["real-retirement-root"]["reap_deadline_unix_ms"].clone();
    let descendant_deadlines_before =
        retained_before["obligations"]["real-retirement-root"]["descendants"][0].clone();
    let closer = capture_live_closer(first.id());
    assert!(matches!(
        source.observe(&descendant),
        InstanceVerdict::SameLive { .. }
    ));
    first.kill().unwrap();
    first.wait().unwrap();
    prove_closer_gone(&closer);
    let _ = root.wait();
    let live_path = ledger.closure_recovery_live_path(generation, "real-retirement-root");
    if rebind_cache {
        use sha2::Digest;
        let mut cached: serde_json::Value =
            serde_json::from_slice(&fs::read(&live_path).unwrap()).unwrap();
        assert_eq!(cached["descendants"].as_array().unwrap().len(), 1);
        cached["descendants"][0]["instance"] = serde_json::to_value(sentinel_instance).unwrap();
        let binding = serde_json::json!({"root":cached["root"],"uid":cached["uid"],"descendants":cached["descendants"]});
        let mut bytes = serde_json::to_vec_pretty(&binding).unwrap();
        bytes.push(b'\n');
        cached["binding_digest"] = format!("{:x}", sha2::Sha256::digest(&bytes)).into();
        fs::write(&live_path, serde_json::to_vec_pretty(&cached).unwrap()).unwrap();
    } else {
        // Only the reconstructible cache is lost; authoritative ancestry remains intact.
        fs::remove_file(live_path).unwrap();
    }
    let mut next = SupervisorGuard::new(supervisor_command(&journal).spawn().unwrap());
    let mut resumed_closer = None;
    let mut resumed_at = None;
    wait_for_condition_within(
        RESTART_READY_WAIT,
        || {
            assert!(
                next.try_wait().unwrap().is_none(),
                "successor exited before descendant drain"
            );
            if resumed_closer.is_none() {
                resumed_closer = try_capture_live_closer(next.id());
                if resumed_closer.is_some() {
                    resumed_at = Some(Instant::now());
                    let retained_now: serde_json::Value = serde_json::from_slice(
                        &fs::read(ledger.census_obligation_path(generation)).unwrap(),
                    )
                    .unwrap();
                    if let Some(live) = retained_now["obligations"].get("real-retirement-root") {
                        assert_eq!(live["escalate_at_unix_ms"], escalation_before);
                        assert_eq!(live["reap_deadline_unix_ms"], reap_before);
                        assert_eq!(
                            live["descendants"][0]["escalate_at_unix_ms"],
                            descendant_deadlines_before["escalate_at_unix_ms"]
                        );
                        // The first KILL may legitimately establish its 500ms reap deadline.
                        if !live["descendants"][0]["escalated"].as_bool().unwrap() {
                            assert_eq!(
                                live["descendants"][0]["reap_deadline_unix_ms"],
                                descendant_deadlines_before["reap_deadline_unix_ms"]
                            );
                        }
                    }
                }
            }
            journal.root.join("health/supervisor.ready").exists()
        },
        "successor readiness after retained descendant drain",
    );
    assert!(
        resumed_closer.is_some(),
        "replacement coordinator must be observed"
    );
    // The 60s heartbeat quarantine is outside recovery. Once a new closer runs,
    // the retained TERM deadline is already past; a renewed 12.5s grace would
    // violate this upper bound. The separate unpaused process test proves the
    // actual first TERM grace, with literal policy controls in the unit suite.
    assert!(
        resumed_at.unwrap().elapsed() < Duration::from_secs(12),
        "retry must not renew expired TERM grace"
    );
    assert_eq!(
        source.observe(&descendant),
        InstanceVerdict::NotSameOrExited
    );
    assert!(matches!(
        source.observe(&sentinel_instance),
        InstanceVerdict::SameLive { .. }
    ));
    let census: serde_json::Value =
        serde_json::from_slice(&fs::read(ledger.census_obligation_path(generation)).unwrap())
            .unwrap();
    assert!(census["obligations"].as_object().unwrap().is_empty());
    assert_one_complete_identity(&ledger, &identity, &original, history_count);
}
