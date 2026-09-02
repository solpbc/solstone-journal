// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Parakeet truth observation and launch-plan staging.
//!
//! This is deliberately narrower than Python's readiness observation: it
//! derives pinned paths and checks that the resolved backend binary and model
//! are regular files, but does not yet inspect manifests, proof state, install
//! progress, or binary host eligibility. Vulkan devices come from the packaged
//! probe helper; `decide_parakeet_auto_placement` / `is_local_provider_needed`
//! co-location remains follow-up work.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
#[cfg(windows)]
use std::{ffi::OsStr, sync::OnceLock};

use serde_json::{Map, Value, json};
use solstone_core_brain::{CanonicalInput, canonical_json, fingerprint_sha256};
#[cfg(windows)]
use solstone_core_distribution::windows_payload::{
    WINDOWS_PARAKEET_MODEL, WINDOWS_PARAKEET_SERVER, verify_windows_payload,
};
use solstone_core_journal_config::read_journal_config;
use solstone_core_local::endpoint::{LocalEndpointResolution, resolve_local_endpoint};
use solstone_core_local::install::pins;
use solstone_core_local::plan::VulkanDevice;
use solstone_core_local::{detect_gpus, select_device};

use super::admission::{ParakeetAdmissionInput, parakeet_stt_admission_latch};
use super::model::{
    ProviderFence, ProviderName, ProviderRuntimeState, ProviderTruthObservation, ReasonCode,
    RuntimePhase,
};
use super::parakeet::{ParakeetLaunchConfig, ParakeetPlacement, ParakeetRuntimeShared};
use super::parakeet_truth::{
    admission_blocked_observation, admission_not_desired_observation, parakeet_platform_can_host,
    platform_cannot_host_not_desired, remote_mode_not_desired,
};
use super::seams::{RuntimeStoreError, TruthObservationSeam};

const GIB: u64 = 1024 * 1024 * 1024;
const LINUX_LOCAL_FLOOR_BYTES: u64 = 4 * GIB;
const DARWIN_ARM64_LOCAL_FLOOR_BYTES: u64 = 2 * GIB;
const PARAKEET_ATT_CONTEXT_ENV: &str = "PARAKEET_ATT_CONTEXT";
const PARAKEET_ATT_CONTEXT: &str = "128";

#[derive(Clone)]
pub struct ParakeetTruthConfig {
    pub journal_path: PathBuf,
    pub remote_mode: bool,
    pub platform: String,
    pub machine: String,
    pub vulkan_devices: Vec<VulkanDevice>,
}

pub struct ParakeetTruthSeam {
    shared: Arc<ParakeetRuntimeShared>,
    config: ParakeetTruthConfig,
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
    pub fn new(
        shared: Arc<ParakeetRuntimeShared>,
        journal_path: impl Into<PathBuf>,
        remote_mode: bool,
    ) -> Self {
        Self::with_config(
            shared,
            ParakeetTruthConfig {
                journal_path: journal_path.into(),
                remote_mode,
                platform: std::env::consts::OS.to_owned(),
                machine: std::env::consts::ARCH.to_owned(),
                vulkan_devices: detect_gpus(),
            },
        )
    }

    pub fn with_config(shared: Arc<ParakeetRuntimeShared>, config: ParakeetTruthConfig) -> Self {
        Self { shared, config }
    }
}

