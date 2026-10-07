// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native process identity and a real Callosum server, with an injected cadence.
//! Windows decision paths run on every host; the native gate proves named pipes.

use super::{Unavailable, fetch, fetch_observe_status, share_for_run};
use crate::{
    args::DoctorArgs,
    checks::{journal_sync, parakeet_cpp_stt_ready, service_running, task_pace, test_support},
    context::{CheckContext, WindowsServiceRegistration},
    vocabulary::{CheckResult, Platform, Severity, Status},
};
use serde_json::{Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketServer};
use solstone_core_system::{
    lifecycle::WriterId,
    process::{ProcessInstance, current_process_identity},
};
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

fn matching(_: &CheckContext) -> WindowsServiceRegistration {
    WindowsServiceRegistration::Present {
        command: "journal.exe".into(),
        mismatch: None,
    }
}

fn context() -> test_support::StagedContext {
    let mut staged = test_support::context();
    staged.context.platform = Platform::Windows;
    staged.context.now = chrono::Utc::now();
    staged.context.service_status_timeout = Duration::from_secs(1);
    staged.context.windows_service_probe = Some(matching);
    fs::create_dir_all(staged.journal_path.join("health")).unwrap();
    write_generation(
        &staged,
        current_process_identity().expect("native self identity"),
    );
    staged
}

fn write_generation(context: &CheckContext, instance: ProcessInstance) {
    fs::write(
        context
            .journal_path
            .join("health/supervisor.process_instance"),
        serde_json::to_vec(&instance).unwrap(),
    )
    .unwrap();
}

struct Bus {
    supervisor_enabled: Arc<AtomicBool>,
    unhealthy: Arc<AtomicBool>,
    observe_mode: Arc<AtomicUsize>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    #[cfg(windows)]
    _namespace: solstone_core_callosum::test_fixture::WindowsPipeNamespace,
}

impl Bus {
    fn start(path: &Path, cadence: Duration) -> Self {
        let supervisor_enabled = Arc::new(AtomicBool::new(true));
        let unhealthy = Arc::new(AtomicBool::new(false));
        // 0 absent, 1 old, 2 future, 3 fresh.
        let observe_mode = Arc::new(AtomicUsize::new(3));
        let enabled = Arc::clone(&supervisor_enabled);
        let bad = Arc::clone(&unhealthy);
        let observe = Arc::clone(&observe_mode);
        let path = path.to_path_buf();
        #[cfg(windows)]
        let namespace = solstone_core_callosum::test_fixture::WindowsPipeNamespace::register(&path);
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let (shutdown, mut stop) = tokio::sync::oneshot::channel();
        let worker = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let server = CallosumSocketServer::bind(path).await.unwrap();
                ready_tx.send(()).unwrap();
                let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + cadence, cadence);
                let mut sequence = 0;
                loop {
                    tokio::select! {
                        _ = &mut stop => break,
                        _ = tick.tick() => {
                            sequence += 1;
                            if enabled.load(Ordering::SeqCst) {
                                let unhealthy = bad.load(Ordering::SeqCst);
                                let payload = json!({
                                    "sample": sequence,
                                    "crashed": if unhealthy {
                                        json!([{"name": format!("sample-{sequence}"), "restart_attempts": 3}])
                                    } else { json!([]) },
                                    "tasks": [{"name": format!("sample-{sequence}"), "slow": unhealthy,
                                        "duration_seconds": 12, "max_runtime_seconds": 10}],
                                    "services": [{"name": "parakeet", "phase": if unhealthy {"starting"} else {"ready"}}],
                                });
                                assert!(server.broadcast(CallosumEnvelope {
                                    tract: "supervisor".into(), event: "status".into(), ts: None,
                                    extra: payload.as_object().unwrap().clone(),
                                }));
                            }
                            let mode = observe.load(Ordering::SeqCst);
                            if mode != 0 {
                                let now = chrono::Utc::now().timestamp_millis();
                                assert!(server.broadcast(CallosumEnvelope {
                                    tract: "observe".into(), event: "status".into(),
                                    ts: match mode {1 => Some(now - 60_000), 2 => Some(now + 60_000), _ => None},
                                    extra: json!({"name": "native.observe", "recent_error_count": sequence})
                                        .as_object().unwrap().clone(),
                                }));
                            }
                        }
                    }
                }
                server.stop().await;
            });
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("real server bound");
        Self {
            supervisor_enabled,
            unhealthy,
            observe_mode,
            shutdown: Some(shutdown),
            worker: Some(worker),
            #[cfg(windows)]
            _namespace: namespace,
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.shutdown.take().unwrap().send(());
        self.worker
            .take()
            .unwrap()
            .join()
            .expect("real server stopped");
    }
}

