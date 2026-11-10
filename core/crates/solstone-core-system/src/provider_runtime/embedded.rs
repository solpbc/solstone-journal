// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! App-owned native workers participate in the ordinary provider fences.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, json};
use solstone_core_local::embedded::{self, Engine, Phase, RuntimeIdentity};
use solstone_core_local::install::{
    pins,
    readiness::{inspect_local, inspect_local_present},
};

use crate::process::{LaunchAuthority, LaunchError};

use super::launch::{LocalLaunchCommon, LocalLaunchConfig, WarmupHealth};
use super::model::{
    LaunchOutcomeStatus, ManagedProcess, ProviderFence, ProviderLaunchOutcome, ProviderName,
    ProviderRuntimeState, ProviderTruthObservation, ReasonCode, RuntimePhase,
};
use super::store::{LocalRuntimeShared, ReadyProcess, RuntimeClock};

/// Host PID is truthful owner metadata only. Every native operation uses the
/// owned Engine, never a process signal or an OS liveness sample.
pub(crate) enum LocalAuthority {
    Process(Box<LaunchAuthority>),
    Embedded {
        // Engine Drop joins native work before the following holds are dropped.
        engine: Box<dyn Engine>,
        startup_permit: Option<solstone_core_local::admission::LocalSlotPermit>,
        inference_retirement: Arc<embedded::InferenceRetirement>,
        generation: u64,
        host_pid: u32,
        _holds: Vec<Arc<dyn Send + Sync>>,
    },
}

impl std::fmt::Debug for LocalAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Process(authority) => authority.fmt(formatter),
            Self::Embedded {
                generation,
                host_pid,
                ..
            } => formatter
                .debug_struct("EmbeddedAuthority")
                .field("generation", generation)
                .field("host_pid", host_pid)
                .finish_non_exhaustive(),
        }
    }
}

impl From<LaunchAuthority> for LocalAuthority {
    fn from(value: LaunchAuthority) -> Self {
        Self::Process(Box::new(value))
    }
}

impl LocalAuthority {
    pub(crate) fn pid(&self) -> u32 {
        match self {
            Self::Process(authority) => authority.pid(),
            Self::Embedded { host_pid, .. } => *host_pid,
        }
    }

    pub(crate) fn poll(&mut self) -> io::Result<Option<i32>> {
        match self {
            Self::Process(authority) => authority.poll(),
            Self::Embedded {
                engine,
                generation,
                startup_permit,
                inference_retirement,
                ..
            } => {
                if inference_retirement.requested() {
                    engine.request_stop();
                }
                if !engine.is_finished() {
                    return Ok(None);
                }
                let snapshot = engine.stop_and_join().map_err(io::Error::other)?;
                if snapshot.generation != *generation
                    || !engine.is_finished()
                    || !matches!(snapshot.phase, Phase::Stopped | Phase::Failed)
                {
                    return Err(io::Error::other("native retirement is not confirmed"));
                }
                startup_permit.take();
                inference_retirement.confirm_joined();
                Ok(Some(snapshot.engine_result))
            }
        }
    }

    pub(crate) fn terminate(&mut self, timeout: Duration) -> Result<(), LaunchError> {
        match self {
            Self::Process(authority) => authority.terminate(timeout),
            Self::Embedded {
                engine,
                generation,
                startup_permit,
                inference_retirement,
                ..
            } => {
                engine.request_stop();
                let snapshot = engine
                    .stop_and_join()
                    .map_err(|error| LaunchError::Terminate(io::Error::other(error)))?;
                if snapshot.generation != *generation
                    || !engine.is_finished()
                    || !matches!(snapshot.phase, Phase::Stopped | Phase::Failed)
                {
                    return Err(LaunchError::Terminate(io::Error::other(
                        "native retirement is not confirmed",
                    )));
                }
                startup_permit.take();
                inference_retirement.confirm_joined();
                Ok(())
            }
        }
    }
}

fn acquire_startup_permit(
    shared: &LocalRuntimeShared,
    journal: &Path,
    timeout: Duration,
) -> Result<
    solstone_core_local::admission::LocalSlotPermit,
    solstone_core_local::admission::CheckedAdmissionError,
> {
    use solstone_core_local::admission::{acquire_local_slot_checked, admission_dir};
    acquire_local_slot_checked(
        &admission_dir(journal),
        1,
        Some(timeout),
        false,
        // Starting the owner's runtime is immediate work with no dated source.
        solstone_core_journal_config::work_priority::WorkPriority::default(),
        &|| shared.launch_credentials_closed() || !shared.launch_binding_valid(journal),
        &mut || {},
    )
}

fn outcome(status: LaunchOutcomeStatus, code: &'static str) -> ProviderLaunchOutcome {
    ProviderLaunchOutcome {
        status,
        reason_code: ReasonCode::known(code),
        managed: None,
    }
}

fn verified_inventory(
    journal: &Path,
    common: &LocalLaunchCommon,
    identity: &RuntimeIdentity,
) -> bool {
    let readiness = inspect_local(Map::from_iter([
        ("journal".into(), json!(journal)),
        ("model_id".into(), json!(common.model_id)),
        ("artifact_key".into(), json!(identity.artifact_key)),
    ]));
    readiness["ready"] == true
        && readiness["host"]["backend"] == identity.backend()
        && readiness["target"]["target_fingerprint_sha256"]
            == common.desired_fingerprint_json["artifact_target_fingerprint_sha256"]
        && readiness["artifacts"]["model_id"] == common.model_id
        && readiness["artifacts"]["model_path"].as_str() == Some(common.model_path.as_str())
        && readiness["artifacts"]["projector_path"].as_str() == common.mmproj_path.as_deref()
}

