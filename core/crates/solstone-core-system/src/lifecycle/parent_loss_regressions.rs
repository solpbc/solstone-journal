// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Regression reproductions for the parent-loss redesign (`req_zgopzsto`).
//!
//! Each case drives an entry point the redesign keeps
//! (`ParentLossLedger::reserve_generation_closing_abandoned`, the coordinator's
//! public `bootstrap`/`run`, `launch_generation_child`) and asserts an
//! outcome, never an internal artifact, so the same tests stay meaningful on
//! the new state model. The process fakes are copied from
//! `parent_loss_closure::tests` on purpose: this file must survive that module
//! being rewritten.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use solstone_core_journal_io::{LockOptions, hold_lock, write_json};
use tempfile::TempDir;

use super::HostedServiceKind;
use super::parent::DeclaredParent;
use super::parent_loss_admission::{
    AdmissionIdentity, AdmissionIntent, AdmissionResult, AdmissionResultState,
    AdmissionSpawnIdentity, acknowledge_parent_loss_admission, write_parent_loss_admission_intent,
    write_parent_loss_admission_result, write_parent_loss_admission_spawn_identity,
};
use super::parent_loss_coordinator::{
    CoordinatorBootstrap, ParentLossCoordinator, write_retire_expected_control,
};
use super::parent_loss_ledger::{
    ActiveGeneration, CoordinatorLease, ParentLossGeneration, ParentLossLedger,
    ParentLossLedgerError, ParentLossPhase, json_options,
};
use super::parent_loss_outstanding::{AdmissionRetirer, ClosingAuthority};
use crate::process::{
    CensusRow, Disposition, ExecutionState, InspectResult, InstanceCensus, LaunchError,
    ProcessBirth, ProcessInstance, ProcessInstanceSource, ProcessOwner, SignalKind, TableBirth,
    TerminationError, launch_generation_child,
};

// ---- fakes (copied from parent_loss_closure::tests, trimmed) ----

fn instance(pid: u32, birth: u64) -> ProcessInstance {
    ProcessInstance {
        pid,
        birth: ProcessBirth::linux(birth, 1, 100),
    }
}

#[derive(Default)]
struct FakeTable {
    live: HashMap<u32, (ProcessInstance, u32)>,
    owners: HashMap<u32, ProcessOwner>,
    table_births: HashMap<u32, TableBirth>,
}

#[derive(Clone, Default)]
struct FakeSource(Arc<Mutex<FakeTable>>);

impl ProcessInstanceSource for FakeSource {
    fn inspect(&self, pid: u32) -> InspectResult {
        let table = self.0.lock().expect("fake table");
        match table.live.get(&pid) {
            Some((instance, uid)) => InspectResult::Present {
                instance: *instance,
                uid: *uid,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: Some(1),
            },
            None => InspectResult::Absent,
        }
    }

    fn census(&self) -> InstanceCensus {
        let table = self.0.lock().expect("fake table");
        InstanceCensus::Complete(
            table
                .live
                .values()
                .map(|(instance, uid)| CensusRow {
                    instance: *instance,
                    uid: *uid,
                    ppid: 1,
                    pgid: 1,
                    execution: ExecutionState::Running,
                })
                .collect(),
        )
    }
}

struct FakeRetirer {
    table: Arc<Mutex<FakeTable>>,
    signals: Mutex<Vec<(u32, &'static str)>>,
    obeys_terminate: HashSet<u32>,
}

impl AdmissionRetirer for FakeRetirer {
    fn signal(
        &self,
        instance: ProcessInstance,
        signal: SignalKind,
    ) -> Result<(), TerminationError> {
        let name = match signal {
            SignalKind::Terminate => "term",
            SignalKind::Kill => "kill",
        };
        self.signals
            .lock()
            .expect("signals")
            .push((instance.pid, name));
        let exits = match signal {
            SignalKind::Terminate => self.obeys_terminate.contains(&instance.pid),
            SignalKind::Kill => true,
        };
        if exits {
            self.table
                .lock()
                .expect("fake table")
                .live
                .remove(&instance.pid);
        }
        Ok(())
    }

    fn owner(&self, pid: u32) -> ProcessOwner {
        let table = self.table.lock().expect("fake table");
        if let Some(owner) = table.owners.get(&pid) {
            return *owner;
        }
        match table.live.get(&pid) {
            Some((_, uid)) => ProcessOwner::Uid(*uid),
            None => ProcessOwner::Absent,
        }
    }

