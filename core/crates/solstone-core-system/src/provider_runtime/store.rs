// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable Local-provider runtime state, retry tokens, and owned port publication.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, JsonWriteOptions, LockOptions, hold_lock, write_json, write_text,
};

use crate::process::{ProcessObservation, ProcessObservationTuple, classify_process_observation};

use super::launch::LocalLaunchConfig;
use super::model::{
    ManagedProcess, ProviderFence, ProviderLaunchOutcome, ProviderName, ProviderProbeOutcome,
    ProviderRuntimeState, ProviderStopCleanupOutcome, ProviderTruthObservation, ReasonCode,
    RuntimePhase,
};
use super::seams::{RetryToken, RuntimeStore, RuntimeStoreError};

const FILE_MODE: u32 = 0o600;
const SCHEMA_VERSION: u64 = 1;
static NEXT_RETRY_TOKEN: AtomicU64 = AtomicU64::new(1);

pub trait RuntimeClock: Send + Sync {
    fn now_utc_rfc3339(&self) -> String;
    fn monotonic_seconds(&self) -> f64;
    fn sleep(&self, duration: Duration);
}

#[derive(Debug)]
pub struct SystemRuntimeClock {
    started: Instant,
}

impl Default for SystemRuntimeClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl RuntimeClock for SystemRuntimeClock {
    fn now_utc_rfc3339(&self) -> String {
        Utc::now().to_rfc3339()
    }