/// The Factory returns the worker before loading. Closing public credentials
/// cancels that load on this worker; the model-generation use lease stays with
/// the engine through every Ready, timeout, failure and joined Stop path.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_embedded(
    shared: &LocalRuntimeShared,
    clock: &dyn RuntimeClock,
    common: &LocalLaunchCommon,
    identity: &RuntimeIdentity,
    state: &ProviderRuntimeState,
    fence: &ProviderFence,
    journal: Option<&Path>,
    warmup_timeout: Duration,
    poll_interval: Duration,
) -> ProviderLaunchOutcome {
    let Some(journal) = journal else {
        return outcome(LaunchOutcomeStatus::LaunchFailed, "launch-failed");
    };
    if shared.launch_credentials_closed() {
        return outcome(LaunchOutcomeStatus::NotReady, "intent-removed");
    }
    let Some(factory) = embedded::factory().filter(|factory| factory.identity() == *identity)
    else {
        return outcome(LaunchOutcomeStatus::HostBlocked, "package-unavailable");
    };
    let Some(model_directory) = Path::new(&common.model_path).parent() else {
        return outcome(LaunchOutcomeStatus::LaunchFailed, "artifact-stale");
    };
    let Ok(model_hold) = pins::hold_model_use(model_directory) else {
        return outcome(LaunchOutcomeStatus::LaunchFailed, "artifact-stale");
    };
    if !verified_inventory(journal, common, identity) {
        return outcome(LaunchOutcomeStatus::LaunchFailed, "artifact-stale");
    }
    if shared.launch_credentials_closed() {
        return outcome(LaunchOutcomeStatus::NotReady, "intent-removed");
    }
    let (Some(projector), Some(model_generation)) = (
        common.mmproj_path.as_ref(),
        pins::model_generation(&common.model_id),
    ) else {
        return outcome(LaunchOutcomeStatus::LaunchFailed, "artifact-stale");
    };
    let mut entropy = [0_u8; 32];
    if getrandom::fill(&mut entropy).is_err() {
        return outcome(LaunchOutcomeStatus::LaunchFailed, "launch-failed");
    }
    let token: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
    let waiting = Instant::now();
    let startup_permit = if cfg!(target_os = "ios") {
        use solstone_core_local::admission::{AdmissionError, CheckedAdmissionError};
        match acquire_startup_permit(shared, journal, warmup_timeout) {
            Ok(permit) => Some(permit),
            Err(CheckedAdmissionError::Cancelled) => {
                return outcome(LaunchOutcomeStatus::NotReady, "intent-removed");
            }
            Err(CheckedAdmissionError::Admission(AdmissionError::Timeout)) => {
                return outcome(LaunchOutcomeStatus::WarmupTimeout, "warmup-timeout");
            }
            Err(CheckedAdmissionError::Admission(AdmissionError::Io(_))) => {
                return outcome(LaunchOutcomeStatus::LaunchFailed, "launch-failed");
            }
        }
    } else {
        None
    };
    if startup_permit.is_some()
        && (shared.launch_credentials_closed() || !shared.launch_binding_valid(journal))
    {
        return outcome(LaunchOutcomeStatus::NotReady, "intent-removed");
    }
    let warmup_timeout = if startup_permit.is_some() {
        warmup_timeout.saturating_sub(waiting.elapsed())
    } else {
        warmup_timeout
    };
    let engine = match factory.start(embedded::Launch {
        identity: identity.clone(),
        generation: state.generation,
        model_generation: model_generation.key().to_owned(),
        model_path: common.model_path.clone().into(),
        projector_path: projector.as_str().into(),
        bearer_token: token.clone(),
    }) {
        Ok(engine) => engine,
        Err(_) => return outcome(LaunchOutcomeStatus::LaunchFailed, "launch-failed"),
    };
    // Windows file locks are Send, but the retained shared holder also needs Sync.
    #[cfg(windows)]
    let model_hold = std::sync::Mutex::new(model_hold);
    let authority = LocalAuthority::Embedded {
        engine,
        startup_permit,
        inference_retirement: Arc::new(embedded::InferenceRetirement::default()),
        generation: state.generation,
        host_pid: std::process::id(),
        _holds: vec![Arc::new(model_hold)],
    };
    warmup_native(
        shared,
        clock,
        authority,
        fence,
        token,
        warmup_timeout,
        poll_interval,
        probe_embedded,
    )
}

fn classify_properties(status: u16, body: &str) -> WarmupHealth {
    if status == 503 && body.to_ascii_lowercase().contains("loading model") {
        return WarmupHealth::Loading;
    }
    let Ok(props) = serde_json::from_str::<serde_json::Value>(body) else {
        return WarmupHealth::Failed;
    };
    // ABI1 is the maintained full32k, one-slot runtime. Never admit a package
    // whose live server has silently reduced that configured context.
    if status == 200
        && props["default_generation_settings"]["n_ctx"] == 32_768
        && props["total_slots"] == 1
    {
        WarmupHealth::Ready
    } else {
        WarmupHealth::Failed
    }
}

fn probe_embedded(port: u16, token: &str) -> WarmupHealth {
    use solstone_core_local::GenerateTransport;
    match solstone_core_local::UreqTransport.get(
        &format!("http://127.0.0.1:{port}"),
        "/props",
        Duration::from_secs(1),
        Some(token),
    ) {
        Ok(response) => classify_properties(response.status, &response.body),
        Err(_) => WarmupHealth::Failed,
    }
}