fn sample(value: Value) -> u64 {
    value["sample"].as_u64().unwrap()
}

#[test]
fn one_run_reuses_one_beacon_but_direct_probes_are_fresh() {
    let staged = context();
    let cadence = Duration::from_millis(100);
    let _bus = Bus::start(&staged.callosum_socket_path, cadence);
    let started = Instant::now();
    let direct: Vec<_> = (0..4).map(|_| sample(fetch(&staged).unwrap())).collect();
    let direct_time = started.elapsed();
    assert!(direct.windows(2).all(|pair| pair[0] < pair[1]));
    let started = Instant::now();
    let shared = {
        let _scope = share_for_run();
        (0..4)
            .map(|_| sample(fetch(&staged).unwrap()))
            .collect::<Vec<_>>()
    };
    let shared_time = started.elapsed();
    assert!(shared.iter().all(|value| *value == shared[0]));
    assert!(sample(fetch(&staged).unwrap()) > shared[0]);
    eprintln!(
        "real Callosum cadence={cadence:?}: direct={direct:?} {direct_time:?}, shared={shared:?} {shared_time:?}"
    );
}

#[test]
fn all_supervisor_check_outcomes_survive_sample_sharing() {
    let staged = context();
    let lifecycle = solstone_core_system::lifecycle::boot(
        &staged.journal_path,
        WriterId::parse("0123456789abcdef0123456789abcdef").unwrap(),
    )
    .unwrap();
    write_generation(&staged, current_process_identity().unwrap());
    let bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(30));
    for unhealthy in [false, true] {
        bus.unhealthy.store(unhealthy, Ordering::SeqCst);
        let checks = || {
            vec![
                service_running::run(
                    &staged,
                    test_support::check("service_running", Severity::Blocker),
                )
                .unwrap(),
                journal_sync::run(
                    &staged,
                    test_support::check("journal_sync", Severity::Blocker),
                )
                .unwrap(),
                task_pace::run(
                    &staged,
                    test_support::check("task_pace", Severity::Advisory),
                )
                .unwrap(),
                // Existing seam exercises the same resident classifier after package proof.
                parakeet_cpp_stt_ready::windows_resident_for_test(
                    &staged,
                    test_support::check("default_stt_ready", Severity::Advisory),
                ),
            ]
        };
        let original = checks();
        let _scope = share_for_run();
        let shared = checks();
        assert_eq!(
            original.iter().map(|row| row.status).collect::<Vec<_>>(),
            shared.iter().map(|row| row.status).collect::<Vec<_>>()
        );
        assert_eq!(
            shared[0].status,
            if unhealthy { Status::Fail } else { Status::Ok }
        );
        assert_eq!(shared[1].status, Status::Ok);
        assert_eq!(
            shared[2].status,
            if unhealthy { Status::Warn } else { Status::Ok }
        );
        assert_eq!(
            shared[3].status,
            if unhealthy { Status::Warn } else { Status::Ok }
        );
    }
    drop(lifecycle);
}

