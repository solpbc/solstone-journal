// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Parakeet truth observation and launch-plan staging.
//!
//! Derives pinned paths and checks that the resolved backend binary and model
//! are regular files. When this seam has an installer, Linux also does a
//! presence check of the current CPU server, Vulkan server, and model
//! manifests and, for a journal that already installed a Parakeet model, starts
//! that installer once if a current pin is missing or mismatched. Presence
//! checks do not hash artifact bytes or probe the binary. Vulkan devices come
//! from the packaged probe helper; `decide_parakeet_auto_placement` /
//! `is_local_provider_needed` co-location remains follow-up work. File checks
//! and the CPU fallback stay the runtime-readiness decision. Starting the
//! installer does not mark the runtime ready.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use serde_json::{Map, Value, json};
use solstone_core_brain::{CanonicalInput, canonical_json, fingerprint_sha256};
use solstone_core_journal_config::read_journal_config;
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};
#[cfg(windows)]
use solstone_core_local::install::parakeet_readiness::verified_windows_parakeet_package;
#[cfg(not(windows))]
use solstone_core_local::install::pins;
use solstone_core_local::plan::VulkanDevice;
#[cfg(not(windows))]
use solstone_core_local::select_device;

use crate::stt_backend_choice::configured_stt_backend;

use super::admission::{ParakeetAdmissionInput, parakeet_stt_admission_latch};
use super::local_follow::LocalInstallerLauncher;
use super::model::{
    ProviderFence, ProviderName, ProviderRuntimeState, ProviderTruthObservation, ReasonCode,
    RuntimePhase,
};
use super::parakeet::{ParakeetLaunchConfig, ParakeetPlacement, ParakeetRuntimeShared};
use super::parakeet_follow::ParakeetFollow;
use super::parakeet_truth::{
    admission_blocked_observation, admission_not_desired_observation, parakeet_platform_can_host,
    platform_cannot_host_not_desired,
};
use super::seams::{RuntimeStoreError, TruthObservationSeam};

const GIB: u64 = 1024 * 1024 * 1024;
const LINUX_LOCAL_FLOOR_BYTES: u64 = 4 * GIB;
const WINDOWS_LOCAL_FLOOR_BYTES: u64 = 4 * GIB;
const DARWIN_ARM64_LOCAL_FLOOR_BYTES: u64 = 0;
const PARAKEET_ATT_CONTEXT_ENV: &str = "PARAKEET_ATT_CONTEXT";
const PARAKEET_ATT_CONTEXT: &str = "128";

#[derive(Clone)]
pub struct ParakeetTruthConfig {
    pub journal_path: PathBuf,
    pub platform: String,
    pub machine: String,
    pub vulkan_devices: Vec<VulkanDevice>,
}

pub struct ParakeetTruthSeam {
    shared: Arc<ParakeetRuntimeShared>,
    config: ParakeetTruthConfig,
    follow: Option<Arc<ParakeetFollow>>,
}

struct ResolvedParakeetPaths {
    binary_path: PathBuf,
    model_path: PathBuf,
    package_root: Option<PathBuf>,
}

struct ParakeetLaunchMetadata {
    journal_path: PathBuf,
    threads: u32,
    desired_fingerprint_json: String,
    desired_fingerprint_sha256: String,
}

impl ParakeetTruthSeam {
    pub fn new(shared: Arc<ParakeetRuntimeShared>, journal_path: impl Into<PathBuf>) -> Self {
        Self::new_with(
            shared,
            journal_path,
            std::env::consts::OS,
            std::env::consts::ARCH,
            crate::vulkan_observe::observe_vulkan_devices,
        )
    }

    pub fn new_with<V>(
        shared: Arc<ParakeetRuntimeShared>,
        journal_path: impl Into<PathBuf>,
        platform: &str,
        machine: &str,
        vulkan_observe: V,
    ) -> Self
    where
        V: FnOnce() -> crate::vulkan_observe::VulkanObservation,
    {
        let vulkan_devices = if platform.eq_ignore_ascii_case("windows") {
            Vec::new()
        } else {
            vulkan_observe().devices
        };
        Self::with_config(
            shared,
            ParakeetTruthConfig {
                journal_path: journal_path.into(),
                platform: platform.to_owned(),
                machine: machine.to_owned(),
                vulkan_devices,
            },
        )
    }

    pub fn with_config(shared: Arc<ParakeetRuntimeShared>, config: ParakeetTruthConfig) -> Self {
        Self {
            shared,
            config,
            follow: None,
        }
    }

    #[must_use]
    pub fn with_installer(mut self, launcher: LocalInstallerLauncher) -> Self {
        self.follow = Some(Arc::new(ParakeetFollow::new(launcher)));
        self
    }
}

#[allow(dead_code)]
pub fn windows_parakeet_placement() -> (&'static str, &'static str) {
    ("cpu", "cpu")
}

impl TruthObservationSeam for ParakeetTruthSeam {
    fn dispatch_truth(&mut self, _: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let config = self.config.clone();
        let follow = self.follow.clone();
        let fence = fence.clone();
        thread::spawn(move || {
            let outcome = observe_parakeet_truth(&shared, &config, follow.as_deref());
            shared.record_truth_result(&fence, outcome);
        });
    }
}

pub fn resolve_parakeet_backend(
    config_device: &str,
    selected_gpu: Option<&VulkanDevice>,
) -> (String, BTreeMap<String, String>, Option<u32>) {
    debug_assert!(matches!(config_device, "auto" | "cpu"));
    let mut env_updates = BTreeMap::from([(
        PARAKEET_ATT_CONTEXT_ENV.to_owned(),
        PARAKEET_ATT_CONTEXT.to_owned(),
    )]);
    if config_device == "cpu" {
        return ("cpu".to_owned(), env_updates, None);
    }
    let Some(gpu) = selected_gpu else {
        return ("cpu".to_owned(), env_updates, None);
    };
    env_updates.insert("GGML_VK_VISIBLE_DEVICES".to_owned(), gpu.index.to_string());
    ("vulkan".to_owned(), env_updates, Some(gpu.index))
}

pub fn parakeet_physical_thread_count() -> u32 {
    if cfg!(target_os = "linux")
        && let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo")
        && let Some(count) = physical_core_count_from_cpuinfo(&cpuinfo)
    {
        return count;
    }
    u32::try_from(
        std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1),
    )
    .unwrap_or(u32::MAX)
    .max(1)
}