#[allow(clippy::too_many_arguments)]
fn warmup_native(
    shared: &LocalRuntimeShared,
    clock: &dyn RuntimeClock,
    mut authority: LocalAuthority,
    fence: &ProviderFence,
    token: String,
    warmup_timeout: Duration,
    poll_interval: Duration,
    mut probe: impl FnMut(u16, &str) -> WarmupHealth,
) -> ProviderLaunchOutcome {
    let pid = authority.pid();
    let process_id = format!(
        "local:embedded:{}:{}:{}",
        fence.incarnation, fence.generation, fence.attempt
    );
    let started_at = Instant::now();
    let deadline = clock.monotonic_seconds() + warmup_timeout.as_secs_f64();
    let mut timed_out = false;
    loop {
        if shared.launch_credentials_closed() {
            if authority.terminate(Duration::ZERO).is_ok() {
                return outcome(LaunchOutcomeStatus::NotReady, "intent-removed");
            }
            break;
        }
        let snapshot = match &authority {
            LocalAuthority::Embedded { engine, .. } => engine.snapshot(),
            LocalAuthority::Process(_) => unreachable!("native authority was constructed above"),
        };
        let Ok(snapshot) = snapshot else { break };
        if snapshot.generation != fence.generation {
            break;
        }
        if matches!(snapshot.phase, Phase::Failed | Phase::Stopped) {
            if authority.terminate(Duration::ZERO).is_ok() {
                return outcome(LaunchOutcomeStatus::Exited, "process-exited");
            }
            break;
        }
        if snapshot.phase == Phase::Ready
            && let Some(port) = snapshot.port
            && probe(port.get(), &token) == WarmupHealth::Ready
        {
            // The startup permit belongs to this authority through failed/late
            // warmup and every unconfirmed join. Only authenticated readiness
            // releases it while the native worker remains alive.
            if let LocalAuthority::Embedded { startup_permit, .. } = &mut authority {
                startup_permit.take();
            }
            shared.stage_launch_credentials(fence, port.get(), token.into_bytes());
            let managed = ManagedProcess {
                id: process_id.clone(),
                pid,
                name: "local".into(),
                running: true,
                fence: Some(fence.clone()),
            };
            shared.register_ready_process(
                fence,
                authority,
                ReadyProcess {
                    process_id,
                    process_name: "local".into(),
                    pid,
                    port: port.get(),
                },
                started_at,
            );
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Ready,
                reason_code: ReasonCode::known("probe-ready"),
                managed: Some(managed),
            };
        }
        if clock.monotonic_seconds() >= deadline {
            timed_out = true;
            break;
        }
        clock.sleep(poll_interval);
    }
    // Failed/late starts still carry their actual owner into coordinator cleanup.
    shared.retain_child(process_id.clone(), authority);
    ProviderLaunchOutcome {
        status: if timed_out {
            LaunchOutcomeStatus::WarmupTimeout
        } else {
            LaunchOutcomeStatus::LaunchFailed
        },
        reason_code: ReasonCode::known(if timed_out {
            "warmup-timeout"
        } else {
            "launch-failed"
        }),
        managed: Some(ManagedProcess {
            id: process_id,
            pid,
            name: "local".into(),
            running: true,
            fence: None,
        }),
    }
}

fn truth(
    phase: RuntimePhase,
    code: &'static str,
    desired_fingerprint: Option<String>,
    identity: Option<&RuntimeIdentity>,
    has_plan: bool,
) -> ProviderTruthObservation {
    ProviderTruthObservation {
        provider: ProviderName::Local,
        phase,
        reason_code: Some(ReasonCode::known(code)),
        desired_fingerprint,
        has_plan,
        boot_required: has_plan,
        detail: Some(json!({
            "runtime_authority": "embedded",
            "pid_role": "host",
            "runtime_identity": identity.map(RuntimeIdentity::pin),
        })),
    }
}