fn failure_sample(rows: &[CheckResult], name: &str) -> u64 {
    rows.iter()
        .find(|row| row.name == name)
        .unwrap()
        .detail
        .split("sample-")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn actual_doctor_entry_shares_only_within_each_call() {
    let mut staged = context();
    // This whole-battery test checks sharing, not expiry. Native journal
    // checks can outlast the helper's one-second budget on a loaded host.
    // Match the production budget; expiry has its own bounded test below.
    staged.context.service_status_timeout = Duration::from_secs(10);
    let bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(80));
    bus.unhealthy.store(true, Ordering::SeqCst);
    let args = DoctorArgs {
        verbose: false,
        json: true,
        jsonl: false,
        port: 5015,
        readiness: false,
    };
    let first = crate::run(&args, &staged);
    let first_sample = failure_sample(&first, "service_running");
    assert_eq!(failure_sample(&first, "task_pace"), first_sample);
    let second = crate::run(&args, &staged);
    let second_sample = failure_sample(&second, "service_running");
    assert_eq!(failure_sample(&second, "task_pace"), second_sample);
    assert!(second_sample > first_sample);
    assert!(sample(fetch(&staged).unwrap()) > second_sample);
}

#[test]
fn expiry_uses_the_existing_status_budget() {
    let mut staged = context();
    staged.context.service_status_timeout = Duration::from_millis(120);
    let _bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _scope = share_for_run();
    let first = sample(fetch(&staged).unwrap());
    thread::sleep(staged.service_status_timeout + Duration::from_millis(10));
    assert!(sample(fetch(&staged).unwrap()) > first);
}

#[test]
fn missing_generation_clears_success_before_record_returns() {
    let staged = context();
    let _bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _scope = share_for_run();
    let first = sample(fetch(&staged).unwrap());
    fs::remove_file(
        staged
            .journal_path
            .join("health/supervisor.process_instance"),
    )
    .unwrap();
    assert_eq!(fetch(&staged).unwrap_err(), Unavailable::NoSocket);
    write_generation(&staged, current_process_identity().unwrap());
    assert!(sample(fetch(&staged).unwrap()) > first);
}

#[cfg(unix)]
#[test]
fn a_different_exact_live_generation_forces_a_new_beacon() {
    use solstone_core_system::process::{
        InspectResult, ProcessInstanceSource, SystemProcessInstanceSource,
    };
    let staged = context();
    let _bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _scope = share_for_run();
    let first = sample(fetch(&staged).unwrap());
    let InspectResult::Present {
        instance: parent, ..
    } = SystemProcessInstanceSource.inspect(
        u32::try_from(
            rustix::process::getppid()
                .expect("native parent PID")
                .as_raw_nonzero()
                .get(),
        )
        .unwrap(),
    )
    else {
        panic!("native parent identity must be readable");
    };
    assert_ne!(parent, current_process_identity().unwrap());
    write_generation(&staged, parent);
    assert!(sample(fetch(&staged).unwrap()) > first);
}

#[test]
#[ignore = "bounded re-exec child, invoked explicitly by the generation control"]
fn generation_child() {
    thread::sleep(Duration::from_secs(8));
}

#[test]
fn owned_exact_live_generation_replacement_fetches_then_reuses() {
    use solstone_core_system::process::{
        Disposition, InstanceVerdict, ManagedLaunchRequest, ProcessInstanceSource, SpawnOptions,
        SystemProcessInstanceSource, launch_managed_request,
    };
    let staged = context();
    let _bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _scope = share_for_run();
    let first = sample(fetch(&staged).unwrap());
    let mut child = launch_managed_request(
        Disposition::IndependentBoundedHelper {
            timeout: Duration::from_secs(2),
        },
        ManagedLaunchRequest {
            #[cfg(windows)]
            read_file_grants: Vec::new(),
            command: vec![
                std::env::current_exe()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                "--exact".into(),
                "checks::service_status::shared_tests::generation_child".into(),
                "--ignored".into(),
            ],
            options: SpawnOptions {
                journal_root: staged.journal_path.clone(),
                reference: "doctor-generation-control".into(),
                day: None,
                sink: None,
                environment: Default::default(),
            },
        },
    )
    .expect("owned bounded child launch");
    let replacement = child
        .process_instance()
        .expect("retained exact child identity");
    assert_ne!(replacement, current_process_identity().unwrap());
    assert!(matches!(
        SystemProcessInstanceSource.observe(&replacement),
        InstanceVerdict::SameLive { .. }
    ));
    write_generation(&staged, replacement);
    let fresh = sample(fetch(&staged).unwrap());
    assert!(fresh > first);
    assert_eq!(sample(fetch(&staged).unwrap()), fresh);
    child
        .terminate_exact(Duration::from_secs(2))
        .expect("bounded exact child stop");
    child.cleanup();
    assert_eq!(
        SystemProcessInstanceSource.observe(&replacement),
        InstanceVerdict::NotSameOrExited
    );
    assert_eq!(fetch(&staged).unwrap_err(), Unavailable::NoSocket);
}

