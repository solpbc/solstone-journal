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
use std::time::{Duration, Instant};

use std::collections::BTreeMap;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use std::collections::BTreeSet;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::process::apply_parent_death_kill;
use crate::process::{
    Disposition, LaunchAuthority, LaunchError, ProcessObservation, ProcessObservationTuple,
    SERVICE_SHUTDOWN_TIMEOUT, classify_process_observation,
};

use super::model::{
    LaunchOutcomeStatus, ManagedProcess, ProviderFence, ProviderLaunchOutcome,
    ProviderProbeOutcome, ProviderRuntimeState, ProviderStopCleanupOutcome,
    ProviderTruthObservation, ReasonCode, StopCleanupStatus,
};
use super::seams::{LifecycleSeam, ProbeSeam};
use super::store::{
    CurrentProcessResolution, FenceKey, ReadyChild, ReadyChildIdentity, ReadyProcess,
    ReadyProcessLookup, RuntimeClock, insert_ready_tuple, remove_ready_tuple,
    resolve_current_process,
};

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
    started_at: Mutex<BTreeMap<FenceKey, Instant>>,
    launch_requests: Mutex<BTreeMap<Option<String>, ParakeetLaunchConfig>>,
    results: Mutex<ParakeetRuntimeResults>,
    result_available: Condvar,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    ready_children: Mutex<BTreeMap<FenceKey, ReadyChild>>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    children: Mutex<BTreeMap<String, LaunchAuthority>>,
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

    #[cfg(test)]
    fn record_ready_observation_for_test(
        &self,
        fence: &ProviderFence,
        process: ReadyProcess,
        started_at: Instant,
    ) {
        let key = FenceKey::from(fence);
        self.ready_processes
            .lock()
            .expect("parakeet runtime shared lock")
            .insert(key.clone(), process);
        self.started_at
            .lock()
            .expect("parakeet runtime shared lock")
            .insert(key, started_at);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn register_ready_process(
        &self,
        fence: &ProviderFence,
        authority: LaunchAuthority,
        process: ReadyProcess,
        started_at: Instant,
    ) {
        let key = FenceKey::from(fence);
        let mut ready_processes = self
            .ready_processes
            .lock()
            .expect("parakeet runtime shared lock");
        let mut ready_children = self
            .ready_children
            .lock()
            .expect("parakeet runtime shared lock");
        let mut started = self
            .started_at
            .lock()
            .expect("parakeet runtime shared lock");
        insert_ready_tuple(
            key,
            process.clone(),
            ReadyChild {
                process_id: process.process_id.clone(),
                authority,
            },
            started_at,
            &mut ready_processes,
            &mut ready_children,
            &mut started,
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn retain_child(&self, process_id: String, authority: LaunchAuthority) {
        self.children
            .lock()
            .expect("parakeet runtime shared lock")
            .insert(process_id, authority);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn take_child(&self, process_id: &str) -> Option<LaunchAuthority> {
        self.children
            .lock()
            .expect("parakeet runtime shared lock")
            .remove(process_id)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn take_ready_child(&self, fence: &ProviderFence) -> Option<(String, LaunchAuthority)> {
        let mut ready_children = self
            .ready_children
            .lock()
            .expect("parakeet runtime shared lock");
        let ready_child = ready_children.remove(&FenceKey::from(fence))?;
        Some((ready_child.process_id, ready_child.authority))
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn retain_ready_child(
        &self,
        fence: &ProviderFence,
        process_id: String,
        authority: LaunchAuthority,
    ) {
        self.ready_children
            .lock()
            .expect("parakeet runtime shared lock")
            .insert(
                FenceKey::from(fence),
                ReadyChild {
                    process_id,
                    authority,
                },
            );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn remove_ready_process(&self, fence: &ProviderFence) {
        let key = FenceKey::from(fence);
        let mut ready_processes = self
            .ready_processes
            .lock()
            .expect("parakeet runtime shared lock");
        let mut ready_children = self
            .ready_children
            .lock()
            .expect("parakeet runtime shared lock");
        let mut started = self
            .started_at
            .lock()
            .expect("parakeet runtime shared lock");
        remove_ready_tuple(
            &key,
            &mut ready_processes,
            &mut ready_children,
            &mut started,
        );
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn remove_ready_process(&self, fence: &ProviderFence) {
        let key = FenceKey::from(fence);
        self.ready_processes
            .lock()
            .expect("parakeet runtime shared lock")
            .remove(&key);
        self.started_at
            .lock()
            .expect("parakeet runtime shared lock")
            .remove(&key);
    }

    pub fn observe_current_process(
        &self,
        processes: &[ManagedProcess],
        now: Instant,
    ) -> ProcessObservation {
        let Ok(ready_processes) = self.ready_processes.lock() else {
            return ProcessObservation::Indeterminate;
        };
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let Ok(mut ready_children) = self.ready_children.lock() else {
                return ProcessObservation::Indeterminate;
            };
            let Ok(started) = self.started_at.lock() else {
                return ProcessObservation::Indeterminate;
            };
            let Ok(children) = self.children.lock() else {
                return ProcessObservation::Indeterminate;
            };
            let ready_child_identities = ready_children
                .iter()
                .map(|(key, child)| {
                    (
                        key.clone(),
                        ReadyChildIdentity {
                            process_id: child.process_id.clone(),
                            pid: child.authority.pid(),
                        },
                    )
                })
                .collect();
            let unfenced_child_ids = children.keys().cloned().collect();
            match resolve_current_process(
                processes,
                &ready_processes,
                &started,
                &ready_child_identities,
                &unfenced_child_ids,
            ) {
                CurrentProcessResolution::Coherent {
                    fence,
                    ready,
                    started_at,
                } => {
                    let Some(child) = ready_children.get_mut(&fence) else {
                        return ProcessObservation::Indeterminate;
                    };
                    classify_process_observation(
                        1,
                        false,
                        Some(ProcessObservationTuple {
                            reference: ready.process_id,
                            pid: ready.pid,
                            started_at,
                            poll: child.authority.poll(),
                        }),
                        now,
                    )
                }
                CurrentProcessResolution::Absent => ProcessObservation::ConfirmedAbsent,
                CurrentProcessResolution::Ambiguous => ProcessObservation::Indeterminate,
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let Ok(started) = self.started_at.lock() else {
                return ProcessObservation::Indeterminate;
            };
            let no_ready_children = BTreeMap::new();
            let no_children = BTreeSet::new();
            match resolve_current_process(
                processes,
                &ready_processes,
                &started,
                &no_ready_children,
                &no_children,
            ) {
                CurrentProcessResolution::Absent => ProcessObservation::ConfirmedAbsent,
                CurrentProcessResolution::Coherent { .. } | CurrentProcessResolution::Ambiguous => {
                    ProcessObservation::Indeterminate
                }
            }
        }
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
            // PR_SET_PDEATHSIG tracks the creating *thread*, Linux-only.
            // Stay alive while the child is live so exiting this worker does
            // not SIGKILL it; stop polling once terminate() (or exit) reaps it.
            #[cfg(target_os = "linux")]
            let hold_pid = outcome.managed.as_ref().map(|managed| managed.pid);
            #[cfg(not(target_os = "linux"))]
            let _hold_pid: Option<u32> = None;
            shared.record_launch_result(&fence, outcome);
            #[cfg(target_os = "linux")]
            if let Some(pid) = hold_pid {
                crate::process::hold_while_instance_live(pid);
            }
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
    let mut authority = match crate::process::launch(
        Disposition::IndependentLongLived,
        || spawn_parakeet(&cmd, &launch.env_updates),
        Box::new(|child, timeout| {
            crate::process::terminate(child, timeout)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(std::io::Error::other(error)))
        }),
    ) {
        Ok(authority) => authority,
        Err(_) => return launch_failed(),
    };
    let started_at = Instant::now();
    let process_id = format!("parakeet:{}", authority.pid());
    let pid = authority.pid();
    let deadline = std::time::Instant::now() + warmup_timeout;
    loop {
        if let Ok(Some(_)) = authority.poll() {
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Exited,
                reason_code: ReasonCode::known("process-exited"),
                managed: None,
            };
        }
        if probe_health(port) {
            let managed = ManagedProcess {
                id: process_id.clone(),
                pid,
                name: PARAKEET_SERVER_PROCESS_NAME.into(),
                running: true,
                fence: Some(fence.clone()),
            };
            shared.register_ready_process(
                fence,
                authority,
                ReadyProcess {
                    process_id,
                    process_name: PARAKEET_SERVER_PROCESS_NAME.into(),
                    pid,
                    port,
                },
                started_at,
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
                pid,
                name: PARAKEET_SERVER_PROCESS_NAME.into(),
                running: true,
                fence: None,
            };
            shared.retain_child(process_id, authority);
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
    let mut command = Command::new(program);
    command.args(arguments).envs(env_updates);
    apply_parent_death_kill(&mut command);
    command.spawn()
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
        let fence = request.managed.fence.as_ref();
        let taken = match fence {
            Some(fence) => shared.take_ready_child(fence),
            None => shared
                .take_child(&request.managed.id)
                .map(|authority| (request.managed.id.clone(), authority)),
        };
        let Some((process_id, mut authority)) = taken else {
            if let Some(fence) = fence {
                shared.remove_ready_process(fence);
            }
            return ProviderStopCleanupOutcome {
                status: StopCleanupStatus::Stopped,
                reason_code,
                managed: None,
            };
        };
        match authority.terminate(termination_timeout) {
            Ok(()) => {
                if let Some(fence) = fence {
                    shared.remove_ready_process(fence);
                }
                ProviderStopCleanupOutcome {
                    status: StopCleanupStatus::Stopped,
                    reason_code,
                    managed: None,
                }
            }
            Err(_) => {
                if let Some(fence) = fence {
                    shared.retain_ready_child(fence, process_id, authority);
                } else {
                    shared.retain_child(process_id, authority);
                }
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
        if let Some(request) = request
            && let Some(fence) = request.managed.fence.as_ref()
        {
            shared.remove_ready_process(fence);
        }
        let _ = termination_timeout;
        ProviderStopCleanupOutcome {
            status: StopCleanupStatus::Stopped,
            reason_code,
            managed: None,
        }
    }
}

// Stop against an already-gone ready process must drop the leftover
// observation. Double-stop of a live managed child lives in tests/parakeet_stop.rs.
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use crate::process::ProcessObservation;
    use crate::provider_runtime::model::{ProviderStopCleanupRequest, RuntimePhase};

    #[test]
    fn already_gone_ready_cleanup_removes_parakeet_observation_residue() {
        let shared = ParakeetRuntimeShared::default();
        let fence = ProviderFence {
            incarnation: "incarnation".to_owned(),
            generation: 2,
            fingerprint: Some("fingerprint".to_owned()),
            attempt: 1,
        };
        shared.record_ready_observation_for_test(
            &fence,
            ReadyProcess {
                process_id: "parakeet:42".to_owned(),
                process_name: PARAKEET_SERVER_PROCESS_NAME.to_owned(),
                pid: 42,
                port: 5016,
            },
            Instant::now(),
        );
        let request = ProviderStopCleanupRequest {
            managed: ManagedProcess {
                id: "parakeet:42".to_owned(),
                pid: 42,
                name: PARAKEET_SERVER_PROCESS_NAME.to_owned(),
                running: true,
                fence: Some(fence),
            },
            reason_code: ReasonCode::known("stale-result-ignored"),
            target_phase: RuntimePhase::Stopped,
            target_reason_code: None,
            admission_exclusive: false,
            orphaned_start_outcome: true,
        };

        assert_eq!(
            shared.observe_current_process(&[], Instant::now()),
            ProcessObservation::Indeterminate,
        );
        assert_eq!(
            stop_parakeet(&shared, Some(&request), false, Duration::ZERO).status,
            StopCleanupStatus::Stopped,
        );
        assert_eq!(
            shared.observe_current_process(&[], Instant::now()),
            ProcessObservation::ConfirmedAbsent,
        );
    }
}