pub(super) fn observe_truth(
    shared: &LocalRuntimeShared,
    journal: &Path,
    model_id: &str,
    artifact_key: &str,
) -> ProviderTruthObservation {
    let Some(identity) = embedded::identity_for_key(artifact_key) else {
        return truth(
            RuntimePhase::HostBlocked,
            "package-unavailable",
            None,
            None,
            false,
        );
    };
    let readiness = inspect_local_present(Map::from_iter([
        ("journal".into(), json!(journal)),
        ("model_id".into(), json!(model_id)),
        ("artifact_key".into(), json!(artifact_key)),
    ]));
    if readiness["install"]["install_state"]
        .as_str()
        .is_some_and(|state| {
            matches!(
                state,
                "resolving" | "downloading" | "verifying" | "installing"
            )
        })
    {
        return truth(
            RuntimePhase::ArtifactNotReady,
            "install-in-progress",
            None,
            Some(&identity),
            false,
        );
    }
    if readiness["ready"] != true {
        let reason = readiness["reason_code"].as_str().unwrap_or("");
        let (phase, code) = match reason {
            "platform_unsupported" | "unsupported_platform" => {
                (RuntimePhase::HostBlocked, "platform-unsupported")
            }
            "package_unavailable" => (RuntimePhase::HostBlocked, "package-unavailable"),
            "package_invalid" => (RuntimePhase::HostBlocked, "package-invalid"),
            "manifest_pin_mismatch"
            | "sha256_mismatch"
            | "inventory_member_missing"
            | "inventory_size_mismatch" => (RuntimePhase::ArtifactNotReady, "artifact-stale"),
            "manifest_missing" => (RuntimePhase::ArtifactNotReady, "manifest-missing"),
            _ if readiness["status"] == "proof-unavailable" => {
                (RuntimePhase::ArtifactNotReady, "artifact-proof-failed")
            }
            _ => (RuntimePhase::ArtifactNotReady, "artifact-missing"),
        };
        return truth(phase, code, None, Some(&identity), false);
    }
    let (Some(model_path), Some(projector), Some(target)) = (
        readiness["artifacts"]["model_path"].as_str(),
        readiness["artifacts"]["projector_path"].as_str(),
        readiness["target"]["target_fingerprint_sha256"].as_str(),
    ) else {
        return truth(
            RuntimePhase::StateUnavailable,
            "truth-observation-failed",
            None,
            Some(&identity),
            false,
        );
    };
    let Ok(desired) = solstone_core_brain::bundled_runtime_desired_fingerprint(
        identity.backend(),
        model_id,
        target,
        None,
        model_path,
        Some(projector),
    ) else {
        return truth(
            RuntimePhase::StateUnavailable,
            "truth-observation-failed",
            None,
            Some(&identity),
            false,
        );
    };
    let fingerprint = desired.sha256;
    shared.record_launch_request(
        Some(fingerprint.clone()),
        LocalLaunchConfig::Embedded {
            common: LocalLaunchCommon {
                desired_fingerprint_json: desired.json,
                desired_fingerprint_sha256: fingerprint.clone(),
                model_id: model_id.to_owned(),
                model_path: model_path.to_owned(),
                mmproj_path: Some(projector.to_owned()),
            },
            identity: identity.clone(),
        },
    );
    truth(
        RuntimePhase::Starting,
        "launch-requested",
        Some(fingerprint),
        Some(&identity),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    struct HeldModel(Arc<AtomicBool>);
    impl Drop for HeldModel {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    struct Native {
        generation: u64,
        finished: Arc<AtomicBool>,
        fail_join: Arc<AtomicBool>,
        joins: Arc<AtomicUsize>,
        held: Arc<AtomicBool>,
        ready: bool,
        stops: Arc<AtomicUsize>,
    }
    impl Engine for Native {
        fn snapshot(&self) -> Result<embedded::Snapshot, String> {
            Ok(embedded::Snapshot {
                generation: self.generation,
                phase: if self.is_finished() {
                    Phase::Stopped
                } else if self.ready {
                    Phase::Ready
                } else {
                    Phase::Loading
                },
                port: self.ready.then(|| std::num::NonZeroU16::new(1234).unwrap()),
                engine_result: 0,
                error: String::new(),
            })
        }
        fn request_stop(&self) {
            self.stops.fetch_add(1, Ordering::SeqCst);
        }
        fn is_finished(&self) -> bool {
            self.finished.load(Ordering::SeqCst)
        }
        fn stop_and_join(&mut self) -> Result<embedded::Snapshot, String> {
            self.joins.fetch_add(1, Ordering::SeqCst);
            if self.fail_join.swap(false, Ordering::SeqCst) {
                return Err("retirement unconfirmed".into());
            }
            self.finished.store(true, Ordering::SeqCst);
            self.snapshot()
        }
    }
    impl Drop for Native {
        fn drop(&mut self) {
            assert!(
                self.held.load(Ordering::SeqCst),
                "model lease outlives native owner"
            );
            self.finished.store(true, Ordering::SeqCst);
        }
    }

    fn native_authority(
        fail_join: bool,
        ready: bool,
    ) -> (
        LocalAuthority,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<AtomicUsize>,
    ) {
        native_authority_at(fail_join, ready, 7)
    }

    fn native_authority_at(
        fail_join: bool,
        ready: bool,
        generation: u64,
    ) -> (
        LocalAuthority,
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<AtomicUsize>,
    ) {
        let held = Arc::new(AtomicBool::new(true));
        let finished = Arc::new(AtomicBool::new(false));
        let joins = Arc::new(AtomicUsize::new(0));
        let authority = LocalAuthority::Embedded {
            engine: Box::new(Native {
                generation,
                finished: Arc::clone(&finished),
                fail_join: Arc::new(AtomicBool::new(fail_join)),
                joins: Arc::clone(&joins),
                held: Arc::clone(&held),
                ready,
                stops: Arc::new(AtomicUsize::new(0)),
            }),
            startup_permit: None,
            inference_retirement: Arc::new(embedded::InferenceRetirement::default()),
            generation,
            host_pid: 19,
            _holds: vec![Arc::new(HeldModel(Arc::clone(&held)))],
        };
        (authority, held, finished, joins)
    }

    #[test]
    fn native_liveness_and_retirement_come_from_the_handle_not_host_pid() {
        let (mut authority, held, finished, joins) = native_authority(false, false);
        assert_eq!(authority.pid(), 19);
        assert_eq!(authority.poll().unwrap(), None);
        assert_eq!(joins.load(Ordering::SeqCst), 0);
        finished.store(true, Ordering::SeqCst);
        assert_eq!(authority.poll().unwrap(), Some(0));
        assert!(held.load(Ordering::SeqCst));
        drop(authority);
        assert!(!held.load(Ordering::SeqCst));
    }

    #[test]
    fn failed_native_join_retains_the_model_lease_for_retry() {
        let (mut authority, held, finished, joins) = native_authority(true, false);
        assert!(authority.terminate(Duration::ZERO).is_err());
        assert!(!finished.load(Ordering::SeqCst));
        assert!(held.load(Ordering::SeqCst));
        assert!(authority.terminate(Duration::ZERO).is_ok());
        assert!(finished.load(Ordering::SeqCst));
        assert!(held.load(Ordering::SeqCst));
        assert_eq!(joins.load(Ordering::SeqCst), 2);
        drop(authority);
        assert!(!held.load(Ordering::SeqCst));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn startup_permit_survives_timeout_and_failed_join_until_real_retirement() {
        use solstone_core_journal_config::work_priority::WorkPriority;
        use solstone_core_local::admission::{AdmissionError, acquire_local_slot, admission_dir};
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap();
        let journal = canonical.as_path();
        let acquire = || {
            acquire_local_slot(
                &admission_dir(journal),
                1,
                Some(Duration::from_millis(100)),
                false,
                WorkPriority::default(),
            )
        };
        // The same bounded probe must acquire an unheld slot before it can
        // establish exclusion while native startup or retirement holds it.
        drop(acquire().unwrap());
        let shared = LocalRuntimeShared::default();
        shared.retain_bound_root(Arc::new(
            solstone_core_journal_io::JournalRoot::open(journal).unwrap(),
        ));
        let (mut authority, _, finished, _) = native_authority(true, false);
        if let LocalAuthority::Embedded { startup_permit, .. } = &mut authority {
            *startup_permit =
                Some(acquire_startup_permit(&shared, journal, Duration::from_secs(1)).unwrap());
        }
        let fence = fence(7);
        let result = warmup_native(
            &shared,
            &clock(None),
            authority,
            &fence,
            "secret".into(),
            Duration::ZERO,
            Duration::ZERO,
            |_, _| WarmupHealth::Loading,
        );
        assert_eq!(result.status, LaunchOutcomeStatus::WarmupTimeout);
        let mut authority = shared.take_child(&result.managed.unwrap().id).unwrap();
        assert!(matches!(acquire(), Err(AdmissionError::Timeout)));
        assert!(authority.terminate(Duration::ZERO).is_err());
        assert!(!finished.load(Ordering::Acquire));
        assert!(matches!(acquire(), Err(AdmissionError::Timeout)));
        authority.terminate(Duration::ZERO).unwrap();
        assert!(finished.load(Ordering::Acquire));
        drop(acquire().unwrap());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn startup_root_replacement_refuses_before_native_entry() {
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let journal = journal.canonicalize().unwrap();
        let shared = LocalRuntimeShared::default();
        shared.retain_bound_root(Arc::new(
            solstone_core_journal_io::JournalRoot::open(&journal).unwrap(),
        ));
        std::fs::rename(&journal, directory.path().join("old")).unwrap();
        std::fs::create_dir(&journal).unwrap();
        assert!(matches!(
            acquire_startup_permit(&shared, &journal, Duration::from_secs(1)),
            Err(solstone_core_local::admission::CheckedAdmissionError::Cancelled)
        ));
        assert!(!journal.join("health/local-inference-admission").exists());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn authenticated_ready_releases_startup_slot_before_generate() {
        use solstone_core_journal_config::work_priority::WorkPriority;
        use solstone_core_local::admission::{acquire_local_slot, admission_dir};
        let directory = tempfile::tempdir().unwrap();
        let canonical = directory.path().canonicalize().unwrap();
        let journal = canonical.as_path();
        let shared = LocalRuntimeShared::default();
        shared.retain_bound_root(Arc::new(
            solstone_core_journal_io::JournalRoot::open(journal).unwrap(),
        ));
        let (mut authority, _, finished, _) = native_authority(false, true);
        if let LocalAuthority::Embedded { startup_permit, .. } = &mut authority {
            *startup_permit =
                Some(acquire_startup_permit(&shared, journal, Duration::from_secs(1)).unwrap());
        }
        let f = fence(7);
        let result = warmup_native(
            &shared,
            &clock(None),
            authority,
            &f,
            "secret".into(),
            Duration::from_secs(1),
            Duration::ZERO,
            |port, token| {
                assert_eq!((port, token), (1234, "secret"));
                assert!(
                    acquire_local_slot(
                        &admission_dir(journal),
                        1,
                        Some(Duration::from_millis(30)),
                        false,
                        WorkPriority::default()
                    )
                    .is_err()
                );
                classify_properties(
                    200,
                    r#"{"default_generation_settings":{"n_ctx":32768},"total_slots":1}"#,
                )
            },
        );
        assert_eq!(result.status, LaunchOutcomeStatus::Ready);
        assert!(!finished.load(Ordering::Acquire));
        let generate = acquire_local_slot(
            &admission_dir(journal),
            1,
            Some(Duration::from_secs(1)),
            false,
            WorkPriority::default(),
        )
        .unwrap();
        drop(generate);
        let (_, mut authority) = shared.take_ready_child(&f).unwrap();
        authority.terminate(Duration::ZERO).unwrap();
        assert!(finished.load(Ordering::Acquire));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn late_generation_retains_startup_slot_until_joined_cleanup() {
        use solstone_core_journal_config::work_priority::WorkPriority;
        use solstone_core_local::admission::{acquire_local_slot, admission_dir};
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().canonicalize().unwrap();
        let shared = LocalRuntimeShared::default();
        shared.retain_bound_root(Arc::new(
            solstone_core_journal_io::JournalRoot::open(&journal).unwrap(),
        ));
        let (mut authority, _, finished, _) = native_authority(false, true);
        if let LocalAuthority::Embedded { startup_permit, .. } = &mut authority {
            *startup_permit =
                Some(acquire_startup_permit(&shared, &journal, Duration::from_secs(1)).unwrap());
        }
        let result = warmup_native(
            &shared,
            &clock(None),
            authority,
            &fence(8),
            "secret".into(),
            Duration::from_secs(1),
            Duration::ZERO,
            |_, _| panic!("stale generation must not probe"),
        );
        assert_eq!(result.status, LaunchOutcomeStatus::LaunchFailed);
        assert!(!finished.load(Ordering::Acquire));
        assert!(
            acquire_local_slot(
                &admission_dir(&journal),
                1,
                Some(Duration::from_millis(30)),
                false,
                WorkPriority::default()
            )
            .is_err()
        );
        let mut authority = shared.take_child(&result.managed.unwrap().id).unwrap();
        authority.terminate(Duration::ZERO).unwrap();
        assert!(finished.load(Ordering::Acquire));
        assert!(
            acquire_local_slot(
                &admission_dir(&journal),
                1,
                Some(Duration::from_secs(1)),
                false,
                WorkPriority::default()
            )
            .is_ok()
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn admitted_error_keeps_asr_excluded_through_active_work_and_failed_join() {
        use solstone_core_journal_config::work_priority::WorkPriority;
        use solstone_core_local::admission::{acquire_local_slot, admission_dir};
        let directory = tempfile::tempdir().unwrap();
        let root = admission_dir(directory.path());
        let retirement = Arc::new(embedded::InferenceRetirement::default());
        let held = Arc::new(AtomicBool::new(true));
        let finished = Arc::new(AtomicBool::new(false));
        let stops = Arc::new(AtomicUsize::new(0));
        let mut authority = LocalAuthority::Embedded {
            engine: Box::new(Native {
                generation: 7,
                finished: finished.clone(),
                fail_join: Arc::new(AtomicBool::new(true)),
                joins: Arc::new(AtomicUsize::new(0)),
                held: held.clone(),
                ready: true,
                stops: stops.clone(),
            }),
            startup_permit: None,
            inference_retirement: retirement.clone(),
            generation: 7,
            host_pid: 19,
            _holds: vec![Arc::new(HeldModel(held))],
        };
        let permit = acquire_local_slot(&root, 1, None, false, WorkPriority::default()).unwrap();
        retirement.request(permit);
        let worker_root = root.clone();
        let (sent, entered) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let permit = acquire_local_slot(
                &worker_root,
                1,
                Some(Duration::from_secs(2)),
                false,
                WorkPriority::default(),
            )
            .unwrap();
            sent.send(()).unwrap();
            drop(permit);
        });
        assert_eq!(authority.poll().unwrap(), None);
        assert!(stops.load(Ordering::Acquire) > 0);
        assert!(!finished.load(Ordering::Acquire));
        assert!(entered.recv_timeout(Duration::from_millis(30)).is_err());
        finished.store(true, Ordering::Release);
        assert!(authority.poll().is_err(), "first actual join was refused");
        assert!(entered.recv_timeout(Duration::from_millis(30)).is_err());
        authority.terminate(Duration::ZERO).unwrap();
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert!(!retirement.is_live());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn accepted_retirement_owner_is_attempt_and_credential_bound() {
        use super::super::store::ReadyProcessLookup;
        use solstone_core_local::admission::{acquire_local_slot, admission_dir};
        let root = tempfile::tempdir().unwrap();
        let shared = LocalRuntimeShared::default();
        let first = fence(7);
        let mut second = fence(7);
        second.attempt = 2;
        for (f, token) in [(&first, "first-secret"), (&second, "second-secret")] {
            let (authority, _, _, _) = native_authority(false, true);
            let outcome = warmup_native(
                &shared,
                &clock(None),
                authority,
                f,
                token.into(),
                Duration::from_secs(1),
                Duration::ZERO,
                |_, _| WarmupHealth::Ready,
            );
            assert_eq!(outcome.status, LaunchOutcomeStatus::Ready);
            shared.accept_ready(f);
            if f.attempt == 1 {
                // Capture the exact old authority before publishing a replacement.
                let old = shared.embedded_inference_credentials().unwrap();
                let permit = acquire_local_slot(
                    &admission_dir(root.path()),
                    1,
                    None,
                    false,
                    Default::default(),
                )
                .unwrap();
                old.retirement.request(permit);
            }
        }
        let current = shared.embedded_inference_credentials().unwrap();
        assert_eq!(
            (current.generation, current.port, current.token.as_slice()),
            (7, 1234, b"second-secret".as_slice())
        );
        assert!(current.retirement.is_live());
        let (_, mut old) = shared.take_ready_child(&first).unwrap();
        old.terminate(Duration::ZERO).unwrap();
        assert!(
            current.retirement.is_live(),
            "an old interrupted request cannot stop its replacement"
        );
        let (_, mut new) = shared.take_ready_child(&second).unwrap();
        new.terminate(Duration::ZERO).unwrap();
        assert!(!current.retirement.is_live());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn interrupted_native_generation_reconciles_to_ready_without_changing_brain_choice() {
        use super::super::store::ReadyProcessLookup;
        use super::super::{
            FileRuntimeStore, LifecycleSeam, LocalLifecycleSeam, NoopWorkers,
            ProviderRuntimeCoordinator, ProviderRuntimeNow, ReconcileContext, RuntimeStore,
            StopCleanupStatus, VecEventSink,
        };
        use solstone_core_local::admission::{acquire_local_slot, admission_dir};

        struct Lifecycle {
            actual: LocalLifecycleSeam,
            starts: Vec<ProviderFence>,
        }
        impl LifecycleSeam for Lifecycle {
            fn dispatch_start(&mut self, _: &ProviderRuntimeState, fence: &ProviderFence) {
                self.starts.push(fence.clone());
            }
            fn dispatch_stop(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
                self.actual.dispatch_stop(state, fence);
            }
        }
        fn reconcile(
            coordinator: &ProviderRuntimeCoordinator,
            seconds: f64,
            state: &mut ProviderRuntimeState,
            processes: &mut Vec<ManagedProcess>,
            lifecycle: &mut Lifecycle,
            store: &mut FileRuntimeStore,
        ) {
            coordinator.reconcile(
                ProviderRuntimeNow {
                    monotonic_seconds: seconds,
                },
                state,
                processes,
                &mut ReconcileContext {
                    truth: &mut NoopWorkers,
                    lifecycle,
                    probe: &mut NoopWorkers,
                    store,
                    sink: &mut VecEventSink::default(),
                    gate: None,
                },
            );
        }

        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().canonicalize().unwrap();
        let config = solstone_core_journal_config::get_journal_config_path(&journal);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let choice = br#"{"setup":{"thinking_choice":"local"},"providers":{"active":{"provider":"local","model":"local/qwen3.5-4b"}}}"#;
        std::fs::write(&config, choice).unwrap();
        let shared = Arc::new(LocalRuntimeShared::default());
        let runtime_clock: Arc<dyn RuntimeClock> = Arc::new(clock(None));
        let coordinator = ProviderRuntimeCoordinator::with_incarnation("owner");
        let first = fence(7);
        let (authority, _, finished, _) = native_authority(false, true);
        let initial = warmup_native(
            &shared,
            runtime_clock.as_ref(),
            authority,
            &first,
            "first-token".into(),
            Duration::from_secs(1),
            Duration::ZERO,
            |_, _| WarmupHealth::Ready,
        );
        shared.accept_ready(&first);
        let mut processes = vec![initial.managed.unwrap()];
        let mut state = ProviderRuntimeState::new(ProviderName::Local);
        state.generation = 7;
        state.retry.attempt_count = 1;
        state.desired_fingerprint = first.fingerprint.clone();
        state.has_plan = true;
        state.latest_phase = RuntimePhase::Ready;
        let mut store = FileRuntimeStore::new(
            &journal,
            ProviderName::Local,
            shared.clone(),
            runtime_clock.clone(),
        );
        let mut lifecycle = Lifecycle {
            actual: LocalLifecycleSeam::new(shared.clone(), runtime_clock.clone()),
            starts: Vec::new(),
        };
        let accepted = shared.embedded_inference_credentials().unwrap();
        accepted.retirement.request(
            acquire_local_slot(&admission_dir(&journal), 1, None, false, Default::default())
                .unwrap(),
        );
        assert!(
            shared
                .observe_exited_current_process(&processes, Instant::now())
                .is_none()
        );
        finished.store(true, Ordering::Release);
        let exited = shared
            .observe_exited_current_process(&processes, Instant::now())
            .unwrap();
        coordinator.observe_process_exit(
            ProviderRuntimeNow {
                monotonic_seconds: 10.0,
            },
            &mut state,
            &processes,
            &exited,
            &mut store,
            &mut VecEventSink::default(),
        );
        assert_eq!(state.latest_phase, RuntimePhase::Backoff);
        assert!(!accepted.retirement.is_live());
        assert!(shared.embedded_inference_credentials().is_none());
        reconcile(
            &coordinator,
            10.0,
            &mut state,
            &mut processes,
            &mut lifecycle,
            &mut store,
        );
        let work = state.stop_cleanup.as_mut().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while work.result.is_none() && Instant::now() < deadline {
            work.result = shared.take_stop_cleanup_result(&work.fence);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            work.result.as_ref().unwrap().status,
            StopCleanupStatus::Stopped
        );
        reconcile(
            &coordinator,
            11.0,
            &mut state,
            &mut processes,
            &mut lifecycle,
            &mut store,
        );
        assert!(processes.is_empty());
        assert!(lifecycle.starts.is_empty());
        // Re-observe the same current desired candidate, as LocalTruthSeam does;
        // neither a retry token nor owner Stop/Start is needed within the budget.
        state.truth.as_mut().unwrap().result = Some(super::super::ProviderTruthObservation {
            provider: ProviderName::Local,
            phase: RuntimePhase::Starting,
            reason_code: Some(ReasonCode::known("launch-requested")),
            desired_fingerprint: first.fingerprint.clone(),
            has_plan: true,
            boot_required: false,
            detail: None,
        });
        reconcile(
            &coordinator,
            12.0,
            &mut state,
            &mut processes,
            &mut lifecycle,
            &mut store,
        );
        let replacement = state.start.as_ref().unwrap().fence.clone();
        assert_eq!(
            lifecycle.starts.as_slice(),
            std::slice::from_ref(&replacement)
        );
        assert_eq!((replacement.generation, replacement.attempt), (8, 2));
        let (authority, _, _, _) = native_authority_at(false, true, replacement.generation);
        let ready = warmup_native(
            &shared,
            runtime_clock.as_ref(),
            authority,
            &replacement,
            "replacement-token".into(),
            Duration::from_secs(1),
            Duration::ZERO,
            |_, _| WarmupHealth::Ready,
        );
        assert_eq!(ready.status, LaunchOutcomeStatus::Ready);
        state.start.as_mut().unwrap().result = Some(ready);
        reconcile(
            &coordinator,
            12.1,
            &mut state,
            &mut processes,
            &mut lifecycle,
            &mut store,
        );
        assert_eq!(state.latest_phase, RuntimePhase::Ready);
        assert!(!state.shutdown_requested);
        assert_eq!(state.desired_fingerprint, first.fingerprint);
        assert_eq!(std::fs::read(&config).unwrap(), choice);
        assert!(
            store
                .read_retry_token(ProviderName::Local)
                .unwrap()
                .is_none()
        );
        let current = shared.embedded_inference_credentials().unwrap();
        assert_eq!(
            (current.generation, current.token.as_slice()),
            (8, b"replacement-token".as_slice())
        );
        assert!(current.retirement.is_live());
        assert!(!Arc::ptr_eq(&current.retirement, &accepted.retirement));
        let (_, mut authority) = shared.take_ready_child(&replacement).unwrap();
        authority.terminate(Duration::ZERO).unwrap();
        shared.remove_ready_process(&replacement);
        assert!(shared.reap_finished_workers());
    }

    #[test]
    fn embedded_launch_never_fabricates_an_executable_plan() {
        let launch = LocalLaunchConfig::Embedded {
            common: LocalLaunchCommon {
                desired_fingerprint_json: json!({}),
                desired_fingerprint_sha256: "fingerprint".into(),
                model_id: "local/qwen3.5-4b".into(),
                model_path: "models/model.gguf".into(),
                mmproj_path: Some("models/projector.gguf".into()),
            },
            identity: RuntimeIdentity {
                artifact_key: "aarch64-android".into(),
                abi: 1,
                source_revision: "1".repeat(40),
                runtime_identity: format!("sha256:{}", "2".repeat(64)),
            },
        };
        assert!(
            launch
                .assemble_plan_input(&ProviderRuntimeState::new(ProviderName::Local), 0)
                .is_none()
        );
    }

    struct Clock {
        elapsed_ms: AtomicU64,
        close: Option<Arc<LocalRuntimeShared>>,
    }
    impl RuntimeClock for Clock {
        fn now_utc_rfc3339(&self) -> String {
            "2026-11-08T00:00:00Z".into()
        }
        fn monotonic_seconds(&self) -> f64 {
            self.elapsed_ms.load(Ordering::SeqCst) as f64 / 1000.0
        }
        fn sleep(&self, duration: Duration) {
            self.elapsed_ms
                .fetch_add(duration.as_millis() as u64, Ordering::SeqCst);
            if let Some(shared) = &self.close {
                shared.close_launch_credentials();
            }
        }
    }
    fn clock(close: Option<Arc<LocalRuntimeShared>>) -> Clock {
        Clock {
            elapsed_ms: AtomicU64::new(0),
            close,
        }
    }
    fn fence(generation: u64) -> ProviderFence {
        ProviderFence {
            incarnation: "owner".into(),
            generation,
            fingerprint: Some("fingerprint".into()),
            attempt: 1,
        }
    }

    #[test]
    fn stop_during_native_loading_joins_before_releasing_the_model_lease() {
        let shared = Arc::new(LocalRuntimeShared::default());
        let (authority, held, finished, joins) = native_authority(false, false);
        let outcome = warmup_native(
            &shared,
            &clock(Some(Arc::clone(&shared))),
            authority,
            &fence(7),
            "token".into(),
            Duration::from_secs(1),
            Duration::from_millis(250),
            |_, _| panic!("loading is not ready"),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::NotReady);
        assert!(outcome.managed.is_none());
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(joins.load(Ordering::SeqCst), 1);
        assert!(!held.load(Ordering::SeqCst));
        assert!(!shared.has_owned_authority());
        assert!(shared.launch_credentials().is_none());
    }

    #[test]
    fn native_ready_only_stages_credentials_until_coordinator_admission() {
        use super::super::store::ReadyProcessLookup;
        let shared = LocalRuntimeShared::default();
        let (authority, held, _, _) = native_authority(false, true);
        let fence = fence(7);
        let outcome = warmup_native(
            &shared,
            &clock(None),
            authority,
            &fence,
            "token".into(),
            Duration::from_secs(1),
            Duration::from_millis(250),
            |port, token| {
                assert_eq!(port, 1234);
                assert_eq!(token, "token");
                WarmupHealth::Ready
            },
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::Ready);
        assert!(held.load(Ordering::SeqCst));
        assert!(shared.launch_credentials().is_none());
        shared.accept_ready(&fence);
        assert_eq!(
            shared.launch_credentials(),
            Some((7, 1234, b"token".to_vec()))
        );
        shared.close_launch_credentials();
        assert!(shared.launch_credentials().is_none());
        let (_, mut authority) = shared.take_ready_child(&fence).unwrap();
        authority.terminate(Duration::ZERO).unwrap();
        drop(authority);
        shared.remove_ready_process(&fence);
        assert!(!held.load(Ordering::SeqCst));
    }

    #[test]
    fn wrong_generation_retains_the_actual_native_owner_for_cleanup() {
        let shared = LocalRuntimeShared::default();
        let (authority, held, _, _) = native_authority(false, true);
        let outcome = warmup_native(
            &shared,
            &clock(None),
            authority,
            &fence(8),
            "token".into(),
            Duration::from_secs(1),
            Duration::from_millis(250),
            |_, _| panic!("wrong generation cannot probe or admit"),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::LaunchFailed);
        assert!(shared.launch_credentials().is_none());
        assert!(held.load(Ordering::SeqCst));
        let managed = outcome.managed.unwrap();
        let mut authority = shared.take_child(&managed.id).unwrap();
        authority.terminate(Duration::ZERO).unwrap();
        drop(authority);
        assert!(!held.load(Ordering::SeqCst));
    }

    #[test]
    fn live_properties_must_keep_the_full_context_and_one_slot() {
        assert_eq!(
            classify_properties(
                200,
                r#"{"default_generation_settings":{"n_ctx":32768},"total_slots":1}"#
            ),
            WarmupHealth::Ready
        );
        for body in [
            r#"{"default_generation_settings":{"n_ctx":16384},"total_slots":1}"#,
            r#"{"default_generation_settings":{"n_ctx":32768},"total_slots":2}"#,
            "{}",
        ] {
            assert_eq!(classify_properties(200, body), WarmupHealth::Failed);
        }
        assert_eq!(classify_properties(401, "{}"), WarmupHealth::Failed);
    }
}
