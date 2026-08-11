// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Parakeet-provider launch, port reservation, warmup, and lifecycle work.
//!
//! Mirrors `launch.rs`'s Local seam shapes (spawn a subprocess, warmup-poll
//! its `/health` endpoint, record readiness) but for parakeet-server's
//! single-binary launch instead of Local's Cuda/Vulkan/Mlx backend
//! selection. Deliberately does not yet decide *whether* to launch --
//! that decision depends on the admission latch (a process-wide fail-closed
//! read of the runtime-health record) and GPU auto-placement, both ported
//! separately. This module is the seam machinery a truth decision hands a
//! plan to, not the decision itself.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io::{Read, Write};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Child, Command};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use std::collections::BTreeMap;

use crate::process::{SERVICE_SHUTDOWN_TIMEOUT, terminate};

use super::model::{
    LaunchOutcomeStatus, ManagedProcess, ProviderFence, ProviderLaunchOutcome,
    ProviderProbeOutcome, ProviderRuntimeState, ProviderStopCleanupOutcome,
    ProviderTruthObservation, ReasonCode, StopCleanupStatus,
};
use super::seams::{LifecycleSeam, ProbeSeam};
use super::store::{FenceKey, ReadyProcess, ReadyProcessLookup, RuntimeClock};

pub const PARAKEET_SERVER_PROCESS_NAME: &str = "parakeet-server";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// Mirrors Python's `ParakeetServerLaunchPlan` field-for-field. `env_updates`
/// and `gpu_index` are populated by GPU auto-placement (ported separately);
/// this struct only carries what a plan already decided.
#[derive(Debug, Clone)]
pub struct ParakeetLaunchConfig {
    pub binary_backend: String,
    pub env_updates: BTreeMap<String, String>,
    pub gpu_index: Option<u32>,
    pub binary_path: PathBuf,
    pub model_path: PathBuf,
    pub threads: u32,
    pub desired_fingerprint_json: String,
    pub desired_fingerprint_sha256: String,
    pub placement: ParakeetPlacement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParakeetPlacement {
    Cpu,
    Gpu,
}

impl ParakeetPlacement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
        }
    }
}

#[derive(Debug, Default)]
struct ParakeetRuntimeResults {
    truth: BTreeMap<FenceKey, ProviderTruthObservation>,
    launch: BTreeMap<FenceKey, ProviderLaunchOutcome>,
    stop_cleanup: BTreeMap<FenceKey, ProviderStopCleanupOutcome>,
    probe: BTreeMap<FenceKey, ProviderProbeOutcome>,
}

/// Parakeet's own in-memory seam-to-reconcile bus -- structurally identical
/// to `LocalRuntimeShared` (fence-keyed result channels, ready-process
/// tracking, retained child handles) but a distinct type because its
/// launch-config staging is typed to `ParakeetLaunchConfig`, not
/// `LocalLaunchConfig`. `FileRuntimeStore` (the durable record) only needs
/// the `ReadyProcessLookup` slice of this, which is why the store did not
/// need to duplicate to gain a second provider.
#[derive(Debug, Default)]
pub struct ParakeetRuntimeShared {
    ready_processes: Mutex<BTreeMap<FenceKey, ReadyProcess>>,
    launch_requests: Mutex<BTreeMap<Option<String>, ParakeetLaunchConfig>>,
    results: Mutex<ParakeetRuntimeResults>,
    result_available: Condvar,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    children: Mutex<BTreeMap<String, Child>>,
}

impl ParakeetRuntimeShared {
    pub fn record_launch_request(
        &self,
        desired_fingerprint: Option<String>,
        config: ParakeetLaunchConfig,
    ) {
        self.launch_requests
            .lock()
            .expect("parakeet runtime shared lock")
            .insert(desired_fingerprint, config);
    }

    pub fn launch_request_for(
        &self,
        desired_fingerprint: &Option<String>,
    ) -> Option<ParakeetLaunchConfig> {
        self.launch_requests
            .lock()
            .expect("parakeet runtime shared lock")
            .get(desired_fingerprint)
            .cloned()
    }