fn physical_core_count_from_cpuinfo(cpuinfo: &str) -> Option<u32> {
    let mut pairs = BTreeSet::new();
    for stanza in cpuinfo.split("\n\n") {
        let mut physical_id = None;
        let mut core_id = None;
        for line in stanza.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            match key.trim() {
                "physical id" => physical_id = Some(value.trim()),
                "core id" => core_id = Some(value.trim()),
                _ => {}
            }
        }
        if let (Some(physical_id), Some(core_id)) = (physical_id, core_id)
            && !physical_id.is_empty()
            && !core_id.is_empty()
        {
            pairs.insert((physical_id.to_owned(), core_id.to_owned()));
        }
    }
    u32::try_from(pairs.len()).ok().filter(|count| *count > 0)
}

fn observe_parakeet_truth(
    shared: &ParakeetRuntimeShared,
    config: &ParakeetTruthConfig,
    follow: Option<&ParakeetFollow>,
) -> ProviderTruthObservation {
    if !parakeet_platform_can_host(&config.platform, &config.machine) {
        return platform_cannot_host_not_desired(&config.platform);
    }
    if !config.journal_path.is_dir() {
        return unavailable_observation("record-unavailable");
    }

    let journal_config = match read_journal_config(&config.journal_path) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(_) => return unavailable_observation("record-unavailable"),
    };
    let transcribe = journal_config
        .get("transcribe")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let configured_backend = match configured_stt_backend(&journal_config) {
        Ok(backend) => backend.map(ToOwned::to_owned),
        Err(_) => return unavailable_observation("truth-observation-failed"),
    };
    let admission_input = ParakeetAdmissionInput {
        platform: config.platform.clone(),
        machine: config.machine.clone(),
        backend: configured_backend,
        local_backend: local_stt_backend(&config.platform, &config.machine).map(ToOwned::to_owned),
        floor_bytes: platform_floor_bytes(&config.platform, &config.machine),
        confidential_lane_active: confidential_channel_plausible(&journal_config),
        confidential_audio_enabled: confidential_audio_enabled(&transcribe),
    };
    let latch = match parakeet_stt_admission_latch(
        &config.journal_path,
        &admission_input,
        &crate::memory_admission::available_physical_bytes,
    ) {
        Ok(latch) => latch,
        Err(RuntimeStoreError::Corrupt) => return corrupt_observation(),
        Err(RuntimeStoreError::Unavailable | RuntimeStoreError::Conflict) => {
            return unavailable_observation("record-unavailable");
        }
    };
    if latch.blocked {
        return admission_blocked_observation(&latch);
    }
    if !latch.desired {
        return admission_not_desired_observation(&latch);
    }

    #[cfg(windows)]
    {
        let _ = follow;
        return observe_windows_parakeet_truth(shared, config, &latch);
    }

    #[cfg(not(windows))]
    {
        let Ok(artifact_key) = pins::parakeet_artifact_key(&config.platform, &config.machine)
        else {
            return platform_cannot_host_not_desired(&config.platform);
        };

        let follow_decision = follow.map(|f| f.observe(&config.journal_path, &artifact_key));
        let attach_pin_follow = |mut obs: ProviderTruthObservation| {
            if let Some(decision) = &follow_decision {
                let follow_str = match decision {
                    super::parakeet_follow::ParakeetFollowDecision::Launch => "launch",
                    super::parakeet_follow::ParakeetFollowDecision::Hold(reason) => reason.as_str(),
                };
                let mut detail_map = obs
                    .detail
                    .and_then(|v| match v {
                        Value::Object(map) => Some(map),
                        _ => None,
                    })
                    .unwrap_or_default();
                detail_map.insert(
                    "pin_follow".to_owned(),
                    Value::String(follow_str.to_owned()),
                );
                obs.detail = Some(Value::Object(detail_map));
            }
            obs
        };

        let Some((fingerprint_json, fingerprint_sha256)) =
            parakeet_target_fingerprint(&config.journal_path, &artifact_key)
        else {
            return attach_pin_follow(unavailable_observation("truth-observation-failed"));
        };
        let (config_device, invalid_device) = configured_parakeet_device(&transcribe);
        if let Some(value) = invalid_device {
            eprintln!("{}", invalid_device_warning(&value));
        }
        let (selected_gpu, auto_without_gpu) = selected_gpu(&config_device, &config.vulkan_devices);
        if auto_without_gpu {
            eprintln!("{}", auto_without_gpu_warning());
        }
        let (mut backend, mut env_updates, mut gpu_index) =
            resolve_parakeet_backend(&config_device, selected_gpu);
        let mut paths = resolved_parakeet_paths(&config.journal_path, &artifact_key, &backend);
        if backend == "vulkan" {
            let vulkan_ready = paths.as_ref().is_some_and(|candidate| {
                regular_files_exist(&candidate.binary_path, &candidate.model_path).unwrap_or(false)
            });
            if !vulkan_ready {
                // A GPU is present, but the Vulkan binary is not installed. Stay
                // on CPU rather than reporting artifact-missing for transcription.
                let cpu = resolve_parakeet_backend("cpu", None);
                backend = cpu.0;
                env_updates = cpu.1;
                gpu_index = cpu.2;
                paths = resolved_parakeet_paths(&config.journal_path, &artifact_key, &backend);
            }
        }
        let Some(paths) = paths else {
            return attach_pin_follow(unavailable_observation("truth-observation-failed"));
        };
        match regular_files_exist(&paths.binary_path, &paths.model_path) {
            Ok(true) => {}
            Ok(false) => {
                return attach_pin_follow(artifact_missing_observation(
                    &fingerprint_json,
                    &fingerprint_sha256,
                ));
            }
            Err(_) => return attach_pin_follow(unavailable_observation("record-unavailable")),
        }

        let launch = build_parakeet_launch_config(
            backend.clone(),
            env_updates,
            gpu_index,
            paths,
            ParakeetLaunchMetadata {
                journal_path: config.journal_path.clone(),
                threads: parakeet_physical_thread_count(),
                desired_fingerprint_json: fingerprint_json.clone(),
                desired_fingerprint_sha256: fingerprint_sha256.clone(),
            },
        );
        shared.record_launch_request(Some(fingerprint_sha256.clone()), launch);
        let Some((after_json, after_sha256)) =
            parakeet_target_fingerprint(&config.journal_path, &artifact_key)
        else {
            return attach_pin_follow(unavailable_observation("truth-observation-failed"));
        };
        if after_sha256 != fingerprint_sha256 {
            return attach_pin_follow(ProviderTruthObservation {
                provider: ProviderName::Parakeet,
                phase: RuntimePhase::StateUnavailable,
                reason_code: Some(ReasonCode::known("observation-raced")),
                desired_fingerprint: None,
                has_plan: false,
                boot_required: true,
                detail: Some(json!({"before": fingerprint_sha256, "after": after_sha256})),
            });
        }
        attach_pin_follow(ProviderTruthObservation {
            provider: ProviderName::Parakeet,
            phase: RuntimePhase::Starting,
            reason_code: Some(ReasonCode::known("launch-requested")),
            desired_fingerprint: Some(after_sha256),
            has_plan: true,
            boot_required: true,
            detail: Some(json!({
                "backend": backend,
                "placement": if backend == "vulkan" { "gpu" } else { "cpu" },
                "stt_admission_latch": latch.to_json(),
                "target_fingerprint_json": after_json,
            })),
        })
    }
}

