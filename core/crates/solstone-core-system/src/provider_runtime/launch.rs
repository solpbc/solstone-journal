// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local-provider launch planning, port reservation, warmup, and lifecycle work.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Child, Command};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use solstone_core_brain::bundled_runtime_desired_fingerprint;
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_local::install::{
    metal_candidate, pins,
    readiness::{inspect_local, inspect_local_present, inspect_local_present_with_package},
};
use solstone_core_local::nvidia::{
    ArtifactTrust, CUDA_EMBEDDED_ARCH_SET, CUDA_MIN_DRIVER_VERSION, NvidiaProbe, probe_nvidia_gpu,
};
use solstone_core_local::plan::{PlanBackend, PlanInput, Platform, VulkanDevice};
use solstone_core_local::plan::{PlanOutcome, plan};
use solstone_core_local::{ConnectInput, ConnectOutcome, LoopbackAddr, connect};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::process::apply_parent_death_kill;
use crate::process::{Disposition, LaunchError, SERVICE_SHUTDOWN_TIMEOUT};

use super::local_follow::{LocalFollow, LocalInstallerLauncher};
use super::model::ManagedProcess;
use super::model::{
    LaunchOutcomeStatus, ProviderFence, ProviderLaunchOutcome, ProviderProbeOutcome,
    ProviderRuntimeState, ProviderStopCleanupOutcome, ReasonCode, StopCleanupStatus,
};
use super::seams::{LifecycleSeam, ProbeSeam, TruthObservationSeam};
use super::store::ReadyProcess;
use super::store::{LocalRuntimeShared, RuntimeClock};

const PLAN_INPUT_SCHEMA: &str = "solstone-local-plan-input-v1";
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
        platform: Platform,
    },
    Metal {
        common: LocalLaunchCommon,
        binary_path: Option<String>,
        unified_memory_mib: Option<u64>,
    },
}

impl LocalLaunchConfig {
    fn common(&self) -> &LocalLaunchCommon {
        match self {
            Self::Cuda { common, .. }
            | Self::Vulkan { common, .. }
            | Self::Metal { common, .. } => common,
        }
    }

    fn default_model_id(&self) -> String {
        self.common().model_id.clone()
    }

    fn platform(&self) -> Platform {
        match self {
            Self::Metal { .. } => Platform::Darwin,
            Self::Cuda { .. } => Platform::Linux,
            Self::Vulkan { platform, .. } => *platform,
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
                cuda_binary_path: binary_path.clone(),
                vulkan_binary_path: None,
                metal_binary_path: None,
                metal_unified_memory_mib: None,
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
                vulkan_probe_ok: None,
            },
            Self::Vulkan {
                common,
                binary_path,
                devices,
                selected_gpu_index,
                selected_gpu_name,
                selected_vram_mib,
                vram_before_mib,
                platform,
            } => PlanInput {
                schema: PLAN_INPUT_SCHEMA.into(),
                platform: *platform,
                backend_override: Some(PlanBackend::Vulkan),
                bind_address: LoopbackAddr::IPV4_LOOPBACK,
                port,
                desired_fingerprint_json: common.desired_fingerprint_json.clone(),
                desired_fingerprint_sha256,
                model_id: common.model_id.clone(),
                model_path: common.model_path.clone(),
                mmproj_path: common.mmproj_path.clone(),
                cuda_binary_path: None,
                vulkan_binary_path: binary_path.clone(),
                metal_binary_path: None,
                metal_unified_memory_mib: None,
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
                vulkan_probe_ok: if *platform == Platform::Windows {
                    Some(true)
                } else {
                    None
                },
            },
            Self::Metal {
                common,
                binary_path,
                unified_memory_mib,
            } => PlanInput {
                schema: PLAN_INPUT_SCHEMA.into(),
                platform: Platform::Darwin,
                backend_override: Some(PlanBackend::Metal),
                bind_address: LoopbackAddr::IPV4_LOOPBACK,
                port,
                desired_fingerprint_json: common.desired_fingerprint_json.clone(),
                desired_fingerprint_sha256,
                model_id: common.model_id.clone(),
                model_path: common.model_path.clone(),
                mmproj_path: common.mmproj_path.clone(),
                cuda_binary_path: None,
                vulkan_binary_path: None,
                metal_binary_path: binary_path.clone(),
                metal_unified_memory_mib: *unified_memory_mib,
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
                vulkan_probe_ok: None,
            },
        }
    }
}

pub struct LocalLifecycleSeam {
    shared: Arc<LocalRuntimeShared>,
    clock: Arc<dyn RuntimeClock>,
    journal_path: Option<PathBuf>,
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
            journal_path: std::env::var_os("SOLSTONE_JOURNAL").map(PathBuf::from),
            warmup_timeout,
            warmup_poll_interval,
            termination_timeout,
        }
    }

    pub fn with_journal(mut self, journal_path: impl Into<PathBuf>) -> Self {
        self.journal_path = Some(journal_path.into());
        self
    }
}

