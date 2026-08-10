// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-provider launch planning, port reservation, warmup, and lifecycle work.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io::{Read, Write};
use std::net::TcpListener;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Child, Command};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_brain::{CanonicalInput, canonical_fingerprint};
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_local::install::readiness::{inspect_local, inspect_mlx};
use solstone_core_local::nvidia::{
    ArtifactTrust, CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION, NvidiaProbe, probe_nvidia_gpu,
};
use solstone_core_local::plan::{PlanBackend, PlanInput, Platform, VulkanDevice};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use solstone_core_local::plan::{PlanOutcome, plan};
use solstone_core_local::{ConnectInput, ConnectOutcome, LoopbackAddr, connect};

use crate::process::{SERVICE_SHUTDOWN_TIMEOUT, terminate};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::model::ManagedProcess;
use super::model::{
    LaunchOutcomeStatus, ProviderFence, ProviderLaunchOutcome, ProviderProbeOutcome,
    ProviderRuntimeState, ProviderStopCleanupOutcome, ReasonCode, StopCleanupStatus,
};
use super::seams::{LifecycleSeam, ProbeSeam, TruthObservationSeam};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::store::ReadyProcess;
use super::store::{LocalRuntimeShared, RuntimeClock};

const PLAN_INPUT_SCHEMA: &str = "solstone-local-plan-input-v1";
const MLX_SERVER_PROCESS_NAME: &str = "mlx-vlm-server";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const WARMUP_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub struct ReservedPort {
    listener: Option<TcpListener>,
    port: u16,
}

impl ReservedPort {
    pub fn reserve() -> std::io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener: Some(listener),
            port,
        })
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn release_for_spawn(&mut self) -> u16 {
        drop(self.listener.take());
        self.port
    }
}

#[derive(Debug, Clone)]
pub struct LocalLaunchCommon {
    pub desired_fingerprint_json: Value,
    pub desired_fingerprint_sha256: String,
    pub model_id: String,
    pub model_path: String,
    pub mmproj_path: Option<String>,
}

#[derive(Debug, Clone)]
pub enum LocalLaunchConfig {
    Cuda {
        common: LocalLaunchCommon,
        binary_path: Option<String>,
        lib_dir: Option<String>,
        nvidia_probe: NvidiaProbe,
        cuda_embedded_arch_set: Vec<String>,
        cuda_min_driver_version: u32,
        cuda_artifact_trust: ArtifactTrust,
        cuda_persisted_installed_cuda_target: bool,
    },
    Vulkan {
        common: LocalLaunchCommon,
        binary_path: Option<String>,
        devices: Vec<VulkanDevice>,
        selected_gpu_index: u32,
        selected_gpu_name: String,
        selected_vram_mib: u64,
        vram_before_mib: Option<u64>,
    },
    Mlx {
        common: LocalLaunchCommon,
        runtime_dir: Option<String>,
        interpreter_path: Option<String>,
    },
}

impl LocalLaunchConfig {
    fn common(&self) -> &LocalLaunchCommon {
        match self {
            Self::Cuda { common, .. } | Self::Vulkan { common, .. } | Self::Mlx { common, .. } => {
                common
            }
        }
    }

    fn default_model_id(&self) -> String {
        self.common().model_id.clone()
    }

    fn platform(&self) -> Platform {
        match self {
            Self::Mlx { .. } => Platform::Darwin,
            Self::Cuda { .. } | Self::Vulkan { .. } => Platform::Linux,
        }
    }

