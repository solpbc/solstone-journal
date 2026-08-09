// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable Local-provider runtime state, retry tokens, and owned port publication.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteOptions, JsonWriteOptions, LockOptions, hold_lock, write_json, write_text,
};

use super::launch::LocalLaunchConfig;
use super::model::{
    ProviderFence, ProviderLaunchOutcome, ProviderName, ProviderProbeOutcome, ProviderRuntimeState,
    ProviderStopCleanupOutcome, ProviderTruthObservation, ReasonCode, RuntimePhase,
};
use super::seams::{RetryToken, RuntimeStore, RuntimeStoreError};

const PROVIDER: ProviderName = ProviderName::Local;
const FILE_MODE: u32 = 0o600;
const SCHEMA_VERSION: u64 = 1;

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
pub struct LocalReadyProcess {
    pub process_id: String,
    pub process_name: String,
    pub pid: u32,
    pub port: u16,
}

#[derive(Debug, Default)]
pub struct LocalRuntimeShared {
    ready_processes: Mutex<BTreeMap<FenceKey, LocalReadyProcess>>,
    launch_requests: Mutex<BTreeMap<LaunchRequestKey, LocalLaunchConfig>>,
    results: Mutex<LocalRuntimeResults>,
    result_available: Condvar,
    children: Mutex<BTreeMap<String, Child>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct LaunchRequestKey {
    generation: u64,
    desired_fingerprint: Option<String>,
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
        generation: u64,
        desired_fingerprint: Option<String>,
        config: LocalLaunchConfig,
    ) {
        self.launch_requests
            .lock()
            .expect("local runtime shared lock")
            .insert(
                LaunchRequestKey {
                    generation,
                    desired_fingerprint,
                },
                config,
            );
    }