    fn monotonic_seconds(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FenceKey {
    incarnation: String,
    generation: u64,
    fingerprint: Option<String>,
    attempt: u32,
}

impl From<&ProviderFence> for FenceKey {
    fn from(fence: &ProviderFence) -> Self {
        Self {
            incarnation: fence.incarnation.clone(),
            generation: fence.generation,
            fingerprint: fence.fingerprint.clone(),
            attempt: fence.attempt,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyProcess {
    pub process_id: String,
    pub process_name: String,
    pub pid: u32,
    pub port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReadyChildIdentity {
    pub process_id: String,
    pub pid: u32,
}

#[derive(Debug)]
pub(crate) struct ReadyChild {
    pub process_id: String,
    pub child: Child,
}

pub(crate) fn insert_ready_tuple<T>(
    key: FenceKey,
    process: ReadyProcess,
    child: T,
    start: Instant,
    ready_processes: &mut BTreeMap<FenceKey, ReadyProcess>,
    ready_children: &mut BTreeMap<FenceKey, T>,
    started_at: &mut BTreeMap<FenceKey, Instant>,
) {
    ready_children.insert(key.clone(), child);
    ready_processes.insert(key.clone(), process);
    started_at.insert(key, start);
}

pub(crate) fn remove_ready_tuple<T>(
    key: &FenceKey,
    ready_processes: &mut BTreeMap<FenceKey, ReadyProcess>,
    ready_children: &mut BTreeMap<FenceKey, T>,
    started_at: &mut BTreeMap<FenceKey, Instant>,
) {
    ready_processes.remove(key);
    ready_children.remove(key);
    started_at.remove(key);
}

/// Fence-keyed result of resolving the one process a status sample may inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CurrentProcessResolution {
    Coherent {
        fence: FenceKey,
        ready: ReadyProcess,
        started_at: Instant,
    },
    Absent,
    Ambiguous,
}

/// Resolve a current provider process by its accepted launch fence, never by PID.
pub(crate) fn resolve_current_process(
    processes: &[ManagedProcess],
    ready_processes: &BTreeMap<FenceKey, ReadyProcess>,
    started_at: &BTreeMap<FenceKey, Instant>,
    ready_children: &BTreeMap<FenceKey, ReadyChildIdentity>,
    unfenced_child_ids: &BTreeSet<String>,
) -> CurrentProcessResolution {
    let current = processes
        .iter()
        .filter(|process| process.running)
        .collect::<Vec<_>>();
    if current.is_empty() {
        return if ready_processes.is_empty()
            && started_at.is_empty()
            && ready_children.is_empty()
            && unfenced_child_ids.is_empty()
        {
            CurrentProcessResolution::Absent
        } else {
            CurrentProcessResolution::Ambiguous
        };
    }
    if current.len() != 1
        || ready_processes.len() != 1
        || started_at.len() != 1
        || ready_children.len() != 1
        || !unfenced_child_ids.is_empty()
    {
        return CurrentProcessResolution::Ambiguous;
    }
    let Some(fence) = current[0].fence.as_ref() else {
        return CurrentProcessResolution::Ambiguous;
    };
    let key = FenceKey::from(fence);
    let (Some(ready), Some(started_at), Some(child)) = (
        ready_processes.get(&key),
        started_at.get(&key),
        ready_children.get(&key),
    ) else {
        return CurrentProcessResolution::Ambiguous;
    };
    if current[0].id != ready.process_id
        || current[0].name != ready.process_name
        || child.process_id != ready.process_id
        || child.pid != ready.pid
    {
        return CurrentProcessResolution::Ambiguous;
    }
    CurrentProcessResolution::Coherent {
        fence: key,
        ready: ready.clone(),
        started_at: *started_at,
    }
}

#[derive(Debug, Default)]
pub struct LocalRuntimeShared {
    ready_processes: Mutex<BTreeMap<FenceKey, ReadyProcess>>,
    started_at: Mutex<BTreeMap<FenceKey, Instant>>,
    launch_requests: Mutex<BTreeMap<Option<String>, LocalLaunchConfig>>,
    results: Mutex<LocalRuntimeResults>,
    result_available: Condvar,
    ready_children: Mutex<BTreeMap<FenceKey, ReadyChild>>,
    children: Mutex<BTreeMap<String, Child>>,
}

#[derive(Debug, Default)]
struct LocalRuntimeResults {
    truth: BTreeMap<FenceKey, ProviderTruthObservation>,
    launch: BTreeMap<FenceKey, ProviderLaunchOutcome>,
    stop_cleanup: BTreeMap<FenceKey, ProviderStopCleanupOutcome>,
    probe: BTreeMap<FenceKey, ProviderProbeOutcome>,
}

impl LocalRuntimeShared {
    pub fn record_launch_request(
        &self,
        desired_fingerprint: Option<String>,
        config: LocalLaunchConfig,
    ) {
        self.launch_requests
            .lock()
            .expect("local runtime shared lock")
            .insert(desired_fingerprint, config);
    }

    pub fn launch_request_for(
        &self,
        desired_fingerprint: &Option<String>,
    ) -> Option<LocalLaunchConfig> {
        self.launch_requests
            .lock()
            .expect("local runtime shared lock")
            .get(desired_fingerprint)
            .cloned()
    }

    pub fn record_truth_result(&self, fence: &ProviderFence, result: ProviderTruthObservation) {
        self.results
            .lock()
            .expect("local runtime shared lock")
            .truth
            .insert(FenceKey::from(fence), result);
        self.result_available.notify_all();
    }

    pub fn take_truth_result(&self, fence: &ProviderFence) -> Option<ProviderTruthObservation> {
        self.results
            .lock()
            .expect("local runtime shared lock")
            .truth
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_truth_result(&self, fence: &ProviderFence) -> ProviderTruthObservation {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("local runtime shared lock");
        loop {
            if let Some(result) = results.truth.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("local runtime shared lock");
        }
    }

    pub fn record_launch_result(&self, fence: &ProviderFence, result: ProviderLaunchOutcome) {
        self.results
            .lock()
            .expect("local runtime shared lock")
            .launch
            .insert(FenceKey::from(fence), result);
        self.result_available.notify_all();
    }

    pub fn take_launch_result(&self, fence: &ProviderFence) -> Option<ProviderLaunchOutcome> {
        self.results
            .lock()
            .expect("local runtime shared lock")
            .launch
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_launch_result(&self, fence: &ProviderFence) -> ProviderLaunchOutcome {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("local runtime shared lock");
        loop {
            if let Some(result) = results.launch.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("local runtime shared lock");
        }
    }

    pub fn record_stop_cleanup_result(
        &self,
        fence: &ProviderFence,
        result: ProviderStopCleanupOutcome,
    ) {
        self.results
            .lock()
            .expect("local runtime shared lock")
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
            .expect("local runtime shared lock")
            .stop_cleanup
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_stop_cleanup_result(
        &self,
        fence: &ProviderFence,
    ) -> ProviderStopCleanupOutcome {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("local runtime shared lock");
        loop {
            if let Some(result) = results.stop_cleanup.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("local runtime shared lock");
        }
    }

    pub fn record_probe_result(&self, fence: &ProviderFence, result: ProviderProbeOutcome) {
        self.results
            .lock()
            .expect("local runtime shared lock")
            .probe
            .insert(FenceKey::from(fence), result);
        self.result_available.notify_all();
    }

    pub fn take_probe_result(&self, fence: &ProviderFence) -> Option<ProviderProbeOutcome> {
        self.results
            .lock()
            .expect("local runtime shared lock")
            .probe
            .remove(&FenceKey::from(fence))
    }

    pub fn wait_for_probe_result(&self, fence: &ProviderFence) -> ProviderProbeOutcome {
        let key = FenceKey::from(fence);
        let mut results = self.results.lock().expect("local runtime shared lock");
        loop {
            if let Some(result) = results.probe.remove(&key) {
                return result;
            }
            results = self
                .result_available
                .wait(results)
                .expect("local runtime shared lock");
        }
    }

    pub fn record_ready_process(&self, fence: &ProviderFence, process: ReadyProcess) {
        self.ready_processes
            .lock()
            .expect("local runtime shared lock")
            .insert(FenceKey::from(fence), process);
    }

    #[cfg(test)]
    pub(crate) fn record_ready_observation_for_test(
        &self,
        fence: &ProviderFence,
        process: ReadyProcess,
        started_at: Instant,
    ) {
        let key = FenceKey::from(fence);
        self.ready_processes
            .lock()
            .expect("local runtime shared lock")
            .insert(key.clone(), process);
        self.started_at
            .lock()
            .expect("local runtime shared lock")
            .insert(key, started_at);
    }

    /// Atomically publish the in-memory tuple a status read needs for a ready child.
    pub(crate) fn register_ready_process(
        &self,
        fence: &ProviderFence,
        child: Child,
        process: ReadyProcess,
        started_at: Instant,
    ) {
        let key = FenceKey::from(fence);
        let mut ready_processes = self
            .ready_processes
            .lock()
            .expect("local runtime shared lock");
        let mut ready_children = self
            .ready_children
            .lock()
            .expect("local runtime shared lock");
        let mut started = self.started_at.lock().expect("local runtime shared lock");
        insert_ready_tuple(
            key,
            process.clone(),
            ReadyChild {
                process_id: process.process_id.clone(),
                child,
            },
            started_at,
            &mut ready_processes,
            &mut ready_children,
            &mut started,
        );
    }

    pub(crate) fn retain_child(&self, process_id: String, child: Child) {
        self.children
            .lock()
            .expect("local runtime shared lock")
            .insert(process_id, child);
    }

    pub(crate) fn take_child(&self, process_id: &str) -> Option<Child> {
        self.children
            .lock()
            .expect("local runtime shared lock")
            .remove(process_id)
    }

    pub(crate) fn take_ready_child(&self, fence: &ProviderFence) -> Option<(String, Child)> {
        let mut ready_children = self
            .ready_children
            .lock()
            .expect("local runtime shared lock");
        let ready_child = ready_children.remove(&FenceKey::from(fence))?;
        Some((ready_child.process_id, ready_child.child))
    }

    pub(crate) fn retain_ready_child(
        &self,
        fence: &ProviderFence,
        process_id: String,
        child: Child,
    ) {
        self.ready_children
            .lock()
            .expect("local runtime shared lock")
            .insert(FenceKey::from(fence), ReadyChild { process_id, child });
    }

    pub(crate) fn remove_ready_process(&self, fence: &ProviderFence) {
        let key = FenceKey::from(fence);
        let mut ready_processes = self
            .ready_processes
            .lock()
            .expect("local runtime shared lock");
        let mut ready_children = self
            .ready_children
            .lock()
            .expect("local runtime shared lock");
        let mut started = self.started_at.lock().expect("local runtime shared lock");
        remove_ready_tuple(
            &key,
            &mut ready_processes,
            &mut ready_children,
            &mut started,
        );
    }

    pub fn observe_current_process(
        &self,
        processes: &[ManagedProcess],
        now: Instant,
    ) -> ProcessObservation {
        let Ok(ready_processes) = self.ready_processes.lock() else {
            return ProcessObservation::Indeterminate;
        };
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
                        pid: child.child.id(),
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
                        poll: child.child.try_wait(),
                    }),
                    now,
                )
            }
            CurrentProcessResolution::Absent => ProcessObservation::ConfirmedAbsent,
            CurrentProcessResolution::Ambiguous => ProcessObservation::Indeterminate,
        }
    }
}

/// What `FileRuntimeStore` needs from a provider's own in-memory
/// seam-to-reconcile bus, so one store implementation (the durable record)
/// can serve any provider without needing that provider's own bus to be the
/// same concrete type -- Local's `LocalRuntimeShared` carries a
/// `LocalLaunchConfig` staging map a Parakeet-equivalent bus has no use for,
/// and does not need to share a type with, to satisfy this.
pub trait ReadyProcessLookup: Send + Sync {
    fn ready_process_for_fence(&self, fence: &ProviderFence) -> Option<ReadyProcess>;
    fn ready_process_for_id(&self, process_id: &str) -> Option<ReadyProcess>;
}

impl ReadyProcessLookup for LocalRuntimeShared {
    fn ready_process_for_fence(&self, fence: &ProviderFence) -> Option<ReadyProcess> {
        self.ready_processes
            .lock()
            .expect("local runtime shared lock")
            .get(&FenceKey::from(fence))
            .cloned()
    }

    fn ready_process_for_id(&self, process_id: &str) -> Option<ReadyProcess> {
        self.ready_processes
            .lock()
            .expect("local runtime shared lock")
            .values()
            .find(|process| process.process_id == process_id)
            .cloned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalReadySideEffect {
    RefreshBrain { expected_fingerprint_sha256: String },
}

#[derive(Debug, Clone)]
struct HealthRecord {
    revision: u64,
    incarnation: Option<String>,
    generation: u64,
    attempt: u32,
    process: Option<Map<String, Value>>,
    detail: Value,
}

#[derive(Debug, Clone)]
struct RetryRecord {
    revision: u64,
    token_id: Option<String>,
    desired_fingerprint: Option<String>,
    requested_at: Option<String>,
    reason_code: Option<ReasonCode>,
    owner: Option<Map<String, Value>>,
}

#[derive(Debug, Clone)]
struct RetryObservation {
    revision: u64,
    desired_fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
struct PortOwner {
    fence: ProviderFence,
    process: ReadyProcess,
}

/// RuntimeStore implementation shared by every provider: the durable record
/// -- `health/providers/runtime/<provider>.json`, `<provider>.retry-token.json`,
/// and the owned-port file -- is one write path, constructed per provider
/// rather than duplicated per provider. `shared` is a trait object so each
/// provider's own in-memory seam-to-reconcile bus (Local's carries a
/// `LocalLaunchConfig` staging map this store never touches) can stay its
/// own concrete type.
pub struct FileRuntimeStore {
    journal_path: PathBuf,
    provider: ProviderName,
    shared: Arc<dyn ReadyProcessLookup>,
    clock: Arc<dyn RuntimeClock>,
    observed_health_revision: Option<u64>,
    observed_retry_tokens: BTreeMap<String, RetryObservation>,
    last_fence: Option<ProviderFence>,
    cleanup_owners: BTreeMap<FenceKey, PortOwner>,
    ready_effect_fences: BTreeSet<FenceKey>,
    ready_side_effects: Vec<LocalReadySideEffect>,
}

impl FileRuntimeStore {
    pub fn new(
        journal_path: impl Into<PathBuf>,
        provider: ProviderName,
        shared: Arc<dyn ReadyProcessLookup>,
        clock: Arc<dyn RuntimeClock>,
    ) -> Self {
        Self {
            journal_path: journal_path.into(),
            provider,
            shared,
            clock,
            observed_health_revision: None,
            observed_retry_tokens: BTreeMap::new(),
            last_fence: None,
            cleanup_owners: BTreeMap::new(),
            ready_effect_fences: BTreeSet::new(),
            ready_side_effects: Vec::new(),
        }
    }

    /// `RefreshBrain` is meaningful only for Local's own bundled-LLM
    /// backend; a store constructed for any other provider never produces
    /// one even if that provider also reaches `Ready`.
    pub fn take_ready_side_effects(&mut self) -> Vec<LocalReadySideEffect> {
        std::mem::take(&mut self.ready_side_effects)
    }

    /// Durably request a fresh retry observation for this store's provider.
    pub fn request_retry_token(
        &mut self,
        desired_fingerprint: Option<String>,
        reason_code: ReasonCode,
        owner: Map<String, Value>,
    ) -> Result<RetryToken, RuntimeStoreError> {
        let _lock = self.lock_operation()?;
        let current = read_retry(&self.retry_path(), self.provider)?;
        let revision = current.revision + 1;
        let token_id = format!(
            "{}-retry-{}",
            self.provider.as_str(),
            NEXT_RETRY_TOKEN.fetch_add(1, Ordering::Relaxed)
        );
        let record = RetryRecord {
            revision,
            token_id: Some(token_id.clone()),
            desired_fingerprint: desired_fingerprint.clone(),
            requested_at: Some(self.clock.now_utc_rfc3339()),
            reason_code: Some(reason_code.clone()),
            owner: Some(owner),
        };
        self.write_retry(&record)?;
        Ok(RetryToken {
            revision,
            token_id,
            desired_fingerprint,
            reason_code,
        })
    }

    fn runtime_directory(&self) -> PathBuf {
        self.journal_path
            .join("health")
            .join("providers")
            .join("runtime")
    }

    fn health_path(&self) -> PathBuf {
        health_record_path(&self.journal_path, self.provider)
    }

    fn retry_path(&self) -> PathBuf {
        self.runtime_directory()
            .join(format!("{}.retry-token.json", self.provider.as_str()))
    }

    fn operation_path(&self) -> PathBuf {
        self.runtime_directory()
            .join(format!("{}.operation", self.provider.as_str()))
    }

    fn port_path(&self) -> PathBuf {
        self.journal_path
            .join("health")
            .join(format!("{}.port", port_service_name(self.provider)))
    }

    /// This store instance only accepts records for the provider it was
    /// constructed for -- one `FileRuntimeStore` per provider, never one
    /// instance silently serving both.
    fn ensure_provider(&self, provider: ProviderName) -> Result<(), RuntimeStoreError> {
        if provider == self.provider {
            Ok(())
        } else {
            Err(RuntimeStoreError::Unavailable)
        }
    }

    fn lock_operation(&self) -> Result<solstone_core_journal_io::FileLock, RuntimeStoreError> {
        hold_lock(
            self.operation_path(),
            LockOptions {
                mode: Some(FILE_MODE),
                ..LockOptions::default()
            },
        )
        .map_err(|_| RuntimeStoreError::Unavailable)
    }

    fn capture_owner_fence(&mut self, state: &ProviderRuntimeState) -> Option<ProviderFence> {
        let fence = state
            .stop_cleanup
            .as_ref()
            .map(|in_flight| in_flight.fence.clone())
            .or_else(|| {
                state
                    .start
                    .as_ref()
                    .map(|in_flight| in_flight.fence.clone())
            })
            .or_else(|| {
                state
                    .truth
                    .as_ref()
                    .map(|in_flight| in_flight.fence.clone())
            })
            .or_else(|| {
                state
                    .probe
                    .as_ref()
                    .map(|in_flight| in_flight.fence.clone())
            });
        if let Some(fence) = fence {
            if let Some(request) = state.pending_stop_request.as_ref()
                && let Some(process) = self.shared.ready_process_for_id(&request.managed.id)
            {
                self.cleanup_owners.insert(
                    FenceKey::from(&fence),
                    PortOwner {
                        fence: fence.clone(),
                        process,
                    },
                );
            }
            self.last_fence = Some(fence.clone());
            return Some(fence);
        }
        self.last_fence.clone()
    }

    fn ready_process_for(&self, fence: Option<&ProviderFence>) -> Option<ReadyProcess> {
        fence.and_then(|fence| self.shared.ready_process_for_fence(fence))
    }

    fn cleanup_owner_for(&self, fence: Option<&ProviderFence>) -> Option<PortOwner> {
        fence.and_then(|fence| self.cleanup_owners.get(&FenceKey::from(fence)).cloned())
    }

    fn clear_port_if_owned(
        &self,
        current: &HealthRecord,
        owner: &PortOwner,
    ) -> Result<(), RuntimeStoreError> {
        if current.incarnation.as_deref() != Some(owner.fence.incarnation.as_str())
            || current.generation != owner.fence.generation
            || current.attempt != owner.fence.attempt
            || current
                .process
                .as_ref()
                .and_then(|process| process.get("port"))
                .and_then(Value::as_u64)
                != Some(u64::from(owner.process.port))
        {
            return Ok(());
        }
        let port_path = self.port_path();
        let text = match fs::read_to_string(&port_path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(RuntimeStoreError::Unavailable),
        };
        if text.trim().parse::<u16>().ok() != Some(owner.process.port) {
            return Ok(());
        }
        match fs::remove_file(port_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(RuntimeStoreError::Unavailable),
        }
    }

    fn write_health(
        &self,
        state: &ProviderRuntimeState,
        revision: u64,
        fence: Option<&ProviderFence>,
        process: Option<ReadyProcess>,
    ) -> Result<(), RuntimeStoreError> {
        let process = process.map(|process| {
            json!({
                "name": process.process_name,
                "pid": process.pid,
                "ref": process.process_id,
                "port": process.port,
            })
        });
        let record = json!({
            "schema_version": SCHEMA_VERSION,
            "provider": self.provider.as_str(),
            "revision": revision,
            "phase": state.latest_phase.as_str(),
            "reason_code": state.latest_reason_code.as_ref().map(ReasonCode::as_str),
            "detail": state.latest_detail.clone().unwrap_or_else(|| json!({})),
            "desired_fingerprint_sha256": state.desired_fingerprint,
            "incarnation": fence.map(|value| value.incarnation.as_str()),
            "generation": state.generation,
            "attempt": fence.map_or(state.retry.attempt_count, |value| value.attempt),
            "process": process,
            "updated_at": self.clock.now_utc_rfc3339(),
            "display_deadline_at": Value::Null,
            "owner": Value::Null,
        });
        write_json(
            self.health_path(),
            &record,
            JsonWriteOptions {
                mode: Some(FILE_MODE),
                indent: Some(2),
                sort_keys: true,
            },
        )
        .map_err(|_| RuntimeStoreError::Unavailable)
    }

    fn write_retry(&self, record: &RetryRecord) -> Result<(), RuntimeStoreError> {
        let value = json!({
            "schema_version": SCHEMA_VERSION,
            "provider": self.provider.as_str(),
            "revision": record.revision,
            "token_id": record.token_id,
            "desired_fingerprint_sha256": record.desired_fingerprint,
            "requested_at": record.requested_at,
            "reason_code": record.reason_code.as_ref().map(ReasonCode::as_str),
            "owner": record.owner,
        });
        write_json(
            self.retry_path(),
            &value,
            JsonWriteOptions {
                mode: Some(FILE_MODE),
                indent: Some(2),
                sort_keys: true,
            },
        )
        .map_err(|_| RuntimeStoreError::Unavailable)
    }
}

impl RuntimeStore for FileRuntimeStore {
    fn read_retry_token(
        &mut self,
        provider: ProviderName,
    ) -> Result<Option<RetryToken>, RuntimeStoreError> {
        self.ensure_provider(provider)?;
        let _lock = self.lock_operation()?;
        let record = read_retry(&self.retry_path(), self.provider)?;
        if let Some(token_id) = record.token_id {
            let reason_code = record.reason_code.ok_or(RuntimeStoreError::Corrupt)?;
            self.observed_retry_tokens.insert(
                token_id.clone(),
                RetryObservation {
                    revision: record.revision,
                    desired_fingerprint: record.desired_fingerprint.clone(),
                },
            );
            return Ok(Some(RetryToken {
                revision: record.revision,
                token_id,
                desired_fingerprint: record.desired_fingerprint,
                reason_code,
            }));
        }
        Ok(None)
    }

    fn consume_retry_token(
        &mut self,
        provider: ProviderName,
        token_id: &str,
    ) -> Result<(), RuntimeStoreError> {
        self.ensure_provider(provider)?;
        let expected = self
            .observed_retry_tokens
            .get(token_id)
            .cloned()
            .ok_or(RuntimeStoreError::Conflict)?;
        let _lock = self.lock_operation()?;
        let current = read_retry(&self.retry_path(), self.provider)?;
        if current.revision != expected.revision
            || current.token_id.as_deref() != Some(token_id)
            || current.desired_fingerprint != expected.desired_fingerprint
        {
            return Err(RuntimeStoreError::Conflict);
        }
        let cleared = RetryRecord {
            revision: current.revision + 1,
            token_id: None,
            desired_fingerprint: None,
            requested_at: None,
            reason_code: None,
            owner: None,
        };
        self.write_retry(&cleared)?;
        self.observed_retry_tokens.remove(token_id);
        Ok(())
    }

    fn publish_state(&mut self, state: &ProviderRuntimeState) -> Result<(), RuntimeStoreError> {
        self.ensure_provider(state.provider)?;
        let _lock = self.lock_operation()?;
        let current = read_health(&self.health_path(), self.provider)?;
        if self
            .observed_health_revision
            .is_some_and(|revision| revision != current.revision)
        {
            return Err(RuntimeStoreError::Conflict);
        }
        let fence = self.capture_owner_fence(state);
        let ready_process = self.ready_process_for(fence.as_ref());
        let cleanup_owner = self.cleanup_owner_for(fence.as_ref());
        if state.latest_phase == RuntimePhase::Stopped
            && let Some(owner) = cleanup_owner.as_ref()
        {
            self.clear_port_if_owned(&current, owner)?;
        }
        let process = match state.latest_phase {
            RuntimePhase::Ready | RuntimePhase::ReadyProofUnavailable => ready_process.clone(),
            RuntimePhase::StopDeferred | RuntimePhase::Stopping | RuntimePhase::CleanupFailed => {
                cleanup_owner.map(|owner| owner.process)
            }
            _ => None,
        };
        let revision = current.revision + 1;
        self.write_health(state, revision, fence.as_ref(), process)?;
        self.observed_health_revision = Some(revision);
        if state.latest_phase == RuntimePhase::Ready
            && let (Some(fence), Some(process)) = (fence.as_ref(), ready_process)
        {
            write_text(
                self.port_path(),
                &process.port.to_string(),
                AtomicWriteOptions::default(),
            )
            .map_err(|_| RuntimeStoreError::Unavailable)?;
            let key = FenceKey::from(fence);
            if self.provider == ProviderName::Local
                && self.ready_effect_fences.insert(key)
                && let Some(fingerprint) = state.desired_fingerprint.clone()
            {
                self.ready_side_effects
                    .push(LocalReadySideEffect::RefreshBrain {
                        expected_fingerprint_sha256: fingerprint,
                    });
            }
        }
        Ok(())
    }
}

/// The owned-port file's service name, which is not always the provider
/// name -- Parakeet's runtime-health/retry-token records use "parakeet"
/// (matching `ProviderName::Parakeet::as_str()`), but its port file is
/// `health/parakeet-cpp.port` (matching Python's `_SERVICE_NAME =
/// "parakeet-cpp"` in `parakeet_server.py`, itself one instance of the
/// generic `write_service_port(service, port)` -> `health/{service}.port`
/// pattern). Local's happens to coincide with its provider name.
fn port_service_name(provider: ProviderName) -> &'static str {
    match provider {
        ProviderName::Local => "local",
        ProviderName::Parakeet => "parakeet-cpp",
    }
}

fn health_record_path(journal_path: &Path, provider: ProviderName) -> PathBuf {
    journal_path
        .join("health")
        .join("providers")
        .join("runtime")
        .join(format!("{}.json", provider.as_str()))
}

/// Reads the durable record's `detail` payload for a provider, propagating
/// `Corrupt`/`Unavailable` on a malformed or unreadable record rather than
/// defaulting to an empty one -- an unreadable record fails closed, exactly
/// like Python's `read_runtime_health`. A genuinely absent record (no file
/// yet) is not an error: it is a synthetic, empty detail, matching Python's
/// "absent records are synthetic and read-only" contract. Standalone (not a
/// `FileRuntimeStore` method) because callers such as the admission latch
/// read the record before, or without ever holding, a store for this
/// provider.
pub fn read_current_detail(
    journal_path: &Path,
    provider: ProviderName,
) -> Result<Value, RuntimeStoreError> {
    Ok(read_health(&health_record_path(journal_path, provider), provider)?.detail)
}

fn read_health(path: &Path, provider: ProviderName) -> Result<HealthRecord, RuntimeStoreError> {
    let value = read_value(path)?;
    let Some(value) = value else {
        return Ok(HealthRecord {
            revision: 0,
            incarnation: None,
            generation: 0,
            attempt: 0,
            process: None,
            detail: json!({}),
        });
    };
    let object = value.as_object().ok_or(RuntimeStoreError::Corrupt)?;
    validate_schema_and_provider(object, provider)?;
    let _phase = object
        .get("phase")
        .and_then(Value::as_str)
        .and_then(runtime_phase_from_wire)
        .ok_or(RuntimeStoreError::Corrupt)?;
    validate_optional_reason(object.get("reason_code"))?;
    if !object.get("detail").is_none_or(Value::is_object) {
        return Err(RuntimeStoreError::Corrupt);
    }
    optional_object(object.get("owner"))?;
    Ok(HealthRecord {
        revision: nonnegative(object.get("revision"))?,
        incarnation: optional_string(object.get("incarnation"))?,
        generation: nonnegative(object.get("generation"))?,
        attempt: u32::try_from(nonnegative(object.get("attempt"))?)
            .map_err(|_| RuntimeStoreError::Corrupt)?,
        process: optional_object(object.get("process"))?,
        detail: object.get("detail").cloned().unwrap_or_else(|| json!({})),
    })
}

fn read_retry(path: &Path, provider: ProviderName) -> Result<RetryRecord, RuntimeStoreError> {
    let value = read_value(path)?;
    let Some(value) = value else {
        return Ok(RetryRecord {
            revision: 0,
            token_id: None,
            desired_fingerprint: None,
            requested_at: None,
            reason_code: None,
            owner: None,
        });
    };
    let object = value.as_object().ok_or(RuntimeStoreError::Corrupt)?;
    validate_schema_and_provider(object, provider)?;
    let token_id = optional_string(object.get("token_id"))?;
    let desired_fingerprint = optional_string(object.get("desired_fingerprint_sha256"))?;
    let requested_at = optional_string(object.get("requested_at"))?;
    let reason_code = optional_reason(object.get("reason_code"))?;
    let owner = optional_object(object.get("owner"))?;
    if (token_id.is_none()
        && (desired_fingerprint.is_some()
            || requested_at.is_some()
            || reason_code.is_some()
            || owner.is_some()))
        || (token_id.is_some() && (requested_at.is_none() || reason_code.is_none()))
    {
        return Err(RuntimeStoreError::Corrupt);
    }
    Ok(RetryRecord {
        revision: nonnegative(object.get("revision"))?,
        token_id,
        desired_fingerprint,
        requested_at,
        reason_code,
        owner,
    })
}

fn read_value(path: &Path) -> Result<Option<Value>, RuntimeStoreError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(RuntimeStoreError::Unavailable),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| RuntimeStoreError::Corrupt)
}

fn validate_schema_and_provider(
    object: &Map<String, Value>,
    provider: ProviderName,
) -> Result<(), RuntimeStoreError> {
    if object
        .get("schema_version")
        .is_some_and(|value| value.as_u64() != Some(SCHEMA_VERSION))
        || object
            .get("provider")
            .is_some_and(|value| value.as_str() != Some(provider.as_str()))
    {
        return Err(RuntimeStoreError::Corrupt);
    }
    Ok(())
}

fn runtime_phase_from_wire(value: &str) -> Option<RuntimePhase> {
    RuntimePhase::ALL
        .into_iter()
        .find(|phase| phase.as_str() == value)
}

fn validate_optional_reason(value: Option<&Value>) -> Result<(), RuntimeStoreError> {
    optional_reason(value).map(|_| ())
}

fn optional_reason(value: Option<&Value>) -> Result<Option<ReasonCode>, RuntimeStoreError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let reason = value.as_str().ok_or(RuntimeStoreError::Corrupt)?;
    let reason = ReasonCode::from_wire(reason);
    reason
        .is_recognized()
        .then_some(Some(reason))
        .ok_or(RuntimeStoreError::Corrupt)
}

fn optional_string(value: Option<&Value>) -> Result<Option<String>, RuntimeStoreError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RuntimeStoreError::Corrupt),
    }
}