    fn table_birth(&self, pid: u32) -> TableBirth {
        let table = self.table.lock().expect("fake table");
        table
            .table_births
            .get(&pid)
            .copied()
            .unwrap_or(TableBirth::Unknown)
    }
}

/// `live` processes are `SameLive`; everything else is absent (exited).
fn fake(live: &[ProcessInstance], obeys_terminate: &[u32]) -> (FakeSource, FakeRetirer) {
    let table = Arc::new(Mutex::new(FakeTable {
        live: live.iter().map(|i| (i.pid, (*i, 501))).collect(),
        ..FakeTable::default()
    }));
    let retirer = FakeRetirer {
        table: Arc::clone(&table),
        signals: Mutex::new(Vec::new()),
        obeys_terminate: obeys_terminate.iter().copied().collect(),
    };
    (FakeSource(table), retirer)
}

// ---- fixtures ----

const AMPLE_BUDGET: Duration = Duration::from_secs(60);
const SUPERVISOR: (u32, u64) = (10, 1);
const COORDINATOR: (u32, u64) = (20, 2);

/// An admitting generation whose supervisor (10) and coordinator (20) are not
/// in any fake table, so both authorities read as exited.
fn open_generation(directory: &Path) -> (ParentLossLedger, ActiveGeneration) {
    let ledger = ParentLossLedger::open(directory).expect("ledger");
    let active = ledger
        .reserve_generation(
            instance(SUPERVISOR.0, SUPERVISOR.1),
            [HostedServiceKind::Sense],
        )
        .expect("reserve generation");
    ledger.initialize_record(&active).expect("record");
    let coordinator = instance(COORDINATOR.0, COORDINATOR.1);
    let active = ledger
        .persist_coordinator_identity(active.generation, coordinator)
        .expect("coordinator identity");
    let active = ledger
        .mark_admitting(active.generation, coordinator)
        .expect("admitting state");
    (ledger, active)
}

fn identity(
    generation: ParentLossGeneration,
    launch_id: &str,
    pid: ProcessInstance,
) -> AdmissionIdentity {
    AdmissionIdentity {
        generation,
        launch_id: launch_id.to_owned(),
        instance: pid,
        uid: 501,
        parent_launch_id: None,
    }
}

fn admissions(ledger: &ParentLossLedger, generation: ParentLossGeneration) -> PathBuf {
    ledger.generation_path(generation).join("admissions")
}

fn write_intent(journal: &Path, generation: ParentLossGeneration, launch_id: &str) {
    write_parent_loss_admission_intent(
        journal,
        &AdmissionIntent::new(generation, launch_id, None, None),
    )
    .expect("intent");
}

/// A task worker exactly as `launch_generation_child` records it: intent,
/// spawn-identity, then an `admitted` result.
fn admit_task_worker(
    journal: &Path,
    generation: ParentLossGeneration,
    launch_id: &str,
    pid: ProcessInstance,
) -> AdmissionIdentity {
    write_intent(journal, generation, launch_id);
    let id = identity(generation, launch_id, pid);
    write_parent_loss_admission_spawn_identity(journal, &id).expect("spawn identity");
    write_parent_loss_admission_result(
        journal,
        generation,
        launch_id,
        &AdmissionResult {
            schema: 1,
            identity: Some(id.clone()),
            state: AdmissionResultState::Admitted,
        },
    )
    .expect("result");
    id
}

fn acquire_lease(ledger: &ParentLossLedger) -> CoordinatorLease {
    ledger
        .acquire_coordinator_lease()
        .expect("lease acquisition")
        .expect("lease free")
}

fn start(
    ledger: &ParentLossLedger,
    lease: &CoordinatorLease,
    source: &FakeSource,
    retirer: &FakeRetirer,
    budget: Duration,
) -> Result<ActiveGeneration, ParentLossLedgerError> {
    ledger.reserve_generation_closing_abandoned(
        instance(11, 3),
        [],
        lease,
        &ClosingAuthority {
            closed_by: instance(30, 3),
            source,
            retirer,
            deadline: Instant::now() + budget,
        },
    )
}

fn set_pointer(ledger: &ParentLossLedger, pointer: &ActiveGeneration) {
    write_json(ledger.active_path(), pointer, json_options()).expect("pointer write");
}

// ---- 1. a Finder .DS_Store ----

#[test]
fn a_finder_ds_store_in_admissions_does_not_stop_a_start() {
    let directory = TempDir::new().expect("temporary root");
    let (ledger, active) = open_generation(directory.path());
    admit_task_worker(
        directory.path(),
        active.generation,
        "worker-a",
        instance(4242, 42),
    );
    fs::write(
        admissions(&ledger, active.generation).join(".DS_Store"),
        b"\0\0\0\x01Bud1\0\0\x10\0",
    )
    .expect("finder metadata");
    let (source, retirer) = fake(&[], &[]);
    let lease = acquire_lease(&ledger);

    let successor = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET)
        .unwrap_or_else(|error| panic!("a .DS_Store in admissions/ refused the start: {error:?}"));
    assert!(successor.generation > active.generation);
}