    pub fn launch_request_for(
        &self,
        generation: u64,
        desired_fingerprint: &Option<String>,
    ) -> Option<LocalLaunchConfig> {
        self.launch_requests
            .lock()
            .expect("local runtime shared lock")
            .get(&LaunchRequestKey {
                generation,
                desired_fingerprint: desired_fingerprint.clone(),
            })
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

    pub fn record_ready_process(&self, fence: &ProviderFence, process: LocalReadyProcess) {
        self.ready_processes
            .lock()
            .expect("local runtime shared lock")
            .insert(FenceKey::from(fence), process);
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

    fn ready_process_for_fence(&self, fence: &ProviderFence) -> Option<LocalReadyProcess> {
        self.ready_processes
            .lock()
            .expect("local runtime shared lock")
            .get(&FenceKey::from(fence))
            .cloned()
    }

    fn ready_process_for_id(&self, process_id: &str) -> Option<LocalReadyProcess> {
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
    process: LocalReadyProcess,
}

/// RuntimeStore implementation for the Local provider only.
pub struct LocalRuntimeStore {
    journal_path: PathBuf,
    shared: Arc<LocalRuntimeShared>,
    clock: Arc<dyn RuntimeClock>,
    observed_health_revision: Option<u64>,
    observed_retry_tokens: BTreeMap<String, RetryObservation>,
    last_fence: Option<ProviderFence>,
    cleanup_owners: BTreeMap<FenceKey, PortOwner>,
    ready_effect_fences: BTreeSet<FenceKey>,
    ready_side_effects: Vec<LocalReadySideEffect>,
}

impl LocalRuntimeStore {
    pub fn new(
        journal_path: impl Into<PathBuf>,
        shared: Arc<LocalRuntimeShared>,
        clock: Arc<dyn RuntimeClock>,
    ) -> Self {
        Self {
            journal_path: journal_path.into(),
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

    pub fn take_ready_side_effects(&mut self) -> Vec<LocalReadySideEffect> {
        std::mem::take(&mut self.ready_side_effects)
    }

    fn runtime_directory(&self) -> PathBuf {
        self.journal_path
            .join("health")
            .join("providers")
            .join("runtime")
    }

    fn health_path(&self) -> PathBuf {
        self.runtime_directory().join("local.json")
    }

    fn retry_path(&self) -> PathBuf {
        self.runtime_directory().join("local.retry-token.json")
    }

    fn operation_path(&self) -> PathBuf {
        self.runtime_directory().join("local.operation")
    }

    fn port_path(&self) -> PathBuf {
        self.journal_path.join("health").join("local.port")
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

    fn ready_process_for(&self, fence: Option<&ProviderFence>) -> Option<LocalReadyProcess> {
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
        process: Option<LocalReadyProcess>,
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
            "provider": "local",
            "revision": revision,
            "phase": state.latest_phase.as_str(),
            "reason_code": state.latest_reason_code.as_ref().map(ReasonCode::as_str),
            "detail": {},
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
            "provider": "local",
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

impl RuntimeStore for LocalRuntimeStore {
    fn read_retry_token(
        &mut self,
        provider: ProviderName,
    ) -> Result<Option<RetryToken>, RuntimeStoreError> {
        ensure_local(provider)?;
        let _lock = self.lock_operation()?;
        let record = read_retry(&self.retry_path())?;
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
        ensure_local(provider)?;
        let expected = self
            .observed_retry_tokens
            .get(token_id)
            .cloned()
            .ok_or(RuntimeStoreError::Conflict)?;
        let _lock = self.lock_operation()?;
        let current = read_retry(&self.retry_path())?;
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
        ensure_local(state.provider)?;
        let _lock = self.lock_operation()?;
        let current = read_health(&self.health_path())?;
        if self
            .observed_health_revision
            .is_some_and(|revision| revision != current.revision)
        {
            return Err(RuntimeStoreError::Conflict);
        }
        let fence = self.capture_owner_fence(state);
        let ready_process = self.ready_process_for(fence.as_ref());
        let cleanup_owner = self.cleanup_owner_for(fence.as_ref());
        if state.latest_phase == RuntimePhase::Stopped {
            if let Some(owner) = cleanup_owner.as_ref() {
                self.clear_port_if_owned(&current, owner)?;
            }
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
            if self.ready_effect_fences.insert(key)
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

fn ensure_local(provider: ProviderName) -> Result<(), RuntimeStoreError> {
    if provider == PROVIDER {
        Ok(())
    } else {
        Err(RuntimeStoreError::Unavailable)
    }
}

fn read_health(path: &Path) -> Result<HealthRecord, RuntimeStoreError> {
    let value = read_value(path)?;
    let Some(value) = value else {
        return Ok(HealthRecord {
            revision: 0,
            incarnation: None,
            generation: 0,
            attempt: 0,
            process: None,
        });
    };
    let object = value.as_object().ok_or(RuntimeStoreError::Corrupt)?;
    validate_schema_and_provider(object)?;
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
    })
}

fn read_retry(path: &Path) -> Result<RetryRecord, RuntimeStoreError> {
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
    validate_schema_and_provider(object)?;
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

fn validate_schema_and_provider(object: &Map<String, Value>) -> Result<(), RuntimeStoreError> {
    if object
        .get("schema_version")
        .is_some_and(|value| value.as_u64() != Some(SCHEMA_VERSION))
        || object
            .get("provider")
            .is_some_and(|value| value.as_str() != Some("local"))
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

    fn store(journal: &TempJournal, shared: Arc<LocalRuntimeShared>) -> LocalRuntimeStore {
        LocalRuntimeStore::new(journal.0.clone(), shared, Arc::new(FixedClock))
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

    fn ready_process(port: u16) -> LocalReadyProcess {
        LocalReadyProcess {
            process_id: "local:42".to_owned(),
            process_name: "local".to_owned(),
            pid: 42,
            port,
        }
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
        assert_eq!(read_health(&store.health_path()).unwrap().revision, 1);

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
        let mut unavailable = LocalRuntimeStore::new(
            file_journal.clone(),
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

    fn prepare_for_clear() -> (TempJournal, LocalRuntimeStore, ProviderRuntimeState) {
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
}