fn optional_object(value: Option<&Value>) -> Result<Option<Map<String, Value>>, RuntimeStoreError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(value)) => Ok(Some(value.clone())),
        Some(_) => Err(RuntimeStoreError::Corrupt),
    }
}

fn nonnegative(value: Option<&Value>) -> Result<u64, RuntimeStoreError> {
    value
        .and_then(Value::as_u64)
        .ok_or(RuntimeStoreError::Corrupt)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::provider_runtime::{InFlight, ManagedProcess, ProviderStopCleanupRequest};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempJournal(PathBuf);

    impl TempJournal {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-local-runtime-store-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("temporary journal");
            Self(path)
        }
    }

    impl Drop for TempJournal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FixedClock;

    impl RuntimeClock for FixedClock {
        fn now_utc_rfc3339(&self) -> String {
            "2026-08-09T12:00:00+00:00".to_owned()
        }

        fn monotonic_seconds(&self) -> f64 {
            0.0
        }

        fn sleep(&self, _: Duration) {}
    }

    fn store(journal: &TempJournal, shared: Arc<LocalRuntimeShared>) -> FileRuntimeStore {
        FileRuntimeStore::new(
            journal.0.clone(),
            ProviderName::Local,
            shared,
            Arc::new(FixedClock),
        )
    }

    fn fence(attempt: u32) -> ProviderFence {
        ProviderFence {
            incarnation: "incarnation".to_owned(),
            generation: 4,
            fingerprint: Some("fingerprint".to_owned()),
            attempt,
        }
    }

    fn state(phase: RuntimePhase) -> ProviderRuntimeState {
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        state.generation = 4;
        state.desired_fingerprint = Some("fingerprint".to_owned());
        state.latest_phase = phase;
        state.latest_reason_code = Some(ReasonCode::known("launch-requested"));
        state
    }

    fn ready_state(start_fence: ProviderFence) -> ProviderRuntimeState {
        let mut state = state(RuntimePhase::Ready);
        state.retry.attempt_count = start_fence.attempt;
        state.start = Some(InFlight {
            fence: start_fence,
            result: None,
        });
        state
    }

    fn ready_process(port: u16) -> ReadyProcess {
        ReadyProcess {
            process_id: "local:42".to_owned(),
            process_name: "local".to_owned(),
            pid: 42,
            port,
        }
    }

    fn current_process(fence: Option<ProviderFence>) -> ManagedProcess {
        ManagedProcess {
            id: "local:42".to_owned(),
            name: "local".to_owned(),
            running: true,
            fence,
        }
    }

    fn coherent_maps(
        fence: &ProviderFence,
        ready: ReadyProcess,
        started_at: Instant,
    ) -> (
        BTreeMap<FenceKey, ReadyProcess>,
        BTreeMap<FenceKey, Instant>,
        BTreeMap<FenceKey, ReadyChildIdentity>,
    ) {
        let key = FenceKey::from(fence);
        let child = ReadyChildIdentity {
            process_id: ready.process_id.clone(),
            pid: ready.pid,
        };
        (
            BTreeMap::from([(key.clone(), ready)]),
            BTreeMap::from([(key.clone(), started_at)]),
            BTreeMap::from([(key, child)]),
        )
    }

    #[test]
    fn current_process_resolution_selects_one_coherent_fence_tuple() {
        let fence = fence(1);
        let ready = ready_process(4312);
        let started_at = Instant::now();
        let (ready_processes, started, ready_children) =
            coherent_maps(&fence, ready.clone(), started_at);
        let key = FenceKey::from(&fence);

        assert_eq!(
            resolve_current_process(
                &[current_process(Some(fence))],
                &ready_processes,
                &started,
                &ready_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Coherent {
                fence: key,
                ready,
                started_at,
            }
        );
    }

    #[test]
    fn current_process_resolution_classifies_absent_and_residual_states() {
        let empty_ready = BTreeMap::new();
        let empty_started = BTreeMap::new();
        let empty_ready_children = BTreeMap::new();
        let empty_children = BTreeSet::new();
        assert_eq!(
            resolve_current_process(
                &[],
                &empty_ready,
                &empty_started,
                &empty_ready_children,
                &empty_children,
            ),
            CurrentProcessResolution::Absent
        );

        let fence = fence(1);
        let (ready_processes, started, ready_children) =
            coherent_maps(&fence, ready_process(4312), Instant::now());
        assert_eq!(
            resolve_current_process(
                &[],
                &ready_processes,
                &empty_started,
                &ready_children,
                &empty_children,
            ),
            CurrentProcessResolution::Ambiguous,
        );
        assert_eq!(
            resolve_current_process(
                &[],
                &empty_ready,
                &started,
                &empty_ready_children,
                &empty_children,
            ),
            CurrentProcessResolution::Ambiguous,
        );
        assert_eq!(
            resolve_current_process(
                &[],
                &empty_ready,
                &empty_started,
                &ready_children,
                &empty_children,
            ),
            CurrentProcessResolution::Ambiguous,
        );
    }

    #[test]
    fn current_process_resolution_rejects_incomplete_and_multiple_tuples() {
        let current_fence = fence(1);
        let ready = ready_process(4312);
        let started_at = Instant::now();
        let (ready_processes, started, ready_children) =
            coherent_maps(&current_fence, ready.clone(), started_at);
        let current = current_process(Some(current_fence.clone()));

        assert_eq!(
            resolve_current_process(
                &[current_process(None)],
                &ready_processes,
                &started,
                &ready_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Ambiguous,
        );
        assert_eq!(
            resolve_current_process(
                std::slice::from_ref(&current),
                &BTreeMap::new(),
                &started,
                &ready_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Ambiguous,
        );
        assert_eq!(
            resolve_current_process(
                std::slice::from_ref(&current),
                &ready_processes,
                &started,
                &BTreeMap::new(),
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Ambiguous,
        );
        assert_eq!(
            resolve_current_process(
                &[current.clone(), current],
                &ready_processes,
                &started,
                &ready_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Ambiguous,
        );

        let stale_fence = fence(2);
        let mut stale_ready = ready_processes.clone();
        stale_ready.insert(FenceKey::from(&stale_fence), ready);
        assert_eq!(
            resolve_current_process(
                &[current_process(Some(current_fence))],
                &stale_ready,
                &started,
                &ready_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Ambiguous,
        );
    }

    #[test]
    fn current_process_resolution_rejects_every_mismatched_identity_member() {
        let current_fence = fence(1);
        let ready = ready_process(4312);
        let started_at = Instant::now();
        let (ready_processes, started, ready_children) =
            coherent_maps(&current_fence, ready, started_at);
        let current = current_process(Some(current_fence.clone()));

        let mut wrong_id = current.clone();
        wrong_id.id = "local:99".to_owned();
        let mut wrong_name = current.clone();
        wrong_name.name = "parakeet-server".to_owned();
        let mut wrong_pid_children = ready_children.clone();
        wrong_pid_children
            .get_mut(&FenceKey::from(&current_fence))
            .expect("ready child")
            .pid += 1;
        for (managed, children) in [
            (wrong_id, ready_children.clone()),
            (wrong_name, ready_children.clone()),
            (current, wrong_pid_children),
        ] {
            assert_eq!(
                resolve_current_process(
                    &[managed],
                    &ready_processes,
                    &started,
                    &children,
                    &BTreeSet::new(),
                ),
                CurrentProcessResolution::Ambiguous,
            );
        }
    }

    #[test]
    fn ready_tuple_insert_and_cleanup_are_fence_isolated_under_reused_identity() {
        let old_fence = fence(1);
        let new_fence = fence(2);
        let old_key = FenceKey::from(&old_fence);
        let new_key = FenceKey::from(&new_fence);
        let old_start = Instant::now();
        let new_start = old_start + Duration::from_secs(7);
        let mut ready_processes = BTreeMap::new();
        let mut ready_children = BTreeMap::new();
        let mut started = BTreeMap::new();

        insert_ready_tuple(
            old_key.clone(),
            ready_process(4312),
            ReadyChildIdentity {
                process_id: "local:42".to_owned(),
                pid: 42,
            },
            old_start,
            &mut ready_processes,
            &mut ready_children,
            &mut started,
        );
        insert_ready_tuple(
            new_key.clone(),
            ready_process(4312),
            ReadyChildIdentity {
                process_id: "local:42".to_owned(),
                pid: 42,
            },
            new_start,
            &mut ready_processes,
            &mut ready_children,
            &mut started,
        );

        assert_eq!(started.get(&old_key), Some(&old_start));
        assert_eq!(started.get(&new_key), Some(&new_start));
        remove_ready_tuple(
            &old_key,
            &mut ready_processes,
            &mut ready_children,
            &mut started,
        );
        assert!(!ready_processes.contains_key(&old_key));
        assert!(!ready_children.contains_key(&old_key));
        assert!(!started.contains_key(&old_key));
        assert!(ready_processes.contains_key(&new_key));
        assert!(ready_children.contains_key(&new_key));
        assert_eq!(started.get(&new_key), Some(&new_start));
    }

    #[test]
    fn current_process_resolution_uses_fence_when_pid_is_reused() {
        let old_fence = fence(1);
        let new_fence = fence(2);
        let old_started_at = Instant::now();
        let new_started_at = old_started_at + Duration::from_secs(1);
        let old_ready = ready_process(4312);
        let new_ready = old_ready.clone();

        let (old_ready_processes, old_started, old_children) =
            coherent_maps(&old_fence, old_ready.clone(), old_started_at);
        let (new_ready_processes, new_started, new_children) =
            coherent_maps(&new_fence, new_ready.clone(), new_started_at);
        assert_eq!(
            resolve_current_process(
                &[current_process(Some(old_fence.clone()))],
                &old_ready_processes,
                &old_started,
                &old_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Coherent {
                fence: FenceKey::from(&old_fence),
                ready: old_ready,
                started_at: old_started_at,
            }
        );
        assert_eq!(
            resolve_current_process(
                &[current_process(Some(new_fence.clone()))],
                &new_ready_processes,
                &new_started,
                &new_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Coherent {
                fence: FenceKey::from(&new_fence),
                ready: new_ready.clone(),
                started_at: new_started_at,
            }
        );
        assert_eq!(
            resolve_current_process(
                &[current_process(Some(old_fence))],
                &new_ready_processes,
                &new_started,
                &new_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Ambiguous,
        );

        let mut both_ready = old_ready_processes;
        both_ready.extend(new_ready_processes);
        let mut both_started = old_started;
        both_started.extend(new_started);
        assert_eq!(
            resolve_current_process(
                &[current_process(Some(new_fence))],
                &both_ready,
                &both_started,
                &new_children,
                &BTreeSet::new(),
            ),
            CurrentProcessResolution::Ambiguous,
        );
    }

    fn write_retry(path: &Path, revision: u64) {
        let record = json!({
            "schema_version": 1,
            "provider": "local",
            "revision": revision,
            "token_id": "retry-1",
            "desired_fingerprint_sha256": "fingerprint",
            "requested_at": "2026-08-09T12:00:00+00:00",
            "reason_code": "retry-token-requested",
            "owner": Value::Null,
        });
        write_json(
            path,
            &record,
            JsonWriteOptions {
                mode: Some(FILE_MODE),
                indent: Some(2),
                sort_keys: true,
            },
        )
        .expect("retry record");
    }

    #[test]
    fn shared_results_are_fence_keyed_and_drained_once() {
        let shared = LocalRuntimeShared::default();
        let first = fence(1);
        let second = fence(2);
        let result = ProviderTruthObservation {
            provider: ProviderName::Local,
            phase: RuntimePhase::Starting,
            reason_code: Some(ReasonCode::known("launch-requested")),
            desired_fingerprint: Some("fingerprint".to_owned()),
            has_plan: true,
            boot_required: false,
            detail: None,
        };

        shared.record_truth_result(&first, result.clone());
        assert_eq!(shared.take_truth_result(&second), None);
        assert_eq!(shared.take_truth_result(&first), Some(result));
        assert_eq!(shared.take_truth_result(&first), None);
    }

    #[test]
    fn publish_state_bumps_revision_and_rejects_a_stale_cached_revision() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut store = store(&journal, shared);
        let state = state(RuntimePhase::Starting);
        store.publish_state(&state).expect("first publish");
        assert_eq!(
            read_health(&store.health_path(), ProviderName::Local)
                .unwrap()
                .revision,
            1
        );

        let mut external =
            serde_json::from_slice::<Value>(&fs::read(store.health_path()).unwrap()).unwrap();
        external["revision"] = json!(2);
        fs::write(store.health_path(), serde_json::to_vec(&external).unwrap()).unwrap();
        assert_eq!(
            store.publish_state(&state),
            Err(RuntimeStoreError::Conflict)
        );
    }

    #[test]
    fn corrupt_and_unavailable_store_failures_are_distinct() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut store = store(&journal, shared);
        fs::create_dir_all(store.runtime_directory()).unwrap();
        fs::write(store.health_path(), b"not json").unwrap();
        assert_eq!(
            store.publish_state(&state(RuntimePhase::Starting)),
            Err(RuntimeStoreError::Corrupt)
        );

        let file_journal = std::env::temp_dir().join(format!(
            "solstone-local-runtime-store-file-{}",
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&file_journal, b"not a directory").unwrap();
        let mut unavailable = FileRuntimeStore::new(
            file_journal.clone(),
            ProviderName::Local,
            Arc::new(LocalRuntimeShared::default()),
            Arc::new(FixedClock),
        );
        assert_eq!(
            unavailable.publish_state(&state(RuntimePhase::Starting)),
            Err(RuntimeStoreError::Unavailable)
        );
        let _ = fs::remove_file(file_journal);
    }

    #[test]
    fn read_current_detail_fails_closed_on_corrupt_and_unavailable_records() {
        let journal = TempJournal::new();
        let runtime_dir = journal.0.join("health").join("providers").join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::write(runtime_dir.join("parakeet.json"), b"not json").unwrap();
        assert_eq!(
            read_current_detail(&journal.0, ProviderName::Parakeet),
            Err(RuntimeStoreError::Corrupt)
        );

        let file_journal = std::env::temp_dir().join(format!(
            "solstone-admission-detail-file-{}",
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&file_journal, b"not a directory").unwrap();
        assert_eq!(
            read_current_detail(&file_journal, ProviderName::Parakeet),
            Err(RuntimeStoreError::Unavailable)
        );
        let _ = fs::remove_file(file_journal);
    }

    #[test]
    fn read_current_detail_on_a_fresh_journal_is_a_synthetic_empty_object() {
        let journal = TempJournal::new();
        assert_eq!(
            read_current_detail(&journal.0, ProviderName::Parakeet),
            Ok(json!({}))
        );
    }

    #[test]
    fn retry_read_without_consume_preserves_and_double_consume_conflicts() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut store = store(&journal, shared);
        fs::create_dir_all(store.runtime_directory()).unwrap();
        write_retry(&store.retry_path(), 7);

        let first = store
            .read_retry_token(ProviderName::Local)
            .unwrap()
            .expect("outstanding retry");
        let second = store
            .read_retry_token(ProviderName::Local)
            .unwrap()
            .expect("retry remains without consumption");
        assert_eq!(first.token_id, second.token_id);
        store
            .consume_retry_token(ProviderName::Local, &second.token_id)
            .unwrap();
        assert!(
            store
                .read_retry_token(ProviderName::Local)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.consume_retry_token(ProviderName::Local, &second.token_id),
            Err(RuntimeStoreError::Conflict)
        );
    }

    #[test]
    fn request_retry_token_persists_owner_and_advances_revision() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut store = store(&journal, shared);
        fs::create_dir_all(store.runtime_directory()).unwrap();

        let first = store
            .request_retry_token(
                Some("fingerprint".to_owned()),
                ReasonCode::known("local-wedge-provider-unavailable"),
                Map::from_iter([("source".to_owned(), json!("provider-runtime-recycle"))]),
            )
            .unwrap();
        let second = store
            .request_retry_token(
                Some("fingerprint".to_owned()),
                ReasonCode::known("local-wedge-provider-unavailable"),
                Map::from_iter([("source".to_owned(), json!("provider-runtime-recycle"))]),
            )
            .unwrap();

        assert_eq!(first.revision, 1);
        assert_eq!(second.revision, 2);
        assert_ne!(first.token_id, second.token_id);
        let record: Value = serde_json::from_slice(&fs::read(store.retry_path()).unwrap()).unwrap();
        assert_eq!(record["revision"], 2);
        assert_eq!(record["reason_code"], "local-wedge-provider-unavailable");
        assert_eq!(record["owner"]["source"], "provider-runtime-recycle");
    }

    #[test]
    fn corrupt_retry_record_is_not_treated_as_absent() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut store = store(&journal, shared);
        fs::create_dir_all(store.runtime_directory()).unwrap();
        fs::write(store.retry_path(), b"not json").unwrap();
        assert_eq!(
            store.read_retry_token(ProviderName::Local),
            Err(RuntimeStoreError::Corrupt)
        );
    }

    #[test]
    fn retry_consume_rejects_a_changed_desired_fingerprint() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut store = store(&journal, shared);
        fs::create_dir_all(store.runtime_directory()).unwrap();
        write_retry(&store.retry_path(), 7);
        let token = store
            .read_retry_token(ProviderName::Local)
            .unwrap()
            .expect("outstanding retry");

        let mut changed =
            serde_json::from_slice::<Value>(&fs::read(store.retry_path()).unwrap()).unwrap();
        changed["desired_fingerprint_sha256"] = json!("newer-fingerprint");
        fs::write(store.retry_path(), serde_json::to_vec(&changed).unwrap()).unwrap();

        assert_eq!(
            store.consume_retry_token(ProviderName::Local, &token.token_id),
            Err(RuntimeStoreError::Conflict)
        );
    }

    fn prepare_for_clear() -> (TempJournal, FileRuntimeStore, ProviderRuntimeState) {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let start_fence = fence(1);
        shared.record_ready_process(&start_fence, ready_process(4312));
        let mut store = store(&journal, shared);
        store.publish_state(&ready_state(start_fence)).unwrap();

        let cleanup_fence = fence(0);
        let mut stopping = state(RuntimePhase::Stopping);
        stopping.stop_cleanup = Some(InFlight {
            fence: cleanup_fence,
            result: None,
        });
        stopping.pending_stop_request = Some(ProviderStopCleanupRequest {
            managed: ManagedProcess {
                id: "local:42".to_owned(),
                name: "local".to_owned(),
                running: true,
                fence: None,
            },
            reason_code: ReasonCode::known("intent-removed"),
            target_phase: RuntimePhase::Stopped,
            target_reason_code: Some(ReasonCode::known("cleanup-succeeded")),
            admission_exclusive: false,
            orphaned_start_outcome: false,
        });
        store.publish_state(&stopping).unwrap();

        let mut stopped = stopping;
        stopped.stop_cleanup = None;
        stopped.pending_stop_request = None;
        stopped.latest_phase = RuntimePhase::Stopped;
        (journal, store, stopped)
    }

    #[test]
    fn stale_port_clear_refuses_a_runtime_health_fence_mismatch() {
        let (_journal, mut store, stopped) = prepare_for_clear();
        let mut health =
            serde_json::from_slice::<Value>(&fs::read(store.health_path()).unwrap()).unwrap();
        health["incarnation"] = json!("newer-incarnation");
        fs::write(store.health_path(), serde_json::to_vec(&health).unwrap()).unwrap();

        store.publish_state(&stopped).unwrap();
        assert_eq!(fs::read_to_string(store.port_path()).unwrap(), "4312");
    }

    #[test]
    fn stale_port_clear_refuses_a_changed_port_file() {
        let (_journal, mut store, stopped) = prepare_for_clear();
        fs::write(store.port_path(), "4999").unwrap();

        store.publish_state(&stopped).unwrap();
        assert_eq!(fs::read_to_string(store.port_path()).unwrap(), "4999");
    }

    #[test]
    fn ready_side_effect_drains_once_per_start_fence() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let start_fence = fence(1);
        shared.record_ready_process(&start_fence, ready_process(4312));
        let mut store = store(&journal, shared);
        let state = ready_state(start_fence);
        store.publish_state(&state).unwrap();
        assert_eq!(
            store.take_ready_side_effects(),
            [LocalReadySideEffect::RefreshBrain {
                expected_fingerprint_sha256: "fingerprint".to_owned(),
            }]
        );
        store.publish_state(&state).unwrap();
        assert!(store.take_ready_side_effects().is_empty());
    }

    fn parakeet_state(phase: RuntimePhase) -> ProviderRuntimeState {
        let mut state = ProviderRuntimeState::new(ProviderName::Parakeet);
        state.generation = 1;
        state.desired_fingerprint = Some("fingerprint".to_owned());
        state.latest_phase = phase;
        state.latest_reason_code = Some(ReasonCode::known("provider-not-needed"));
        state
    }

    #[test]
    fn a_store_built_for_parakeet_writes_parakeet_paths_not_local_ones() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut store = FileRuntimeStore::new(
            journal.0.clone(),
            ProviderName::Parakeet,
            shared,
            Arc::new(FixedClock),
        );
        store
            .publish_state(&parakeet_state(RuntimePhase::NotDesired))
            .unwrap();
        assert_eq!(
            store.health_path(),
            journal.0.join("health/providers/runtime/parakeet.json")
        );
        assert_eq!(
            store.retry_path(),
            journal
                .0
                .join("health/providers/runtime/parakeet.retry-token.json")
        );
        // The port file's service name is "parakeet-cpp" (matching Python's
        // parakeet_server.py _SERVICE_NAME), not "parakeet" -- the two are
        // allowed to differ per provider.
        assert_eq!(
            store.port_path(),
            journal.0.join("health/parakeet-cpp.port")
        );
        assert!(
            !journal
                .0
                .join("health/providers/runtime/local.json")
                .exists()
        );

        let on_disk: Value =
            serde_json::from_slice(&fs::read(store.health_path()).unwrap()).unwrap();
        assert_eq!(on_disk["provider"], "parakeet");
    }

    #[test]
    fn a_store_built_for_one_provider_refuses_the_other_providers_state() {
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let mut local_store = store(&journal, shared);
        assert_eq!(
            local_store.publish_state(&parakeet_state(RuntimePhase::NotDesired)),
            Err(RuntimeStoreError::Unavailable)
        );
        assert_eq!(
            local_store.read_retry_token(ProviderName::Parakeet),
            Err(RuntimeStoreError::Unavailable)
        );
    }

    #[test]
    fn parakeet_reaching_ready_never_produces_a_refresh_brain_side_effect() {
        // RefreshBrain is meaningful only for Local's own bundled-LLM
        // backend; Parakeet reaching Ready must never queue one even though
        // publish_state's Ready-phase branch runs for every provider.
        let journal = TempJournal::new();
        let shared = Arc::new(LocalRuntimeShared::default());
        let start_fence = fence(1);
        shared.record_ready_process(&start_fence, ready_process(5150));
        let mut store = FileRuntimeStore::new(
            journal.0.clone(),
            ProviderName::Parakeet,
            shared,
            Arc::new(FixedClock),
        );
        let mut state = parakeet_state(RuntimePhase::Ready);
        state.retry.attempt_count = start_fence.attempt;
        state.start = Some(InFlight {
            fence: start_fence,
            result: None,
        });
        store.publish_state(&state).unwrap();
        assert!(store.take_ready_side_effects().is_empty());
    }
}