    pub fn assemble_plan_input(&self, state: &ProviderRuntimeState, port: u16) -> PlanInput {
        let common = self.common();
        let desired_fingerprint_sha256 = state
            .desired_fingerprint
            .clone()
            .unwrap_or_else(|| common.desired_fingerprint_sha256.clone());
        match self {
            Self::Cuda {
                common,
                binary_path,
                lib_dir,
                nvidia_probe,
                cuda_embedded_arch_set,
                cuda_min_driver_version,
                cuda_artifact_trust,
                cuda_persisted_installed_cuda_target,
            } => PlanInput {
                schema: PLAN_INPUT_SCHEMA.into(),
                platform: Platform::Linux,
                backend_override: Some(PlanBackend::Cuda),
                bind_address: LoopbackAddr::IPV4_LOOPBACK,
                port,
                desired_fingerprint_json: common.desired_fingerprint_json.clone(),
                desired_fingerprint_sha256,
                model_id: common.model_id.clone(),
                model_path: common.model_path.clone(),
                mmproj_path: common.mmproj_path.clone(),
                runtime_dir: None,
                mlx_interpreter_path: None,
                cuda_binary_path: binary_path.clone(),
                vulkan_binary_path: None,
                lib_dir: lib_dir.clone(),
                inherited_ld_library_path: std::env::var("LD_LIBRARY_PATH").ok(),
                nvidia_probe: Some(nvidia_probe.clone()),
                cuda_embedded_arch_set: cuda_embedded_arch_set.clone(),
                cuda_min_driver_version: Some(*cuda_min_driver_version),
                cuda_artifact_trust: Some(*cuda_artifact_trust),
                cuda_persisted_installed_cuda_target: Some(*cuda_persisted_installed_cuda_target),
                vulkan_devices: None,
                vulkan_selected_gpu_index: None,
                vulkan_selected_gpu_name: None,
                vulkan_selected_vram_mib: None,
                vram_before_mib: None,
            },
            Self::Vulkan {
                common,
                binary_path,
                devices,
                selected_gpu_index,
                selected_gpu_name,
                selected_vram_mib,
                vram_before_mib,
            } => PlanInput {
                schema: PLAN_INPUT_SCHEMA.into(),
                platform: Platform::Linux,
                backend_override: Some(PlanBackend::Vulkan),
                bind_address: LoopbackAddr::IPV4_LOOPBACK,
                port,
                desired_fingerprint_json: common.desired_fingerprint_json.clone(),
                desired_fingerprint_sha256,
                model_id: common.model_id.clone(),
                model_path: common.model_path.clone(),
                mmproj_path: common.mmproj_path.clone(),
                runtime_dir: None,
                mlx_interpreter_path: None,
                cuda_binary_path: None,
                vulkan_binary_path: binary_path.clone(),
                lib_dir: None,
                inherited_ld_library_path: None,
                nvidia_probe: None,
                cuda_embedded_arch_set: Vec::new(),
                cuda_min_driver_version: None,
                cuda_artifact_trust: None,
                cuda_persisted_installed_cuda_target: None,
                vulkan_devices: Some(devices.clone()),
                vulkan_selected_gpu_index: Some(*selected_gpu_index),
                vulkan_selected_gpu_name: Some(selected_gpu_name.clone()),
                vulkan_selected_vram_mib: Some(*selected_vram_mib),
                vram_before_mib: *vram_before_mib,
            },
            Self::Mlx {
                common,
                runtime_dir,
                interpreter_path,
            } => PlanInput {
                schema: PLAN_INPUT_SCHEMA.into(),
                platform: Platform::Darwin,
                backend_override: Some(PlanBackend::Mlx),
                bind_address: LoopbackAddr::IPV4_LOOPBACK,
                port,
                desired_fingerprint_json: common.desired_fingerprint_json.clone(),
                desired_fingerprint_sha256,
                model_id: common.model_id.clone(),
                model_path: common.model_path.clone(),
                mmproj_path: common.mmproj_path.clone(),
                runtime_dir: runtime_dir.clone(),
                mlx_interpreter_path: interpreter_path.clone(),
                cuda_binary_path: None,
                vulkan_binary_path: None,
                lib_dir: None,
                inherited_ld_library_path: None,
                nvidia_probe: None,
                cuda_embedded_arch_set: Vec::new(),
                cuda_min_driver_version: None,
                cuda_artifact_trust: None,
                cuda_persisted_installed_cuda_target: None,
                vulkan_devices: None,
                vulkan_selected_gpu_index: None,
                vulkan_selected_gpu_name: None,
                vulkan_selected_vram_mib: None,
                vram_before_mib: None,
            },
        }
    }
}