#[cfg(not(windows))]
fn configured_parakeet_device(transcribe: &Map<String, Value>) -> (String, Option<String>) {
    let value = transcribe
        .get("parakeet-cpp")
        .and_then(Value::as_object)
        .and_then(|parakeet| parakeet.get("device"));
    match value {
        None => ("auto".to_owned(), None),
        Some(Value::String(device)) if matches!(device.as_str(), "auto" | "cpu") => {
            (device.clone(), None)
        }
        Some(value) => ("auto".to_owned(), Some(value.to_string())),
    }
}

#[cfg(not(windows))]
fn invalid_device_warning(value: &str) -> String {
    format!(
        "supervisor: WARN: invalid transcribe.parakeet-cpp.device={value}; defaulting to \"auto\""
    )
}

#[cfg(not(windows))]
fn auto_without_gpu_warning() -> &'static str {
    "supervisor: WARN: transcribe.parakeet-cpp.device=\"auto\" has no Vulkan GPU available; falling back to \"cpu\""
}

#[cfg(not(windows))]
fn selected_gpu<'a>(
    config_device: &str,
    vulkan_devices: &'a [VulkanDevice],
) -> (Option<&'a VulkanDevice>, bool) {
    if config_device != "auto" {
        return (None, false);
    }
    let selected = select_device(vulkan_devices, None).and_then(|picked| {
        vulkan_devices
            .iter()
            .find(|device| device.index == picked.index)
    });
    (selected, selected.is_none())
}

fn build_parakeet_launch_config(
    binary_backend: String,
    env_updates: BTreeMap<String, String>,
    gpu_index: Option<u32>,
    paths: ResolvedParakeetPaths,
    metadata: ParakeetLaunchMetadata,
) -> ParakeetLaunchConfig {
    let placement = if binary_backend == "vulkan" {
        ParakeetPlacement::Gpu
    } else {
        ParakeetPlacement::Cpu
    };
    ParakeetLaunchConfig {
        binary_backend,
        env_updates,
        gpu_index,
        binary_path: paths.binary_path,
        model_path: paths.model_path,
        package_root: paths.package_root,
        journal_path: metadata.journal_path,
        threads: metadata.threads,
        desired_fingerprint_json: metadata.desired_fingerprint_json,
        desired_fingerprint_sha256: metadata.desired_fingerprint_sha256,
        placement,
    }
}

#[cfg(not(windows))]
fn parakeet_target_fingerprint(
    journal_path: &Path,
    artifact_key: &str,
) -> Option<(String, String)> {
    let cpu = pins::parakeet_backend_identity(artifact_key, "cpu")?;
    let vulkan = pins::parakeet_backend_identity(artifact_key, "vulkan")?;
    let target = json!({
        "provider": "parakeet",
        "runtime": "parakeet.cpp",
        "artifact_key": artifact_key,
        "binary_pins": [cpu, vulkan],
        "model_pin": pins::parakeet_model_identity(),
        "cache_root": pins::parakeet_cache_root(journal_path).display().to_string(),
        "launch_env": {PARAKEET_ATT_CONTEXT_ENV: PARAKEET_ATT_CONTEXT},
    });
    let input_json = canonical_json(&CanonicalInput::Json(target)).ok()?;
    let input_sha256 = fingerprint_sha256(&input_json);
    Some((input_json, input_sha256))
}

#[cfg(not(windows))]
fn path_from_value(paths: &Value, key: &str) -> Option<PathBuf> {
    paths.get(key)?.as_str().map(PathBuf::from)
}

#[cfg(not(windows))]
fn resolved_parakeet_paths(
    journal_path: &Path,
    artifact_key: &str,
    backend: &str,
) -> Option<ResolvedParakeetPaths> {
    let paths = pins::parakeet_paths(journal_path, artifact_key);
    let binary_key = if backend == "vulkan" {
        "binary_path_vulkan"
    } else {
        "binary_path_cpu"
    };
    Some(ResolvedParakeetPaths {
        binary_path: path_from_value(&paths, binary_key)?,
        model_path: path_from_value(&paths, "model_path")?,
        package_root: None,
    })
}