impl LifecycleSeam for LocalLifecycleSeam {
    fn dispatch_start(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let clock = Arc::clone(&self.clock);
        let launch = shared.launch_request_for(&state.desired_fingerprint);
        let state = state.clone();
        let fence = fence.clone();
        let journal_path = self.journal_path.clone();
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
                        journal_path.as_deref(),
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
        if !stop_cancelled
            && let Some(fence) = request.as_ref().and_then(|r| r.managed.fence.as_ref())
        {
            shared.revoke_launch_credentials(fence);
        }
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

    /// Run the same local health probe synchronously for an immediate supervisor decision.
    pub fn probe_now(&self, state: &ProviderRuntimeState) -> ProviderProbeOutcome {
        self.shared
            .launch_request_for(&state.desired_fingerprint)
            .map(|launch| probe_local(&self.journal_path, &launch, &self.shared))
            .unwrap_or_else(probe_unavailable)
    }
}

impl ProbeSeam for LocalProbeSeam {
    fn dispatch_probe(&mut self, state: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let journal_path = self.journal_path.clone();
        let fence = fence.clone();
        let launch = shared.launch_request_for(&state.desired_fingerprint);
        thread::spawn(move || {
            let outcome = launch
                .map(|launch| probe_local(&journal_path, &launch, &shared))
                .unwrap_or_else(probe_unavailable);
            shared.record_probe_result(&fence, outcome);
        });
    }
}

fn probe_local(
    journal_path: &std::path::Path,
    launch: &LocalLaunchConfig,
    shared: &LocalRuntimeShared,
) -> ProviderProbeOutcome {
    let input = ConnectInput {
        schema: "solstone-local-connect-input-v1".into(),
        journal_path: journal_path.display().to_string(),
        bind_address: LoopbackAddr::IPV4_LOOPBACK,
        default_model_id: launch.default_model_id(),
        platform: launch.platform(),
    };
    let outcome = if let Some((generation, port, token)) = shared.probe_launch_credentials() {
        let token_str = String::from_utf8(token).unwrap_or_default();
        let mut auth =
            solstone_core_local::LocalInferenceAuthority::new(generation, port, token_str, None);
        solstone_core_local::connect_with_authority(input, Some(&mut auth))
    } else {
        connect(input)
    };
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalHost {
    Linux,
    Darwin,
    Windows,
}

#[derive(Clone)]
pub struct LocalTruthConfig {
    pub journal_path: PathBuf,
    pub platform: LocalHost,
    pub arch: &'static str,
    pub nvidia_probe: Option<NvidiaProbe>,
    pub vulkan: crate::vulkan_observe::VulkanObservation,
    pub windows_package: Option<solstone_core_local::install::windows_engine::WindowsLlamaPackage>,
}

pub struct LocalTruthSeam {
    shared: Arc<LocalRuntimeShared>,
    config: LocalTruthConfig,
    follow: Option<Arc<LocalFollow>>,
}

impl LocalTruthSeam {
    pub fn new(shared: Arc<LocalRuntimeShared>, journal_path: impl Into<PathBuf>) -> Self {
        Self::with_config(
            shared,
            LocalTruthConfig {
                journal_path: journal_path.into(),
                platform: if cfg!(windows) {
                    LocalHost::Windows
                } else if cfg!(target_os = "macos") {
                    LocalHost::Darwin
                } else {
                    LocalHost::Linux
                },
                arch: std::env::consts::ARCH,
                nvidia_probe: None,
                vulkan: crate::vulkan_observe::observe_vulkan_devices(),
                windows_package: None,
            },
        )
    }

    pub fn with_config(shared: Arc<LocalRuntimeShared>, config: LocalTruthConfig) -> Self {
        Self {
            shared,
            config,
            follow: None,
        }
    }

    /// Start the owner's own installer when a release has moved the pins of a
    /// local provider the owner already installed (see `local_follow`).
    /// Linux and macOS launch the installer process. Windows posts to this
    /// journal's convey Thinking installer. The shared follow rule is the same.
    #[must_use]
    pub fn with_installer(mut self, launcher: LocalInstallerLauncher) -> Self {
        self.follow = Some(Arc::new(LocalFollow::new(launcher)));
        self
    }
}

impl TruthObservationSeam for LocalTruthSeam {
    fn dispatch_truth(&mut self, _: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let config = self.config.clone();
        let follow = self.follow.clone();
        let fence = fence.clone();
        thread::spawn(move || {
            let outcome = observe_truth(&shared, &config, follow.as_deref());
            shared.record_truth_result(&fence, outcome);
        });
    }
}

fn observe_truth(
    shared: &LocalRuntimeShared,
    config: &LocalTruthConfig,
    follow: Option<&LocalFollow>,
) -> super::model::ProviderTruthObservation {
    if !config.journal_path.is_dir() {
        return truth_unavailable();
    }
    let (journal_config, config_present) =
        match solstone_core_journal_config::read_journal_config(&config.journal_path) {
            Ok(read) => {
                let present = read.config.is_some();
                (read.config.unwrap_or_default(), present)
            }
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
    let configured_model_id = journal_config
        .get("providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get("active"))
        .and_then(Value::as_object)
        .and_then(|active| active.get("model"))
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or("local/qwen3.5-4b")
        .to_owned();
    // Bundled macOS inference has one shipped model. Read journals written by
    // the retired MLX runtime, but never let their old selection relabel the
    // native 4B artifacts or desired fingerprint.
    let model_id = if config.platform == LocalHost::Darwin {
        "local/qwen3.5-4b".to_owned()
    } else if pins::model_identity(&configured_model_id).is_some() {
        configured_model_id
    } else {
        "local/qwen3.5-4b".to_owned()
    };
    if config.platform == LocalHost::Windows {
        if config.arch != "x86_64" {
            return truth(
                super::model::RuntimePhase::HostBlocked,
                "platform-unsupported",
                None,
                false,
                false,
            );
        }
        match solstone_core_local::gpu_device_local_verdict(
            config.vulkan.succeeded,
            &config.vulkan.devices,
            None,
        ) {
            solstone_core_local::GpuDeviceLocalVerdict::Unknown => {
                return truth(
                    super::model::RuntimePhase::HostBlocked,
                    "gpu-probe-failed",
                    None,
                    false,
                    false,
                );
            }
            solstone_core_local::GpuDeviceLocalVerdict::NoHardwareDevice => {
                return truth(
                    super::model::RuntimePhase::HostBlocked,
                    "gpu-unavailable",
                    None,
                    false,
                    false,
                );
            }
            solstone_core_local::GpuDeviceLocalVerdict::BelowBar { .. } => {
                return truth(
                    super::model::RuntimePhase::HostBlocked,
                    "gpu-memory-insufficient",
                    None,
                    false,
                    false,
                );
            }
            solstone_core_local::GpuDeviceLocalVerdict::Eligible { .. } => {}
        }
        let device = solstone_core_local::select_device(&config.vulkan.devices, None)
            .expect("eligible verdict guarantees selected hardware device");
        let pkg = match config.windows_package.clone().map(Ok).unwrap_or_else(
            solstone_core_local::install::windows_engine::verified_windows_llama_package,
        ) {
            Ok(pkg) => pkg,
            Err(
                solstone_core_local::install::windows_engine::WindowsLlamaPackageError::Missing(_),
            ) => {
                return truth(
                    super::model::RuntimePhase::HostBlocked,
                    "package-unavailable",
                    None,
                    false,
                    false,
                );
            }
            Err(
                solstone_core_local::install::windows_engine::WindowsLlamaPackageError::Invalid(_),
            ) => {
                return truth(
                    super::model::RuntimePhase::HostBlocked,
                    "package-invalid",
                    None,
                    false,
                    false,
                );
            }
        };
        let readiness_input = Map::from_iter([
            (
                "journal".into(),
                Value::String(config.journal_path.display().to_string()),
            ),
            ("model_id".into(), Value::String(model_id.clone())),
            ("backend".into(), Value::String("vulkan".into())),
            (
                "artifact_key".into(),
                Value::String("x86_64-windows".into()),
            ),
        ]);
        let readiness = if let Some(pkg) = config.windows_package.clone() {
            inspect_local_present_with_package(readiness_input, Some(pkg))
        } else {
            inspect_local_present(readiness_input)
        };
        if let Some(follow) = follow {
            follow.observe(
                &config.journal_path,
                config_present.then_some(&journal_config),
                &readiness,
            );
        }
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
                "package_invalid" => (super::model::RuntimePhase::HostBlocked, "package-invalid"),
                "manifest_pin_mismatch"
                | "sha256_mismatch"
                | "inventory_member_missing"
                | "inventory_size_mismatch" => (
                    super::model::RuntimePhase::ArtifactNotReady,
                    "artifact-stale",
                ),
                "manifest_missing" => (
                    super::model::RuntimePhase::ArtifactNotReady,
                    "manifest-missing",
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
            .and_then(|artifacts| artifacts.get("model_path"))
            .and_then(Value::as_str)
        else {
            return truth_unavailable();
        };
        let projector_path = artifacts
            .and_then(|artifacts| artifacts.get("projector_path"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let target_fingerprint = object
            .get("target")
            .and_then(|target| target.get("target_fingerprint_sha256"))
            .and_then(Value::as_str);
        let Some(target_fingerprint_sha256) = target_fingerprint else {
            return truth_unavailable();
        };
        let Ok(desired) = bundled_runtime_desired_fingerprint(
            "vulkan",
            &model_id,
            target_fingerprint_sha256,
            Some(&pkg.engine.display().to_string()),
            model_path,
            projector_path.as_deref(),
        ) else {
            return truth_unavailable();
        };
        let fingerprint = desired.sha256.clone();
        let common = LocalLaunchCommon {
            desired_fingerprint_json: desired.json,
            desired_fingerprint_sha256: fingerprint.clone(),
            model_id,
            model_path: model_path.into(),
            mmproj_path: projector_path,
        };
        let launch = LocalLaunchConfig::Vulkan {
            common,
            binary_path: Some(pkg.engine.display().to_string()),
            devices: config.vulkan.devices.clone(),
            selected_gpu_index: device.index,
            selected_gpu_name: device.name,
            selected_vram_mib: device.vram_mib,
            vram_before_mib: None,
            platform: Platform::Windows,
        };
        shared.record_launch_request(Some(fingerprint.clone()), launch);
        return truth(
            super::model::RuntimePhase::Starting,
            "launch-requested",
            Some(fingerprint),
            true,
            true,
        );
    }
    let probe = config.nvidia_probe.clone().unwrap_or_else(probe_nvidia_gpu);
    let readiness = match config.platform {
        LocalHost::Linux => inspect_local_present(Map::from_iter([
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
        LocalHost::Darwin => {
            let input = Map::from_iter([
                (
                    "journal".into(),
                    Value::String(config.journal_path.display().to_string()),
                ),
                ("model_id".into(), Value::String(model_id.clone())),
                ("backend".into(), Value::String("metal".into())),
            ]);
            metal_candidate::inspect_present_with(&input, "aarch64-apple-darwin").unwrap_or_else(
                |_| {
                    json!({
                        "provider":"local",
                        "ready":false,
                        "status":"proof-unavailable",
                        "reason_code":"readiness_unavailable",
                    })
                },
            )
        }
        LocalHost::Windows => unreachable!(),
    };
    if let Some(follow) = follow {
        follow.observe(
            &config.journal_path,
            config_present.then_some(&journal_config),
            &readiness,
        );
    }
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
            "manifest_pin_mismatch"
            | "sha256_mismatch"
            | "inventory_member_missing"
            | "inventory_size_mismatch" => (
                super::model::RuntimePhase::ArtifactNotReady,
                "artifact-stale",
            ),
            "manifest_missing" => (
                super::model::RuntimePhase::ArtifactNotReady,
                "manifest-missing",
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
        .and_then(|artifacts| artifacts.get("model_path"))
        .and_then(Value::as_str)
    else {
        return truth_unavailable();
    };
    let backend = object
        .get("host")
        .and_then(Value::as_object)
        .and_then(|host| host.get("backend"))
        .and_then(Value::as_str)
        .unwrap_or("metal");
    let binary_path = artifacts
        .and_then(|artifacts| artifacts.get("binary_path"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let projector_path = artifacts
        .and_then(|artifacts| artifacts.get("projector_path"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let artifact_target_fingerprint = object
        .get("target")
        .and_then(Value::as_object)
        .and_then(|target| target.get("target_fingerprint_sha256"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let Ok(desired) = bundled_runtime_desired_fingerprint(
        backend,
        &model_id,
        artifact_target_fingerprint,
        binary_path.as_deref(),
        model_path,
        projector_path.as_deref(),
    ) else {
        return truth_unavailable();
    };
    let fingerprint = desired.sha256.clone();
    let common = LocalLaunchCommon {
        desired_fingerprint_json: desired.json,
        desired_fingerprint_sha256: fingerprint.clone(),
        model_id,
        model_path: model_path.into(),
        mmproj_path: projector_path,
    };
    let launch = match (config.platform, backend) {
        (LocalHost::Darwin, "metal") => LocalLaunchConfig::Metal {
            common,
            binary_path,
            unified_memory_mib: None,
        },
        (LocalHost::Linux, "cuda") => LocalLaunchConfig::Cuda {
            common,
            binary_path,
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
        (LocalHost::Linux, "vulkan") => {
            let Some(device) = solstone_core_local::select_device(&config.vulkan.devices, None)
            else {
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
                binary_path,
                devices: config.vulkan.devices.clone(),
                selected_gpu_index: device.index,
                selected_gpu_name: device.name,
                selected_vram_mib: device.vram_mib,
                vram_before_mib: None,
                platform: Platform::Linux,
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

#[allow(clippy::too_many_arguments)]
fn start_local(
    shared: &LocalRuntimeShared,
    clock: &dyn RuntimeClock,
    launch: &LocalLaunchConfig,
    state: &ProviderRuntimeState,
    fence: &ProviderFence,
    journal_path: Option<&std::path::Path>,
    warmup_timeout: Duration,
    warmup_poll_interval: Duration,
) -> ProviderLaunchOutcome {
    if launch.platform() == Platform::Windows {
        return start_local_windows(
            shared,
            clock,
            launch,
            state,
            fence,
            journal_path,
            warmup_timeout,
            warmup_poll_interval,
        );
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        start_local_unix(
            shared,
            clock,
            launch,
            state,
            fence,
            journal_path,
            warmup_timeout,
            warmup_poll_interval,
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        launch_failed()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn start_local_unix(
    shared: &LocalRuntimeShared,
    clock: &dyn RuntimeClock,
    launch: &LocalLaunchConfig,
    state: &ProviderRuntimeState,
    fence: &ProviderFence,
    journal_path: Option<&std::path::Path>,
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
    let Some(journal_path) = journal_path else {
        return launch_failed();
    };
    if !verify_launch_artifacts(journal_path, &plan, launch) {
        return ProviderLaunchOutcome {
            status: LaunchOutcomeStatus::LaunchFailed,
            reason_code: ReasonCode::known("artifact-stale"),
            managed: None,
        };
    }
    let port = reservation.release_for_spawn();
    #[cfg(unix)]
    let authority_res = {
        let launch_id = crate::lifecycle::generate_helper_launch_id("local-provider");
        crate::process::launch_generation_child(
            Disposition::IndependentLongLived,
            journal_path,
            launch_id,
            || spawn_plan(&plan),
            Box::new(|child, timeout| {
                crate::process::terminate(child, timeout)
                    .map(|_| ())
                    .map_err(|error| LaunchError::Terminate(std::io::Error::other(error)))
            }),
        )
    };
    #[cfg(not(unix))]
    let authority_res = crate::process::launch(
        Disposition::IndependentLongLived,
        || spawn_plan(&plan),
        Box::new(|child, timeout| {
            crate::process::terminate(child, timeout)
                .map(|_| ())
                .map_err(|error| LaunchError::Terminate(std::io::Error::other(error)))
        }),
    );
    let mut authority = match authority_res {
        Ok(authority) => authority,
        Err(_) => return launch_failed(),
    };
    let started_at = Instant::now();
    let process_id = format!("local:{}", authority.pid());
    let pid = authority.pid();
    let deadline = clock.monotonic_seconds() + warmup_timeout.as_secs_f64();
    loop {
        if let Ok(Some(_)) = authority.poll() {
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Exited,
                reason_code: ReasonCode::known("process-exited"),
                managed: None,
            };
        }
        if warmup_health_probe(port) == WarmupHealth::Ready {
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
        if clock.monotonic_seconds() >= deadline {
            let managed = ManagedProcess {
                id: process_id.clone(),
                pid,
                name: "local".into(),
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
        clock.sleep(warmup_poll_interval);
    }
}

/// Truth observation only checks installed inventory. Recheck the exact planned
/// artifacts with SHA-256 before a new provider process can be spawned.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn verify_launch_artifacts(
    journal: &std::path::Path,
    plan: &solstone_core_local::plan::LaunchPlan,
    launch: &LocalLaunchConfig,
) -> bool {
    let mut input = Map::from_iter([
        (
            "journal".into(),
            Value::String(journal.display().to_string()),
        ),
        ("model_id".into(), Value::String(plan.model_id.clone())),
    ]);
    let readiness = match plan.backend {
        PlanBackend::Metal => {
            input.insert("backend".into(), Value::String("metal".into()));
            match metal_candidate::inspect_with(&input, "aarch64-apple-darwin") {
                Ok(readiness) => readiness,
                Err(_) => return false,
            }
        }
        PlanBackend::Cuda | PlanBackend::Vulkan => {
            if let LocalLaunchConfig::Cuda { nvidia_probe, .. } = launch {
                let Ok(probe) = serde_json::to_value(nvidia_probe) else {
                    return false;
                };
                input.insert("nvidia_probe".into(), probe);
            }
            inspect_local(input)
        }
    };
    let backend = match plan.backend {
        PlanBackend::Cuda => "cuda",
        PlanBackend::Vulkan => "vulkan",
        PlanBackend::Metal => "metal",
    };
    readiness["ready"] == true
        && readiness["host"]["backend"] == backend
        && readiness["target"]["target_fingerprint_sha256"]
            .as_str()
            .unwrap_or("")
            == plan.desired_fingerprint_json["artifact_target_fingerprint_sha256"]
                .as_str()
                .unwrap_or("")
        && readiness["artifacts"]["model_id"] == plan.model_id
        && readiness["artifacts"]["binary_path"].as_str() == plan.binary_path.as_deref()
        && readiness["artifacts"]["model_path"].as_str() == Some(plan.model_path.as_str())
        && readiness["artifacts"]["projector_path"].as_str() == plan.mmproj_path.as_deref()
}

fn verify_launch_artifacts_windows(
    journal: &std::path::Path,
    plan: &solstone_core_local::plan::LaunchPlan,
) -> bool {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        if let Some(res) = test_windows_hooks::with_hooks(|h| {
            h.as_ref()
                .and_then(|h| h.verify_artifacts_fn.as_ref().map(|f| f(journal, plan)))
        }) {
            return res;
        }
    }
    let input = Map::from_iter([
        (
            "journal".into(),
            Value::String(journal.display().to_string()),
        ),
        ("model_id".into(), Value::String(plan.model_id.clone())),
        ("backend".into(), Value::String("vulkan".into())),
        (
            "artifact_key".into(),
            Value::String("x86_64-windows".into()),
        ),
    ]);
    let readiness = inspect_local(input);
    readiness["ready"] == true
        && readiness["host"]["backend"] == "vulkan"
        && readiness["target"]["target_fingerprint_sha256"]
            .as_str()
            .unwrap_or("")
            == plan.desired_fingerprint_json["artifact_target_fingerprint_sha256"]
                .as_str()
                .unwrap_or("")
        && readiness["artifacts"]["model_id"] == plan.model_id
        && readiness["artifacts"]["binary_path"].as_str() == plan.binary_path.as_deref()
        && readiness["artifacts"]["model_path"].as_str() == Some(plan.model_path.as_str())
        && readiness["artifacts"]["projector_path"].as_str() == plan.mmproj_path.as_deref()
}

const WINDOWS_STARTUP_LINE_LIMIT: usize = 128;
const WINDOWS_STARTUP_LINE_BYTES: usize = 4096;
type WindowsStartupLines =
    Arc<std::sync::Mutex<Option<std::collections::VecDeque<(String, u32, String)>>>>;

struct WindowsLocalLineCollectorSink {
    lines: WindowsStartupLines,
}

impl crate::process::ProcessEventSink for WindowsLocalLineCollectorSink {
    fn emit(&self, event: crate::process::ProcessEvent) {
        if let crate::process::ProcessEvent::Line {
            reference,
            pid,
            line,
            ..
        } = event
            && reference == "local-provider"
            && let Ok(mut state) = self.lines.lock()
            && let Some(lines) = state.as_mut()
        {
            // Keep only bounded startup evidence. The operational log writer
            // still owns the full output; a ready process stops collecting here.
            if line.len() > WINDOWS_STARTUP_LINE_BYTES {
                return;
            }
            if lines.len() == WINDOWS_STARTUP_LINE_LIMIT {
                lines.pop_front();
            }
            lines.push_back((reference, pid, line));
        }
    }
}

pub(crate) fn is_vulkan_allocation_failure(
    lines: &[(String, u32, String)],
    expected_ref: &str,
    expected_pid: u32,
) -> bool {
    let owned: Vec<_> = lines
        .iter()
        .filter(|(reference, pid, _)| reference == expected_ref && *pid == expected_pid)
        .map(|(_, _, line)| line.trim())
        .collect();
    owned.windows(2).any(|pair| {
        // Both wrappers in ggml-vulkan-buffers.cpp catch every vk::SystemError.
        // The following exception line must identify device-memory exhaustion;
        // DeviceLost and host-memory failures have the same allocation prefix.
        (pair[0].starts_with("ggml_vulkan: Device memory allocation of size ")
            || pair[0].starts_with("ggml_vulkan: Memory allocation of size "))
            && pair[0].ends_with(" failed.")
            && pair[1].starts_with("ggml_vulkan: ")
            && pair[1].ends_with(": ErrorOutOfDeviceMemory")
    })
}

#[allow(clippy::too_many_arguments)]
fn start_local_windows(
    shared: &LocalRuntimeShared,
    clock: &dyn RuntimeClock,
    launch: &LocalLaunchConfig,
    state: &ProviderRuntimeState,
    fence: &ProviderFence,
    journal_path: Option<&std::path::Path>,
    warmup_timeout: Duration,
    warmup_poll_interval: Duration,
) -> ProviderLaunchOutcome {
    let (selected_gpu_index, selected_gpu_name, selected_vram_mib) = match launch {
        LocalLaunchConfig::Vulkan {
            selected_gpu_index,
            selected_gpu_name,
            selected_vram_mib,
            ..
        } => (*selected_gpu_index, selected_gpu_name, *selected_vram_mib),
        _ => return launch_failed(),
    };

    // 1. Re-observe vulkan devices and enforce 6 GiB device-local minimum BEFORE plan() and spawn
    let vulkan_obs = get_windows_vulkan_observation();
    match solstone_core_local::gpu_device_local_verdict(
        vulkan_obs.succeeded,
        &vulkan_obs.devices,
        Some(selected_gpu_index),
    ) {
        solstone_core_local::GpuDeviceLocalVerdict::Unknown => {
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::HostBlocked,
                reason_code: ReasonCode::known("gpu-probe-failed"),
                managed: None,
            };
        }
        solstone_core_local::GpuDeviceLocalVerdict::NoHardwareDevice => {
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::HostBlocked,
                reason_code: ReasonCode::known("gpu-unavailable"),
                managed: None,
            };
        }
        solstone_core_local::GpuDeviceLocalVerdict::BelowBar { .. } => {
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::HostBlocked,
                reason_code: ReasonCode::known("gpu-memory-insufficient"),
                managed: None,
            };
        }
        solstone_core_local::GpuDeviceLocalVerdict::Eligible { .. } => {}
    }

    let mut reservation = match ReservedPort::reserve() {
        Ok(reservation) => reservation,
        Err(_) => return launch_failed(),
    };
    let port = reservation.port();
    let input = launch.assemble_plan_input(state, port);
    let plan = match plan(input) {
        PlanOutcome::Launch(plan) => plan,
        PlanOutcome::Rejected { .. } => return launch_failed(),
    };
    let Some(journal_path) = journal_path else {
        return launch_failed();
    };

    let Some(current_device) = vulkan_obs
        .devices
        .iter()
        .find(|d| d.index == selected_gpu_index)
    else {
        return launch_failed();
    };
    if !solstone_core_local::is_hardware_device(current_device)
        || current_device.name != *selected_gpu_name
        || current_device.vram_mib != selected_vram_mib
    {
        return launch_failed();
    }

    // 2. Verify launch artifacts on Windows
    if !verify_launch_artifacts_windows(journal_path, &plan) {
        return ProviderLaunchOutcome {
            status: LaunchOutcomeStatus::LaunchFailed,
            reason_code: ReasonCode::known("artifact-stale"),
            managed: None,
        };
    }

    // 3. Generate 32 bytes of secure random entropy
    let mut token_bytes = [0u8; 32];
    if fill_windows_token_entropy(&mut token_bytes).is_err() {
        return launch_failed();
    }
    let auth_token: String = token_bytes.iter().map(|b| format!("{b:02x}")).collect();

    // 4. Binary path and cwd
    let Some(binary_path_str) = &plan.binary_path else {
        return launch_failed();
    };
    let binary_path = PathBuf::from(binary_path_str);
    let Some(bin_dir) = binary_path.parent() else {
        return launch_failed();
    };
    let Some(package_root) = bin_dir.parent() else {
        return launch_failed();
    };
    let current_directory = bin_dir.to_path_buf();

    // 5. SystemRoot and Environment
    let system_root = get_windows_system_root();
    let Some(system_root) = system_root.filter(|s| !s.is_empty()) else {
        return launch_failed();
    };
    let mut environment = std::collections::BTreeMap::new();
    environment.insert(std::ffi::OsString::from("SystemRoot"), system_root);
    for (k, v) in solstone_core_distribution::manifest_verify::signed_package_pin_environment() {
        environment.insert(k, v);
    }
    environment.insert(
        std::ffi::OsString::from("GGML_VK_VISIBLE_DEVICES"),
        std::ffi::OsString::from(current_device.index.to_string()),
    );
    environment.insert(
        std::ffi::OsString::from("LLAMA_API_KEY"),
        std::ffi::OsString::from(auth_token.clone()),
    );

    let arguments = plan.argv[1..].to_vec();

    let drained_lines: WindowsStartupLines = Arc::new(std::sync::Mutex::new(Some(
        std::collections::VecDeque::new(),
    )));
    let sink: Arc<dyn crate::process::ProcessEventSink> = Arc::new(WindowsLocalLineCollectorSink {
        lines: Arc::clone(&drained_lines),
    });

    let request = crate::process::IndependentProviderRequest {
        package_root: package_root.to_path_buf(),
        executable: binary_path.clone(),
        current_directory,
        arguments,
        environment,
        resource_limits: None,
        spawn_options: crate::process::SpawnOptions {
            journal_root: journal_path.to_path_buf(),
            reference: "local-provider".to_owned(),
            day: None,
            sink: Some(sink),
            environment: std::collections::BTreeMap::new(),
        },
    };

    let port = reservation.release_for_spawn();
    let mut authority = match spawn_windows_provider(request) {
        Ok(authority) => authority,
        Err(_) => return launch_failed(),
    };

    let started_at = Instant::now();
    let process_id = format!("local:{}", authority.pid());
    let pid = authority.pid();
    let deadline = clock.monotonic_seconds() + warmup_timeout.as_secs_f64();
    loop {
        if let Ok(Some(_)) = authority.poll() {
            let drained =
                authority.cleanup_until(Instant::now() + crate::process::DRAIN_JOIN_TIMEOUT);
            let lines: Vec<_> = drained_lines
                .lock()
                .ok()
                .and_then(|mut state| state.take())
                .unwrap_or_default()
                .into_iter()
                .collect();
            let is_allocation_failure =
                drained && is_vulkan_allocation_failure(&lines, "local-provider", pid);
            let reason = if is_allocation_failure {
                "gpu-allocation-failed"
            } else {
                "process-exited"
            };
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Exited,
                reason_code: ReasonCode::known(reason),
                managed: None,
            };
        }
        if probe_windows_props_warmup(port, &auth_token) == WarmupHealth::Ready {
            if let Ok(mut state) = drained_lines.lock() {
                *state = None;
            }
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
                    port,
                },
                started_at,
            );
            shared.stage_launch_credentials(fence, port, auth_token.into_bytes());
            return ProviderLaunchOutcome {
                status: LaunchOutcomeStatus::Ready,
                reason_code: ReasonCode::known("probe-ready"),
                managed: Some(managed),
            };
        }
        if clock.monotonic_seconds() >= deadline {
            if let Ok(mut state) = drained_lines.lock() {
                *state = None;
            }
            let managed = ManagedProcess {
                id: process_id.clone(),
                pid,
                name: "local".into(),
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
        clock.sleep(warmup_poll_interval);
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub mod test_windows_hooks {
    use super::*;
    use std::sync::Mutex;

    type EntropyHook = dyn Fn(&mut [u8]) -> Result<(), ()> + Send + Sync;
    type SpawnHook = dyn Fn(
            crate::process::IndependentProviderRequest,
        ) -> Result<crate::process::LaunchAuthority, LaunchError>
        + Send
        + Sync;
    type WarmupHook = dyn Fn(u16, &str) -> WarmupHealth + Send + Sync;
    type ArtifactHook =
        dyn Fn(&std::path::Path, &solstone_core_local::plan::LaunchPlan) -> bool + Send + Sync;

    #[derive(Default)]
    pub struct WindowsLaunchHooks {
        pub vulkan_observation:
            Option<Box<dyn Fn() -> crate::vulkan_observe::VulkanObservation + Send + Sync>>,
        pub entropy_fn: Option<Box<EntropyHook>>,
        pub system_root: Option<std::ffi::OsString>,
        pub spawn_fn: Option<Box<SpawnHook>>,
        pub warmup_probe_fn: Option<Box<WarmupHook>>,
        pub verify_artifacts_fn: Option<Box<ArtifactHook>>,
    }

    static HOOKS: Mutex<Option<WindowsLaunchHooks>> = Mutex::new(None);

    #[cfg(all(test, feature = "full-tests"))]
    pub fn set_hooks(hooks: WindowsLaunchHooks) {
        *HOOKS.lock().unwrap() = Some(hooks);
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub fn clear_hooks() {
        *HOOKS.lock().unwrap() = None;
    }

    pub(super) fn with_hooks<R>(f: impl FnOnce(&Option<WindowsLaunchHooks>) -> R) -> R {
        let guard = HOOKS.lock().unwrap();
        f(&guard)
    }
}

fn get_windows_vulkan_observation() -> crate::vulkan_observe::VulkanObservation {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        if let Some(obs) = test_windows_hooks::with_hooks(|h| {
            h.as_ref()
                .and_then(|h| h.vulkan_observation.as_ref().map(|f| f()))
        }) {
            return obs;
        }
    }
    crate::vulkan_observe::observe_vulkan_devices()
}

fn fill_windows_token_entropy(buf: &mut [u8]) -> Result<(), ()> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        if let Some(res) = test_windows_hooks::with_hooks(|h| {
            h.as_ref()
                .and_then(|h| h.entropy_fn.as_ref().map(|f| f(buf)))
        }) {
            return res;
        }
    }
    getrandom::fill(buf).map_err(|_| ())
}

fn get_windows_system_root() -> Option<std::ffi::OsString> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        if let Some(sr) =
            test_windows_hooks::with_hooks(|h| h.as_ref().and_then(|h| h.system_root.clone()))
        {
            return Some(sr);
        }
    }
    std::env::var_os("SystemRoot")
}

fn spawn_windows_provider(
    request: crate::process::IndependentProviderRequest,
) -> Result<crate::process::LaunchAuthority, LaunchError> {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        let has_hook =
            test_windows_hooks::with_hooks(|h| h.as_ref().is_some_and(|h| h.spawn_fn.is_some()));
        if has_hook {
            return test_windows_hooks::with_hooks(|h| {
                h.as_ref()
                    .and_then(|h| h.spawn_fn.as_ref().map(|f| f(request)))
            })
            .expect("spawn hook was checked present");
        }
    }
    #[cfg(windows)]
    {
        crate::process::launch_independent_provider(request)
            .map_err(|error| LaunchError::Admission(error.to_string()))
    }
    #[cfg(not(windows))]
    {
        let _ = request;
        Err(LaunchError::Spawn(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "windows independent provider cannot spawn on non-windows host without hook",
        )))
    }
}

fn probe_windows_props_warmup(port: u16, auth_token: &str) -> WarmupHealth {
    #[cfg(any(test, feature = "test-hooks"))]
    {
        if let Some(res) = test_windows_hooks::with_hooks(|h| {
            h.as_ref()
                .and_then(|h| h.warmup_probe_fn.as_ref().map(|f| f(port, auth_token)))
        }) {
            return res;
        }
    }
    warmup_props_probe(port, auth_token)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn spawn_plan(plan: &solstone_core_local::plan::LaunchPlan) -> std::io::Result<Child> {
    let (program, arguments) = plan.argv.split_first().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local launch plan has no argv",
        )
    })?;
    let mut command = Command::new(program);
    command.args(arguments).envs(&plan.extra_env);
    apply_parent_death_kill(&mut command);
    command.spawn()
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
    let fence = request.managed.fence.as_ref();
    if let Some(fence) = fence {
        shared.revoke_launch_credentials(fence);
    }
    let taken = match fence {
        Some(fence) => shared.take_ready_child(fence),
        None => shared
            .take_child(&request.managed.id)
            .map(|authority| (request.managed.id.clone(), authority)),
    };
    let Some((process_id, mut authority)) = taken else {
        if let Some(fence) = fence {
            shared.remove_ready_process(fence);
            shared.revoke_launch_credentials(fence);
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
                shared.revoke_launch_credentials(fence);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmupHealth {
    Ready,
    Loading,
    Failed,
}

fn warmup_props_probe(port: u16, auth_token: &str) -> WarmupHealth {
    warmup_props_probe_until(port, auth_token, Instant::now() + WARMUP_PROBE_TIMEOUT)
}

fn warmup_props_probe_until(port: u16, auth_token: &str, deadline: Instant) -> WarmupHealth {
    const MAX_RESPONSE_BYTES: usize = 64 * 1024;
    let remaining = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
    };
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Some(timeout) = remaining() else {
        return WarmupHealth::Failed;
    };
    let mut stream = match TcpStream::connect_timeout(&address, timeout) {
        Ok(stream) => stream,
        Err(_) => return WarmupHealth::Failed,
    };
    let req = format!(
        "GET /props HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        auth_token
    );
    let mut pending = req.as_bytes();
    while !pending.is_empty() {
        let Some(timeout) = remaining() else {
            return WarmupHealth::Failed;
        };
        if stream.set_write_timeout(Some(timeout)).is_err() {
            return WarmupHealth::Failed;
        }
        match stream.write(pending) {
            Ok(0) | Err(_) => return WarmupHealth::Failed,
            Ok(count) => pending = &pending[count..],
        }
    }
    let mut response = Vec::new();
    loop {
        let Some(timeout) = remaining() else {
            return WarmupHealth::Failed;
        };
        if stream.set_read_timeout(Some(timeout)).is_err() {
            return WarmupHealth::Failed;
        }
        let mut buffer = [0; 4096];
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) if response.len() + count <= MAX_RESPONSE_BYTES => {
                response.extend_from_slice(&buffer[..count])
            }
            _ => return WarmupHealth::Failed,
        }
    }
    if remaining().is_none() {
        return WarmupHealth::Failed;
    }
    let Ok(response) = std::str::from_utf8(&response) else {
        return WarmupHealth::Failed;
    };
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn warmup_health_probe(port: u16) -> WarmupHealth {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&address, WARMUP_PROBE_TIMEOUT) {
        Ok(stream) => stream,
        Err(_) => return WarmupHealth::Failed,
    };
    let req = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.set_read_timeout(Some(WARMUP_PROBE_TIMEOUT)).is_err()
        || stream
            .set_write_timeout(Some(WARMUP_PROBE_TIMEOUT))
            .is_err()
        || stream.write_all(req.as_bytes()).is_err()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{LaunchAuthority, ProcessObservation};
    use crate::provider_runtime::model::{ProviderFence, ProviderStopCleanupRequest, RuntimePhase};
    use crate::provider_runtime::store::ReadyProcessLookup;
    #[cfg(all(test, feature = "full-tests"))]
    struct TestClock {
        now: std::sync::atomic::AtomicU64,
    }

    #[cfg(all(test, feature = "full-tests"))]
    impl Default for TestClock {
        fn default() -> Self {
            Self {
                now: std::sync::atomic::AtomicU64::new(0),
            }
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    impl RuntimeClock for TestClock {
        fn monotonic_seconds(&self) -> f64 {
            self.now.load(std::sync::atomic::Ordering::SeqCst) as f64
        }
        fn now_utc_rfc3339(&self) -> String {
            "2026-09-26T00:00:00Z".to_string()
        }
        fn sleep(&self, _duration: Duration) {
            self.now.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn already_gone_ready_cleanup_removes_local_observation_residue() {
        let shared = LocalRuntimeShared::default();
        let fence = ProviderFence {
            incarnation: "incarnation".to_owned(),
            generation: 2,
            fingerprint: Some("fingerprint".to_owned()),
            attempt: 1,
        };
        shared.record_ready_observation_for_test(
            &fence,
            ReadyProcess {
                process_id: "local:42".to_owned(),
                process_name: "local".to_owned(),
                pid: 42,
                port: 5015,
            },
            Instant::now(),
        );
        let request = ProviderStopCleanupRequest {
            managed: ManagedProcess {
                id: "local:42".to_owned(),
                pid: 42,
                name: "local".to_owned(),
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
            stop_local(&shared, Some(&request), false, Duration::ZERO).status,
            StopCleanupStatus::Stopped,
        );
        assert_eq!(
            shared.observe_current_process(&[], Instant::now()),
            ProcessObservation::ConfirmedAbsent,
        );
    }

    #[test]
    fn stop_local_revokes_credentials_for_exact_generation() {
        let shared = LocalRuntimeShared::default();
        let fence = ProviderFence {
            incarnation: "incarnation".to_owned(),
            generation: 3,
            fingerprint: Some("fingerprint".to_owned()),
            attempt: 1,
        };
        shared.stage_launch_credentials(&fence, 8080, b"token".to_vec());
        shared.accept_ready(&fence);
        assert!(shared.launch_credentials().is_some());

        let request = ProviderStopCleanupRequest {
            managed: ManagedProcess {
                id: "local:42".to_owned(),
                pid: 42,
                name: "local".to_owned(),
                running: true,
                fence: Some(fence),
            },
            reason_code: ReasonCode::known("target-changed"),
            target_phase: RuntimePhase::Stopped,
            target_reason_code: None,
            admission_exclusive: false,
            orphaned_start_outcome: false,
        };
        let outcome = stop_local(&shared, Some(&request), false, Duration::ZERO);
        assert_eq!(outcome.status, StopCleanupStatus::Stopped);
        assert!(shared.launch_credentials().is_none());
    }

    #[test]
    fn stop_local_retains_child_on_terminate_failure() {
        let shared = LocalRuntimeShared::default();
        let fence = ProviderFence {
            incarnation: "incarnation".to_owned(),
            generation: 1,
            fingerprint: Some("fingerprint".to_owned()),
            attempt: 1,
        };
        let authority = LaunchAuthority::scripted(
            1234,
            Box::new(|| Ok(None)),
            Box::new(|_| {
                Err(LaunchError::Terminate(std::io::Error::other(
                    "terminate failed",
                )))
            }),
        );
        shared.register_ready_process(
            &fence,
            authority,
            ReadyProcess {
                process_id: "local:1234".to_owned(),
                process_name: "local".to_owned(),
                pid: 1234,
                port: 8080,
            },
            Instant::now(),
        );
        shared.stage_launch_credentials(&fence, 8080, b"token".to_vec());
        shared.accept_ready(&fence);

        let request = ProviderStopCleanupRequest {
            managed: ManagedProcess {
                id: "local:1234".to_owned(),
                pid: 1234,
                name: "local".to_owned(),
                running: true,
                fence: Some(fence),
            },
            reason_code: ReasonCode::known("target-changed"),
            target_phase: RuntimePhase::Stopped,
            target_reason_code: None,
            admission_exclusive: false,
            orphaned_start_outcome: false,
        };
        let outcome = stop_local(&shared, Some(&request), false, Duration::ZERO);
        assert_eq!(outcome.status, StopCleanupStatus::CleanupFailed);
        // A failed termination retains the Job, never public admission.
        assert!(shared.launch_credentials().is_none());
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn sample_windows_launch_fixture(
        root: &std::path::Path,
    ) -> (LocalLaunchConfig, ProviderFence, ProviderRuntimeState) {
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("cache/models/qwen3.5-4b")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let bin_path = root.join("bin/llama-server.exe");
        let model_path = root.join("cache/models/qwen3.5-4b/model.gguf");
        std::fs::write(&bin_path, b"mz").unwrap();
        std::fs::write(&model_path, b"gguf").unwrap();

        let model_id = "local/qwen3.5-4b".to_string();
        let common = LocalLaunchCommon {
            desired_fingerprint_json: json!({
                "schema": "solstone-local-runtime-fingerprint-v1",
                "backend": "vulkan",
                "model_id": model_id,
                "artifact_target_fingerprint_sha256": "fp",
                "engine_binary_path": bin_path.display().to_string(),
                "model_path": model_path.display().to_string(),
                "projector_path": null,
            }),
            desired_fingerprint_sha256: "desired_sha256".into(),
            model_id,
            model_path: model_path.display().to_string(),
            mmproj_path: None,
        };
        let launch = LocalLaunchConfig::Vulkan {
            common,
            binary_path: Some(bin_path.display().to_string()),
            devices: vec![solstone_core_local::VulkanDevice {
                index: 0,
                name: "RTX 4090".into(),
                device_type: Some(2), // discrete GPU
                vram_mib: 24_000,
            }],
            selected_gpu_index: 0,
            selected_gpu_name: "RTX 4090".into(),
            selected_vram_mib: 24_000,
            vram_before_mib: None,
            platform: Platform::Windows,
        };
        let fence = ProviderFence {
            incarnation: "inc1".into(),
            generation: 1,
            fingerprint: Some("desired_sha256".into()),
            attempt: 1,
        };
        let mut state = ProviderRuntimeState::new(super::super::model::ProviderName::Local);
        state.desired_fingerprint = Some("desired_sha256".into());
        (launch, fence, state)
    }

    #[cfg(all(test, feature = "full-tests"))]
    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn start_local_windows_fails_when_device_mismatches_or_not_hardware() {
        let _lock = TEST_MUTEX.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (launch, fence, state) = sample_windows_launch_fixture(root.path());
        let shared = LocalRuntimeShared::default();
        let clock = TestClock::default();

        // 1. Observation failed
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![],
                succeeded: false,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(move |_| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LaunchAuthority::scripted(
                    9999,
                    Box::new(|| Ok(None)),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: None,
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::HostBlocked);
        assert_eq!(outcome.reason_code.as_str(), "gpu-probe-failed");
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 0);

        // 2. Device type is CPU (type 4) or software ICD name instead of discrete/integrated GPU
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "llvmpipe (LLVM 15.0.7, 256 bits)".into(),
                    device_type: Some(4),
                    vram_mib: 24_000,
                }],
                succeeded: true,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(move |_| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LaunchAuthority::scripted(
                    9999,
                    Box::new(|| Ok(None)),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: None,
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::HostBlocked);
        assert_eq!(outcome.reason_code.as_str(), "gpu-unavailable");
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 0);

        // 3. Name or VRAM changed
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 3080".into(),
                    device_type: Some(2),
                    vram_mib: 10_000,
                }],
                succeeded: true,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(move |_| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LaunchAuthority::scripted(
                    9999,
                    Box::new(|| Ok(None)),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: None,
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::LaunchFailed);
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        test_windows_hooks::clear_hooks();
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn start_local_windows_fails_when_entropy_fails_or_system_root_missing() {
        let _lock = TEST_MUTEX.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (launch, fence, state) = sample_windows_launch_fixture(root.path());
        let shared = LocalRuntimeShared::default();
        let clock = TestClock::default();

        // Entropy failure
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 4090".into(),
                    device_type: Some(2),
                    vram_mib: 24_000,
                }],
                succeeded: true,
            })),
            entropy_fn: Some(Box::new(|_| Err(()))),
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(move |_| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LaunchAuthority::scripted(
                    9999,
                    Box::new(|| Ok(None)),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: None,
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::LaunchFailed);
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 0);

        // SystemRoot empty / missing
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 4090".into(),
                    device_type: Some(2),
                    vram_mib: 24_000,
                }],
                succeeded: true,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::new()),
            spawn_fn: Some(Box::new(move |_| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LaunchAuthority::scripted(
                    9999,
                    Box::new(|| Ok(None)),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: None,
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::LaunchFailed);
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        test_windows_hooks::clear_hooks();
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn start_local_windows_successful_lifecycle_publishes_credentials() {
        let _lock = TEST_MUTEX.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (launch, fence, state) = sample_windows_launch_fixture(root.path());
        let shared = LocalRuntimeShared::default();
        let clock = TestClock::default();

        let spawned_req = Arc::new(std::sync::Mutex::new(None));
        let req_capture = Arc::clone(&spawned_req);
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);

        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 4090".into(),
                    device_type: Some(2), // discrete GPU
                    vram_mib: 24_000,
                }],
                succeeded: true,
            })),
            entropy_fn: Some(Box::new(|buf| {
                buf.fill(0xab);
                Ok(())
            })),
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(move |req| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                *req_capture.lock().unwrap() = Some(req);
                Ok(LaunchAuthority::scripted(
                    9999,
                    Box::new(|| Ok(None)),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: Some(Box::new(|_port, token| {
                if token == "abababababababababababababababababababababababababababababababab" {
                    WarmupHealth::Ready
                } else {
                    WarmupHealth::Failed
                }
            })),
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });

        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::Ready);
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(shared.launch_credentials().is_none());
        shared.accept_ready(&fence);
        let creds = shared.launch_credentials().expect("credentials admitted");
        assert_eq!(creds.0, 1); // generation
        assert_eq!(
            creds.2,
            b"abababababababababababababababababababababababababababababababab"
        );

        let req = spawned_req
            .lock()
            .unwrap()
            .take()
            .expect("request captured");
        assert_eq!(req.executable, root.path().join("bin/llama-server.exe"));
        assert_eq!(req.current_directory, root.path().join("bin"));
        assert_eq!(req.package_root, root.path().to_path_buf());
        assert!(!req.arguments.contains(&"--api-key".to_string()));
        assert_eq!(
            req.environment.get(std::ffi::OsStr::new("SystemRoot")),
            Some(&std::ffi::OsString::from("C:\\Windows"))
        );
        assert_eq!(
            req.environment
                .get(std::ffi::OsStr::new("GGML_VK_VISIBLE_DEVICES")),
            Some(&std::ffi::OsString::from("0"))
        );
        assert_eq!(
            req.environment.get(std::ffi::OsStr::new("LLAMA_API_KEY")),
            Some(&std::ffi::OsString::from(
                "abababababababababababababababababababababababababababababababab"
            ))
        );
        assert!(!req.environment.contains_key(std::ffi::OsStr::new("PATH")));

        test_windows_hooks::clear_hooks();
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn start_local_windows_child_exit_and_warmup_timeout() {
        let _lock = TEST_MUTEX.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (launch, fence, state) = sample_windows_launch_fixture(root.path());
        let shared = LocalRuntimeShared::default();
        let clock = TestClock::default();

        // 1. Child exits
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 4090".into(),
                    device_type: Some(2),
                    vram_mib: 24_000,
                }],
                succeeded: true,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(|_| {
                Ok(LaunchAuthority::scripted(
                    9998,
                    Box::new(|| Ok(Some(0))),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: Some(Box::new(|_, _| WarmupHealth::Loading)),
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });

        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::Exited);

        // 2. Warmup timeout
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 4090".into(),
                    device_type: Some(2),
                    vram_mib: 24_000,
                }],
                succeeded: true,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(|_| {
                Ok(LaunchAuthority::scripted(
                    9997,
                    Box::new(|| Ok(None)),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: Some(Box::new(|_, _| WarmupHealth::Loading)),
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });

        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::ZERO,
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::WarmupTimeout);
        // Child must be retained in shared
        assert!(shared.take_child("local:9997").is_some());

        test_windows_hooks::clear_hooks();
    }

    #[test]
    fn windows_startup_collector_is_bounded_and_retires() {
        use crate::process::{OutputStream, ProcessEvent, ProcessEventSink};
        let lines = Arc::new(std::sync::Mutex::new(Some(
            std::collections::VecDeque::new(),
        )));
        let sink = WindowsLocalLineCollectorSink {
            lines: Arc::clone(&lines),
        };
        let emit = |reference: &str, line: String| {
            sink.emit(ProcessEvent::Line {
                reference: reference.into(),
                name: "fixture".into(),
                pid: 7,
                stream: OutputStream::Stderr,
                line,
            })
        };
        emit("other-provider", "foreign output".into());
        emit("local-provider", "x".repeat(WINDOWS_STARTUP_LINE_BYTES + 1));
        assert!(lines.lock().unwrap().as_ref().unwrap().is_empty());
        for index in 0..WINDOWS_STARTUP_LINE_LIMIT + 10 {
            emit("local-provider", index.to_string());
        }
        let state = lines.lock().unwrap();
        let collected = state.as_ref().unwrap();
        assert_eq!(collected.len(), WINDOWS_STARTUP_LINE_LIMIT);
        assert_eq!(collected.front().unwrap().2, "10");
        drop(state);
        *lines.lock().unwrap() = None;
        emit("local-provider", "later output".into());
        assert!(lines.lock().unwrap().is_none());
    }

    #[test]
    fn windows_is_vulkan_allocation_failure_matching() {
        let prefix = "ggml_vulkan: Device memory allocation of size 4294967296 failed.";
        let device_error = "ggml_vulkan: vk::Device::allocateMemory: ErrorOutOfDeviceMemory";
        let lines = |reference: &str, pid: u32, first: &str, second: &str| {
            vec![
                (reference.to_owned(), pid, first.to_owned()),
                (reference.to_owned(), pid, second.to_owned()),
            ]
        };
        assert!(is_vulkan_allocation_failure(
            &lines("local-provider", 1234, prefix, device_error),
            "local-provider",
            1234
        ));
        assert!(is_vulkan_allocation_failure(
            &lines(
                "local-provider",
                1234,
                "ggml_vulkan: Memory allocation of size 2147483648 failed.",
                device_error
            ),
            "local-provider",
            1234
        ));
        for error in [
            "ggml_vulkan: vk::Device::allocateMemory: ErrorDeviceLost",
            "ggml_vulkan: vk::Device::allocateMemory: ErrorOutOfHostMemory",
            "normal process exit",
        ] {
            assert!(!is_vulkan_allocation_failure(
                &lines("local-provider", 1234, prefix, error),
                "local-provider",
                1234
            ));
        }
        assert!(!is_vulkan_allocation_failure(
            &lines("other", 1234, prefix, device_error),
            "local-provider",
            1234
        ));
        assert!(!is_vulkan_allocation_failure(
            &lines("local-provider", 9999, prefix, device_error),
            "local-provider",
            1234
        ));
        assert!(!is_vulkan_allocation_failure(
            &[("local-provider".into(), 1234, prefix.into())],
            "local-provider",
            1234
        ));
        assert!(!is_vulkan_allocation_failure(&[], "local-provider", 1234));
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn windows_host_reports_gpu_unavailable_for_empty_vulkan_or_package_unavailable() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(root.path().join("config/journal.json"), b"{}").unwrap();

        let hardware = solstone_core_local::VulkanDevice {
            index: 0,
            name: "NVIDIA RTX".into(),
            device_type: Some(1),
            vram_mib: 16_384,
        };

        // 1. Empty successful probe -> gpu-unavailable
        let shared = LocalRuntimeShared::default();
        let config = LocalTruthConfig {
            journal_path: root.path().to_path_buf(),
            platform: LocalHost::Windows,
            arch: "x86_64",
            nvidia_probe: Some(solstone_core_local::nvidia::NvidiaProbe::absent()),
            vulkan: crate::vulkan_observe::VulkanObservation {
                devices: Vec::new(),
                succeeded: true,
            },
            windows_package: None,
        };
        let observation = observe_truth(&shared, &config, None);
        assert_eq!(observation.phase, RuntimePhase::HostBlocked);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("gpu-unavailable")
        );
        assert!(
            shared
                .launch_request_for(&observation.desired_fingerprint)
                .is_none()
        );

        // 2. Failed probe -> gpu-probe-failed
        let shared = LocalRuntimeShared::default();
        let config = LocalTruthConfig {
            journal_path: root.path().to_path_buf(),
            platform: LocalHost::Windows,
            arch: "x86_64",
            nvidia_probe: Some(solstone_core_local::nvidia::NvidiaProbe::absent()),
            vulkan: crate::vulkan_observe::VulkanObservation {
                devices: Vec::new(),
                succeeded: false,
            },
            windows_package: None,
        };
        let observation = observe_truth(&shared, &config, None);
        assert_eq!(observation.phase, RuntimePhase::HostBlocked);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("gpu-probe-failed")
        );
        assert!(
            shared
                .launch_request_for(&observation.desired_fingerprint)
                .is_none()
        );

        // 3. Known below-bar capacity -> gpu-memory-insufficient.
        for below_vram in [4096, 6143] {
            let shared = LocalRuntimeShared::default();
            let config = LocalTruthConfig {
                journal_path: root.path().to_path_buf(),
                platform: LocalHost::Windows,
                arch: "x86_64",
                nvidia_probe: Some(solstone_core_local::nvidia::NvidiaProbe::absent()),
                vulkan: crate::vulkan_observe::VulkanObservation {
                    devices: vec![solstone_core_local::VulkanDevice {
                        index: 0,
                        name: "RTX".into(),
                        device_type: Some(1),
                        vram_mib: below_vram,
                    }],
                    succeeded: true,
                },
                windows_package: None,
            };
            let observation = observe_truth(&shared, &config, None);
            assert_eq!(observation.phase, RuntimePhase::HostBlocked);
            assert_eq!(
                observation.reason_code.as_ref().map(ReasonCode::as_str),
                Some("gpu-memory-insufficient")
            );
            assert!(
                shared
                    .launch_request_for(&observation.desired_fingerprint)
                    .is_none()
            );
        }

        // 4. Eligible (6144) -> not gpu-memory-insufficient
        let shared = LocalRuntimeShared::default();
        let config = LocalTruthConfig {
            journal_path: root.path().to_path_buf(),
            platform: LocalHost::Windows,
            arch: "x86_64",
            nvidia_probe: Some(solstone_core_local::nvidia::NvidiaProbe::absent()),
            vulkan: crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX".into(),
                    device_type: Some(1),
                    vram_mib: 6144,
                }],
                succeeded: true,
            },
            windows_package: None,
        };
        let observation = observe_truth(&shared, &config, None);
        assert_ne!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("gpu-memory-insufficient")
        );

        // aarch64 platform unsupported
        let shared = LocalRuntimeShared::default();
        let config = LocalTruthConfig {
            journal_path: root.path().to_path_buf(),
            platform: LocalHost::Windows,
            arch: "aarch64",
            nvidia_probe: Some(solstone_core_local::nvidia::NvidiaProbe::absent()),
            vulkan: crate::vulkan_observe::VulkanObservation {
                devices: vec![hardware],
                succeeded: true,
            },
            windows_package: None,
        };
        let observation = observe_truth(&shared, &config, None);
        assert_eq!(observation.phase, RuntimePhase::HostBlocked);
        assert_eq!(
            observation.reason_code.as_ref().map(ReasonCode::as_str),
            Some("platform-unsupported")
        );
    }

    #[test]
    #[cfg(all(test, feature = "full-tests"))]
    fn windows_launch_vulkan_admission_and_allocation() {
        let _lock = TEST_MUTEX.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let (mut launch, fence, state) = sample_windows_launch_fixture(root.path());
        if let LocalLaunchConfig::Vulkan {
            ref mut selected_gpu_name,
            ref mut selected_vram_mib,
            ref mut devices,
            ..
        } = launch
        {
            *selected_gpu_name = "RTX 4060".into();
            *selected_vram_mib = 6144;
            *devices = vec![solstone_core_local::VulkanDevice {
                index: 0,
                name: "RTX 4060".into(),
                device_type: Some(2),
                vram_mib: 6144,
            }];
        }
        let shared = LocalRuntimeShared::default();
        let clock = TestClock::default();

        // 1. Below 6 GiB floor (4096 MiB) does not call spawn hook and blocks
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 3050".into(),
                    device_type: Some(2),
                    vram_mib: 4096,
                }],
                succeeded: true,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(move |_| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LaunchAuthority::scripted(
                    9998,
                    Box::new(|| Ok(Some(0))),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: Some(Box::new(|_, _| WarmupHealth::Loading)),
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::HostBlocked);
        assert_eq!(outcome.reason_code.as_str(), "gpu-memory-insufficient");
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 0);

