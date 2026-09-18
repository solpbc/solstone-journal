// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(unix)]
mod tests {
    use std::fs;
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use solstone_core_system::lifecycle::{
        AdmissionFinding, AdmissionIdentity, AdmissionIntent, BootstrapRecoveryReason,
        ClosingAuthority, ParentLossLedger, ParentLossLedgerError, SystemAdmissionRetirer,
        generate_helper_launch_id, write_parent_loss_admission_intent,
        write_parent_loss_admission_spawn_identity,
    };
    use solstone_core_system::process::{
        InspectResult, InstanceVerdict, ProcessBirth, ProcessInstance, ProcessInstanceSource,
        SystemProcessInstanceSource, set_closer_skip_test_fault, set_retirement_skip_test_fault,
    };

    struct TempJournal {
        root: PathBuf,
    }

    impl TempJournal {
        fn new(name: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = std::env::temp_dir().join(format!("solstone-never-admitted-{name}-{stamp}"));
            fs::create_dir_all(root.join("config")).expect("config directory");
            fs::create_dir_all(root.join("health")).expect("health directory");
            Self { root }
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn synth_instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    fn spawn_child(ignore_sigterm: bool) -> (Child, ProcessInstance) {
        let mut command = if ignore_sigterm {
            let mut command = Command::new("python3");
            command.args([
                "-c",
                "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(60)",
            ]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 60"]);
            command
        };
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child");
        let instance = match SystemProcessInstanceSource.inspect(child.id()) {
            InspectResult::Present { instance, .. } => instance,
            other => panic!("own child must be inspectable: {other:?}"),
        };
        (child, instance)
    }

    fn close_with_budget(
        ledger: &ParentLossLedger,
        budget: Duration,
    ) -> Result<u64, ParentLossLedgerError> {
        let lease = ledger
            .acquire_coordinator_lease()
            .expect("lease acquisition")
            .expect("lease free");
        let authority = ClosingAuthority {
            closed_by: synth_instance(30, 3),
            source: &SystemProcessInstanceSource,
            retirer: &SystemAdmissionRetirer,
            deadline: Instant::now() + budget,
        };
        ledger
            .reserve_generation_closing_abandoned(synth_instance(11, 3), [], &lease, &authority)
            .map(|active| active.generation)
    }

    // A. Pre-admission task worker + provider-like child recorded in dead generation
    #[test]
    fn pre_admission_helpers_are_retired_before_successor_reserved() {
        let journal = TempJournal::new("pre-admission-helpers");
        let ledger = ParentLossLedger::open(&journal.root).expect("ledger");
        let active = ledger
            .reserve_generation(synth_instance(10, 1), [])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let coordinator = synth_instance(20, 2);
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting state");

        // Spawn task worker helper
        let (mut task_child, task_instance) = spawn_child(false);
        let task_launch_id = generate_helper_launch_id("task-worker");
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(active.generation, &task_launch_id, None, None),
        )
        .expect("write intent");
        let task_identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: task_launch_id.clone(),
            instance: task_instance,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &task_identity)
            .expect("write task spawn identity");

        // Spawn local provider helper
        let (mut provider_child, provider_instance) = spawn_child(false);
        let provider_launch_id = generate_helper_launch_id("local-provider");
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(active.generation, &provider_launch_id, None, None),
        )
        .expect("write intent");
        let provider_identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: provider_launch_id.clone(),
            instance: provider_instance,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &provider_identity)
            .expect("write provider spawn identity");

        // Successor closes the abandoned generation
        let successor_gen = close_with_budget(&ledger, Duration::from_secs(5))
            .expect("closer retires pre-admission helpers and reserves successor");
        assert_eq!(successor_gen, active.generation + 1);

        // Both helpers must be gone before successor proceeds
        assert_eq!(
            SystemProcessInstanceSource.observe(&task_instance),
            InstanceVerdict::NotSameOrExited
        );
        assert_eq!(
            SystemProcessInstanceSource.observe(&provider_instance),
            InstanceVerdict::NotSameOrExited
        );
        let _ = task_child.wait();
        let _ = provider_child.wait();