#[test]
fn failure_is_retried_in_the_same_run() {
    let mut staged = context();
    staged.context.service_status_timeout = Duration::from_millis(120);
    let bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    bus.supervisor_enabled.store(false, Ordering::SeqCst);
    let _scope = share_for_run();
    assert_eq!(fetch(&staged).unwrap_err(), Unavailable::Timeout);
    bus.supervisor_enabled.store(true, Ordering::SeqCst);
    assert!(sample(fetch(&staged).unwrap()) > 0);
}

#[test]
fn unverifiable_birth_cannot_reuse_a_success() {
    let staged = context();
    let _bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _scope = share_for_run();
    let first = sample(fetch(&staged).unwrap());
    let actual = current_process_identity().unwrap();
    write_generation(
        &staged,
        ProcessInstance {
            pid: actual.pid,
            birth: solstone_core_system::process::ProcessBirth::unknown(),
        },
    );
    assert_eq!(fetch(&staged).unwrap_err(), Unavailable::Unverifiable);
    write_generation(&staged, actual);
    assert!(sample(fetch(&staged).unwrap()) > first);
}

#[test]
fn another_journal_and_endpoint_cannot_reuse_a_sample() {
    let first = context();
    let second = context();
    let _first_bus = Bus::start(&first.callosum_socket_path, Duration::from_millis(20));
    let second_bus = Bus::start(&second.callosum_socket_path, Duration::from_millis(20));
    second_bus.unhealthy.store(true, Ordering::SeqCst);
    let _scope = share_for_run();
    let value = fetch(&first).unwrap();
    assert!(value["crashed"].as_array().unwrap().is_empty());
    assert!(
        !fetch(&second).unwrap()["crashed"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        fetch(&first).unwrap()["crashed"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn unix_status_probes_remain_fresh_inside_a_run() {
    let mut staged = context();
    staged.context.platform = Platform::Linux;
    let _bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _scope = share_for_run();
    let first = sample(fetch(&staged).unwrap());
    assert!(sample(fetch(&staged).unwrap()) > first);
}

#[test]
fn nested_calls_and_other_threads_cannot_borrow_a_runs_sample() {
    let staged = context();
    let _bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _outer = share_for_run();
    let first = sample(fetch(&staged).unwrap());
    {
        let _inner = share_for_run();
        assert!(sample(fetch(&staged).unwrap()) > first);
    }
    assert_eq!(sample(fetch(&staged).unwrap()), first);
    let other = staged.context.clone();
    let different_thread = thread::spawn(move || {
        let _scope = share_for_run();
        sample(fetch(&other).unwrap())
    })
    .join()
    .unwrap();
    assert!(different_thread > first);
    assert_eq!(sample(fetch(&staged).unwrap()), first);
}

#[test]
fn observe_requires_a_new_beacon_even_with_a_shared_supervisor_sample() {
    let mut staged = context();
    staged.context.service_status_timeout = Duration::from_millis(200);
    let bus = Bus::start(&staged.callosum_socket_path, Duration::from_millis(20));
    let _scope = share_for_run();
    let _ = fetch(&staged).unwrap();
    for mode in [0, 1, 2] {
        bus.observe_mode.store(mode, Ordering::SeqCst);
        assert_eq!(
            fetch_observe_status(&staged).unwrap_err(),
            Unavailable::Timeout
        );
    }
    bus.observe_mode.store(3, Ordering::SeqCst);
    let first = fetch_observe_status(&staged).unwrap();
    let second = fetch_observe_status(&staged).unwrap();
    assert!(
        second["recent_error_count"].as_u64().unwrap()
            > first["recent_error_count"].as_u64().unwrap()
    );
}