impl TruthObservationSeam for ParakeetTruthSeam {
    fn dispatch_truth(&mut self, _: &ProviderRuntimeState, fence: &ProviderFence) {
        let shared = Arc::clone(&self.shared);
        let config = self.config.clone();
        let fence = fence.clone();
        thread::spawn(move || {
            let outcome = observe_parakeet_truth(&shared, &config);
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
) -> ProviderTruthObservation {
    if config.remote_mode {
        return remote_mode_not_desired();
    }
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
    let admission_input = ParakeetAdmissionInput {
        platform: config.platform.clone(),
        machine: config.machine.clone(),
        backend: transcribe
            .get("backend")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        local_backend: local_stt_backend(&config.platform, &config.machine).map(ToOwned::to_owned),
        floor_bytes: platform_floor_bytes(&config.platform, &config.machine),
        confidential_lane_active: confidential_channel_plausible(&journal_config),
        confidential_audio_enabled: confidential_audio_enabled(&transcribe),
    };
    let latch = match parakeet_stt_admission_latch(
        &config.journal_path,
        &admission_input,
        &read_available_bytes,
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
    return observe_windows_parakeet_truth(shared, config, &latch);

    let Ok(artifact_key) = pins::parakeet_artifact_key(&config.platform, &config.machine) else {
        return platform_cannot_host_not_desired(&config.platform);
    };
    let Some((fingerprint_json, fingerprint_sha256)) =
        parakeet_target_fingerprint(&config.journal_path, &artifact_key)
    else {
        return unavailable_observation("truth-observation-failed");
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
        return unavailable_observation("truth-observation-failed");
    };
    match regular_files_exist(&paths.binary_path, &paths.model_path) {
        Ok(true) => {}
        Ok(false) => return artifact_missing_observation(&fingerprint_json, &fingerprint_sha256),
        Err(_) => return unavailable_observation("record-unavailable"),
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
        return unavailable_observation("truth-observation-failed");
    };
    if after_sha256 != fingerprint_sha256 {
        return ProviderTruthObservation {
            provider: ProviderName::Parakeet,
            phase: RuntimePhase::StateUnavailable,
            reason_code: Some(ReasonCode::known("observation-raced")),
            desired_fingerprint: None,
            has_plan: false,
            boot_required: true,
            detail: Some(json!({"before": fingerprint_sha256, "after": after_sha256})),
        };
    }
    ProviderTruthObservation {
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
    }
}

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

fn invalid_device_warning(value: &str) -> String {
    format!(
        "supervisor: WARN: invalid transcribe.parakeet-cpp.device={value}; defaulting to \"auto\""
    )
}

fn auto_without_gpu_warning() -> &'static str {
    "supervisor: WARN: transcribe.parakeet-cpp.device=\"auto\" has no Vulkan GPU available; falling back to \"cpu\""
}

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

fn path_from_value(paths: &Value, key: &str) -> Option<PathBuf> {
    paths.get(key)?.as_str().map(PathBuf::from)
}

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
    let package = match verified_windows_provider_package() {
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
    let launch = build_parakeet_launch_config(
        "cpu".to_owned(),
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
            "backend": "cpu",
            "placement": "cpu",
            "stt_admission_latch": latch.to_json(),
            "target_fingerprint_json": fingerprint_json,
        })),
    }
}

#[cfg(windows)]
#[derive(Clone)]
struct WindowsParakeetPackage {
    package_root: PathBuf,
    server: PathBuf,
    model: PathBuf,
}

#[cfg(windows)]
fn verified_windows_provider_package() -> Result<WindowsParakeetPackage, String> {
    static PACKAGE: OnceLock<Result<WindowsParakeetPackage, String>> = OnceLock::new();
    match PACKAGE.get_or_init(|| {
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not determine the running journal executable: {error}"))?;
        let bin = executable.parent().ok_or_else(|| {
            format!(
                "running journal executable has no containing directory: {}",
                executable.display()
            )
        })?;
        if bin.file_name() != Some(OsStr::new("bin")) {
            return Err(format!(
                "running journal executable is not in the package bin directory: {}",
                executable.display()
            ));
        }
        let package_root = bin.parent().ok_or_else(|| {
            format!(
                "package bin directory has no package root: {}",
                bin.display()
            )
        })?;
        let payload = verify_windows_payload(package_root)
            .map_err(|error| format!("could not verify the signed Parakeet app payload: {error}"))?;
        Ok(WindowsParakeetPackage {
            package_root: package_root.to_path_buf(),
            server: payload.parakeet_server_path().map_err(|error| {
                format!(
                    "signed Parakeet app payload does not declare {WINDOWS_PARAKEET_SERVER}: {error}"
                )
            })?,
            model: payload.parakeet_model_path().map_err(|error| {
                format!(
                    "signed Parakeet app payload does not declare {WINDOWS_PARAKEET_MODEL}: {error}"
                )
            })?,
        })
    }) {
        Ok(package) => Ok(package.clone()),
        Err(error) => Err(error.clone()),
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

fn read_available_bytes() -> Option<u64> {
    if std::env::consts::OS != "linux" {
        return None;
    }
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let available = meminfo_value_kib(&meminfo, "MemAvailable")?;
    let total = meminfo_value_kib(&meminfo, "MemTotal")?;
    (available > 0 && total > 0 && available <= total).then(|| available.checked_mul(1024))?
}

fn meminfo_value_kib(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let (found, value) = line.split_once(':')?;
        (found == key)
            .then(|| value.split_whitespace().next()?.parse().ok())
            .flatten()
    })
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
                journal_path: PathBuf::from("/nonexistent-journal-for-remote-mode-test"),
                remote_mode: true,
                platform: "linux".to_owned(),
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
        // cadence. remote_mode short-circuits to a fast, host-independent
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
}