        let record = ledger
            .record(active.generation)
            .expect("read record")
            .expect("record exists");
        let closure = record.closure.expect("closure recorded");
        assert_eq!(closure.admissions.len(), 2);
    }

    // B. Descendant-holds-resource: recorded task-worker root spawned a descendant holding bound resource
    #[test]
    fn descendant_holding_resource_is_terminated_allowing_successor_to_bind() {
        let journal = TempJournal::new("descendant-resource");
        let ledger = ParentLossLedger::open(&journal.root).expect("ledger");
        let active = ledger
            .reserve_generation(synth_instance(10, 1), [])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let coordinator = synth_instance(20, 2);
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting state");

        // Find an open port first
        let listener = TcpListener::bind("127.0.0.1:0").expect("find port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Spawn a shell child (task worker) that spawns a background listener holding the port
        let script = format!(
            "python3 -c \"import socket, time; s = socket.socket(); s.bind(('127.0.0.1', {port})); s.listen(1); time.sleep(60)\" & sleep 60"
        );
        let mut root_child = Command::new("sh")
            .args(["-c", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn root child");
        let root_instance = match SystemProcessInstanceSource.inspect(root_child.id()) {
            InspectResult::Present { instance, .. } => instance,
            other => panic!("own child inspectable: {other:?}"),
        };

        // Wait briefly for descendant to bind the port
        let mut bound = false;
        for _ in 0..100 {
            if TcpListener::bind(format!("127.0.0.1:{port}")).is_err() {
                bound = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(bound, "descendant must have bound the port");

        let launch_id = generate_helper_launch_id("task-worker");
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(active.generation, &launch_id, None, None),
        )
        .expect("write intent");
        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.clone(),
            instance: root_instance,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &identity)
            .expect("write spawn identity");

        // Successor closes abandoned generation, terminating root and descendants
        let successor_gen = close_with_budget(&ledger, Duration::from_secs(5))
            .expect("closer terminates root and descendants");
        assert_eq!(successor_gen, active.generation + 1);

        let _ = root_child.wait();

        // Successor can now bind the port because the descendant was terminated
        let mut reacquired = false;
        for _ in 0..100 {
            if TcpListener::bind(format!("127.0.0.1:{port}")).is_ok() {
                reacquired = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            reacquired,
            "successor must be able to bind the port released by descendant termination"
        );
    }

    // C. macOS two-journal isolation
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_two_journal_isolation_does_not_signal_unrelated_journal_children() {
        let journal_a = TempJournal::new("macos-a");
        let journal_b = TempJournal::new("macos-b");

        let ledger_a = ParentLossLedger::open(&journal_a.root).expect("ledger a");
        let active_a = ledger_a
            .reserve_generation(synth_instance(10, 1), [])
            .expect("reserve a");
        ledger_a.initialize_record(&active_a).expect("record a");
        let coord_a = synth_instance(20, 2);
        ledger_a
            .persist_coordinator_identity(active_a.generation, coord_a)
            .expect("coord a");
        ledger_a
            .mark_admitting(active_a.generation, coord_a)
            .expect("admitting a");

        let ledger_b = ParentLossLedger::open(&journal_b.root).expect("ledger b");
        let active_b = ledger_b
            .reserve_generation(synth_instance(110, 11), [])
            .expect("reserve b");
        ledger_b.initialize_record(&active_b).expect("record b");
        let coord_b = synth_instance(120, 12);
        ledger_b
            .persist_coordinator_identity(active_b.generation, coord_b)
            .expect("coord b");
        ledger_b
            .mark_admitting(active_b.generation, coord_b)
            .expect("admitting b");

        // Journal A child (to be retired)
        let (mut child_a, instance_a) = spawn_child(false);
        let launch_id_a = generate_helper_launch_id("task-worker");
        write_parent_loss_admission_intent(
            &journal_a.root,
            &AdmissionIntent::new(active_a.generation, &launch_id_a, None, None),
        )
        .expect("intent a");
        let identity_a = AdmissionIdentity {
            generation: active_a.generation,
            launch_id: launch_id_a,
            instance: instance_a,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal_a.root, &identity_a)
            .expect("spawn identity a");

        // Same-named journal B child, recorded by B's own generation (must stay alive).
        let (mut child_b, instance_b) = spawn_child(false);
        let launch_id_b = generate_helper_launch_id("task-worker");
        write_parent_loss_admission_intent(
            &journal_b.root,
            &AdmissionIntent::new(active_b.generation, &launch_id_b, None, None),
        )
        .expect("intent b");
        let identity_b = AdmissionIdentity {
            generation: active_b.generation,
            launch_id: launch_id_b,
            instance: instance_b,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal_b.root, &identity_b)
            .expect("spawn identity b");

        // Close journal A
        let succ_a = close_with_budget(&ledger_a, Duration::from_secs(5)).expect("close a");
        assert_eq!(succ_a, active_a.generation + 1);

        // Child A is dead
        assert_eq!(
            SystemProcessInstanceSource.observe(&instance_a),
            InstanceVerdict::NotSameOrExited
        );
        let _ = child_a.wait();

        // Child B is still live!
        assert!(matches!(
            SystemProcessInstanceSource.observe(&instance_b),
            InstanceVerdict::SameLive { .. }
        ));
        let _ = child_b.kill();
        let _ = child_b.wait();
    }

    // D. Closer-disabled / retirement-disabled drive outcomes
    #[test]
    fn closer_skip_and_retirement_skip_faults_drive_refusal_outcomes() {
        struct FaultGuard;
        impl Drop for FaultGuard {
            fn drop(&mut self) {
                set_closer_skip_test_fault(false);
                set_retirement_skip_test_fault(false);
            }
        }
        let _guard = FaultGuard;

        let journal = TempJournal::new("fault-outcomes");
        let ledger = ParentLossLedger::open(&journal.root).expect("ledger");
        let active = ledger
            .reserve_generation(synth_instance(10, 1), [])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let coordinator = synth_instance(20, 2);
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting state");

        let (mut child, instance) = spawn_child(false);
        let launch_id = generate_helper_launch_id("task-worker");
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(active.generation, &launch_id, None, None),
        )
        .expect("write intent");
        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.clone(),
            instance,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &identity)
            .expect("write spawn identity");

        // 1. Retirement-skip fault: returns AbandonedAdmissionLive, child stays live
        set_retirement_skip_test_fault(true);
        let result = close_with_budget(&ledger, Duration::from_secs(5));
        assert!(matches!(
            result,
            Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionLive
            ))
        ));
        assert!(matches!(
            SystemProcessInstanceSource.observe(&instance),
            InstanceVerdict::SameLive { .. }
        ));
        set_retirement_skip_test_fault(false);

        // 2. Closer-skip fault: returns CoordinatorNotLive (or ActiveCoordinator)
        set_closer_skip_test_fault(true);
        let result = close_with_budget(&ledger, Duration::from_secs(5));
        assert!(matches!(
            result,
            Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::CoordinatorNotLive
            ))
        ));
        set_closer_skip_test_fault(false);

        let _ = child.kill();
        let _ = child.wait();
    }

    // E. Live-process case with trap '' TERM that requires SIGKILL and records Retired { escalated: true }
    #[test]
    fn stubborn_live_child_is_escalated_and_recorded_as_escalated() {
        let journal = TempJournal::new("escalated-child");
        let ledger = ParentLossLedger::open(&journal.root).expect("ledger");
        let active = ledger
            .reserve_generation(synth_instance(10, 1), [])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let coordinator = synth_instance(20, 2);
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting state");

        // Spawn child ignoring SIGTERM
        let (mut stubborn_child, stubborn_instance) = spawn_child(true);
        thread::sleep(Duration::from_millis(200));

        let launch_id = generate_helper_launch_id("task-worker");
        write_parent_loss_admission_intent(
            &journal.root,
            &AdmissionIntent::new(active.generation, &launch_id, None, None),
        )
        .expect("write intent");
        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.clone(),
            instance: stubborn_instance,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        write_parent_loss_admission_spawn_identity(&journal.root, &identity)
            .expect("write spawn identity");

        let successor_gen = close_with_budget(&ledger, Duration::from_secs(5))
            .expect("closer escalates to SIGKILL and closes generation");
        assert_eq!(successor_gen, active.generation + 1);

        assert_eq!(
            SystemProcessInstanceSource.observe(&stubborn_instance),
            InstanceVerdict::NotSameOrExited
        );
        let _ = stubborn_child.wait();

        let record = ledger
            .record(active.generation)
            .expect("read record")
            .expect("record exists");
        let closure = record.closure.expect("closure exists");
        assert_eq!(closure.admissions.len(), 1);
        assert_eq!(
            closure.admissions[0].finding,
            AdmissionFinding::Retired { escalated: true }
        );
    }
}

#[cfg(not(unix))]
#[test]
fn never_admitted_generation_children_is_unix_only() {}