pub struct LocalLifecycleSeam {
    shared: Arc<LocalRuntimeShared>,
    clock: Arc<dyn RuntimeClock>,
    warmup_timeout: Duration,
    warmup_poll_interval: Duration,
    termination_timeout: Duration,
}

impl LocalLifecycleSeam {
    pub fn new(shared: Arc<LocalRuntimeShared>, clock: Arc<dyn RuntimeClock>) -> Self {
        Self::with_timeouts(
            shared,
            clock,
            Duration::from_secs(120),
            Duration::from_millis(250),
            SERVICE_SHUTDOWN_TIMEOUT,
        )
    }

    pub fn with_timeouts(
        shared: Arc<LocalRuntimeShared>,
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

impl LifecycleSeam for LocalLifecycleSeam {
    fn dispatch_start(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let clock = Arc::clone(&self.clock);
        let launch = shared.launch_request_for(&state.desired_fingerprint);
        let state = state.clone();
        let fence = fence.clone();
        let warmup_timeout = self.warmup_timeout;
        let warmup_poll_interval = self.warmup_poll_interval;
        thread::spawn(move || {
            let outcome = launch
                .map(|launch| {
                    start_local(
                        &shared,
                        clock.as_ref(),
                        &launch,
                        &state,
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
            let outcome = stop_local(
                &shared,
                request.as_ref(),
                stop_cancelled,
                termination_timeout,
            );
            shared.record_stop_cleanup_result(&fence, outcome);
        });
    }
}

pub struct LocalProbeSeam {
    shared: Arc<LocalRuntimeShared>,
    journal_path: PathBuf,
}

impl LocalProbeSeam {
    pub fn new(shared: Arc<LocalRuntimeShared>, journal_path: impl Into<PathBuf>) -> Self {
        Self {
            shared,
            journal_path: journal_path.into(),
        }
    }
}

impl ProbeSeam for LocalProbeSeam {
    fn dispatch_probe(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let journal_path = self.journal_path.clone();
        let fence = fence.clone();
        let launch = shared.launch_request_for(&state.desired_fingerprint);
        thread::spawn(move || {
            let outcome = match launch {
                Some(launch) => probe_local(&journal_path, &launch),
                None => probe_unavailable(),
            };
            shared.record_probe_result(&fence, outcome);
        });
    }
}

fn probe_local(journal_path: &std::path::Path, launch: &LocalLaunchConfig) -> ProviderProbeOutcome {
    let outcome = connect(ConnectInput {
        schema: "solstone-local-connect-input-v1".into(),
        journal_path: journal_path.display().to_string(),
        bind_address: LoopbackAddr::IPV4_LOOPBACK,
        default_model_id: launch.default_model_id(),
        platform: launch.platform(),
    });
    match outcome {
        ConnectOutcome::Ready { .. } => ProviderProbeOutcome {
            status: super::model::ProbeStatus::Ready,
            reason_code: ReasonCode::known("probe-ready"),
        },
        ConnectOutcome::Loading { .. } => ProviderProbeOutcome {
            status: super::model::ProbeStatus::NotReady,
            reason_code: ReasonCode::known("probe-not-ready"),
        },
        ConnectOutcome::NotReady { .. } | ConnectOutcome::Failed { .. } => probe_unavailable(),
    }
}

fn probe_unavailable() -> ProviderProbeOutcome {
    ProviderProbeOutcome {
        status: super::model::ProbeStatus::Unavailable,
        reason_code: ReasonCode::known("proof-observation-unavailable"),
    }
}

#[derive(Clone)]
pub struct LocalTruthConfig {
    pub journal_path: PathBuf,
    pub platform: Platform,
    pub nvidia_probe: Option<NvidiaProbe>,
    pub vulkan_devices: Vec<VulkanDevice>,
}

pub struct LocalTruthSeam {
    shared: Arc<LocalRuntimeShared>,
    config: LocalTruthConfig,
}

impl LocalTruthSeam {
    pub fn new(shared: Arc<LocalRuntimeShared>, journal_path: impl Into<PathBuf>) -> Self {
        Self::with_config(
            shared,
            LocalTruthConfig {
                journal_path: journal_path.into(),
                platform: if cfg!(target_os = "macos") {
                    Platform::Darwin
                } else {
                    Platform::Linux
                },
                nvidia_probe: None,
                vulkan_devices: Vec::new(),
            },
        )
    }

    pub fn with_config(shared: Arc<LocalRuntimeShared>, config: LocalTruthConfig) -> Self {
        Self { shared, config }
    }
}

impl TruthObservationSeam for LocalTruthSeam {
    fn dispatch_truth(&mut self, _: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let config = self.config.clone();
        let fence = fence.clone();
        thread::spawn(move || {
            let outcome = observe_truth(&shared, &config);
            shared.record_truth_result(&fence, outcome);
        });
    }
}

fn observe_truth(
    shared: &LocalRuntimeShared,
    config: &LocalTruthConfig,
) -> super::model::ProviderTruthObservation {
    if !config.journal_path.is_dir() {
        return truth_unavailable();
    }
    let journal_config =
        match solstone_core_journal_config::read_journal_config(&config.journal_path) {
            Ok(read) => read.config.unwrap_or_default(),
            Err(_) => return truth_unavailable(),
        };
    if matches!(
        resolve_local_endpoint(&journal_config),
        LocalEndpointResolution::Byo(_)
    ) {
        return truth(
            super::model::RuntimePhase::NotDesired,
            "provider-not-needed",
            None,
            false,
            false,
        );
    }
    let model_id = journal_config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active"))
        .and_then(Value::as_object)
        .and_then(|active| active.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or(if config.platform == Platform::Darwin {
            "qwen3.5:9b"
        } else {
            "local/qwen3.5-4b"
        })
        .to_owned();
    let probe = config.nvidia_probe.clone().unwrap_or_else(probe_nvidia_gpu);
    let readiness = match config.platform {
        Platform::Linux => inspect_local(Map::from_iter([
            (
                "journal".into(),
                Value::String(config.journal_path.display().to_string()),
            ),
            ("model_id".into(), Value::String(model_id.clone())),
            (
                "nvidia_probe".into(),
                serde_json::to_value(&probe).expect("NvidiaProbe serialization"),
            ),
        ])),
        Platform::Darwin => inspect_mlx(Map::from_iter([
            (
                "journal".into(),
                Value::String(config.journal_path.display().to_string()),
            ),
            ("model_id".into(), Value::String(model_id.clone())),
            ("platform_supported".into(), Value::Bool(true)),
            ("mlx_vlm_importable".into(), Value::Bool(true)),
        ])),
    };
    let Some(object) = readiness.as_object() else {
        return truth_unavailable();
    };
    if object
        .get("install")
        .and_then(Value::as_object)
        .and_then(|install| install.get("install_state"))
        .and_then(Value::as_str)
        .is_some_and(|state| {
            matches!(
                state,
                "resolving" | "downloading" | "verifying" | "installing"
            )
        })
    {
        return truth(
            super::model::RuntimePhase::ArtifactNotReady,
            "install-in-progress",
            None,
            false,
            false,
        );
    }
    if object.get("ready").and_then(Value::as_bool) != Some(true) {
        let reason = object
            .get("reason_code")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (phase, code) = match reason {
            "platform_unsupported" | "unsupported_platform" => (
                super::model::RuntimePhase::HostBlocked,
                "platform-unsupported",
            ),
            "package_unavailable" => (
                super::model::RuntimePhase::HostBlocked,
                "package-unavailable",
            ),
            "manifest_pin_mismatch" | "sha256_mismatch" | "inventory_member_missing" => (
                super::model::RuntimePhase::ArtifactNotReady,
                "artifact-stale",
            ),
            _ if object.get("status").and_then(Value::as_str) == Some("proof-unavailable") => (
                super::model::RuntimePhase::ArtifactNotReady,
                "artifact-proof-failed",
            ),
            _ => (
                super::model::RuntimePhase::ArtifactNotReady,
                "artifact-missing",
            ),
        };
        return truth(phase, code, None, false, false);
    }
    let artifacts = object.get("artifacts").and_then(Value::as_object);
    let Some(model_path) = artifacts
        .and_then(|artifacts| {
            artifacts
                .get("model_path")
                .or_else(|| artifacts.get("snapshot_dir"))
        })
        .and_then(Value::as_str)
    else {
        return truth_unavailable();
    };
    let backend = object
        .get("host")
        .and_then(Value::as_object)
        .and_then(|host| host.get("backend"))
        .and_then(Value::as_str)
        .unwrap_or("mlx");
    let desired =
        json!({"provider":"local","backend":backend,"model_id":model_id,"model_path":model_path});
    let Ok(fingerprint) = canonical_fingerprint(&CanonicalInput::Json(desired.clone())) else {
        return truth_unavailable();
    };
    let common = LocalLaunchCommon {
        desired_fingerprint_json: desired,
        desired_fingerprint_sha256: fingerprint.clone(),
        model_id,
        model_path: model_path.into(),
        mmproj_path: None,
    };
    let launch = match (config.platform, backend) {
        (Platform::Darwin, _) => LocalLaunchConfig::Mlx {
            common,
            runtime_dir: Some(model_path.to_owned()),
            interpreter_path: std::env::current_exe().ok().map(|path| {
                path.with_file_name(MLX_SERVER_PROCESS_NAME)
                    .display()
                    .to_string()
            }),
        },
        (Platform::Linux, "cuda") => LocalLaunchConfig::Cuda {
            common,
            binary_path: artifacts
                .and_then(|artifacts| artifacts.get("binary_path"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            lib_dir: None,
            nvidia_probe: probe,
            cuda_embedded_arch_set: CUDA_EMBEDDED_ARCH_SET
                .iter()
                .map(|value| (*value).into())
                .collect(),
            cuda_min_driver_version: CUDA_MIN_DRIVER_VERSION,
            cuda_artifact_trust: ArtifactTrust::Trusted,
            cuda_persisted_installed_cuda_target: false,
        },
        (Platform::Linux, "vulkan") => {
            let Some(device) = config.vulkan_devices.first().cloned() else {
                return truth(
                    super::model::RuntimePhase::HostBlocked,
                    "gpu-unavailable",
                    None,
                    false,
                    false,
                );
            };
            LocalLaunchConfig::Vulkan {
                common,
                binary_path: artifacts
                    .and_then(|artifacts| artifacts.get("binary_path"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                devices: config.vulkan_devices.clone(),
                selected_gpu_index: device.index,
                selected_gpu_name: device.name,
                selected_vram_mib: device.vram_mib,
                vram_before_mib: None,
            }
        }
        _ => {
            return truth(
                super::model::RuntimePhase::HostBlocked,
                "gpu-unavailable",
                None,
                false,
                false,
            );
        }
    };
    shared.record_launch_request(Some(fingerprint.clone()), launch);
    truth(
        super::model::RuntimePhase::Starting,
        "launch-requested",
        Some(fingerprint),
        true,
        true,
    )
}

fn truth(
    phase: super::model::RuntimePhase,
    code: &'static str,
    desired_fingerprint: Option<String>,
    has_plan: bool,
    boot_required: bool,
) -> super::model::ProviderTruthObservation {
    super::model::ProviderTruthObservation {
        provider: super::model::ProviderName::Local,
        phase,
        reason_code: Some(ReasonCode::known(code)),
        desired_fingerprint,
        has_plan,
        boot_required,
        detail: None,
    }
}

fn truth_unavailable() -> super::model::ProviderTruthObservation {
    truth(
        super::model::RuntimePhase::StateUnavailable,
        "truth-observation-failed",
        None,
        false,
        true,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn start_local(
    shared: &LocalRuntimeShared,
    clock: &dyn RuntimeClock,
    launch: &LocalLaunchConfig,
    state: &ProviderRuntimeState,
    fence: &ProviderFence,
    warmup_timeout: Duration,
    warmup_poll_interval: Duration,
) -> ProviderLaunchOutcome {
    let mut reservation = match ReservedPort::reserve() {
        Ok(reservation) => reservation,
        Err(_) => return launch_failed(),
    };
    let input = launch.assemble_plan_input(state, reservation.port());
    let plan = match plan(input) {
        PlanOutcome::Launch(plan) => plan,
        PlanOutcome::Rejected { .. } => return launch_failed(),
    };
    let port = reservation.release_for_spawn();
    let mut child = match spawn_plan(&plan) {
        Ok(child) => child,
        Err(_) => return launch_failed(),
    };
    let process_id = format!("local:{}", child.id());
    let pid = child.id();
    let deadline = clock.monotonic_seconds() + warmup_timeout.as_secs_f64();
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Exited,
                reason_code: ReasonCode::known("process-exited"),
                managed: None,
            };
        }
        if warmup_health_probe(port) == WarmupHealth::Ready {
            let managed = ManagedProcess {
                id: process_id.clone(),
                name: "local".into(),
                running: true,
            };
            shared.retain_child(process_id.clone(), child);
            shared.record_ready_process(
                fence,
                ReadyProcess {
                    process_id,
                    process_name: "local".into(),
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
        if clock.monotonic_seconds() >= deadline {
            let managed = ManagedProcess {
                id: process_id.clone(),
                name: "local".into(),
                running: true,
            };
            shared.retain_child(process_id, child);
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::WarmupTimeout,
                reason_code: ReasonCode::known("warmup-timeout"),
                managed: Some(managed),
            };
        }
        clock.sleep(warmup_poll_interval);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn start_local(
    _: &LocalRuntimeShared,
    _: &dyn RuntimeClock,
    _: &LocalLaunchConfig,
    _: &ProviderRuntimeState,
    _: &ProviderFence,
    _: Duration,
    _: Duration,
) -> ProviderLaunchOutcome {
    launch_failed()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn spawn_plan(plan: &solstone_core_local::plan::LaunchPlan) -> std::io::Result<Child> {
    let (program, arguments) = plan.argv.split_first().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local launch plan has no argv",
        )
    })?;
    Command::new(program)
        .args(arguments)
        .envs(&plan.extra_env)
        .spawn()
}

fn launch_failed() -> ProviderLaunchOutcome {
    ProviderLaunchOutcome {
        status: LaunchOutcomeStatus::LaunchFailed,
        reason_code: ReasonCode::known("launch-failed"),
        managed: None,
    }
}

fn stop_local(
    shared: &LocalRuntimeShared,
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WarmupHealth {
    Ready,
    Loading,
    Failed,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn warmup_health_probe(port: u16) -> WarmupHealth {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&address, WARMUP_PROBE_TIMEOUT) {
        Ok(stream) => stream,
        Err(_) => return WarmupHealth::Failed,
    };
    if stream.set_read_timeout(Some(WARMUP_PROBE_TIMEOUT)).is_err()
        || stream
            .set_write_timeout(Some(WARMUP_PROBE_TIMEOUT))
            .is_err()
        || stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .is_err()
    {
        return WarmupHealth::Failed;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return WarmupHealth::Failed;
    }
    let Some((status_line, body)) = response.split_once("\r\n") else {
        return WarmupHealth::Failed;
    };
    if status_line.split_whitespace().nth(1) == Some("200") {
        return WarmupHealth::Ready;
    }
    if status_line.split_whitespace().nth(1) == Some("503")
        && body.to_ascii_lowercase().contains("loading model")
    {
        return WarmupHealth::Loading;
    }
    WarmupHealth::Failed
}