        // 2. Failed probe does not call spawn hook and blocks
        let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = Arc::clone(&spawn_count);
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "RTX 4060".into(),
                    device_type: Some(2),
                    vram_mib: 6144,
                }],
                succeeded: false,
            })),
            entropy_fn: None,
            system_root: Some(std::ffi::OsString::from("C:\\Windows")),
            spawn_fn: Some(Box::new(move |_| {
                sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(LaunchAuthority::scripted(
                    9998,
                    Box::new(|| Ok(Some(0))),
                    Box::new(|_| Ok(())),
                ))
            })),
            warmup_probe_fn: Some(Box::new(|_, _| WarmupHealth::Loading)),
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::HostBlocked);
        assert_eq!(outcome.reason_code.as_str(), "gpu-probe-failed");
        assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 0);

        // A successfully enumerated device whose heap size is unreadable is
        // unknown, even when a previously constructed launch plan was eligible.
        test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
            vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                devices: vec![solstone_core_local::VulkanDevice {
                    index: 0,
                    name: "unreadable heap".into(),
                    device_type: Some(2),
                    vram_mib: 0,
                }],
                succeeded: true,
            })),
            entropy_fn: None,
            system_root: None,
            spawn_fn: Some(Box::new(|_| panic!("unknown memory must not spawn"))),
            warmup_probe_fn: None,
            verify_artifacts_fn: Some(Box::new(|_, _| true)),
        });
        let outcome = start_local_windows(
            &shared,
            &clock,
            &launch,
            &state,
            &fence,
            Some(root.path()),
            Duration::from_secs(1),
            Duration::from_millis(10),
        );
        assert_eq!(outcome.status, LaunchOutcomeStatus::HostBlocked);
        assert_eq!(outcome.reason_code.as_str(), "gpu-probe-failed");

        // At the floor, a real launch reaches spawn. Its terminal-output
        // evidence arrives only when cleanup polls after the first exit read.
        for (line_ref, line_pid, exception, drain_complete, expected_reason) in [
            (
                "local-provider",
                9998,
                "ErrorOutOfDeviceMemory",
                true,
                "gpu-allocation-failed",
            ),
            (
                "local-provider",
                9998,
                "ErrorDeviceLost",
                true,
                "process-exited",
            ),
            (
                "local-provider",
                9998,
                "ErrorOutOfHostMemory",
                true,
                "process-exited",
            ),
            (
                "different-ref",
                9998,
                "ErrorOutOfDeviceMemory",
                true,
                "process-exited",
            ),
            (
                "local-provider",
                9997,
                "ErrorOutOfDeviceMemory",
                true,
                "process-exited",
            ),
            ("local-provider", 9998, "", true, "process-exited"),
            (
                "local-provider",
                9998,
                "ErrorOutOfDeviceMemory",
                false,
                "process-exited",
            ),
        ] {
            let spawn_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let sc = Arc::clone(&spawn_count);
            let exception = exception.to_owned();
            let line_ref = line_ref.to_owned();
            test_windows_hooks::set_hooks(test_windows_hooks::WindowsLaunchHooks {
                vulkan_observation: Some(Box::new(|| crate::vulkan_observe::VulkanObservation {
                    devices: vec![solstone_core_local::VulkanDevice {
                        index: 0,
                        name: "RTX 4060".into(),
                        device_type: Some(2),
                        vram_mib: 6144,
                    }],
                    succeeded: true,
                })),
                entropy_fn: None,
                system_root: Some(std::ffi::OsString::from("C:\\Windows")),
                spawn_fn: Some(Box::new(move |req| {
                    sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let sink = req.spawn_options.sink.clone();
                    let exception = exception.clone();
                    let line_ref = line_ref.clone();
                    let mut exited = false;
                    Ok(LaunchAuthority::scripted(
                        9998,
                        Box::new(move || {
                            if exited {
                                if let Some(sink) = &sink {
                                    let lines = if exception.is_empty() {
                                        vec!["normal process exit".to_owned()]
                                    } else {
                                        vec![
                                    "ggml_vulkan: Device memory allocation of size 4294967296 failed.".to_owned(),
                                    format!("ggml_vulkan: vk::Device::allocateMemory: {exception}"),
                                ]
                                    };
                                    for line in lines {
                                        sink.emit(crate::process::ProcessEvent::Line {
                                            reference: line_ref.clone(),
                                            name: "local".into(),
                                            pid: line_pid,
                                            stream: crate::process::OutputStream::Stderr,
                                            line,
                                        });
                                    }
                                }
                                return Ok(drain_complete.then_some(0));
                            }
                            exited = true;
                            Ok(Some(0))
                        }),
                        Box::new(|_| Ok(())),
                    ))
                })),
                warmup_probe_fn: Some(Box::new(|_, _| WarmupHealth::Loading)),
                verify_artifacts_fn: Some(Box::new(|_, _| true)),
            });
            let outcome = start_local_windows(
                &shared,
                &clock,
                &launch,
                &state,
                &fence,
                Some(root.path()),
                Duration::from_secs(1),
                Duration::from_millis(10),
            );
            assert_eq!(outcome.status, LaunchOutcomeStatus::Exited);
            assert_eq!(outcome.reason_code.as_str(), expected_reason);
            assert_eq!(spawn_count.load(std::sync::atomic::Ordering::SeqCst), 1);
        }

        test_windows_hooks::clear_hooks();
    }
    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn authenticated_warmup_bounds_progress_and_response_bytes() {
        for (body_bytes, drip, expected) in [
            (0, false, WarmupHealth::Ready),
            (70 * 1024, false, WarmupHealth::Failed),
            (100, true, WarmupHealth::Failed),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let port = listener.local_addr().unwrap().port();
            let peer = thread::spawn(move || {
                let accept_deadline = Instant::now() + Duration::from_secs(3);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(pair) => break pair,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= accept_deadline {
                                return;
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("warmup fixture accept: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 512];
                let _ = stream.read(&mut request);
                if stream
                    .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                    .is_err()
                {
                    return;
                }
                if drip {
                    for _ in 0..body_bytes {
                        if stream.write_all(b"x").is_err() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                } else {
                    let _ = stream.write_all(&vec![b'x'; body_bytes]);
                }
            });
            let budget = if drip {
                Duration::from_millis(80)
            } else {
                Duration::from_secs(2)
            };
            assert_eq!(
                warmup_props_probe_until(port, "token", Instant::now() + budget),
                expected
            );
            peer.join().unwrap();
        }
    }
}
