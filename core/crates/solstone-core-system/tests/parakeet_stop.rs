// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use solstone_core_system::provider_runtime::{
    LaunchOutcomeStatus, LifecycleSeam, ManagedProcess, PARAKEET_SERVER_PROCESS_NAME,
    ParakeetLaunchConfig, ParakeetLifecycleSeam, ParakeetPlacement, ParakeetRuntimeShared,
    ProviderFence, ProviderName, ProviderRuntimeState, ProviderStopCleanupRequest, ReasonCode,
    RuntimeClock, RuntimePhase, StopCleanupStatus, SystemRuntimeClock,
};

const FIXTURE: &str = env!("CARGO_BIN_EXE_solstone-system-test-child");

fn fence(attempt: u32) -> ProviderFence {
    ProviderFence {
        incarnation: "test".to_owned(),
        generation: 1,
        fingerprint: Some("fingerprint".to_owned()),
        attempt,
    }
}

fn launch(model_path: &str) -> ParakeetLaunchConfig {
    ParakeetLaunchConfig {
        binary_backend: "cpu".to_owned(),
        env_updates: BTreeMap::new(),
        gpu_index: None,
        binary_path: PathBuf::from(FIXTURE),
        model_path: PathBuf::from(model_path),
        threads: 1,
        desired_fingerprint_json: "{}".to_owned(),
        desired_fingerprint_sha256: "desired".to_owned(),
        placement: ParakeetPlacement::Cpu,
    }
}

fn seam(
    shared: Arc<ParakeetRuntimeShared>,
    warmup_timeout: Duration,
    termination_timeout: Duration,
) -> ParakeetLifecycleSeam {
    let clock: Arc<dyn RuntimeClock> = Arc::new(SystemRuntimeClock::default());
    ParakeetLifecycleSeam::with_timeouts(
        shared,
        clock,
        warmup_timeout,
        Duration::from_millis(1),
        termination_timeout,
    )
}

fn start(
    lifecycle: &mut ParakeetLifecycleSeam,
    shared: &ParakeetRuntimeShared,
    model_path: &str,
    start_fence: &ProviderFence,
) -> solstone_core_system::provider_runtime::ProviderLaunchOutcome {
    let mut state = ProviderRuntimeState::new(ProviderName::Parakeet);
    state.desired_fingerprint = Some("desired".to_owned());
    shared.record_launch_request(state.desired_fingerprint.clone(), launch(model_path));
    lifecycle.dispatch_start(&state, start_fence);
    shared.wait_for_launch_result(start_fence)
}

fn stop_state(managed: ManagedProcess) -> ProviderRuntimeState {
    let mut state = ProviderRuntimeState::new(ProviderName::Parakeet);
    state.pending_stop_request = Some(ProviderStopCleanupRequest {
        managed,
        reason_code: ReasonCode::known("cleanup-succeeded"),
        target_phase: RuntimePhase::Stopped,
        target_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
        admission_exclusive: false,
        orphaned_start_outcome: false,
    });
    state
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn stopping_the_same_managed_process_twice_is_idempotent() {
    let shared = Arc::new(ParakeetRuntimeShared::default());
    let mut lifecycle = seam(
        shared.clone(),
        Duration::from_millis(200),
        Duration::from_secs(2),
    );
    let started = start(&mut lifecycle, &shared, "test-hold", &fence(1));
    assert_eq!(started.status, LaunchOutcomeStatus::WarmupTimeout);
    let managed = started.managed.expect("warmup timeout retains the child");
    assert_eq!(managed.name, PARAKEET_SERVER_PROCESS_NAME);
    assert!(managed.fence.is_none());

    let first_fence = fence(2);
    lifecycle.dispatch_stop(&stop_state(managed.clone()), &first_fence);
    let first = shared.wait_for_stop_cleanup_result(&first_fence);
    assert_eq!(first.status, StopCleanupStatus::Stopped);

    let second_fence = fence(3);
    lifecycle.dispatch_stop(&stop_state(managed), &second_fence);
    let second = shared.wait_for_stop_cleanup_result(&second_fence);
    assert_eq!(second.status, StopCleanupStatus::Stopped);
    assert_eq!(second.managed, None);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn dispatch_stop_reports_cleanup_failed_for_a_term_resistant_child() {
    let shared = Arc::new(ParakeetRuntimeShared::default());
    let mut starting = seam(
        shared.clone(),
        Duration::from_secs(5),
        Duration::from_secs(1),
    );
    let started = start(&mut starting, &shared, "test-ready-block-term", &fence(1));
    assert_eq!(started.status, LaunchOutcomeStatus::Ready);
    let managed = started.managed.expect("ready child");

    let mut failing = seam(shared.clone(), Duration::from_secs(5), Duration::ZERO);
    let stop_fence = fence(2);
    failing.dispatch_stop(&stop_state(managed), &stop_fence);
    let failed = shared.wait_for_stop_cleanup_result(&stop_fence);
    assert_eq!(failed.status, StopCleanupStatus::CleanupFailed);
    assert_eq!(
        failed.reason_code,
        ReasonCode::known("cleanup-attempt-failed")
    );
}

#[test]
fn dispatch_stop_reports_cancelled_without_a_child() {
    let shared = Arc::new(ParakeetRuntimeShared::default());
    let mut lifecycle = seam(
        shared.clone(),
        Duration::from_secs(1),
        Duration::from_secs(1),
    );
    let mut cancelled = stop_state(ManagedProcess {
        id: "parakeet:cancelled".to_owned(),
        pid: 0,
        name: PARAKEET_SERVER_PROCESS_NAME.to_owned(),
        running: true,
        fence: None,
    });
    cancelled.stop_cancelled = true;
    let stop_fence = fence(1);
    lifecycle.dispatch_stop(&cancelled, &stop_fence);
    let outcome = shared.wait_for_stop_cleanup_result(&stop_fence);
    assert_eq!(outcome.status, StopCleanupStatus::Cancelled);
}

#[cfg(target_os = "linux")]
fn wait_for_ready(path: &Path) {
    for _ in 0..200 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
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

#[cfg(target_os = "linux")]
#[test]
fn linux_sigkill_of_spawner_kills_direct_child() {
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
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        process_is_gone(grandchild_pid),
        "direct child {grandchild_pid} survived SIGKILL of its spawner"
    );
    let _ = spawner.wait();
}
