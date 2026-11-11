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
    HostedServiceKind, ParentLossLedger, read_parent_loss_admission_spawn_identity,
    write_parent_loss_admission_intent, write_parent_loss_admission_result,
    write_parent_loss_admission_spawn_identity,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
struct SealedLedgerEntry {
    launch_id: String,
    service: Option<HostedServiceKind>,
    identity: AdmissionIdentity,
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

    // Every service root the stop sealed is settled by the successor's
    // closer and leaves the outstanding set.
    let summary = closure.summary.expect("closure summary");
    assert_eq!(
        summary.total() as usize,
        sealed_launch_ids.len(),
        "every sealed root is accounted for: {summary:?}"
    );
    for launch_id in &pre_signal_launch_ids {
        assert!(
            !admissions_dir.join(launch_id).exists(),
            "{launch_id} was settled and discharged"
        );
    }
    let reread = fs::read(&sealed_path).expect("re-read sealed ledger");
    assert_eq!(
        record_after.sealed_ledger_digest,
        Some(format!(
            "{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(&reread)
        )),
        "the closed record binds its ledger"
    );

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
    let root = closure
        .admissions
        .iter()
        .find(|a| a.launch_id == identity.launch_id)
        .expect("the retired root is a noteworthy row");
    assert!(matches!(root.finding, AdmissionFinding::Retired { .. }));
    assert_eq!(root.instance, Some(identity.instance));
    assert_eq!(root.uid, Some(identity.uid));
    let summary = closure.summary.as_ref().expect("summary");
    assert_eq!(summary.retired, 1);
    assert_eq!(summary.spawn_failed as usize, history_count);
    let bytes = fs::read(ledger.sealed_ledger_path(identity.generation)).unwrap();
    assert_eq!(
        record.sealed_ledger_digest,
        Some(format!("{:x}", sha2::Sha256::digest(&bytes)))
    );
    // The settled entry left the outstanding set; its drops went with it.
    for (path, _) in original {
        assert!(!path.exists(), "{} is still outstanding", path.display());
    }
}

#[test]
fn reparented_term_resistant_descendant_survives_closer_death_then_is_reaped_from_census() {
    retained_descendant_recovery(false);
}

fn retained_descendant_recovery(rebind_cache: bool) {
    assert!(!rebind_cache, "the live cache is no longer ancestry");
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
                    let retained_now: serde_json::Value =
                        fs::read(ledger.census_obligation_path(generation))
                            .ok()
                            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                            .unwrap_or_default();
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
    assert!(
        !ledger.census_obligation_path(generation).exists(),
        "the census is deleted once the generation is closed"
    );
    assert_one_complete_identity(&ledger, &identity, &original, history_count);
}