    pub fn record_truth_result(&self, fence: &ProviderFence, result: ProviderTruthObservation) {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .truth
            .insert(FenceKey::from(fence), result);
        self.result_available.notify_all();
    }

    pub fn take_truth_result(&self, fence: &ProviderFence) -> Option<ProviderTruthObservation> {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .truth
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_truth_result(&self, fence: &ProviderFence) -> ProviderTruthObservation {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("parakeet runtime shared lock");
        loop {
            if let Some(result) = results.truth.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("parakeet runtime shared lock");
        }
    }

    pub fn record_launch_result(&self, fence: &ProviderFence, result: ProviderLaunchOutcome) {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .launch
            .insert(FenceKey::from(fence), result);
        self.result_available.notify_all();
    }

    pub fn take_launch_result(&self, fence: &ProviderFence) -> Option<ProviderLaunchOutcome> {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .launch
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_launch_result(&self, fence: &ProviderFence) -> ProviderLaunchOutcome {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("parakeet runtime shared lock");
        loop {
            if let Some(result) = results.launch.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("parakeet runtime shared lock");
        }
    }

    pub fn record_stop_cleanup_result(
        &self,
        fence: &ProviderFence,
        result: ProviderStopCleanupOutcome,
    ) {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .stop_cleanup
            .insert(FenceKey::from(fence), result);
        self.result_available.notify_all();
    }

    pub fn take_stop_cleanup_result(
        &self,
        fence: &ProviderFence,
    ) -> Option<ProviderStopCleanupOutcome> {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .stop_cleanup
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_stop_cleanup_result(
        &self,
        fence: &ProviderFence,
    ) -> ProviderStopCleanupOutcome {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("parakeet runtime shared lock");
        loop {
            if let Some(result) = results.stop_cleanup.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("parakeet runtime shared lock");
        }
    }

    pub fn record_probe_result(&self, fence: &ProviderFence, result: ProviderProbeOutcome) {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .probe
            .insert(FenceKey::from(fence), result);
        self.result_available.notify_all();
    }

    pub fn take_probe_result(&self, fence: &ProviderFence) -> Option<ProviderProbeOutcome> {
        self.results
            .lock()
            .expect("parakeet runtime shared lock")
            .probe
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_probe_result(&self, fence: &ProviderFence) -> ProviderProbeOutcome {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("parakeet runtime shared lock");
        loop {
            if let Some(result) = results.probe.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("parakeet runtime shared lock");
        }
    }

    pub fn record_ready_process(&self, fence: &ProviderFence, process: ReadyProcess) {
        self.ready_processes
            .lock()
            .expect("parakeet runtime shared lock")
            .insert(FenceKey::from(fence), process);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn retain_child(&self, process_id: String, child: Child) {
        self.children
            .lock()
            .expect("parakeet runtime shared lock")
            .insert(process_id, child);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn take_child(&self, process_id: &str) -> Option<Child> {
        self.children
            .lock()
            .expect("parakeet runtime shared lock")
            .remove(process_id)
    }
}

impl ReadyProcessLookup for ParakeetRuntimeShared {
    fn ready_process_for_fence(&self, fence: &ProviderFence) -> Option<ReadyProcess> {
        self.ready_processes
            .lock()
            .expect("parakeet runtime shared lock")
            .get(&FenceKey::from(fence))
            .cloned()
    }

    fn ready_process_for_id(&self, process_id: &str) -> Option<ReadyProcess> {
        self.ready_processes
            .lock()
            .expect("parakeet runtime shared lock")
            .values()
            .find(|process| process.process_id == process_id)
            .cloned()
    }
}

pub struct ParakeetLifecycleSeam {
    shared: Arc<ParakeetRuntimeShared>,
    clock: Arc<dyn RuntimeClock>,
    warmup_timeout: Duration,
    warmup_poll_interval: Duration,
    termination_timeout: Duration,
}

impl ParakeetLifecycleSeam {
    pub fn new(shared: Arc<ParakeetRuntimeShared>, clock: Arc<dyn RuntimeClock>) -> Self {
        Self::with_timeouts(
            shared,
            clock,
            Duration::from_secs(300),
            Duration::from_millis(50),
            SERVICE_SHUTDOWN_TIMEOUT,
        )
    }

    pub fn with_timeouts(
        shared: Arc<ParakeetRuntimeShared>,
        clock: Arc<dyn RuntimeClock>,
        warmup_timeout: Duration,
        warmup_poll_interval: Duration,
        termination_timeout: Duration,
    ) -> Self {
        Self {
            shared,
            clock,
            warmup_timeout,
            warmup_poll_interval,
            termination_timeout,
        }
    }
}

impl LifecycleSeam for ParakeetLifecycleSeam {
    fn dispatch_start(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let _clock = Arc::clone(&self.clock);
        let launch = shared.launch_request_for(&state.desired_fingerprint);
        let fence = fence.clone();
        let warmup_timeout = self.warmup_timeout;
        let warmup_poll_interval = self.warmup_poll_interval;
        thread::spawn(move || {
            let outcome = launch
                .map(|launch| {
                    start_parakeet(
                        &shared,
                        &launch,
                        &fence,
                        warmup_timeout,
                        warmup_poll_interval,
                    )
                })
                .unwrap_or_else(launch_failed);
            shared.record_launch_result(&fence, outcome);
        });
    }

    fn dispatch_stop(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let fence = fence.clone();
        let request = state.pending_stop_request.clone();
        let stop_cancelled = state.stop_cancelled;
        let termination_timeout = self.termination_timeout;
        thread::spawn(move || {
            let outcome = stop_parakeet(
                &shared,
                request.as_ref(),
                stop_cancelled,
                termination_timeout,
            );
            shared.record_stop_cleanup_result(&fence, outcome);
        });
    }
}

pub struct ParakeetProbeSeam {
    shared: Arc<ParakeetRuntimeShared>,
    journal_path: PathBuf,
}

impl ParakeetProbeSeam {
    pub fn new(shared: Arc<ParakeetRuntimeShared>, journal_path: impl Into<PathBuf>) -> Self {
        Self {
            shared,
            journal_path: journal_path.into(),
        }
    }
}

impl ProbeSeam for ParakeetProbeSeam {
    fn dispatch_probe(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let journal_path = self.journal_path.clone();
        let fence = fence.clone();
        let launch = shared.launch_request_for(&state.desired_fingerprint);
        thread::spawn(move || {
            let outcome = match launch {
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                Some(_) => probe_parakeet(&journal_path),
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                Some(_) => probe_unavailable(),
                None => probe_unavailable(),
            };
            shared.record_probe_result(&fence, outcome);
        });
    }
}

/// Mirrors `solstone_core_local::connect`'s pattern exactly: read the
/// *durable* owned-port file the store already publishes
/// (`health/parakeet-cpp.port`), not an in-memory fence-keyed lookup. A
/// probe's own fence is not the launch's fence -- looking up
/// `ready_process_for_fence(&probe_fence)` would only work by coincidence.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn probe_parakeet(journal_path: &std::path::Path) -> ProviderProbeOutcome {
    let Some(port) = std::fs::read_to_string(journal_path.join("health/parakeet-cpp.port"))
        .ok()
        .and_then(|text| text.trim().parse::<u16>().ok())
    else {
        return probe_unavailable();
    };
    if probe_health(port) {
        ProviderProbeOutcome {
            status: super::model::ProbeStatus::Ready,
            reason_code: ReasonCode::known("probe-ready"),
        }
    } else {
        probe_unavailable()
    }
}

/// Read the durable Parakeet port and perform a bounded `/health` state read.
/// This is the doctor-facing sibling of the lifecycle's one-second probe.
pub fn probe_parakeet_cpp_server(
    journal_path: &std::path::Path,
    timeout: Duration,
) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let port = std::fs::read_to_string(journal_path.join("health/parakeet-cpp.port"))
            .map_err(|error| error.to_string())?
            .trim()
            .parse::<u16>()
            .map_err(|error| error.to_string())?;
        probe_health_with_timeout(port, timeout)
            .then_some(())
            .ok_or_else(|| "health endpoint did not return HTTP 200".to_owned())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (journal_path, timeout);
        Err("parakeet-cpp health probe is unsupported on this platform".to_owned())
    }
}

fn probe_unavailable() -> ProviderProbeOutcome {
    ProviderProbeOutcome {
        status: super::model::ProbeStatus::Unavailable,
        reason_code: ReasonCode::known("proof-observation-unavailable"),
    }
}

fn launch_failed() -> ProviderLaunchOutcome {
    ProviderLaunchOutcome {
        status: LaunchOutcomeStatus::LaunchFailed,
        reason_code: ReasonCode::known("launch-failed"),
        managed: None,
    }
}

/// Mirrors `_build_parakeet_cmd` in supervisor.py verbatim -- flag spellings
/// are the Python authors' own "re-verify at live bring-up" placeholders,
/// carried forward rather than invented independently.
fn build_parakeet_cmd(
    binary_path: &std::path::Path,
    model_path: &std::path::Path,
    port: u16,
    threads: u32,
) -> Vec<String> {
    vec![
        binary_path.display().to_string(),
        "--model".to_owned(),
        model_path.display().to_string(),
        "--host".to_owned(),
        "127.0.0.1".to_owned(),
        "--port".to_owned(),
        port.to_string(),
        "--threads".to_owned(),
        threads.to_string(),
    ]
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn start_parakeet(
    shared: &ParakeetRuntimeShared,
    launch: &ParakeetLaunchConfig,
    fence: &ProviderFence,
    warmup_timeout: Duration,
    warmup_poll_interval: Duration,
) -> ProviderLaunchOutcome {
    let mut reservation = match super::launch::ReservedPort::reserve() {
        Ok(reservation) => reservation,
        Err(_) => return launch_failed(),
    };
    // The reservation's listener must be dropped before spawn -- otherwise
    // this process still holds the port and the child's own bind() to it
    // fails with "Address already in use".
    let port = reservation.release_for_spawn();
    let cmd = build_parakeet_cmd(
        &launch.binary_path,
        &launch.model_path,
        port,
        launch.threads,
    );
    let mut child = match spawn_parakeet(&cmd, &launch.env_updates) {
        Ok(child) => child,
        Err(_) => return launch_failed(),
    };
    let process_id = format!("parakeet:{}", child.id());
    let pid = child.id();
    let deadline = std::time::Instant::now() + warmup_timeout;
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Exited,
                reason_code: ReasonCode::known("process-exited"),
                managed: None,
            };
        }
        if probe_health(port) {
            let managed = ManagedProcess {
                id: process_id.clone(),
                name: PARAKEET_SERVER_PROCESS_NAME.into(),
                running: true,
            };
            shared.retain_child(process_id.clone(), child);
            shared.record_ready_process(
                fence,
                ReadyProcess {
                    process_id,
                    process_name: PARAKEET_SERVER_PROCESS_NAME.into(),
                    pid,
                    port,
                },
            );
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Ready,
                reason_code: ReasonCode::known("probe-ready"),
                managed: Some(managed),
            };
        }
        if std::time::Instant::now() >= deadline {
            let managed = ManagedProcess {
                id: process_id.clone(),
                name: PARAKEET_SERVER_PROCESS_NAME.into(),
                running: true,
            };
            shared.retain_child(process_id, child);
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::WarmupTimeout,
                reason_code: ReasonCode::known("warmup-timeout"),
                managed: Some(managed),
            };
        }
        thread::sleep(warmup_poll_interval);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn start_parakeet(
    _: &ParakeetRuntimeShared,
    _: &ParakeetLaunchConfig,
    _: &ProviderFence,
    _: Duration,
    _: Duration,
) -> ProviderLaunchOutcome {
    launch_failed()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn spawn_parakeet(
    cmd: &[String],
    env_updates: &BTreeMap<String, String>,
) -> std::io::Result<Child> {
    let (program, arguments) = cmd.split_first().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "parakeet launch command has no argv",
        )
    })?;
    Command::new(program)
        .args(arguments)
        .envs(env_updates)
        .spawn()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn probe_health(port: u16) -> bool {
    probe_health_with_timeout(port, HEALTH_PROBE_TIMEOUT)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn probe_health_with_timeout(port: u16, timeout: Duration) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&address, timeout) {
        Ok(stream) => stream,
        Err(_) => return false,
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
        || stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .is_err()
    {
        return false;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }
    response
        .split_once("\r\n")
        .map(|(status_line, _)| status_line.split_whitespace().nth(1) == Some("200"))
        .unwrap_or(false)
}