// ---- 2. stray directories ----

#[test]
fn a_stray_directory_in_admissions_does_not_stop_a_start() {
    let mut failures = Vec::new();
    for (case, stray) in [
        ("bad name, no drops", "Untitled folder"),
        ("valid name, no drops", "stray-empty"),
    ] {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(directory.path());
        admit_task_worker(
            directory.path(),
            active.generation,
            "worker-a",
            instance(4242, 42),
        );
        fs::create_dir_all(admissions(&ledger, active.generation).join(stray)).expect("stray dir");
        let (source, retirer) = fake(&[], &[]);
        let lease = acquire_lease(&ledger);
        match start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET) {
            Ok(successor) => assert!(successor.generation > active.generation),
            Err(error) => failures.push(format!("{case} ({stray:?}): {error:?}")),
        }
    }
    assert!(
        failures.is_empty(),
        "stray directories refused the start: {failures:#?}"
    );
}

// ---- 3. a census refusal marker ----

#[test]
fn a_census_refusal_marker_clears_once_its_cause_is_gone() {
    let directory = TempDir::new().expect("temporary root");
    let (ledger, active) = open_generation(directory.path());
    admit_task_worker(
        directory.path(),
        active.generation,
        "worker-a",
        instance(4242, 42),
    );
    // The released closer's own shape (`CensusRefusalFile`).
    fs::write(
        ledger
            .generation_path(active.generation)
            .join("census-refusal.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "generation": active.generation,
            "sealed_phase": "sealed",
            "reason": "live cache damaged after root exit",
        }))
        .expect("marker json"),
    )
    .expect("refusal marker");
    let (source, retirer) = fake(&[], &[]);
    let lease = acquire_lease(&ledger);

    let mut refusals = Vec::new();
    for _ in 0..3 {
        match start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET) {
            Ok(_) => return,
            Err(error) => refusals.push(format!("{error:?}")),
        }
    }
    panic!(
        "every admitted process has exited, yet census-refusal.json refused 3 starts: {refusals:?}"
    );
}

// ---- 4. acknowledgement and result disagree ----

#[test]
fn an_acknowledgement_that_disagrees_with_the_result_settles_both() {
    let directory = TempDir::new().expect("temporary root");
    let (ledger, active) = open_generation(directory.path());
    let g = active.generation;
    let launch_id = "hosted-mismatch";
    write_intent(directory.path(), g, launch_id);
    // The child acknowledged as one instance; the launcher recorded another
    // (the shape a rejected non-matching acknowledgement leaves).
    acknowledge_parent_loss_admission(directory.path(), identity(g, launch_id, instance(4242, 42)))
        .expect("ack");
    write_parent_loss_admission_result(
        directory.path(),
        g,
        launch_id,
        &AdmissionResult {
            schema: 1,
            identity: Some(identity(g, launch_id, instance(4343, 43))),
            state: AdmissionResultState::Admitted,
        },
    )
    .expect("result");
    let (source, retirer) = fake(&[], &[]);
    let lease = acquire_lease(&ledger);

    start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap_or_else(|error| {
        panic!("both identities exited, yet the disagreement refused the start: {error:?}")
    });
}

// ---- 5. a drop with another schema ----

