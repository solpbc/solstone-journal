// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(target_os = "linux")]
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::json;
use solstone_core_local::nvidia::{ArtifactTrust, NvidiaProbe};
use solstone_core_local::plan::VulkanDevice;
use solstone_core_system::provider_runtime::{
    LaunchOutcomeStatus, LifecycleSeam, LocalLaunchCommon, LocalLaunchConfig, LocalLifecycleSeam,
    LocalRuntimeShared, ManagedProcess, ProviderFence, ProviderRuntimeState,
    ProviderStopCleanupRequest, ReasonCode, ReservedPort, RuntimeClock, RuntimePhase,
    StopCleanupStatus,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_solstone-system-test-child");

struct TestClock {
    millis: AtomicU64,
}

impl RuntimeClock for TestClock {
    fn now_utc_rfc3339(&self) -> String {
        "2026-08-09T12:00:00+00:00".into()
    }

    fn monotonic_seconds(&self) -> f64 {
        self.millis.load(Ordering::Relaxed) as f64 / 1_000.0
    }

    fn sleep(&self, duration: Duration) {
        self.millis.fetch_add(
            u64::try_from(duration.as_millis()).unwrap_or(1).max(1),
            Ordering::Relaxed,
        );
        thread::yield_now();
    }
}

fn clock() -> Arc<dyn RuntimeClock> {
    Arc::new(TestClock {
        millis: AtomicU64::new(0),
    })
}

fn common(model_path: &str) -> LocalLaunchCommon {
    LocalLaunchCommon {
        desired_fingerprint_json: json!({"provider":"local"}),
        desired_fingerprint_sha256: "fingerprint".into(),
        model_id: "local/test".into(),
        model_path: model_path.into(),
        mmproj_path: None,
    }
}

fn nvidia() -> NvidiaProbe {
    NvidiaProbe {
        schema: "solstone-local-nvidia-probe-v1".into(),
        detected: true,
        gpu_index: Some(0),
        gpu_name: Some("test GPU".into()),
        compute_cap: Some("8.9".into()),
        arch: Some("sm_89".into()),
        driver_cuda_major: Some(13),
        vram_mib: Some(16_000),
        unified_memory_mib: None,
        probe_error: None,
    }
}

fn cuda(model_path: &str) -> LocalLaunchConfig {
    LocalLaunchConfig::Cuda {
        common: common(model_path),
        binary_path: Some(FIXTURE.into()),
        lib_dir: None,
        nvidia_probe: nvidia(),
        cuda_embedded_arch_set: vec!["sm_89".into()],
        cuda_min_driver_version: 13,
        cuda_artifact_trust: ArtifactTrust::Trusted,
        cuda_persisted_installed_cuda_target: false,
    }
}

fn fence(attempt: u32) -> ProviderFence {
    ProviderFence {
        incarnation: "test".into(),
        generation: 1,
        fingerprint: Some("fingerprint".into()),
        attempt,
    }
}

fn state() -> ProviderRuntimeState {
    let mut state =
        ProviderRuntimeState::new(solstone_core_system::provider_runtime::ProviderName::Local);
    state.desired_fingerprint = Some("fingerprint".into());
    state
}

fn lifecycle(shared: Arc<LocalRuntimeShared>, termination_timeout: Duration) -> LocalLifecycleSeam {
    LocalLifecycleSeam::with_timeouts(
        shared,
        clock(),
        Duration::from_secs(10),
        Duration::from_millis(1),
        termination_timeout,
    )
}

fn start(
    lifecycle: &mut LocalLifecycleSeam,
    shared: &LocalRuntimeShared,
    launch: LocalLaunchConfig,
    start_fence: &ProviderFence,
) -> solstone_core_system::provider_runtime::ProviderLaunchOutcome {
    let state = state();
    shared.record_launch_request(state.desired_fingerprint.clone(), launch);
    lifecycle.dispatch_start(&state, start_fence);
    wait_launch(shared, start_fence)
}

fn wait_launch(
    shared: &LocalRuntimeShared,
    fence: &ProviderFence,
) -> solstone_core_system::provider_runtime::ProviderLaunchOutcome {
    shared.wait_for_launch_result(fence)
}

fn wait_stop(
    shared: &LocalRuntimeShared,
    fence: &ProviderFence,
) -> solstone_core_system::provider_runtime::ProviderStopCleanupOutcome {
    shared.wait_for_stop_cleanup_result(fence)
}

#[cfg(target_os = "linux")]
fn wait_for_ready(path: &Path) {
    for _ in 0..200 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("fixture did not signal readiness");
}

#[cfg(target_os = "linux")]
fn process_is_gone(pid: u32) -> bool {
    let pid = i32::try_from(pid).expect("fixture pid fits i32");
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

fn stop_state(managed: ManagedProcess) -> ProviderRuntimeState {
    let mut state = state();
    state.pending_stop_request = Some(ProviderStopCleanupRequest {
        managed,
        reason_code: ReasonCode::known("intent-removed"),
        target_phase: RuntimePhase::Stopped,
        target_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
        admission_exclusive: false,
        orphaned_start_outcome: false,
    });
    state
}

#[test]
fn ac8_reservation_stays_held_until_after_plan_assembly() {
    let mut events = Vec::new();
    let mut reservation = ReservedPort::reserve().expect("reserve port");
    events.push("reserve");
    assert!(std::net::TcpListener::bind(("127.0.0.1", reservation.port())).is_err());
    let _plan_input_port = reservation.port();
    events.push("plan");
    let port = reservation.release_for_spawn();
    events.push("release");
    // ⚠ Bounded retry, not a single shot: once the reservation is released the port
    // is an ordinary ephemeral port, and under parallel suite load another process
    // can take it in the window before this bind. The property under test is that
    // release makes the port bindable AT ALL, not that it is bindable on the first
    // instruction, so a single `expect` here was a race that failed intermittently.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let spawned = loop {
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => break listener,
            Err(error) if std::time::Instant::now() >= deadline => {
                panic!("port {port} never became bindable after release: {error}")
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    events.push("spawn");
    drop(spawned);
    assert_eq!(events, ["reserve", "plan", "release", "spawn"]);
}

#[test]
fn each_backend_plan_rejection_maps_to_launch_failed() {
    let configs = [
        LocalLaunchConfig::Cuda {
            common: common("test-exit"),
            binary_path: None,
            lib_dir: None,
            nvidia_probe: nvidia(),
            cuda_embedded_arch_set: vec!["sm_89".into()],
            cuda_min_driver_version: 13,
            cuda_artifact_trust: ArtifactTrust::Trusted,
            cuda_persisted_installed_cuda_target: false,
        },
        LocalLaunchConfig::Vulkan {
            common: common("test-exit"),
            binary_path: None,
            devices: vec![VulkanDevice {
                index: 0,
                name: "test GPU".into(),
                device_type: None,
                vram_mib: 16_000,
            }],
            selected_gpu_index: 0,
            selected_gpu_name: "test GPU".into(),
            selected_vram_mib: 16_000,
            vram_before_mib: None,
        },
        LocalLaunchConfig::Metal {
            common: common(""),
            binary_path: None,
            unified_memory_mib: None,
        },
    ];
    for (attempt, launch) in configs.into_iter().enumerate() {
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut lifecycle = lifecycle(shared.clone(), Duration::from_secs(1));
        let start_fence = fence(u32::try_from(attempt).unwrap());
        let outcome = start(&mut lifecycle, &shared, launch, &start_fence);
        assert_eq!(outcome.status, LaunchOutcomeStatus::LaunchFailed);
        assert_eq!(outcome.reason_code, ReasonCode::known("launch-failed"));
    }
}

#[test]
fn missing_launch_request_maps_to_launch_failed() {
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut lifecycle = lifecycle(shared.clone(), Duration::from_secs(1));
    let start_fence = fence(0);
    lifecycle.dispatch_start(&state(), &start_fence);
    let outcome = wait_launch(&shared, &start_fence);
    assert_eq!(outcome.status, LaunchOutcomeStatus::LaunchFailed);
    assert_eq!(outcome.reason_code, ReasonCode::known("launch-failed"));
}

#[test]
fn warmup_reports_ready_exited_and_timeout_without_wall_clock_waits() {
    let cases = [
        ("test-ready", LaunchOutcomeStatus::Ready, "probe-ready"),
        ("test-exit", LaunchOutcomeStatus::Exited, "process-exited"),
        (
            "test-hold",
            LaunchOutcomeStatus::WarmupTimeout,
            "warmup-timeout",
        ),
    ];
    for (attempt, (model_path, status, reason)) in cases.into_iter().enumerate() {
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut lifecycle = lifecycle(shared.clone(), Duration::from_secs(1));
        let start_fence = fence(u32::try_from(attempt).unwrap());
        let outcome = start(&mut lifecycle, &shared, cuda(model_path), &start_fence);
        assert_eq!(outcome.status, status);
        assert_eq!(outcome.reason_code, ReasonCode::known(reason));
        if let Some(managed) = outcome.managed {
            let mut stopping = stop_state(managed);
            let stop_fence = fence(100 + u32::try_from(attempt).unwrap());
            lifecycle.dispatch_stop(&stopping, &stop_fence);
            assert_eq!(
                wait_stop(&shared, &stop_fence).status,
                StopCleanupStatus::Stopped
            );
            stopping.pending_stop_request = None;
        }
    }
}

#[test]
fn dispatch_stop_reports_stopped_cleanup_failed_cancelled_and_already_gone() {
    let shared = Arc::new(LocalRuntimeShared::default());
    let mut seam = lifecycle(shared.clone(), Duration::from_secs(1));
    let start_fence = fence(1);
    let started = start(&mut seam, &shared, cuda("test-ready"), &start_fence)
        .managed
        .expect("ready child");
    let stop_fence = fence(2);
    seam.dispatch_stop(&stop_state(started), &stop_fence);
    assert_eq!(
        wait_stop(&shared, &stop_fence).status,
        StopCleanupStatus::Stopped
    );

    let mut failing = lifecycle(shared.clone(), Duration::ZERO);
    let failing_start_fence = fence(3);
    let managed = start(
        &mut failing,
        &shared,
        cuda("test-ready-block-term"),
        &failing_start_fence,
    )
    .managed
    .expect("ready resistant child");
    let failed_stop_fence = fence(4);
    failing.dispatch_stop(&stop_state(managed), &failed_stop_fence);
    let failed = wait_stop(&shared, &failed_stop_fence);
    assert_eq!(failed.status, StopCleanupStatus::CleanupFailed);
    assert_eq!(
        failed.reason_code,
        ReasonCode::known("cleanup-attempt-failed")
    );

    let missing = ManagedProcess {
        id: "local:already-gone".into(),
        pid: 0,
        name: "local".into(),
        running: false,
        fence: None,
    };
    let missing_fence = fence(5);
    seam.dispatch_stop(&stop_state(missing), &missing_fence);
    assert_eq!(
        wait_stop(&shared, &missing_fence).status,
        StopCleanupStatus::Stopped
    );

    let mut cancelled = stop_state(ManagedProcess {
        id: "local:cancelled".into(),
        pid: 0,
        name: "local".into(),
        running: true,
        fence: None,
    });
    cancelled.stop_cancelled = true;
    let cancelled_fence = fence(6);
    seam.dispatch_stop(&cancelled, &cancelled_fence);
    assert_eq!(
        wait_stop(&shared, &cancelled_fence).status,
        StopCleanupStatus::Cancelled
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ac9_linux_sigkill_of_spawner_kills_direct_child() {
    let root = tempfile::tempdir().expect("temporary journal");
    let ready = root.path().join("host-death-ready");
    let mut spawner = std::process::Command::new(FIXTURE)
        .args(["host-death-direct", ready.to_str().expect("utf8")])
        .spawn()
        .expect("spawn host-death-direct fixture");
    wait_for_ready(&ready);
    let grandchild_pid: u32 = std::fs::read_to_string(&ready)
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
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        process_is_gone(grandchild_pid),
        "direct child {grandchild_pid} survived SIGKILL of its spawner"
    );
    let _ = spawner.wait();
}