#[cfg(windows)]
fn observe_windows_parakeet_truth(
    shared: &ParakeetRuntimeShared,
    config: &ParakeetTruthConfig,
    latch: &super::admission::ParakeetAdmissionLatch,
) -> ProviderTruthObservation {
    let package = match verified_windows_parakeet_package() {
        Ok(package) => package,
        Err(_) => return unavailable_observation("artifact-missing"),
    };
    let paths = ResolvedParakeetPaths {
        binary_path: package.server,
        model_path: package.model,
        package_root: Some(package.package_root),
    };
    match regular_files_exist(&paths.binary_path, &paths.model_path) {
        Ok(true) => {}
        Ok(false) => return unavailable_observation("artifact-missing"),
        Err(_) => return unavailable_observation("record-unavailable"),
    }
    let target = json!({
        "provider": "parakeet",
        "runtime": "parakeet.cpp",
        "artifact_key": "x86_64-pc-windows-msvc",
        "package_root": paths.package_root.as_ref().map(|path| path.display().to_string()),
        "server": paths.binary_path.display().to_string(),
        "model": paths.model_path.display().to_string(),
        "launch_env": {PARAKEET_ATT_CONTEXT_ENV: PARAKEET_ATT_CONTEXT},
    });
    let Some((fingerprint_json, fingerprint_sha256)) =
        canonical_json(&CanonicalInput::Json(target))
            .ok()
            .map(|json| {
                let sha256 = fingerprint_sha256(&json);
                (json, sha256)
            })
    else {
        return unavailable_observation("truth-observation-failed");
    };
    let (backend, placement) = windows_parakeet_placement();
    let launch = build_parakeet_launch_config(
        backend.to_owned(),
        BTreeMap::from([(
            PARAKEET_ATT_CONTEXT_ENV.to_owned(),
            PARAKEET_ATT_CONTEXT.to_owned(),
        )]),
        None,
        paths,
        ParakeetLaunchMetadata {
            journal_path: config.journal_path.clone(),
            threads: parakeet_physical_thread_count(),
            desired_fingerprint_json: fingerprint_json.clone(),
            desired_fingerprint_sha256: fingerprint_sha256.clone(),
        },
    );
    shared.record_launch_request(Some(fingerprint_sha256.clone()), launch);
    ProviderTruthObservation {
        provider: ProviderName::Parakeet,
        phase: RuntimePhase::Starting,
        reason_code: Some(ReasonCode::known("launch-requested")),
        desired_fingerprint: Some(fingerprint_sha256),
        has_plan: true,
        boot_required: true,
        detail: Some(json!({
            "backend": backend,
            "placement": placement,
            "stt_admission_latch": latch.to_json(),
            "target_fingerprint_json": fingerprint_json,
        })),
    }
}