#[test]
fn a_drop_with_another_schema_does_not_refuse_forever() {
    let directory = TempDir::new().expect("temporary root");
    let (ledger, active) = open_generation(directory.path());
    let g = active.generation;
    let launch_id = "worker-schema2";
    write_intent(directory.path(), g, launch_id);
    let id = identity(g, launch_id, instance(4242, 42));
    write_parent_loss_admission_spawn_identity(directory.path(), &id).expect("spawn identity");
    write_parent_loss_admission_result(
        directory.path(),
        g,
        launch_id,
        &AdmissionResult {
            schema: 2,
            identity: Some(id),
            state: AdmissionResultState::Admitted,
        },
    )
    .expect("result");
    let (source, retirer) = fake(&[], &[]);
    let lease = acquire_lease(&ledger);

    let mut refusals = Vec::new();
    for _ in 0..3 {
        match start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET) {
            Ok(_) => return,
            Err(error) => refusals.push(format!("{error:?}")),
        }
    }
    panic!("a schema-2 result beside an exited spawn identity refused 3 starts: {refusals:?}");
}

// ---- 6. a malformed publication.json beside a closed record ----

#[test]
fn a_malformed_publication_json_beside_a_closed_record_does_not_refuse() {
    let directory = TempDir::new().expect("temporary root");
    let (ledger, active) = open_generation(directory.path());
    admit_task_worker(
        directory.path(),
        active.generation,
        "worker-a",
        instance(4242, 42),
    );
    let (source, retirer) = fake(&[], &[]);
    let lease = acquire_lease(&ledger);
    start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("first closure succeeds");
    assert!(
        ledger
            .record(active.generation)
            .expect("record")
            .expect("record kept")
            .closure
            .is_some(),
        "fixture: the first start wrote a closed record"
    );
    // Today's publication manifest lives at closure-recovery/phase/publication.json
    // (parent_loss_closure_publication.rs `read_publication`).
    let publication = ledger
        .generation_path(active.generation)
        .join("closure-recovery/phase/publication.json");
    fs::create_dir_all(publication.parent().expect("phase dir")).expect("phase dir");
    fs::write(&publication, b"{\"schema\":1,\"generation\":").expect("torn manifest");
    // Crash after the closed record landed but before the successor pointer:
    // the pointer still names the closed generation, sealed for closure.
    let mut pointer = active.clone();
    pointer.phase = ParentLossPhase::Sealed;
    set_pointer(&ledger, &pointer);

    start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap_or_else(|error| {
        panic!("a malformed publication.json beside a closed record refused the start: {error:?}")
    });
}

// ---- 7. a spurious higher generation directory ----