fn stop_parakeet(
    shared: &ParakeetRuntimeShared,
    request: Option<&super::model::ProviderStopCleanupRequest>,
    stop_cancelled: bool,
    termination_timeout: Duration,
) -> ProviderStopCleanupOutcome {
    let reason_code = request
        .and_then(|request| request.target_reason_code.clone())
        .unwrap_or_else(|| ReasonCode::known("cleanup-succeeded"));
    if stop_cancelled || request.is_none() {
        return ProviderStopCleanupOutcome {
            status: StopCleanupStatus::Cancelled,
            reason_code,
            managed: None,
        };
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let request = request.expect("checked above");
        let Some(mut child) = shared.take_child(&request.managed.id) else {
            return ProviderStopCleanupOutcome {
                status: StopCleanupStatus::Stopped,
                reason_code,
                managed: None,
            };
        };
        match terminate(&mut child, termination_timeout) {
            Ok(_) => ProviderStopCleanupOutcome {
                status: StopCleanupStatus::Stopped,
                reason_code,
                managed: None,
            },
            Err(_) => {
                shared.retain_child(request.managed.id.clone(), child);
                ProviderStopCleanupOutcome {
                    status: StopCleanupStatus::CleanupFailed,
                    reason_code: ReasonCode::known("cleanup-attempt-failed"),
                    managed: None,
                }
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (shared, termination_timeout);
        ProviderStopCleanupOutcome {
            status: StopCleanupStatus::Stopped,
            reason_code,
            managed: None,
        }
    }
}

// AC11: cleanup on stop is idempotent, mirroring `_cleanup_parakeet_launch` /
// `_terminate_cleanup_handle` -- a second stop call for the same managed
// process must not attempt a second kill or otherwise fail, since nothing
// guarantees a caller sees the reconciler retire the request before a
// duplicate stop dispatch lands.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::provider_runtime::model::ProviderStopCleanupRequest;

    fn stop_request(managed_id: &str) -> ProviderStopCleanupRequest {
        ProviderStopCleanupRequest {
            managed: ManagedProcess {
                id: managed_id.to_owned(),
                name: PARAKEET_SERVER_PROCESS_NAME.to_owned(),
                running: true,
            },
            reason_code: ReasonCode::known("cleanup-succeeded"),
            target_phase: super::super::model::RuntimePhase::NotDesired,
            target_reason_code: None,
            admission_exclusive: false,
            orphaned_start_outcome: false,
        }
    }

    #[test]
    fn stopping_the_same_managed_process_twice_is_idempotent() {
        let shared = ParakeetRuntimeShared::default();
        // Plain `sleep`, not the crate's own test-child fixture:
        // `CARGO_BIN_EXE_<bin>` is only injected for `tests/`/`examples/`/
        // `benches/` targets, not for the lib's own unit tests, and `sleep`
        // is present on every platform this test is cfg-gated to.
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep child");
        shared.retain_child("managed-1".to_owned(), child);
        let request = stop_request("managed-1");

        let first = stop_parakeet(&shared, Some(&request), false, Duration::from_secs(5));
        assert_eq!(first.status, StopCleanupStatus::Stopped);

        let second = stop_parakeet(&shared, Some(&request), false, Duration::from_secs(5));
        assert_eq!(second.status, StopCleanupStatus::Stopped);
        assert_eq!(second.managed, None);
    }
}