fn regular_files_exist(binary_path: &Path, model_path: &Path) -> Result<bool, std::io::Error> {
    for path in [binary_path, model_path] {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

fn platform_floor_bytes(platform: &str, machine: &str) -> Option<u64> {
    match (platform, machine) {
        ("windows", "x86_64") => Some(WINDOWS_LOCAL_FLOOR_BYTES),
        ("darwin", "arm64") => Some(DARWIN_ARM64_LOCAL_FLOOR_BYTES),
        (platform, "x86_64" | "aarch64" | "arm64") if platform.starts_with("linux") => {
            Some(LINUX_LOCAL_FLOOR_BYTES)
        }
        _ => None,
    }
}

fn local_stt_backend(platform: &str, machine: &str) -> Option<&'static str> {
    platform_floor_bytes(platform, machine).map(|_| "parakeet")
}

fn confidential_channel_plausible(config: &Map<String, Value>) -> bool {
    let confidential = config
        .get("services")
        .and_then(Value::as_object)
        .and_then(|services| services.get("confidential"))
        .is_some_and(Value::is_object);
    confidential
        && matches!(
            resolve_local_endpoint(config),
            LocalEndpointResolution::Byo(endpoint)
                if endpoint.is_confidential && endpoint.credential.is_some()
        )
}

fn confidential_audio_enabled(transcribe: &Map<String, Value>) -> bool {
    transcribe
        .get("confidential_audio")
        .is_none_or(|value| value.as_bool().unwrap_or(false))
}

#[cfg(not(windows))]
fn artifact_missing_observation(
    fingerprint_json: &str,
    fingerprint_sha256: &str,
) -> ProviderTruthObservation {
    ProviderTruthObservation {
        provider: ProviderName::Parakeet,
        phase: RuntimePhase::ArtifactNotReady,
        reason_code: Some(ReasonCode::known("artifact-missing")),
        desired_fingerprint: Some(fingerprint_sha256.to_owned()),
        has_plan: false,
        boot_required: true,
        detail: Some(json!({"target_fingerprint_json": fingerprint_json})),
    }
}

fn corrupt_observation() -> ProviderTruthObservation {
    ProviderTruthObservation {
        provider: ProviderName::Parakeet,
        phase: RuntimePhase::StateCorrupt,
        reason_code: Some(ReasonCode::known("record-malformed")),
        desired_fingerprint: None,
        has_plan: false,
        boot_required: true,
        detail: None,
    }
}

fn unavailable_observation(reason_code: &'static str) -> ProviderTruthObservation {
    ProviderTruthObservation {
        provider: ProviderName::Parakeet,
        phase: RuntimePhase::StateUnavailable,
        reason_code: Some(ReasonCode::known(reason_code)),
        desired_fingerprint: None,
        has_plan: false,
        boot_required: true,
        detail: None,
    }
}

#[cfg(test)]
mod windows_stt_selection_tests {
    use super::*;

    #[test]
    fn windows_default_latch_uses_cpu_floor_and_preserves_memory_refusal() {
        let input = ParakeetAdmissionInput {
            platform: "windows".to_owned(),
            machine: "x86_64".to_owned(),
            backend: None,
            local_backend: local_stt_backend("windows", "x86_64").map(ToOwned::to_owned),
            floor_bytes: platform_floor_bytes("windows", "x86_64"),
            confidential_lane_active: false,
            confidential_audio_enabled: false,
        };
        assert_eq!(input.local_backend.as_deref(), Some("parakeet"));
        assert_eq!(input.floor_bytes, Some(4 * GIB));
        assert_eq!(local_stt_backend("windows", "aarch64"), None);
        for (available, desired, blocked) in [
            (Some(4 * GIB), true, false),
            (Some(4 * GIB - 1), false, true),
            (Some(0), false, true),
            (None, false, true),
        ] {
            let journal = tempfile::tempdir().unwrap();
            let latch =
                parakeet_stt_admission_latch(journal.path(), &input, &|| available).unwrap();
            assert_eq!((latch.desired, latch.blocked), (desired, blocked));
        }
        for backend in ["parakeet", "parakeet-cpp"] {
            let journal = tempfile::tempdir().unwrap();
            let mut explicit = input.clone();
            explicit.backend = Some(backend.to_owned());
            let latch = parakeet_stt_admission_latch(journal.path(), &explicit, &|| {
                panic!("explicit backend must preserve the existing memory-read bypass")
            })
            .unwrap();
            assert!(latch.desired);
            assert!(!latch.blocked);
        }
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    fn gpu(index: u32) -> VulkanDevice {
        VulkanDevice {
            index,
            name: "fixture".to_owned(),
            device_type: None,
            vram_mib: 8_000,
        }
    }

    #[test]
    fn cpu_backend_ignores_an_injected_gpu() {
        let (backend, env, index) = resolve_parakeet_backend("cpu", Some(&gpu(2)));
        assert_eq!(backend, "cpu");
        assert_eq!(
            env,
            BTreeMap::from([(
                PARAKEET_ATT_CONTEXT_ENV.to_owned(),
                PARAKEET_ATT_CONTEXT.to_owned(),
            )])
        );
        assert_eq!(index, None);
    }

    #[test]
    fn auto_backend_uses_an_injected_gpu() {
        let gpu = gpu(2);
        let (backend, env, index) = resolve_parakeet_backend("auto", Some(&gpu));
        assert_eq!(backend, "vulkan");
        assert_eq!(
            env,
            BTreeMap::from([
                (
                    PARAKEET_ATT_CONTEXT_ENV.to_owned(),
                    PARAKEET_ATT_CONTEXT.to_owned(),
                ),
                ("GGML_VK_VISIBLE_DEVICES".to_owned(), "2".to_owned()),
            ])
        );
        assert_eq!(index, Some(2));
    }

    #[test]
    fn auto_backend_without_an_injected_gpu_uses_cpu() {
        let (backend, env, index) = resolve_parakeet_backend("auto", None);
        assert_eq!(backend, "cpu");
        assert_eq!(
            env,
            BTreeMap::from([(
                PARAKEET_ATT_CONTEXT_ENV.to_owned(),
                PARAKEET_ATT_CONTEXT.to_owned(),
            )])
        );
        assert_eq!(index, None);
    }

    #[test]
    fn target_fingerprint_carries_the_forced_attention_context() {
        let (fingerprint_json, _) =
            parakeet_target_fingerprint(Path::new("/fixture-journal"), "x86_64-unknown-linux-gnu")
                .expect("fingerprint");
        let fingerprint: Value = serde_json::from_str(&fingerprint_json).expect("json");
        assert_eq!(
            fingerprint["launch_env"][PARAKEET_ATT_CONTEXT_ENV].as_str(),
            Some(PARAKEET_ATT_CONTEXT)
        );
    }

    #[test]
    fn invalid_device_is_normalized_and_marked_for_warning() {
        let config = Map::from_iter([("parakeet-cpp".to_owned(), json!({"device": "bogus"}))]);
        let (device, warning_value) = configured_parakeet_device(&config);
        assert_eq!(device, "auto");
        assert_eq!(warning_value.as_deref(), Some("\"bogus\""));
        assert_eq!(
            invalid_device_warning(warning_value.as_deref().expect("warning value")),
            "supervisor: WARN: invalid transcribe.parakeet-cpp.device=\"bogus\"; defaulting to \"auto\""
        );
    }

    #[test]
    fn auto_without_gpu_is_marked_for_warning() {
        assert_eq!(selected_gpu("auto", &[]), (None, true));
        assert_eq!(
            auto_without_gpu_warning(),
            "supervisor: WARN: transcribe.parakeet-cpp.device=\"auto\" has no Vulkan GPU available; falling back to \"cpu\""
        );
    }

    #[test]
    fn selected_gpu_prefers_hardware_over_software() {
        let devices = [
            VulkanDevice {
                index: 0,
                name: "llvmpipe (LLVM 19.1.7, 256 bits)".to_owned(),
                device_type: Some(4),
                vram_mib: 31_752,
            },
            VulkanDevice {
                index: 1,
                name: "Intel(R) Graphics (RKL GT1)".to_owned(),
                device_type: Some(1),
                vram_mib: 15_876,
            },
        ];
        let (selected, auto_without) = selected_gpu("auto", &devices);
        assert_eq!(selected.map(|device| device.index), Some(1));
        assert!(!auto_without);
        assert_eq!(selected_gpu("cpu", &devices), (None, false));
    }

    #[test]
    fn physical_core_parser_distinguishes_sockets() {
        let cpuinfo = "physical id : 0\ncore id : 0\n\nphysical id : 0\ncore id : 1\n\nphysical id : 1\ncore id : 0\n";
        assert_eq!(physical_core_count_from_cpuinfo(cpuinfo), Some(3));
    }

    #[test]
    fn physical_core_parser_deduplicates_logical_processors_on_one_socket() {
        let cpuinfo = "physical id : 0\ncore id : 0\n\nphysical id : 0\ncore id : 0\n\nphysical id : 0\ncore id : 1\n";
        assert_eq!(physical_core_count_from_cpuinfo(cpuinfo), Some(2));
    }

    #[test]
    fn physical_core_parser_rejects_incomplete_input() {
        assert_eq!(physical_core_count_from_cpuinfo("processor : 0\n"), None);
    }

    #[test]
    fn plan_builder_maps_vulkan_backend_to_gpu_placement_and_cpu_to_cpu_placement() {
        let journal = PathBuf::from("/fixture-journal");
        let vulkan_paths = resolved_parakeet_paths(&journal, "x86_64-unknown-linux-gnu", "vulkan")
            .expect("pinned vulkan paths");
        let vulkan_launch = build_parakeet_launch_config(
            "vulkan".to_owned(),
            BTreeMap::from([("GGML_VK_VISIBLE_DEVICES".to_owned(), "0".to_owned())]),
            Some(0),
            vulkan_paths,
            ParakeetLaunchMetadata {
                journal_path: journal.clone(),
                threads: 8,
                desired_fingerprint_json: "{}".to_owned(),
                desired_fingerprint_sha256: "fingerprint".to_owned(),
            },
        );
        assert_eq!(vulkan_launch.placement, ParakeetPlacement::Gpu);

        let cpu_paths = resolved_parakeet_paths(&journal, "x86_64-unknown-linux-gnu", "cpu")
            .expect("pinned cpu paths");
        let cpu_launch = build_parakeet_launch_config(
            "cpu".to_owned(),
            BTreeMap::new(),
            None,
            cpu_paths,
            ParakeetLaunchMetadata {
                journal_path: journal.clone(),
                threads: 8,
                desired_fingerprint_json: "{}".to_owned(),
                desired_fingerprint_sha256: "fingerprint".to_owned(),
            },
        );
        assert_eq!(cpu_launch.placement, ParakeetPlacement::Cpu);
    }

    #[test]
    fn dispatch_truth_re_fires_and_records_two_independent_results() {
        let shared = Arc::new(ParakeetRuntimeShared::default());
        let mut seam = ParakeetTruthSeam::with_config(
            shared.clone(),
            ParakeetTruthConfig {
                journal_path: PathBuf::from("/nonexistent-journal-for-unhosted-platform-test"),
                platform: "plan9".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        );
        let state = ProviderRuntimeState::new(ProviderName::Parakeet);
        let fence_of = |attempt: u32| ProviderFence {
            incarnation: "incarnation".to_owned(),
            generation: 4,
            fingerprint: None,
            attempt,
        };

        // Two independent dispatch cycles on the same real (non-fixture) seam,
        // matching how the reconciler re-fires truth observation on its
        // cadence. An unhosted platform short-circuits to a fast, host-independent
        // result so this test does not depend on real host state.
        let first_fence = fence_of(0);
        seam.dispatch_truth(&state, &first_fence);
        let first = shared.wait_for_truth_result(&first_fence);
        assert_eq!(first.phase, RuntimePhase::NotDesired);
        assert_eq!(
            first.reason_code.as_ref().map(ReasonCode::as_str),
            Some("provider-not-needed")
        );

        let second_fence = fence_of(1);
        seam.dispatch_truth(&state, &second_fence);
        let second = shared.wait_for_truth_result(&second_fence);
        assert_eq!(second.phase, RuntimePhase::NotDesired);
        assert_eq!(
            second.reason_code.as_ref().map(ReasonCode::as_str),
            Some("provider-not-needed")
        );

        // Each cycle's result was recorded and retrieved independently under
        // its own fence -- proving the seam and its shared result channel
        // support re-firing, not a single one-shot dispatch.
        assert!(shared.take_truth_result(&first_fence).is_none());
        assert!(shared.take_truth_result(&second_fence).is_none());
    }

    #[test]
    fn plan_builder_uses_injected_threads_and_pinned_paths() {
        let journal = PathBuf::from("/fixture-journal");
        let paths = resolved_parakeet_paths(&journal, "x86_64-unknown-linux-gnu", "cpu")
            .expect("pinned paths");
        let binary_path = paths.binary_path.clone();
        let model_path = paths.model_path.clone();
        let launch = build_parakeet_launch_config(
            "cpu".to_owned(),
            BTreeMap::new(),
            None,
            paths,
            ParakeetLaunchMetadata {
                journal_path: PathBuf::from("/fixture-journal"),
                threads: 37,
                desired_fingerprint_json: "{}".to_owned(),
                desired_fingerprint_sha256: "fingerprint".to_owned(),
            },
        );
        assert_eq!(launch.threads, 37);
        assert_eq!(launch.binary_path, binary_path);
        assert_eq!(launch.model_path, model_path);
        assert!(
            launch
                .binary_path
                .starts_with(journal.join("cache/providers/parakeet"))
        );
        assert!(
            launch
                .model_path
                .starts_with(journal.join("cache/providers/parakeet"))
        );
        assert_ne!(launch.binary_path, PathBuf::from("parakeet-server"));
        assert_ne!(launch.model_path, PathBuf::from("parakeet"));
    }

    #[test]
    fn windows_parakeet_skips_vulkan_observation_and_forces_cpu_placement() {
        let mut vulkan_called = 0;
        let shared = Arc::new(ParakeetRuntimeShared::default());
        let seam =
            ParakeetTruthSeam::new_with(shared, "/fixture-journal", "windows", "x86_64", || {
                vulkan_called += 1;
                crate::vulkan_observe::VulkanObservation {
                    devices: Vec::new(),
                    succeeded: true,
                }
            });
        assert_eq!(vulkan_called, 0);
        assert!(seam.config.vulkan_devices.is_empty());
        assert_eq!(windows_parakeet_placement(), ("cpu", "cpu"));
    }
}

#[cfg(all(test, feature = "full-tests", not(windows)))]
mod composition_tests {
    use super::*;
    use solstone_core_local::install::manifest::{
        artifact_manifest_path, build_manifest, write_manifest,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    fn setup_journal_with_config(device_auto: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let config_val = if device_auto {
            json!({
                "transcribe": {
                    "backend": "parakeet",
                    "parakeet-cpp": {
                        "device": "auto"
                    }
                }
            })
        } else {
            json!({
                "transcribe": {
                    "backend": "parakeet"
                }
            })
        };
        std::fs::write(
            config_dir.join("journal.json"),
            serde_json::to_vec(&config_val).expect("serialize config"),
        )
        .expect("write journal.json");
        dir
    }

    fn write_manifest_and_member(
        manifest_dir: &Path,
        unit: &str,
        pin_identity: Value,
        member_name: &str,
        content: &[u8],
    ) {
        std::fs::create_dir_all(manifest_dir).expect("manifest dir");
        let member_file = manifest_dir.join(member_name);
        std::fs::write(&member_file, content).expect("write member");

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(content);
        let sha256_hex = format!("{:02x}", hasher.finalize());

        let inventory = vec![json!({
            "relative_path": member_name,
            "role": "binary",
            "size": content.len() as u64,
            "sha256": sha256_hex,
        })];

        let manifest_val = build_manifest(
            "parakeet",
            unit,
            "test-fingerprint",
            json!({"pin_identity": pin_identity}),
            inventory,
            None,
            None,
        )
        .expect("build manifest");

        write_manifest(&artifact_manifest_path(manifest_dir), &manifest_val)
            .expect("write manifest");
    }

    fn write_old_revision_opt_in(journal: &Path) {
        let (repo, ..) = pins::PARAKEET_MODEL;
        let old_rev_dir = pins::parakeet_cache_root(journal)
            .join("models")
            .join(repo.replace('/', "__"))
            .join("old-revision-not-current");
        std::fs::create_dir_all(&old_rev_dir).expect("create old rev dir");
        let manifest_path = artifact_manifest_path(&old_rev_dir);
        std::fs::write(
            manifest_path,
            json!({
                "schema_version": 1,
                "provider": "parakeet",
                "unit": "parakeet-model",
                "inventory": []
            })
            .to_string(),
        )
        .expect("write opt-in manifest");
    }

    #[test]
    fn case_a_opt_in_only_repeated_dispatch_single_launch_then_backoff() {
        let dir = setup_journal_with_config(false);
        let journal = dir.path();
        write_old_revision_opt_in(journal);

        let shared = Arc::new(ParakeetRuntimeShared::default());
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();

        let mut seam = ParakeetTruthSeam::with_config(
            shared.clone(),
            ParakeetTruthConfig {
                journal_path: journal.to_path_buf(),
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
        .with_installer(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let state = ProviderRuntimeState::new(ProviderName::Parakeet);
        let fence1 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 1,
        };
        seam.dispatch_truth(&state, &fence1);
        let obs1 = shared.wait_for_truth_result(&fence1);
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(obs1.phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            obs1.reason_code.as_ref().map(ReasonCode::as_str),
            Some("artifact-missing")
        );
        assert_eq!(
            obs1.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("launch")
        );

        let fence2 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 2,
        };
        seam.dispatch_truth(&state, &fence2);
        let obs2 = shared.wait_for_truth_result(&fence2);
        assert_eq!(obs2.phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            obs2.reason_code.as_ref().map(ReasonCode::as_str),
            Some("artifact-missing")
        );
        assert_eq!(
            obs2.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("backoff")
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn case_b_runtime_ready_model_absent_then_present() {
        let dir = setup_journal_with_config(false);
        let journal = dir.path();
        write_old_revision_opt_in(journal);

        let key = "x86_64-unknown-linux-gnu";
        let paths = pins::parakeet_paths(journal, key);
        let cpu_binary_path = PathBuf::from(paths["binary_path_cpu"].as_str().unwrap());
        let vulkan_binary_path = PathBuf::from(paths["binary_path_vulkan"].as_str().unwrap());
        let model_path = PathBuf::from(paths["model_path"].as_str().unwrap());

        let (_, _, _, cpu_bin_name) = pins::parakeet_backend_pin(key, "cpu").unwrap();
        let (_, _, _, vulkan_bin_name) = pins::parakeet_backend_pin(key, "vulkan").unwrap();

        write_manifest_and_member(
            cpu_binary_path.parent().unwrap(),
            "parakeet-server",
            pins::parakeet_backend_identity(key, "cpu").unwrap(),
            cpu_bin_name,
            b"cpu binary bytes",
        );
        write_manifest_and_member(
            vulkan_binary_path.parent().unwrap(),
            "parakeet-server",
            pins::parakeet_backend_identity(key, "vulkan").unwrap(),
            vulkan_bin_name,
            b"vulkan binary bytes",
        );

        let shared = Arc::new(ParakeetRuntimeShared::default());
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();

        let mut seam = ParakeetTruthSeam::with_config(
            shared.clone(),
            ParakeetTruthConfig {
                journal_path: journal.to_path_buf(),
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
        .with_installer(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let state = ProviderRuntimeState::new(ProviderName::Parakeet);
        let fence1 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 1,
        };
        seam.dispatch_truth(&state, &fence1);
        let obs1 = shared.wait_for_truth_result(&fence1);
        assert_eq!(obs1.phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            obs1.reason_code.as_ref().map(ReasonCode::as_str),
            Some("artifact-missing")
        );
        assert_eq!(
            obs1.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("launch")
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);

        // Write only model file at model_path (no current model manifest)
        std::fs::create_dir_all(model_path.parent().unwrap()).expect("create model dir");
        std::fs::write(&model_path, b"model file bytes").expect("write model file");

        let fence2 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 2,
        };
        seam.dispatch_truth(&state, &fence2);
        let obs2 = shared.wait_for_truth_result(&fence2);
        assert_eq!(obs2.phase, RuntimePhase::Starting);
        assert_eq!(
            obs2.reason_code.as_ref().map(ReasonCode::as_str),
            Some("launch-requested")
        );
        assert!(obs2.has_plan);
        assert_eq!(
            obs2.detail
                .as_ref()
                .and_then(|d| d.get("placement"))
                .and_then(Value::as_str),
            Some("cpu")
        );
        assert_eq!(
            obs2.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("backoff")
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn case_c_cpu_and_model_ready_vulkan_absent_device_auto() {
        let dir = setup_journal_with_config(true);
        let journal = dir.path();
        write_old_revision_opt_in(journal);

        let key = "x86_64-unknown-linux-gnu";
        let paths = pins::parakeet_paths(journal, key);
        let cpu_binary_path = PathBuf::from(paths["binary_path_cpu"].as_str().unwrap());

        let (_, _, _, cpu_bin_name) = pins::parakeet_backend_pin(key, "cpu").unwrap();
        let (repo, filename, revision, ..) = pins::PARAKEET_MODEL;

        write_manifest_and_member(
            cpu_binary_path.parent().unwrap(),
            "parakeet-server",
            pins::parakeet_backend_identity(key, "cpu").unwrap(),
            cpu_bin_name,
            b"cpu binary bytes",
        );

        let model_dir = pins::parakeet_cache_root(journal)
            .join("models")
            .join(repo.replace('/', "__"))
            .join(revision);
        write_manifest_and_member(
            &model_dir,
            "parakeet-model",
            pins::parakeet_model_identity(),
            filename,
            b"model bytes",
        );

        let shared = Arc::new(ParakeetRuntimeShared::default());
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = called.clone();

        let mut seam = ParakeetTruthSeam::with_config(
            shared.clone(),
            ParakeetTruthConfig {
                journal_path: journal.to_path_buf(),
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
        .with_installer(Arc::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let state = ProviderRuntimeState::new(ProviderName::Parakeet);
        let fence = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 1,
        };
        seam.dispatch_truth(&state, &fence);
        let obs = shared.wait_for_truth_result(&fence);

        assert_eq!(obs.phase, RuntimePhase::Starting);
        assert_eq!(
            obs.reason_code.as_ref().map(ReasonCode::as_str),
            Some("launch-requested")
        );
        assert_eq!(
            obs.detail
                .as_ref()
                .and_then(|d| d.get("placement"))
                .and_then(Value::as_str),
            Some("cpu")
        );
        assert!(obs.has_plan);
        assert_eq!(
            obs.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("launch")
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn case_d_malformed_and_size_mismatched_manifests() {
        let key = "x86_64-unknown-linux-gnu";
        let (_, _, _, cpu_bin_name) = pins::parakeet_backend_pin(key, "cpu").unwrap();

        // Subcase 1: manifest is "{"
        let dir1 = setup_journal_with_config(false);
        let j1 = dir1.path();
        write_old_revision_opt_in(j1);
        let paths1 = pins::parakeet_paths(j1, key);
        let cpu_path1 = PathBuf::from(paths1["binary_path_cpu"].as_str().unwrap());
        let manifest_dir1 = cpu_path1.parent().unwrap();
        std::fs::create_dir_all(manifest_dir1).expect("create dir");
        std::fs::write(artifact_manifest_path(manifest_dir1), b"{").expect("write malformed");

        let shared1 = Arc::new(ParakeetRuntimeShared::default());
        let called1 = Arc::new(AtomicU32::new(0));
        let called1_clone = called1.clone();
        let mut seam1 = ParakeetTruthSeam::with_config(
            shared1.clone(),
            ParakeetTruthConfig {
                journal_path: j1.to_path_buf(),
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
        .with_installer(Arc::new(move |_| {
            called1_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let state = ProviderRuntimeState::new(ProviderName::Parakeet);
        let fence1 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 1,
        };
        seam1.dispatch_truth(&state, &fence1);
        let obs1 = shared1.wait_for_truth_result(&fence1);
        assert_eq!(obs1.phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            obs1.reason_code.as_ref().map(ReasonCode::as_str),
            Some("artifact-missing")
        );
        assert_eq!(
            obs1.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("manifest_malformed")
        );
        assert_eq!(called1.load(Ordering::SeqCst), 0);
        assert!(!j1.join("health/providers/parakeet.json").exists());
        assert!(!j1.join("health/providers/parakeet.lease").exists());

        // Subcase 2: size mismatch
        let dir2 = setup_journal_with_config(false);
        let j2 = dir2.path();
        write_old_revision_opt_in(j2);
        let paths2 = pins::parakeet_paths(j2, key);
        let cpu_path2 = PathBuf::from(paths2["binary_path_cpu"].as_str().unwrap());
        let manifest_dir2 = cpu_path2.parent().unwrap();
        std::fs::create_dir_all(manifest_dir2).expect("create dir");
        std::fs::write(manifest_dir2.join(cpu_bin_name), b"actual bytes 12").expect("write binary");

        let inventory = vec![json!({
            "relative_path": cpu_bin_name,
            "role": "binary",
            "size": 99999u64, // mismatched size
            "sha256": "fakehash",
        })];
        let manifest_val2 = build_manifest(
            "parakeet",
            "parakeet-server",
            "test-fingerprint",
            json!({"pin_identity": pins::parakeet_backend_identity(key, "cpu").unwrap()}),
            inventory,
            None,
            None,
        )
        .expect("build manifest");
        write_manifest(&artifact_manifest_path(manifest_dir2), &manifest_val2)
            .expect("write manifest");

        let shared2 = Arc::new(ParakeetRuntimeShared::default());
        let called2 = Arc::new(AtomicU32::new(0));
        let called2_clone = called2.clone();
        let mut seam2 = ParakeetTruthSeam::with_config(
            shared2.clone(),
            ParakeetTruthConfig {
                journal_path: j2.to_path_buf(),
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
        .with_installer(Arc::new(move |_| {
            called2_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let fence2 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 1,
        };
        seam2.dispatch_truth(&state, &fence2);
        let obs2 = shared2.wait_for_truth_result(&fence2);
        assert_eq!(obs2.phase, RuntimePhase::ArtifactNotReady);
        assert_eq!(
            obs2.reason_code.as_ref().map(ReasonCode::as_str),
            Some("artifact-missing")
        );
        assert_eq!(
            obs2.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("inventory_size_mismatch")
        );
        assert_eq!(called2.load(Ordering::SeqCst), 0);
        assert!(!j2.join("health/providers/parakeet.json").exists());
        assert!(!j2.join("health/providers/parakeet.lease").exists());
    }

    #[test]
    fn case_e_never_installed_and_owner_cancelled() {
        let state = ProviderRuntimeState::new(ProviderName::Parakeet);

        // Subcase 1: no model manifest (bin only)
        let dir1 = setup_journal_with_config(false);
        let j1 = dir1.path();
        let bin_dir = pins::parakeet_cache_root(j1).join("bin");
        std::fs::create_dir_all(&bin_dir).expect("create bin dir");
        std::fs::write(bin_dir.join("some_binary"), b"bin only").expect("write file");

        let shared1 = Arc::new(ParakeetRuntimeShared::default());
        let called1 = Arc::new(AtomicU32::new(0));
        let called1_clone = called1.clone();
        let mut seam1 = ParakeetTruthSeam::with_config(
            shared1.clone(),
            ParakeetTruthConfig {
                journal_path: j1.to_path_buf(),
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
        .with_installer(Arc::new(move |_| {
            called1_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let fence1 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 1,
        };
        seam1.dispatch_truth(&state, &fence1);
        let obs1 = shared1.wait_for_truth_result(&fence1);
        assert_eq!(
            obs1.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("never-installed")
        );
        assert_eq!(called1.load(Ordering::SeqCst), 0);

        // Subcase 2: old opt-in, missing manifests, failed + install_cancelled status
        let dir2 = setup_journal_with_config(false);
        let j2 = dir2.path();
        write_old_revision_opt_in(j2);

        let status_dir = j2.join("health/providers");
        std::fs::create_dir_all(&status_dir).expect("create status dir");
        let status_content = r#"{"schema_version":1,"provider":"parakeet","revision":1,"install_state":"failed","attempt_id":null,"target_fingerprint_json":null,"target_fingerprint_sha256":null,"started_at":null,"last_transition_at":"2026-09-01T12:00:00Z","last_progress_at":null,"completed_at":"2026-09-01T12:00:00Z","progress_bytes_received":null,"progress_bytes_total":null,"install_error":null,"error_code":"install_cancelled","owner":null}"#;
        std::fs::write(status_dir.join("parakeet.json"), status_content.as_bytes())
            .expect("write status");

        let shared2 = Arc::new(ParakeetRuntimeShared::default());
        let called2 = Arc::new(AtomicU32::new(0));
        let called2_clone = called2.clone();
        let mut seam2 = ParakeetTruthSeam::with_config(
            shared2.clone(),
            ParakeetTruthConfig {
                journal_path: j2.to_path_buf(),
                platform: "linux".to_owned(),
                machine: "x86_64".to_owned(),
                vulkan_devices: Vec::new(),
            },
        )
        .with_installer(Arc::new(move |_| {
            called2_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));

        let fence2 = ProviderFence {
            incarnation: "inc".to_owned(),
            generation: 1,
            fingerprint: None,
            attempt: 1,
        };
        seam2.dispatch_truth(&state, &fence2);
        let obs2 = shared2.wait_for_truth_result(&fence2);
        assert_eq!(
            obs2.detail
                .as_ref()
                .and_then(|d| d.get("pin_follow"))
                .and_then(Value::as_str),
            Some("owner-cancelled")
        );
        assert_eq!(called2.load(Ordering::SeqCst), 0);

        let read_back =
            std::fs::read_to_string(status_dir.join("parakeet.json")).expect("read status");
        assert_eq!(read_back, status_content);
        assert!(!status_dir.join("parakeet.lease").exists());
    }
}