#[test]
fn a_spurious_higher_generation_directory_does_not_hide_an_older_unresolved_one() {
    let mut failures = Vec::new();
    for case in ["pointer absent", "pointer set aside"] {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(directory.path());
        let g = active.generation;
        let live = instance(5555, 55);
        admit_task_worker(directory.path(), g, "live-worker", live);
        fs::create_dir_all(ledger.generation_path(g + 1)).expect("spurious generation dir");
        match case {
            "pointer absent" => fs::remove_file(ledger.active_path()).expect("remove pointer"),
            _ => fs::write(ledger.active_path(), b"{\"schema\":").expect("damaged pointer"),
        }
        let (source, retirer) = fake(&[live], &[live.pid]);
        let lease = acquire_lease(&ledger);
        let outcome = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        let signalled = retirer
            .signals
            .lock()
            .expect("signals")
            .iter()
            .any(|(pid, _)| *pid == live.pid);
        match outcome {
            Err(_) => {}
            Ok(_) if signalled => {}
            Ok(successor) => failures.push(format!(
                "{case}: advanced to generation {} past generation {g} without signalling its \
                 live admission (pid {})",
                successor.generation, live.pid
            )),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

// ---- 8. a torn, non-empty drop at start ----

#[test]
fn a_torn_nonempty_drop_does_not_refuse_forever() {
    let directory = TempDir::new().expect("temporary root");
    let (ledger, active) = open_generation(directory.path());
    let g = active.generation;
    let launch_id = "worker-torn";
    write_intent(directory.path(), g, launch_id);
    let id = identity(g, launch_id, instance(4242, 42));
    write_parent_loss_admission_spawn_identity(directory.path(), &id).expect("spawn identity");
    let whole = serde_json::to_vec_pretty(&AdmissionResult {
        schema: 1,
        identity: Some(id),
        state: AdmissionResultState::Admitted,
    })
    .expect("result json");
    fs::write(
        admissions(&ledger, g).join(launch_id).join("result.json"),
        &whole[..whole.len() / 2],
    )
    .expect("torn result");
    let (source, retirer) = fake(&[], &[]);
    let lease = acquire_lease(&ledger);

    let mut refusals = Vec::new();
    for _ in 0..3 {
        match start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET) {
            Ok(_) => return,
            Err(error) => refusals.push(format!("{error:?}")),
        }
    }
    panic!("a torn result.json beside an exited spawn identity refused 3 starts: {refusals:?}");
}

// ---- 9. a torn drop at stop ----

#[test]
fn a_torn_drop_at_stop_still_publishes_a_terminal() {
    let journal = TempDir::new().expect("temporary journal");
    let declared_parent = DeclaredParent::capture_current().expect("live direct parent");
    let capability = b"parent-loss-regression-capability".to_vec();
    let (coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
        journal: journal.path().to_path_buf(),
        supervisor: declared_parent.instance(),
        enabled: Vec::new(),
        supervisor_heartbeat_filename: "solstone-v2-test-test.check".to_owned(),
        capability: capability.clone(),
    })
    .expect("coordinator bootstrap");
    let g = coordinator.generation();
    let ledger = ParentLossLedger::open(journal.path()).expect("ledger");

    // A task worker that ran and was reaped: its pid is gone. Its launcher
    // was killed while writing result.json, so the result is torn.
    let mut exited = Command::new("/bin/true").spawn().expect("spawn /bin/true");
    let pid = exited.id();
    exited.wait().expect("reap /bin/true");
    let launch_id = "worker-torn-at-stop";
    write_intent(journal.path(), g, launch_id);
    let id = identity(g, launch_id, instance(pid, 7));
    write_parent_loss_admission_spawn_identity(journal.path(), &id).expect("spawn identity");
    let whole = serde_json::to_vec_pretty(&AdmissionResult {
        schema: 1,
        identity: Some(id),
        state: AdmissionResultState::Admitted,
    })
    .expect("result json");
    fs::write(
        admissions(&ledger, g).join(launch_id).join("result.json"),
        &whole[..whole.len() / 2],
    )
    .expect("torn result");

    write_retire_expected_control(journal.path(), g, declared_parent.instance(), &capability)
        .expect("graceful stop instruction");
    let outcome = coordinator.run();

    let record = ledger
        .record(g)
        .expect("record readable")
        .expect("record kept");
    assert!(
        record.terminal.is_some(),
        "a graceful stop over a torn drop published no terminal; run returned {outcome:?}"
    );
}

// ---- 10. a task launch racing the seal ----

#[test]
fn a_task_launch_cannot_spawn_after_the_seal() {
    let directory = TempDir::new().expect("temporary root");
    let (ledger, active) = open_generation(directory.path());
    let g = active.generation;
    // Stand in for the coordinator's `seal_admissions`, which seals while
    // holding this lock.
    let lock = hold_lock(
        ledger.admission_lock_path(g),
        LockOptions {
            timeout: Duration::from_secs(2),
            poll_interval: Duration::from_millis(10),
            mode: Some(0o600),
        },
    )
    .expect("admission lock");

    let spawned = Arc::new(AtomicBool::new(false));
    let journal = directory.path().to_path_buf();
    let launcher = {
        let spawned = Arc::clone(&spawned);
        thread::spawn(move || -> Result<(), String> {
            let result = launch_generation_child(
                Disposition::InheritedParentScope,
                &journal,
                "after-seal".to_owned(),
                move || {
                    spawned.store(true, Ordering::SeqCst);
                    Command::new("/bin/sleep").arg("30").spawn()
                },
                Box::new(|child, _| child.kill().map_err(LaunchError::Terminate)),
            );
            match result {
                Ok(mut authority) => {
                    let _ = authority.terminate(Duration::from_secs(2));
                    Ok(())
                }
                Err(error) => Err(error.to_string()),
            }
        })
    };
    // The launcher reads the Admitting pointer, then polls this lock every
    // 10 ms for up to 5 s. 500 ms puts it well inside that wait.
    thread::sleep(Duration::from_millis(500));
    assert!(
        !spawned.load(Ordering::SeqCst),
        "fixture: spawn ran while the lock was held"
    );
    ledger
        .seal(g, instance(COORDINATOR.0, COORDINATOR.1))
        .expect("seal generation");
    drop(lock);
    let outcome = launcher.join().expect("launcher thread");

    assert!(
        !spawned.load(Ordering::SeqCst),
        "a task launch spawned a child after its generation was sealed (launch returned {outcome:?})"
    );
    assert!(outcome.is_err(), "a launch after the seal must be refused");
}

// ---- 11. recovery cost vs. launch history ----

/// Write `count` exited task-worker admissions directly (no per-file fsync,
/// so fixture time stays small), in the exact shape `launch_generation_child`
/// leaves: intent, spawn-identity, admitted result.
fn write_history(ledger: &ParentLossLedger, generation: ParentLossGeneration, count: usize) {
    let root = admissions(ledger, generation);
    for index in 0..count {
        let launch_id = format!("worker-{index:08x}");
        let dir = root.join(&launch_id);
        fs::create_dir_all(&dir).expect("admission dir");
        let id = identity(
            generation,
            &launch_id,
            instance(100_000 + index as u32, 1_000 + index as u64),
        );
        let intent = AdmissionIntent::new(generation, launch_id.clone(), None, None);
        let spawn = AdmissionSpawnIdentity {
            schema: 1,
            identity: id.clone(),
        };
        let result = AdmissionResult {
            schema: 1,
            identity: Some(id),
            state: AdmissionResultState::Admitted,
        };
        let mut bytes = serde_json::to_vec_pretty(&intent).expect("intent json");
        bytes.push(b'\n');
        fs::write(dir.join("intent.json"), &bytes).expect("intent");
        let mut bytes = serde_json::to_vec_pretty(&spawn).expect("spawn json");
        bytes.push(b'\n');
        fs::write(dir.join("spawn-identity.json"), &bytes).expect("spawn identity");
        let mut bytes = serde_json::to_vec_pretty(&result).expect("result json");
        bytes.push(b'\n');
        fs::write(dir.join("result.json"), &bytes).expect("result");
    }
}

/// Time to close an abandoned generation holding `completed` exited launches
/// plus 3 live (fake) ones that obey TERM. Retries a refused start, summing
/// time, so a budget-sliced closer is measured whole.
fn recovery_time(completed: usize) -> (Duration, usize) {
    let directory = tempfile::Builder::new()
        .prefix("pl-regress-")
        .tempdir_in("/var/tmp")
        .expect("disk-backed temporary root");
    let (ledger, active) = open_generation(directory.path());
    let g = active.generation;
    write_history(&ledger, g, completed);
    let live: Vec<ProcessInstance> = (0..3)
        .map(|i| instance(900_001 + i, 9_000 + i as u64))
        .collect();
    for (i, pid) in live.iter().enumerate() {
        admit_task_worker(directory.path(), g, &format!("live-{i}"), *pid);
    }
    let pids: Vec<u32> = live.iter().map(|i| i.pid).collect();
    let (source, retirer) = fake(&live, &pids);
    let lease = acquire_lease(&ledger);
    let mut total = Duration::ZERO;
    for attempt in 1..=20 {
        let began = Instant::now();
        let outcome = start(&ledger, &lease, &source, &retirer, Duration::from_secs(600));
        total += began.elapsed();
        match outcome {
            Ok(_) => return (total, attempt),
            Err(error) => eprintln!("  K={completed} attempt {attempt} refused: {error:?}"),
        }
    }
    panic!("K={completed}: no start succeeded in 20 attempts ({total:?})");
}

#[test]
/// Measured on extro (debug build, nvme /var/tmp) at a211a4d5b: K=1,000 closed
/// in 1.35 s and K=100,000 in 42.4 s, one start each (31x). Under the 60 s
/// line, so it runs by default; the whole test takes about 56 s.
fn recovery_cost_does_not_grow_with_completed_launches() {
    let (small, small_attempts) = recovery_time(1_000);
    let (large, large_attempts) = recovery_time(100_000);
    eprintln!(
        "recovery: K=1,000 -> {small:?} ({small_attempts} start(s)); \
         K=100,000 -> {large:?} ({large_attempts} start(s)); ratio {:.1}x",
        large.as_secs_f64() / small.as_secs_f64().max(1e-9)
    );
    assert!(
        large <= small * 3,
        "recovery with 3 alive grew with launch history: 1k={small:?}, 100k={large:?}"
    );
}
