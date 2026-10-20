// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::result_large_err)] // The public error intentionally carries the JSON envelope.

//! JSON transport surface for `solstone-core local install`.
//!
//! Every command writes one [`InstallEnvelope`]: successful requests use
//! `{"schema":"solstone-local-install-v1","outcome":"ok","result":...}`;
//! refused or invalid requests use `outcome:"error"` with `kind`, `reason_code`,
//! and a human-readable `message`.  The `run` verbs return exit code 75 for a
//! held lease so callers can distinguish busy from an internal failure.

pub mod apple_memory;
pub mod archive;
pub mod capability_status;
pub mod ced_readiness;
pub mod ced_runtime;
pub mod coreml_install;
pub mod fingerprint;
pub mod fit_report;
pub mod lease;
pub mod manifest;
pub mod metal_candidate;
pub mod migration;
mod model_cache;
pub mod onnx_readiness;
pub mod parakeet_readiness;
pub mod pdfium_readiness;
pub mod pins;
pub mod readiness;
pub(crate) mod rfdetr_compiled_expectation;
pub mod rfdetr_install;
pub mod rfdetr_readiness;
pub mod rfdetr_windows;
pub mod status;
pub mod windows_engine;
#[cfg(windows)]
pub mod windows_member_path;

/// Fixture-only driver for the registered installer integration targets.
///
/// Keeping this behind a non-default feature prevents fixture artifacts and
/// injected filesystem seams from becoming an ordinary public API.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub mod test_hooks {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};
    #[cfg(any(test, feature = "test-hooks"))]
    use solstone_core_assets::Artifact;
    #[cfg(any(test, feature = "test-hooks"))]
    use solstone_core_journal_config::JournalConfigRead;
    #[cfg(feature = "test-hooks")]
    use solstone_core_journal_config::parakeet_coreml::ParakeetCoremlSentinel;

    #[cfg(feature = "test-hooks")]
    use super::coreml_install::{
        CoremlAdmission, install_with_rows_and_seams, install_with_rows_for_test,
    };
    #[cfg(any(test, feature = "test-hooks"))]
    use super::coreml_install::{CoremlInstallError, install_minted_rows_for_test};
    #[cfg(feature = "test-hooks")]
    use super::rfdetr_install::{
        EngineSpec, ModelSpec, RfdetrInstallError, RfdetrInstallRecord,
        check_rfdetr_model_with_artifacts,
    };
    use super::{InstallVerb, dispatch, manifest, pins};
    #[cfg(feature = "runtime-fetch-test")]
    pub use solstone_core_artifact_download::{
        FakeRuntimeFetch, RuntimeFetchLoopback, with_builder_fetch_loopback,
        with_fake_runtime_fetch, with_runtime_fetch_loopback,
    };

    pub struct ParakeetFixture {
        pub cpu_path: PathBuf,
        pub vulkan_path: PathBuf,
        pub model_path: PathBuf,
    }

    fn write_parakeet_binary(path: &Path, executable: bool) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write Parakeet binary fixture");
        #[cfg(not(unix))]
        let _ = executable;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = if executable { 0o755 } else { 0o644 };
            fs::set_permissions(path, fs::Permissions::from_mode(mode))
                .expect("set Parakeet binary fixture mode");
        }
    }

    fn write_parakeet_manifest(root: &Path, unit: &str, identity: Value, inventory: Vec<Value>) {
        let built = manifest::build_manifest(
            "parakeet",
            unit,
            "target",
            json!({"pin_identity":identity}),
            inventory,
            None,
            None,
        )
        .expect("build Parakeet fixture manifest");
        manifest::write_manifest(&manifest::artifact_manifest_path(root), &built)
            .expect("write Parakeet fixture manifest");
    }

    pub fn stage_ready_parakeet(
        journal: &Path,
        artifact_key: &str,
        cpu_executable: bool,
    ) -> ParakeetFixture {
        let cache_root = pins::parakeet_cache_root(journal);
        let (cpu_release, _, _, binary_name) =
            pins::parakeet_backend_pin(artifact_key, "cpu").expect("CPU Parakeet pin");
        let (vulkan_release, _, _, _) =
            pins::parakeet_backend_pin(artifact_key, "vulkan").expect("Vulkan Parakeet pin");
        let cpu_root = cache_root
            .join("bin")
            .join(artifact_key)
            .join("cpu")
            .join(cpu_release);
        let vulkan_root = cache_root
            .join("bin")
            .join(artifact_key)
            .join("vulkan")
            .join(vulkan_release);
        let (repo, filename, revision, ..) = pins::PARAKEET_MODEL;
        let model_root = cache_root
            .join("models")
            .join(repo.replace('/', "__"))
            .join(revision);
        fs::create_dir_all(&cpu_root).expect("create CPU fixture root");
        fs::create_dir_all(&vulkan_root).expect("create Vulkan fixture root");
        fs::create_dir_all(&model_root).expect("create model fixture root");
        let cpu_path = cpu_root.join(binary_name);
        let vulkan_path = vulkan_root.join(binary_name);
        let model_path = model_root.join(filename);
        write_parakeet_binary(&cpu_path, cpu_executable);
        write_parakeet_binary(&vulkan_path, true);
        fs::write(&model_path, b"parakeet model").expect("write model fixture");
        write_parakeet_manifest(
            &cpu_root,
            "parakeet-server",
            pins::parakeet_backend_identity(artifact_key, "cpu").expect("CPU identity"),
            manifest::runtime_inventory(&cpu_root, &[]).expect("CPU inventory"),
        );
        write_parakeet_manifest(
            &vulkan_root,
            "parakeet-server",
            pins::parakeet_backend_identity(artifact_key, "vulkan").expect("Vulkan identity"),
            manifest::runtime_inventory(&vulkan_root, &[]).expect("Vulkan inventory"),
        );
        write_parakeet_manifest(
            &model_root,
            "parakeet-model",
            pins::parakeet_model_identity(),
            manifest::inventory_for_tree(&model_root, "model").expect("model inventory"),
        );
        ParakeetFixture {
            cpu_path,
            vulkan_path,
            model_path,
        }
    }

    pub fn inspect_parakeet(journal: &Path, artifact_key: &str) -> Value {
        dispatch(
            InstallVerb::InspectParakeet,
            json!({"journal":journal,"artifact_key":artifact_key}),
        )
        .expect("inspect Parakeet fixture")
        .result
        .expect("inspection result")
    }

    #[cfg(feature = "test-hooks")]
    pub fn install_coreml_with_rows(
        home_dir: &Path,
        config: &JournalConfigRead,
        force: bool,
        rows: &[&Artifact],
    ) -> Result<PathBuf, CoremlInstallError> {
        install_with_rows_for_test(home_dir, config, force, rows)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn install_coreml_minted_rows(
        home_dir: &Path,
        config: &JournalConfigRead,
        force: bool,
        rows: &[&Artifact],
    ) -> Result<PathBuf, CoremlInstallError> {
        install_minted_rows_for_test(home_dir, config, force, rows)
    }

    #[allow(clippy::too_many_arguments)] // Fixture-only write seams prove atomic ordering.
    #[cfg(feature = "test-hooks")]
    pub fn install_coreml_with_seams(
        home_dir: &Path,
        config: &JournalConfigRead,
        force: bool,
        platform: (&str, &str),
        rows: &[&Artifact],
        publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
        write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
    ) -> Result<PathBuf, CoremlInstallError> {
        install_with_rows_and_seams(
            home_dir,
            config,
            force,
            platform,
            rows,
            CoremlAdmission::Fixture,
            publish,
            write,
            &mut |_, _| {},
        )
    }

    #[cfg(feature = "test-hooks")]
    pub fn check_rfdetr_model_with_fixture_artifacts(
        journal: &Path,
        key: &str,
        engine: &EngineSpec,
        model: &ModelSpec,
    ) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
        check_rfdetr_model_with_artifacts(journal, key, engine, model)
    }
}

static PUBLISH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
pub(crate) mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_assets::{Artifact, Backend as AssetBackend, Platform as AssetPlatform, resolve};
use solstone_core_installed_payload::windows_payload::{
    WINDOWS_LLAMA_SERVER, WINDOWS_VULKAN_LOADER,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallVerb {
    PinsLocal,
    PathsLocal,
    FingerprintLocal,
    VerifySha256,
    CudaTrust,
    ManifestVulkan,
    ManifestCuda,
    ManifestModel,
    InspectLocal,
    InspectParakeet,
    ProbeBinary,
    RunLocal,
    PinsParakeet,
    PathsParakeet,
    FingerprintParakeet,
    RunParakeet,
}

#[derive(Debug, Serialize)]
pub struct InstallEnvelope {
    pub schema: &'static str,
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<InstallError>,
}
#[derive(Debug, Serialize)]
pub struct InstallError {
    pub kind: String,
    pub reason_code: String,
    pub message: String,
}
#[derive(Debug)]
pub struct DispatchError {
    pub envelope: InstallEnvelope,
    pub exit_code: u8,
}

impl InstallEnvelope {
    fn ok(result: Value) -> Self {
        Self {
            schema: "solstone-local-install-v1",
            outcome: "ok",
            result: Some(result),
            error: None,
        }
    }
    fn error(kind: &str, reason_code: &str, message: impl ToString) -> Self {
        Self {
            schema: "solstone-local-install-v1",
            outcome: "error",
            result: None,
            error: Some(InstallError {
                kind: kind.to_owned(),
                reason_code: reason_code.to_owned(),
                message: message.to_string(),
            }),
        }
    }
}

pub fn dispatch(verb: InstallVerb, request: Value) -> Result<InstallEnvelope, DispatchError> {
    let object = request.as_object().cloned().ok_or_else(|| {
        failure(
            "input",
            "invalid_request",
            "request must be a JSON object",
            65,
        )
    })?;
    let result = match verb {
        InstallVerb::PinsLocal => pins::pins_json(),
        InstallVerb::PathsLocal => {
            let journal = journal(&object)?;
            pins::paths(
                &journal,
                string(&object, "artifact_key")
                    .unwrap_or_else(pins::platform_key)
                    .as_str(),
                object.get("model_id").and_then(Value::as_str),
            )
        }
        InstallVerb::FingerprintLocal => {
            let journal = journal(&object)?;
            let target = local_target(
                &journal,
                &required(&object, "model_id")?,
                local_backend(&object)?,
            )?;
            resolved_fingerprint(target)?
        }
        InstallVerb::VerifySha256 => {
            let digest = archive::verify_sha256(
                Path::new(&required(&object, "path")?),
                &required(&object, "sha256")?,
            )
            .map_err(|error| failure("verification", "sha256_mismatch", error, 65))?;
            json!({"sha256":digest,"verified":true})
        }
        InstallVerb::CudaTrust => {
            let declared = array_strings(&object, "declared_arch_set")?;
            manifest::cuda_trust(Path::new(&required(&object, "artifact_path")?), &declared)
        }
        InstallVerb::ManifestVulkan => write_manifest("vulkan", &object)?,
        InstallVerb::ManifestCuda => write_manifest("cuda", &object)?,
        InstallVerb::ManifestModel => write_manifest("model", &object)?,
        InstallVerb::InspectLocal => match local_backend(&object)? {
            LocalBackend::Existing => readiness::inspect_local(object),
            LocalBackend::Metal => metal_candidate::inspect(&object)?,
        },
        InstallVerb::InspectParakeet => readiness::inspect_parakeet(object),
        InstallVerb::ProbeBinary => readiness::probe_binary(&object),
        InstallVerb::RunLocal => {
            local_backend(&object)?;
            run_local(&object)?
        }
        InstallVerb::PinsParakeet => pins::parakeet_pins_json(),
        InstallVerb::PathsParakeet => {
            let journal = journal(&object)?;
            pins::parakeet_paths(&journal, &parakeet_key(&object)?)
        }
        InstallVerb::FingerprintParakeet => {
            let journal = journal(&object)?;
            let target = parakeet_target(&journal)?;
            resolved_fingerprint(target)?
        }
        InstallVerb::RunParakeet => run_parakeet(&object)?,
    };
    Ok(InstallEnvelope::ok(result))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalBackend {
    Existing,
    Metal,
}

fn local_backend(object: &Map<String, Value>) -> Result<LocalBackend, DispatchError> {
    local_backend_for_key(object, &pins::platform_key())
}

fn local_backend_for_key(
    object: &Map<String, Value>,
    platform_key: &str,
) -> Result<LocalBackend, DispatchError> {
    match object.get("backend") {
        None if platform_key == "aarch64-apple-darwin" => Ok(LocalBackend::Metal),
        None => Ok(LocalBackend::Existing),
        Some(Value::String(value)) if value == "metal" => Ok(LocalBackend::Metal),
        Some(_) => Err(failure(
            "input",
            "unsupported_backend",
            "backend must be \"metal\" when supplied",
            65,
        )),
    }
}

fn parakeet_key(object: &Map<String, Value>) -> Result<String, DispatchError> {
    match string(object, "artifact_key") {
        Some(key) => Ok(key),
        None => pins::parakeet_host_artifact_key()
            .map_err(|error| failure("platform", "unsupported_platform", error, 65)),
    }
}

fn artifact_platform(key: &str) -> Result<AssetPlatform, DispatchError> {
    match key {
        "aarch64-apple-darwin" => Ok(AssetPlatform::MacosArm64),
        "x86_64-unknown-linux-gnu" => Ok(AssetPlatform::LinuxX64),
        "aarch64-unknown-linux-gnu" => Ok(AssetPlatform::LinuxArm64),
        _ => Err(failure(
            "registry",
            "artifact_registry_mismatch",
            format!("no catalog platform mapping for {key}"),
            65,
        )),
    }
}

pub(crate) fn select_artifact(
    unit: &str,
    platform: Option<AssetPlatform>,
    backend: Option<AssetBackend>,
    artifact_key: Option<&str>,
    filename: Option<&str>,
) -> Result<&'static Artifact, DispatchError> {
    let mut matches = resolve(unit, platform, backend)
        .into_iter()
        .filter(|artifact| artifact_key.is_none_or(|key| artifact.artifact_key == Some(key)))
        .filter(|artifact| filename.is_none_or(|name| artifact.filename == name));
    let Some(artifact) = matches.next() else {
        return Err(failure(
            "registry",
            "artifact_registry_mismatch",
            format!("catalog has no matching {unit} artifact"),
            65,
        ));
    };
    if matches.next().is_some() {
        return Err(failure(
            "registry",
            "artifact_registry_mismatch",
            format!("catalog has multiple matching {unit} artifacts"),
            65,
        ));
    }
    Ok(artifact)
}

fn download_artifact_reason_code<'a>(
    error: &archive::ArchiveError,
    fallback_reason_code: &'a str,
) -> &'a str {
    match error {
        archive::ArchiveError::HostRefused { .. } => "download_host_refused",
        archive::ArchiveError::InsecureScheme { .. } => "download_insecure_scheme",
        archive::ArchiveError::UrlUserinfoRefused { .. } => "download_url_userinfo_refused",
        archive::ArchiveError::SizeMismatch { .. } => "download_size_mismatch",
        archive::ArchiveError::DigestMismatch { .. } => "download_digest_mismatch",
        archive::ArchiveError::RedirectHopLimitExceeded { .. } => {
            "download_redirect_hop_limit_exceeded"
        }
        archive::ArchiveError::OriginUnavailable { .. } => "download_origin_unreachable",
        archive::ArchiveError::Io(error) if error.kind() == std::io::ErrorKind::StorageFull => {
            "disk_space_insufficient"
        }
        archive::ArchiveError::Io(_) | archive::ArchiveError::Download(_) => fallback_reason_code,
        archive::ArchiveError::PathEscape(_) => fallback_reason_code,
    }
}

fn fetch_runtime_member(
    query: &solstone_core_assets::RuntimeFetchQuery<'_>,
    destination: &Path,
    progress: impl FnMut(u64, Option<u64>),
    fallback_reason_code: &str,
) -> Result<bool, DispatchError> {
    #[cfg(any(test, feature = "test-hooks"))]
    TEST_FETCH_RUNTIME_MEMBER_COUNT.with(|count| count.set(count.get() + 1));
    #[cfg(any(test, feature = "test-hooks"))]
    TEST_ACQUIRE_MINT_COUNT.with(|count| count.set(count.get() + 1));
    #[cfg(any(test, feature = "test-hooks"))]
    if TEST_ACQUIRER_SHORT_CIRCUIT.with(|c| c.get()) {
        return Ok(true);
    }

    let handle = solstone_core_assets::mint_runtime_fetch(query).map_err(|err| {
        failure(
            "package",
            solstone_core_assets::FetchMintError::REASON_CODE,
            err,
            65,
        )
    })?;
    solstone_core_artifact_download::download_runtime_fetch(&handle, destination, progress).map_err(
        |error| {
            failure(
                "download",
                download_artifact_reason_code(&error, fallback_reason_code),
                error,
                74,
            )
        },
    )
}

fn write_manifest(kind: &str, object: &Map<String, Value>) -> Result<Value, DispatchError> {
    let root = PathBuf::from(required(object, "root")?);
    let path = PathBuf::from(required(object, "manifest_path")?);
    let target = required(object, "target_fingerprint_sha256")?;
    let attempt = string(object, "attempt_id");
    let identity = object.get("pin_identity").cloned().ok_or_else(|| {
        failure(
            "input",
            "missing_field",
            "pin_identity must be an object",
            65,
        )
    })?;
    let inventory = if kind == "model" {
        manifest::inventory_for_tree(&root, "model")
    } else {
        manifest::runtime_inventory(
            &root,
            &array_strings(object, "exclude_names").unwrap_or_default(),
        )
    }
    .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?;
    let unit = match kind {
        "vulkan" => "llama-server-vulkan",
        "cuda" => "llama-server-cuda",
        "model" => "local-model",
        _ => unreachable!(),
    };
    let built = manifest::build_manifest(
        "local",
        unit,
        &target,
        json!({"pin_identity":identity}),
        inventory,
        None,
        attempt.as_deref(),
    )
    .map_err(|error| failure("input", "manifest_invalid", error, 65))?;
    manifest::write_manifest(&path, &built)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Local,
    Parakeet,
}
impl RunKind {
    fn status_provider(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Parakeet => "parakeet",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalHardwareAdmission {
    WindowsUnobserved,
    WindowsObserved {
        probe_ok: bool,
        devices: Vec<crate::VulkanDevice>,
        override_index: Option<u32>,
    },
    AppleSilicon {
        total_bytes: Option<u64>,
    },
    UnsupportedDarwin,
    GpuHost {
        nvidia_detected: bool,
        nvidia_memory_bytes: Option<u64>,
        probe_ok: bool,
        devices: Vec<crate::VulkanDevice>,
        override_index: Option<u32>,
    },
}

const UNREADABLE_MEMORY_ADMISSION_WARNING: &str =
    "memory couldn't be checked before this installation";

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    pub static TEST_FETCH_RUNTIME_MEMBER_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub static TEST_INSTALL_MODEL_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub static TEST_ACQUIRE_MINT_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub static TEST_ACQUIRER_SHORT_CIRCUIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub static TEST_LOCAL_HARDWARE_ADMISSION: std::cell::RefCell<Option<LocalHardwareAdmission>> = const { std::cell::RefCell::new(None) };
}

/// A thread-scoped hardware admission override for tests and test-hooks.
#[cfg(any(test, feature = "test-hooks"))]
pub struct LocalHardwareAdmissionTestGuard {
    previous: Option<LocalHardwareAdmission>,
    thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl LocalHardwareAdmissionTestGuard {
    pub fn new(admission: LocalHardwareAdmission) -> Self {
        Self {
            previous: TEST_LOCAL_HARDWARE_ADMISSION.with(|slot| slot.replace(Some(admission))),
            thread_bound: std::marker::PhantomData,
        }
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for LocalHardwareAdmissionTestGuard {
    fn drop(&mut self) {
        TEST_LOCAL_HARDWARE_ADMISSION.with(|slot| slot.replace(self.previous.take()));
    }
}

/// A thread-scoped acquisition receipt. No production build exposes this seam.
#[cfg(any(test, feature = "test-hooks"))]
pub struct AcquisitionTestGuard {
    previous: bool,
    previous_counts: (u64, u64, u64),
    thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl AcquisitionTestGuard {
    pub fn new(short_circuit: bool) -> Self {
        let previous_counts = (
            TEST_FETCH_RUNTIME_MEMBER_COUNT.with(|count| count.replace(0)),
            TEST_INSTALL_MODEL_COUNT.with(|count| count.replace(0)),
            TEST_ACQUIRE_MINT_COUNT.with(|count| count.replace(0)),
        );
        Self {
            previous: TEST_ACQUIRER_SHORT_CIRCUIT.with(|flag| flag.replace(short_circuit)),
            previous_counts,
            thread_bound: std::marker::PhantomData,
        }
    }
    pub fn acquisitions(&self) -> u64 {
        TEST_ACQUIRE_MINT_COUNT.with(std::cell::Cell::get)
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Drop for AcquisitionTestGuard {
    fn drop(&mut self) {
        TEST_ACQUIRER_SHORT_CIRCUIT.with(|flag| flag.set(self.previous));
        TEST_FETCH_RUNTIME_MEMBER_COUNT.with(|count| count.set(self.previous_counts.0));
        TEST_INSTALL_MODEL_COUNT.with(|count| count.set(self.previous_counts.1));
        TEST_ACQUIRE_MINT_COUNT.with(|count| count.set(self.previous_counts.2));
    }
}

pub fn run_local_with_gpu_admission(
    object: &Map<String, Value>,
    admission: LocalHardwareAdmission,
) -> Result<Value, DispatchError> {
    let journal = journal(object)?;

    let (refusal, admission_warning) = match admission {
        LocalHardwareAdmission::WindowsUnobserved => (
            Some((
                "gpu_memory_unknown",
                "Vulkan GPU observation required on Windows".to_owned(),
            )),
            None,
        ),
        LocalHardwareAdmission::WindowsObserved {
            probe_ok,
            devices,
            override_index,
        } => {
            let refusal =
                match crate::vulkan::gpu_device_local_verdict(probe_ok, &devices, override_index) {
                    crate::vulkan::GpuDeviceLocalVerdict::Eligible { .. } => None,
                    crate::vulkan::GpuDeviceLocalVerdict::BelowBar { available_bytes } => Some((
                        "gpu_memory_insufficient",
                        format!("device-local VRAM below 6 GiB minimum: {available_bytes} bytes"),
                    )),
                    crate::vulkan::GpuDeviceLocalVerdict::Unknown => Some((
                        "gpu_memory_unknown",
                        "Vulkan probe failed to report memory".to_owned(),
                    )),
                    crate::vulkan::GpuDeviceLocalVerdict::NoHardwareDevice => {
                        Some(("gpu_unavailable", "no hardware Vulkan GPU found".to_owned()))
                    }
                };
            (refusal, None)
        }
        LocalHardwareAdmission::AppleSilicon { total_bytes } => {
            match apple_memory::apple_total_memory_verdict(total_bytes) {
                apple_memory::AppleTotalMemoryVerdict::Eligible => (None, None),
                apple_memory::AppleTotalMemoryVerdict::BelowBar => (
                    Some((
                        "memory_insufficient",
                        "local thinking needs at least 16 GiB of memory".to_owned(),
                    )),
                    None,
                ),
                apple_memory::AppleTotalMemoryVerdict::Unreadable => {
                    (None, Some(UNREADABLE_MEMORY_ADMISSION_WARNING.to_owned()))
                }
            }
        }
        LocalHardwareAdmission::UnsupportedDarwin => (None, None),
        LocalHardwareAdmission::GpuHost {
            nvidia_detected,
            nvidia_memory_bytes,
            probe_ok,
            devices,
            override_index,
        } => {
            if nvidia_detected {
                if let Some(bytes) = nvidia_memory_bytes {
                    match crate::vulkan::device_local_capacity_verdict(bytes) {
                        crate::vulkan::GpuDeviceLocalVerdict::Eligible { .. } => (None, None),
                        crate::vulkan::GpuDeviceLocalVerdict::BelowBar { available_bytes } => (
                            Some((
                                "gpu_memory_insufficient",
                                format!(
                                    "device-local VRAM below 6 GiB minimum: {available_bytes} bytes"
                                ),
                            )),
                            None,
                        ),
                        crate::vulkan::GpuDeviceLocalVerdict::Unknown => {
                            (None, Some(UNREADABLE_MEMORY_ADMISSION_WARNING.to_owned()))
                        }
                        crate::vulkan::GpuDeviceLocalVerdict::NoHardwareDevice => {
                            (None, Some(UNREADABLE_MEMORY_ADMISSION_WARNING.to_owned()))
                        }
                    }
                } else {
                    (None, Some(UNREADABLE_MEMORY_ADMISSION_WARNING.to_owned()))
                }
            } else {
                match crate::vulkan::gpu_device_local_verdict(probe_ok, &devices, override_index) {
                    crate::vulkan::GpuDeviceLocalVerdict::Eligible { .. } => (None, None),
                    crate::vulkan::GpuDeviceLocalVerdict::BelowBar { available_bytes } => (
                        Some((
                            "gpu_memory_insufficient",
                            format!(
                                "device-local VRAM below 6 GiB minimum: {available_bytes} bytes"
                            ),
                        )),
                        None,
                    ),
                    crate::vulkan::GpuDeviceLocalVerdict::NoHardwareDevice => (
                        Some(("gpu_unavailable", "no hardware Vulkan GPU found".to_owned())),
                        None,
                    ),
                    crate::vulkan::GpuDeviceLocalVerdict::Unknown => {
                        (None, Some(UNREADABLE_MEMORY_ADMISSION_WARNING.to_owned()))
                    }
                }
            }
        }
    };
    if let Some((reason_code, message)) = refusal {
        status::record_terminal_refusal(&journal, "local", reason_code, &message)
            .map_err(|error| failure("state", "terminal_write_failed", error, 74))?;
        return Err(failure("gpu", reason_code, message, 65));
    }

    let provider = RunKind::Local.status_provider();
    let Some(lease) = lease::acquire(&journal, provider)
        .map_err(|error| failure("io", "lease_error", error, 74))?
    else {
        return Err(failure(
            "busy",
            "install_busy",
            format!("{provider} install lease is held"),
            lease::BUSY_EXIT_CODE,
        ));
    };
    run_with_lease(
        object,
        RunKind::Local,
        journal,
        lease,
        None,
        admission_warning,
        |_| {},
    )
}

fn run_local(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    let mut admission = observe_local_hardware_admission();
    if let LocalHardwareAdmission::GpuHost { override_index, .. } = &mut admission {
        let journal = journal(object)?;
        let config = solstone_core_journal_config::read_journal_config(&journal)
            .map_err(|error| failure("state", "config_unreadable", error, 74))?
            .config
            .unwrap_or_default();
        *override_index = config
            .get("providers")
            .and_then(Value::as_object)
            .and_then(|providers| providers.get("local"))
            .and_then(Value::as_object)
            .and_then(|local| local.get("vulkan_device_index"))
            .and_then(crate::vulkan::local_vulkan_device_index);
    }
    run_local_with_gpu_admission(object, admission)
}

fn observe_local_hardware_admission() -> LocalHardwareAdmission {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(admission) = TEST_LOCAL_HARDWARE_ADMISSION.with(|slot| slot.borrow().clone()) {
        return admission;
    }

    let key = pins::platform_key();
    if key == "x86_64-windows" {
        LocalHardwareAdmission::WindowsUnobserved
    } else if key == "aarch64-apple-darwin" {
        LocalHardwareAdmission::AppleSilicon {
            total_bytes: apple_memory::apple_physical_memory_bytes(),
        }
    } else if key.ends_with("-apple-darwin") {
        LocalHardwareAdmission::UnsupportedDarwin
    } else {
        let probe = crate::nvidia::probe_nvidia_gpu();
        let (devices, probe_ok) = crate::vulkan::vulkan_probe_snapshot();
        let nvidia_memory_bytes = probe
            .vram_mib
            .or(probe.unified_memory_mib)
            .map(|mib| mib.saturating_mul(1024 * 1024));
        LocalHardwareAdmission::GpuHost {
            nvidia_detected: probe.detected,
            nvidia_memory_bytes,
            probe_ok,
            devices,
            override_index: None,
        }
    }
}
fn run_parakeet(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    run(object, RunKind::Parakeet)
}

pub(crate) fn admit_parakeet_attempt(
    journal: &Path,
    _lease: &lease::InstallLease,
    fingerprint_json: String,
    fingerprint_sha256: String,
    state: &str,
    on_admitted: impl FnOnce(&status::InstallStatus),
) -> Result<status::InstallStatus, status::StatusError> {
    let written = status::begin_or_replace(
        journal,
        "parakeet",
        fingerprint_json,
        fingerprint_sha256,
        None,
        state,
        None,
    )?;
    on_admitted(&written);
    Ok(written)
}

pub fn install_parakeet_with_lease_admitted(
    journal: &Path,
    os_name: &str,
    arch: &str,
    lease: lease::InstallLease,
    on_admitted: impl FnOnce(&status::InstallStatus),
) -> Result<Value, DispatchError> {
    let mut object = Map::new();
    object.insert(
        "journal".to_owned(),
        Value::String(journal.display().to_string()),
    );
    run_with_lease(
        &object,
        RunKind::Parakeet,
        journal.to_path_buf(),
        lease,
        Some((os_name, arch)),
        None,
        on_admitted,
    )
}

pub fn install_parakeet_with_lease(
    journal: &Path,
    os_name: &str,
    arch: &str,
    lease: lease::InstallLease,
) -> Result<Value, DispatchError> {
    install_parakeet_with_lease_admitted(journal, os_name, arch, lease, |_| {})
}

fn run(object: &Map<String, Value>, kind: RunKind) -> Result<Value, DispatchError> {
    let journal = journal(object)?;
    let provider = kind.status_provider();
    let Some(lease) = lease::acquire(&journal, provider)
        .map_err(|error| failure("io", "lease_error", error, 74))?
    else {
        return Err(failure(
            "busy",
            "install_busy",
            format!("{provider} install lease is held"),
            lease::BUSY_EXIT_CODE,
        ));
    };
    run_with_lease(object, kind, journal, lease, None, None, |_| {})
}

fn run_with_lease(
    object: &Map<String, Value>,
    kind: RunKind,
    journal: PathBuf,
    lease: lease::InstallLease,
    parakeet_platform: Option<(&str, &str)>,
    admission_warning: Option<String>,
    on_admitted: impl FnOnce(&status::InstallStatus),
) -> Result<Value, DispatchError> {
    let provider = kind.status_provider();
    let resolved = (|| {
        let fingerprint = match kind {
            RunKind::Local => local_target(
                &journal,
                &string(object, "model_id").unwrap_or_else(|| "local/qwen3.5-4b".to_owned()),
                local_backend(object)?,
            )?,
            RunKind::Parakeet => parakeet_target_for_install(&journal, parakeet_platform)?,
        };
        resolved_fingerprint(fingerprint)
    })();
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(error) => {
            let code = error
                .envelope
                .error
                .as_ref()
                .map_or("install_failed", |e| e.reason_code.as_str());
            let message = error
                .envelope
                .error
                .as_ref()
                .map_or("install failed", |e| e.message.as_str());
            status::record_terminal_refusal_with_lease(&journal, provider, code, message, &lease)
                .map_err(|write_error| failure("state", "terminal_write_failed", write_error, 74))?;
            return Err(error);
        }
    };
    let fingerprint_json = resolved["target_fingerprint_json"]
        .as_str()
        .expect("resolved fingerprint JSON")
        .to_owned();
    let fingerprint_sha256 = resolved["target_fingerprint_sha256"]
        .as_str()
        .expect("resolved fingerprint SHA-256")
        .to_owned();
    let owner = object.get("owner").cloned();
    let mut state = status::begin_or_replace(
        &journal,
        provider,
        fingerprint_json,
        fingerprint_sha256,
        owner,
        "resolving",
        admission_warning,
    )
    .map_err(|error| failure("state", "begin_failed", error, 74))?;
    on_admitted(&state);
    let start = Instant::now();
    let result = match kind {
        RunKind::Local => run_local_install(object, &mut state, start),
        RunKind::Parakeet => run_parakeet_install(&journal, &mut state),
    };
    match result {
        Ok(result) => {
            status::assert_current(&journal, &state)
                .map_err(|error| failure("state", "attempt_superseded", error, 74))?;
            let terminal = status::transition(state, "installed", None, None)
                .and_then(|value| status::write_status(&journal, value))
                .map_err(|error| failure("state", "terminal_write_failed", error, 74))?;
            Ok(json!({"status":terminal,"install":result}))
        }
        Err(error) => {
            let reason_code = error
                .envelope
                .error
                .as_ref()
                .map(|e| e.reason_code.clone())
                .unwrap_or_else(|| "install_failed".to_owned());
            let terminal = status::transition(
                state,
                "failed",
                Some(
                    error
                        .envelope
                        .error
                        .as_ref()
                        .map_or("install failed", |value| &value.message)
                        .to_owned(),
                ),
                Some(reason_code),
            )
            .and_then(|value| status::write_status(&journal, value));
            if terminal.is_err() {
                return Err(failure(
                    "state",
                    "terminal_write_failed",
                    "native install failed and failed to write status",
                    74,
                ));
            }
            Err(error)
        }
    }
}

fn local_target(
    journal: &Path,
    model_id: &str,
    backend: LocalBackend,
) -> Result<Value, DispatchError> {
    local_target_with_probe(journal, model_id, backend, None)
}

fn local_target_with_probe(
    journal: &Path,
    model_id: &str,
    backend: LocalBackend,
    nvidia_probe: Option<crate::NvidiaProbe>,
) -> Result<Value, DispatchError> {
    let key = pins::platform_key();
    local_target_for_key_with_probe(journal, model_id, backend, &key, nvidia_probe)
}

pub(crate) fn local_target_for_windows_package(
    package: &windows_engine::WindowsLlamaPackage,
    model_id: &str,
) -> Result<Value, DispatchError> {
    let model_pin = pins::model_identity(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    let runtime_pin = json!({
        "unit": "llama-server-vulkan",
        "artifact_key": "x86_64-windows",
        "source": "signed-package",
        "members": [
            {
                "path": WINDOWS_LLAMA_SERVER,
                "sha256": package.engine_sha256,
            },
            {
                "path": WINDOWS_VULKAN_LOADER,
                "sha256": package.loader_sha256,
            },
            {
                "path": "bin/solstone-core-vulkan-probe.exe",
                "sha256": package.probe_sha256,
            },
        ],
    });
    Ok(json!({
        "provider": "local",
        "runtime": "llama.cpp",
        "backend": "vulkan",
        "backend_reason": "Windows packaged Vulkan runtime",
        "runtime_pin": runtime_pin,
        "model_pin": model_pin,
    }))
}

fn local_target_for_key(
    journal: &Path,
    model_id: &str,
    backend: LocalBackend,
    key: &str,
) -> Result<Value, DispatchError> {
    local_target_for_key_with_probe(journal, model_id, backend, key, None)
}

fn local_target_for_key_with_probe(
    journal: &Path,
    model_id: &str,
    backend: LocalBackend,
    key: &str,
    nvidia_probe: Option<crate::NvidiaProbe>,
) -> Result<Value, DispatchError> {
    if backend == LocalBackend::Metal && key != "aarch64-apple-darwin" {
        return Err(failure(
            "platform",
            "unsupported_platform",
            format!("Metal local inference is unsupported on {key}"),
            65,
        ));
    }

    if key == "x86_64-windows" {
        let pkg = windows_engine::verified_windows_llama_package().map_err(|err| match err {
            windows_engine::WindowsLlamaPackageError::Missing(msg) => {
                failure("package", "package_unavailable", msg, 65)
            }
            windows_engine::WindowsLlamaPackageError::Invalid(msg) => {
                failure("package", "package_invalid", msg, 65)
            }
        })?;
        return local_target_for_windows_package(&pkg, model_id);
    }
    let (runtime_pin, backend_name, backend_reason) = match backend {
        LocalBackend::Metal => (
            pins::vulkan_identity(key),
            "metal",
            "Darwin Metal runtime".to_owned(),
        ),
        LocalBackend::Existing => {
            let choice = match local_backend_choice(journal, nvidia_probe) {
                crate::nvidia::BackendSelection::Selected(choice) => choice,
                crate::nvidia::BackendSelection::IntegrityBlocked => {
                    return Err(failure(
                        "verification",
                        "cuda_runtime_integrity",
                        "cuda runtime integrity failure",
                        65,
                    ));
                }
            };
            let (identity, name) = match choice.backend {
                crate::Backend::Cuda => (pins::cuda_identity(key), "cuda"),
                crate::Backend::Vulkan => (pins::vulkan_identity(key), "vulkan"),
            };
            (identity, name, choice.reason)
        }
    };
    let runtime_pin = runtime_pin.ok_or_else(|| {
        failure(
            "platform",
            "unsupported_platform",
            format!("no pin for {key}"),
            65,
        )
    })?;
    let model_pin = pins::model_identity(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    Ok(
        json!({"provider":"local","runtime":"llama.cpp","backend":backend_name,"backend_reason":backend_reason,"runtime_pin":runtime_pin,"model_pin":model_pin}),
    )
}

/// Darwin always ships `local/qwen3.5-4b`. Elsewhere a configured pin wins.
pub fn resolve_bundled_model_id(configured: &str, darwin: bool) -> String {
    if darwin || pins::model_identity(configured).is_none() {
        "local/qwen3.5-4b".to_owned()
    } else {
        configured.to_owned()
    }
}

fn live_nvidia_probe_is_transient_undetected(probe: &crate::NvidiaProbe) -> bool {
    if probe.detected {
        return false;
    }
    let Some(error) = probe.probe_error.as_deref() else {
        return false;
    };
    error == "NVIDIA GPU probe timed out after 10s"
        || error.starts_with("NVIDIA GPU probe could not start:")
        || error.starts_with("NVIDIA GPU probe could not wait for nvidia-smi:")
        || error.starts_with("NVIDIA GPU probe exited with status")
}

pub fn assess_cuda_installation(dir: &Path, key: &str) -> crate::ArtifactTrust {
    let launcher = dir.join("llama-server");
    if !launcher.exists() {
        return crate::ArtifactTrust::Unavailable;
    }
    let manifest_path = manifest::artifact_manifest_path(dir);
    let Ok(manifest_bytes) = fs::read(&manifest_path) else {
        return crate::ArtifactTrust::Integrity;
    };
    let Ok(manifest_json) = serde_json::from_slice::<Value>(&manifest_bytes) else {
        return crate::ArtifactTrust::Integrity;
    };
    let Some(selected_identity) = pins::cuda_identity(key) else {
        return crate::ArtifactTrust::Integrity;
    };
    let Some(manifest_pin) = manifest_json
        .get("source")
        .and_then(|s| s.get("pin_identity"))
        .or_else(|| manifest_json.get("pin_identity"))
    else {
        return crate::ArtifactTrust::Integrity;
    };
    let Ok(canonical_manifest_pin) = fingerprint::canonical(manifest_pin.clone()) else {
        return crate::ArtifactTrust::Integrity;
    };
    let Ok(canonical_selected_pin) = fingerprint::canonical(selected_identity) else {
        return crate::ArtifactTrust::Integrity;
    };
    if canonical_manifest_pin != canonical_selected_pin {
        return crate::ArtifactTrust::Integrity;
    }
    let Some(inventory) = manifest_json.get("inventory").and_then(Value::as_array) else {
        return crate::ArtifactTrust::Integrity;
    };
    assess_cuda_bytes(dir, inventory)
}

pub fn assess_cuda_bytes(dir: &Path, inventory: &[Value]) -> crate::ArtifactTrust {
    let launcher = dir.join("llama-server");
    let Ok(meta) = fs::symlink_metadata(&launcher) else {
        return crate::ArtifactTrust::Unavailable;
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return crate::ArtifactTrust::Integrity;
    }
    let Ok(launcher_bytes) = fs::read(&launcher) else {
        return crate::ArtifactTrust::Integrity;
    };
    let declared = crate::CUDA_EMBEDDED_ARCH_SET;
    let launcher_text = String::from_utf8_lossy(&launcher_bytes);
    let launcher_contains_count = declared
        .iter()
        .filter(|arch| launcher_text.contains(*arch))
        .count();

    if launcher_contains_count == declared.len() {
        if carrier_hash_matches(inventory, "llama-server", &launcher) {
            return crate::ArtifactTrust::Trusted;
        }
        return crate::ArtifactTrust::Integrity;
    }
    if launcher_contains_count == 0 {
        let twin = dir.join("libggml-cuda.so");
        let Ok(meta) = fs::symlink_metadata(&twin) else {
            return crate::ArtifactTrust::Integrity;
        };
        if !meta.is_file() || meta.file_type().is_symlink() {
            return crate::ArtifactTrust::Integrity;
        }
        let Ok(twin_bytes) = fs::read(&twin) else {
            return crate::ArtifactTrust::Integrity;
        };
        let twin_text = String::from_utf8_lossy(&twin_bytes);
        if !declared.iter().all(|arch| twin_text.contains(arch)) {
            return crate::ArtifactTrust::Integrity;
        }
        if carrier_hash_matches(inventory, "libggml-cuda.so", &twin) {
            return crate::ArtifactTrust::Trusted;
        }
        return crate::ArtifactTrust::Integrity;
    }

    crate::ArtifactTrust::Integrity
}

fn carrier_hash_matches(inventory: &[Value], name: &str, path: &Path) -> bool {
    let Some(expected_hash) = inventory_entry_sha256(inventory, name) else {
        return false;
    };
    manifest::sha256_file(path).ok().as_deref() == Some(expected_hash.as_str())
}

fn inventory_entry_sha256(inventory: &[Value], name: &str) -> Option<String> {
    for entry in inventory {
        if entry.get("relative_path").and_then(Value::as_str) == Some(name) {
            return entry
                .get("sha256")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
    }
    None
}

pub fn verify_required_oracle(
    root: &Path,
    release_tag: &str,
    artifact_key: &str,
    is_cuda: bool,
) -> Result<(), String> {
    if is_cuda {
        let arch = pins::cuda_runtime_arch(artifact_key)
            .ok_or_else(|| format!("unsupported cuda arch key {artifact_key}"))?;
        let wanted =
            pins::cuda_wanted_files(arch).ok_or_else(|| format!("unsupported cuda arch {arch}"))?;
        for name in wanted {
            let path = root.join(&name);
            let meta = fs::symlink_metadata(&path)
                .map_err(|_| format!("required cuda member missing: {name}"))?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(format!(
                    "required cuda member is not a regular file: {name}"
                ));
            }
        }
        return Ok(());
    }

    let Some(members) = pins::required_members_for(release_tag, artifact_key) else {
        return Ok(());
    };
    for member in members {
        match member {
            pins::RequiredMember::Regular(name) => {
                let path = root.join(name);
                let meta = fs::symlink_metadata(&path)
                    .map_err(|_| format!("required regular member missing: {name}"))?;
                if !meta.is_file() || meta.file_type().is_symlink() {
                    return Err(format!(
                        "required regular member is not a regular file: {name}"
                    ));
                }
            }
            pins::RequiredMember::Link { name, target } => {
                let path = root.join(name);
                let meta = fs::symlink_metadata(&path)
                    .map_err(|_| format!("required link member missing: {name}"))?;
                if !meta.file_type().is_symlink() {
                    return Err(format!("required link member is not a symlink: {name}"));
                }
                let link_target =
                    fs::read_link(&path).map_err(|e| format!("cannot read link {name}: {e}"))?;
                if link_target != Path::new(target) {
                    return Err(format!(
                        "link {name} target mismatch: expected {target}, got {}",
                        link_target.display()
                    ));
                }
                let mut current = path.clone();
                let mut hops = 0;
                while hops < 20 {
                    let meta = fs::symlink_metadata(&current)
                        .map_err(|_| format!("broken link in chain for {name}"))?;
                    if meta.file_type().is_symlink() {
                        let link = fs::read_link(&current)
                            .map_err(|_| format!("cannot read link in chain for {name}"))?;
                        let parent = current.parent().unwrap_or(root);
                        current = parent.join(link);
                        hops += 1;
                    } else if meta.is_file() {
                        break;
                    } else {
                        return Err(format!("link {name} does not resolve to regular file"));
                    }
                }
                if hops >= 20 {
                    return Err(format!("link cycle detected for {name}"));
                }
            }
        }
    }
    Ok(())
}

pub fn local_backend_choice(
    journal: &Path,
    nvidia_probe: Option<crate::NvidiaProbe>,
) -> crate::nvidia::BackendSelection {
    local_backend_choice_with(journal, nvidia_probe, true)
}

/// Backend candidate for read paths. The runtime binary's CUDA architectures
/// are checked by `local_backend_choice` when execution is admitted.
pub fn local_backend_choice_present(
    journal: &Path,
    nvidia_probe: Option<crate::NvidiaProbe>,
) -> crate::nvidia::BackendSelection {
    local_backend_choice_with(journal, nvidia_probe, false)
}

pub fn local_thinking_catalog_plan(
    journal: &Path,
    nvidia_probe: Option<crate::NvidiaProbe>,
) -> Option<Vec<pins::ThinkingDownloadRow>> {
    let key = pins::platform_key();
    if key == "x86_64-windows" {
        return pins::thinking_download_rows(crate::Backend::Vulkan, &key);
    }
    match local_backend_choice_present(journal, nvidia_probe) {
        crate::BackendSelection::Selected(choice) => {
            pins::thinking_download_rows(choice.backend, &key)
        }
        crate::BackendSelection::IntegrityBlocked => None,
    }
}

pub fn inspect_parakeet_manifests(journal: &Path, artifact_key: &str) -> (Value, Value, Value) {
    let paths = pins::parakeet_paths(journal, artifact_key);

    let cpu_proof = inspect_server_manifest(&paths, "binary_path_cpu", artifact_key, "cpu");
    let vulkan_proof =
        inspect_server_manifest(&paths, "binary_path_vulkan", artifact_key, "vulkan");
    let model_proof = inspect_model_manifest(&paths);

    (cpu_proof, vulkan_proof, model_proof)
}

pub fn inspect_server_manifest(
    paths: &Value,
    binary_path_key: &str,
    artifact_key: &str,
    backend: &str,
) -> Value {
    let Some(binary_path_str) = paths.get(binary_path_key).and_then(Value::as_str) else {
        return serde_json::json!({
            "status": "missing-or-mismatched",
            "reason_code": "manifest_missing",
            "cache_hit": false
        });
    };
    let binary_path = Path::new(binary_path_str);
    let Some(manifest_dir) = binary_path.parent() else {
        return serde_json::json!({
            "status": "missing-or-mismatched",
            "reason_code": "manifest_missing",
            "cache_hit": false
        });
    };
    let Some(identity) = pins::parakeet_backend_identity(artifact_key, backend) else {
        return serde_json::json!({
            "status": "missing-or-mismatched",
            "reason_code": "manifest_missing",
            "cache_hit": false
        });
    };
    let Some((_, _, _, binary_name)) = pins::parakeet_backend_pin(artifact_key, backend) else {
        return serde_json::json!({
            "status": "missing-or-mismatched",
            "reason_code": "manifest_missing",
            "cache_hit": false
        });
    };
    let manifest_path = manifest::artifact_manifest_path(manifest_dir);
    manifest::inspect_manifest_required(&manifest_path, &identity, &[binary_name])
}

pub fn inspect_model_manifest(paths: &Value) -> Value {
    let Some(model_path_str) = paths.get("model_path").and_then(Value::as_str) else {
        return serde_json::json!({
            "status": "missing-or-mismatched",
            "reason_code": "manifest_missing",
            "cache_hit": false
        });
    };
    let model_path = Path::new(model_path_str);
    let Some(manifest_dir) = model_path.parent() else {
        return serde_json::json!({
            "status": "missing-or-mismatched",
            "reason_code": "manifest_missing",
            "cache_hit": false
        });
    };
    let identity = pins::parakeet_model_identity();
    let filename = identity
        .get("filename")
        .and_then(Value::as_str)
        .unwrap_or(pins::PARAKEET_MODEL.1);
    let manifest_path = manifest::artifact_manifest_path(manifest_dir);
    manifest::inspect_manifest_required(&manifest_path, &identity, &[filename])
}

pub fn parakeet_manifests_ready(journal: &Path, os: &str, arch: &str) -> bool {
    let Ok(artifact_key) = pins::parakeet_artifact_key(os, arch) else {
        return false;
    };
    let (cpu, vulkan, model) = inspect_parakeet_manifests(journal, &artifact_key);
    cpu.get("status").and_then(Value::as_str) == Some("ready")
        && vulkan.get("status").and_then(Value::as_str) == Some("ready")
        && model.get("status").and_then(Value::as_str) == Some("ready")
}

fn local_backend_choice_with(
    journal: &Path,
    nvidia_probe: Option<crate::NvidiaProbe>,
    inspect_cuda_binary: bool,
) -> crate::nvidia::BackendSelection {
    let key = pins::platform_key();
    if key == "x86_64-windows" {
        return crate::nvidia::BackendSelection::Selected(crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: "Windows packaged Vulkan runtime".to_owned(),
        });
    }
    let probe = nvidia_probe.unwrap_or_else(|| {
        let first = crate::probe_nvidia_gpu();
        if live_nvidia_probe_is_transient_undetected(&first) {
            crate::probe_nvidia_gpu()
        } else {
            first
        }
    });
    let pin = pins::cuda_pin(&key);
    if let Some(rejection) = crate::hardware_backend_rejection(
        &probe,
        &crate::CUDA_EMBEDDED_ARCH_SET,
        crate::CUDA_MIN_DRIVER_VERSION,
    ) {
        return crate::nvidia::BackendSelection::Selected(rejection);
    }
    if pin.is_none() {
        return crate::nvidia::BackendSelection::Selected(crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: "CUDA runtime is not published for this platform".to_owned(),
        });
    }
    let trust = if inspect_cuda_binary {
        pin.as_ref()
            .map(|(_, digest, _)| {
                let install_dir = pins::cache_root(journal)
                    .join("cuda")
                    .join(&key)
                    .join(digest);
                assess_cuda_installation(&install_dir, &key)
            })
            .unwrap_or(crate::ArtifactTrust::Unavailable)
    } else {
        crate::ArtifactTrust::Unavailable
    };
    if trust == crate::ArtifactTrust::Integrity {
        return crate::nvidia::BackendSelection::IntegrityBlocked;
    }
    if trust == crate::ArtifactTrust::Absent {
        let arch = probe
            .arch
            .as_deref()
            .expect("hardware rejection checked arch");
        let driver_cuda_major = probe
            .driver_cuda_major
            .expect("hardware rejection checked driver CUDA major");
        return crate::nvidia::BackendSelection::Selected(crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: format!(
                "compute_cap {arch} covered; driver CUDA {driver_cuda_major} >= {}; CUDA runtime artifact does not cover this GPU",
                crate::CUDA_MIN_DRIVER_VERSION
            ),
        });
    }
    // Hardware qualifies and a CUDA pin is published. Select CUDA so first
    // install downloads it. Post-download `cuda_trust` still verifies the
    // binary. Launch-time planning keeps using `select_local_backend`, which
    // requires the artifact to be present.
    let arch = probe
        .arch
        .as_deref()
        .expect("hardware rejection checked arch");
    let driver_cuda_major = probe
        .driver_cuda_major
        .expect("hardware rejection checked driver CUDA major");
    crate::nvidia::BackendSelection::Selected(crate::BackendChoice {
        backend: crate::Backend::Cuda,
        reason: format!(
            "compute_cap {arch} covered; driver CUDA {driver_cuda_major} >= {}",
            crate::CUDA_MIN_DRIVER_VERSION
        ),
    })
}

/// Mirrors Python's `parakeet_install.target_fingerprint` field-for-field:
/// `provider`, `runtime`, `artifact_key`, `binary_pins` (cpu then vulkan,
/// matching `PARAKEET_CPP_BINARY_BACKENDS`'s order), `model_pin`, `cache_root`.
fn parakeet_target(journal: &Path) -> Result<Value, DispatchError> {
    parakeet_target_for_install(journal, None)
}

fn parakeet_target_for_install(
    journal: &Path,
    platform: Option<(&str, &str)>,
) -> Result<Value, DispatchError> {
    match platform {
        Some((os_name, arch)) => parakeet_target_for_platform(journal, os_name, arch),
        None => parakeet_target_for_platform(journal, std::env::consts::OS, std::env::consts::ARCH),
    }
}

pub fn parakeet_target_for_platform(
    journal: &Path,
    os_name: &str,
    arch: &str,
) -> Result<Value, DispatchError> {
    let key = pins::parakeet_artifact_key(os_name, arch)
        .map_err(|error| failure("platform", "unsupported_platform", error, 65))?;
    let binary_pins: Vec<Value> = ["cpu", "vulkan"]
        .into_iter()
        .map(|backend| {
            pins::parakeet_backend_identity(&key, backend)
                .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))
        })
        .collect::<Result<_, _>>()?;
    Ok(json!({
        "provider": "parakeet",
        "runtime": "parakeet.cpp",
        "artifact_key": key,
        "binary_pins": binary_pins,
        "model_pin": pins::parakeet_model_identity(),
        "cache_root": pins::parakeet_cache_root(journal),
    }))
}

fn resolved_fingerprint(target: Value) -> Result<Value, DispatchError> {
    let target_fingerprint_json = fingerprint::canonical(target)
        .map_err(|error| failure("input", "fingerprint_invalid", error, 65))?;
    let target_fingerprint_sha256 = fingerprint::sha256(&target_fingerprint_json);
    Ok(json!({
        "target_fingerprint_json": target_fingerprint_json,
        "target_fingerprint_sha256": target_fingerprint_sha256,
    }))
}

fn local_model_download_bytes(model_id: &str) -> Result<u64, DispatchError> {
    let identity = pins::model_identity(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    ["filename", "mmproj_filename"]
        .into_iter()
        .filter_map(|key| identity[key].as_str())
        .map(|filename| {
            select_artifact("local-model", None, None, None, Some(filename))
                .map(|row| row.size_bytes)
        })
        .sum()
}

fn parakeet_download_bytes(key: &str) -> Result<u64, DispatchError> {
    let mut total = select_artifact(
        "parakeet-model",
        None,
        None,
        None,
        Some(pins::PARAKEET_MODEL.1),
    )?
    .size_bytes;
    for backend in [AssetBackend::Cpu, AssetBackend::Vulkan] {
        total += select_artifact(
            "parakeet-server",
            Some(artifact_platform(key)?),
            Some(backend),
            Some(key),
            None,
        )?
        .size_bytes;
    }
    Ok(total)
}

fn fetch_install_member(
    journal: &Path,
    state: &mut status::InstallStatus,
    artifact: &Artifact,
    destination: &Path,
    completed: u64,
    total: u64,
    error_code: &str,
) -> Result<(), DispatchError> {
    fetch_install_member_with(
        journal,
        state,
        artifact,
        destination,
        completed,
        total,
        |query, destination, progress| {
            fetch_runtime_member(query, destination, progress, error_code)
        },
    )
}

fn fetch_install_member_with(
    journal: &Path,
    state: &mut status::InstallStatus,
    artifact: &Artifact,
    destination: &Path,
    completed: u64,
    total: u64,
    fetch: impl FnOnce(
        &solstone_core_assets::RuntimeFetchQuery<'_>,
        &Path,
        &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<bool, DispatchError>,
) -> Result<(), DispatchError> {
    let query = solstone_core_assets::RuntimeFetchQuery {
        unit: Some(artifact.unit),
        origin_key: Some(artifact.origin_key),
        ..Default::default()
    };
    let mut at = Instant::now();
    let mut progress_error = None;
    fetch(&query, destination, &mut |received, _| {
        if progress_error.is_none() {
            match status::bump_progress(
                state.clone(),
                Some(completed.saturating_add(received)),
                Some(total),
                &mut at,
            )
            .and_then(|next| {
                next.map(|next| status::write_status(journal, next))
                    .transpose()
            }) {
                Ok(Some(written)) => *state = written,
                Ok(None) => {}
                Err(error) => progress_error = Some(error),
            }
        }
    })?;
    if let Some(error) = progress_error {
        return Err(failure("state", "status_write_failed", error, 74));
    }
    state.progress_bytes_received = Some(completed.saturating_add(artifact.size_bytes));
    state.progress_bytes_total = Some(total);
    *state = status::write_status(journal, state.clone())
        .map_err(|error| failure("state", "status_write_failed", error, 74))?;
    Ok(())
}

fn run_local_install(
    object: &Map<String, Value>,
    status_value: &mut status::InstallStatus,
    _start: Instant,
) -> Result<Value, DispatchError> {
    let journal = journal(object)?;
    let model_id = string(object, "model_id").unwrap_or_else(|| "local/qwen3.5-4b".to_owned());
    let target: Value = serde_json::from_str(
        status_value
            .target_fingerprint_json
            .as_deref()
            .ok_or_else(|| {
                failure(
                    "state",
                    "fingerprint_missing",
                    "attempt fingerprint missing",
                    74,
                )
            })?,
    )
    .map_err(|error| failure("state", "fingerprint_malformed", error, 74))?;
    let backend = target["backend"]
        .as_str()
        .ok_or_else(|| failure("state", "fingerprint_malformed", "backend missing", 74))?;
    let key = target["runtime_pin"]["artifact_key"]
        .as_str()
        .ok_or_else(|| {
            failure(
                "state",
                "fingerprint_malformed",
                "runtime artifact key missing",
                74,
            )
        })?
        .to_owned();
    let model_bytes = local_model_download_bytes(&model_id)?;
    let root = pins::cache_root(&journal);
    if key == "x86_64-windows" {
        let pkg = windows_engine::verified_windows_llama_package().map_err(|err| match err {
            windows_engine::WindowsLlamaPackageError::Missing(msg) => {
                failure("package", "package_unavailable", msg, 65)
            }
            windows_engine::WindowsLlamaPackageError::Invalid(msg) => {
                failure("package", "package_invalid", msg, 65)
            }
        })?;
        let model_identity = pins::model_identity(&model_id)
            .ok_or_else(|| failure("model", "unsupported_model", &model_id, 65))?;
        let model_file = model_identity
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or("");
        let projector_file = model_identity
            .get("mmproj_filename")
            .and_then(Value::as_str)
            .unwrap_or("");
        let model_root = pins::model_directory(&journal, &model_id);
        let model_proof = manifest::prove_manifest_required(
            &manifest::artifact_manifest_path(&model_root),
            &model_identity,
            &[model_file, projector_file],
        );
        if model_proof["status"] != "ready" {
            install_model(&journal, &model_id, status_value, 0, model_bytes)?;
        }
        return Ok(json!({
            "backend": "vulkan",
            "binary_path": pkg.engine,
            "model_id": model_id,
        }));
    }
    if key.ends_with("-windows") {
        return Err(failure(
            "platform",
            "unsupported_platform",
            format!("unsupported Windows platform: {key}"),
            65,
        ));
    }
    let (release_tag, filename, install_dir, pin_identity, exclude_names, cuda) =
        if backend == "cuda" {
            let (_, digest, _) = pins::cuda_pin(&key)
                .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
            let install_dir = root.join("cuda").join(&key).join(digest);
            let identity = pins::cuda_identity(&key).unwrap();
            let release_tag = identity
                .get("release_tag")
                .and_then(Value::as_str)
                .unwrap_or("b10068")
                .to_owned();
            (
                release_tag,
                format!("llama-{digest}.tar.gz"),
                install_dir,
                identity,
                Vec::new(),
                true,
            )
        } else {
            let (release, filename, _digest, _) = pins::vulkan_pin(&key)
                .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
            (
                release.to_owned(),
                filename.to_owned(),
                root.join("bin").join(&key).join(release),
                pins::vulkan_identity(&key).unwrap(),
                vec![filename.to_owned()],
                false,
            )
        };
    let artifact = select_artifact(
        if cuda {
            "llama-server-cuda"
        } else {
            "llama-server-vulkan"
        },
        Some(artifact_platform(&key)?),
        None,
        Some(&key),
        None,
    )?;
    let staging = install_dir.parent().unwrap().join(format!(
        ".{}.staging",
        install_dir.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .map_err(|error| failure("io", "staging_create_failed", error, 74))?;
    let archive_path = pins::cache_root(&journal)
        .join(".fetch")
        .join(artifact.sha256)
        .join(&filename);
    *status_value = status::write_status(
        &journal,
        status::transition(status_value.clone(), "downloading", None, None)
            .map_err(|error| failure("state", "transition_failed", error, 74))?,
    )
    .map_err(|error| failure("state", "status_write_failed", error, 74))?;
    let total_bytes = artifact.size_bytes + model_bytes;
    fetch_install_member(
        &journal,
        status_value,
        artifact,
        &archive_path,
        0,
        total_bytes,
        "download_failed",
    )?;
    archive::verify_sha256(&archive_path, artifact.sha256)
        .map_err(|e| failure("archive", "digest_mismatch", e, 65))?;
    archive::extract_tar_gz(&archive_path, &staging)
        .map_err(|error| failure("archive", "extract_failed", error, 65))?;
    let binary = find_file(&staging, "llama-server").ok_or_else(|| {
        failure(
            "archive",
            "binary_missing",
            "llama-server missing from archive",
            65,
        )
    })?;
    let final_binary = staging.join("llama-server");
    hoist_binary(&staging, &binary, backend).map_err(|error| {
        failure(
            "io",
            if backend == "metal" || backend == "vulkan" {
                "binary_bundle_move_failed"
            } else {
                "binary_move_failed"
            },
            error,
            74,
        )
    })?;
    archive::make_executable(&final_binary)
        .map_err(|error| failure("io", "chmod_failed", error, 74))?;
    archive::clear_macos_quarantine(&staging)
        .map_err(|error| failure("io", "quarantine_clear_failed", error, 74))?;
    commit_staged_local_runtime(
        &staging,
        &install_dir,
        LocalRuntimeAdmission {
            release_tag: &release_tag,
            artifact_key: &key,
            is_cuda: cuda,
            pin_identity: &pin_identity,
        },
        status_value.target_fingerprint_sha256.as_deref().unwrap(),
        status_value.attempt_id.as_deref(),
        &exclude_names,
    )?;
    install_model(
        &journal,
        &model_id,
        status_value,
        artifact.size_bytes,
        total_bytes,
    )?;
    let _ = fs::remove_file(&archive_path);
    Ok(
        json!({"backend":backend,"binary_path":install_dir.join("llama-server"),"model_id":model_id}),
    )
}

pub struct LocalRuntimeAdmission<'a> {
    pub release_tag: &'a str,
    pub artifact_key: &'a str,
    pub is_cuda: bool,
    pub pin_identity: &'a Value,
}

pub fn commit_staged_local_runtime(
    staging: &Path,
    install_dir: &Path,
    admission: LocalRuntimeAdmission<'_>,
    target_fingerprint_sha256: &str,
    attempt_id: Option<&str>,
    exclude_names: &[String],
) -> Result<(), DispatchError> {
    let LocalRuntimeAdmission {
        release_tag,
        artifact_key,
        is_cuda,
        pin_identity,
    } = admission;
    if let Err(error) = verify_required_oracle(staging, release_tag, artifact_key, is_cuda) {
        let _ = fs::remove_dir_all(staging);
        return Err(failure(
            "verification",
            "required_member_missing",
            error,
            65,
        ));
    }
    let inventory = match manifest::runtime_inventory(staging, exclude_names) {
        Ok(inventory) => inventory,
        Err(error) => {
            let _ = fs::remove_dir_all(staging);
            return Err(failure("io", "manifest_inventory_failed", error, 74));
        }
    };
    if is_cuda {
        let trust = assess_cuda_bytes(staging, &inventory);
        if trust != crate::ArtifactTrust::Trusted {
            let _ = fs::remove_dir_all(staging);
            return Err(failure(
                "verification",
                "cuda_runtime_integrity",
                "cuda runtime integrity failure",
                65,
            ));
        }
    }
    let manifest_value = match manifest::build_manifest(
        "local",
        if is_cuda {
            "llama-server-cuda"
        } else {
            "llama-server-vulkan"
        },
        target_fingerprint_sha256,
        json!({"pin_identity": pin_identity}),
        inventory,
        None,
        attempt_id,
    ) {
        Ok(manifest_value) => manifest_value,
        Err(error) => {
            let _ = fs::remove_dir_all(staging);
            return Err(failure("io", "manifest_build_failed", error, 74));
        }
    };
    if let Err(error) =
        manifest::write_manifest(&manifest::artifact_manifest_path(staging), &manifest_value)
    {
        let _ = fs::remove_dir_all(staging);
        return Err(failure("io", "manifest_write_failed", error, 74));
    }
    publish_staged_tree(staging, install_dir)
        .map_err(|error| failure("io", "publish_failed", error, 74))?;
    Ok(())
}

/// Mirrors `run_local_install`'s download-extract-chmod-manifest-publish
/// shape, but for both parakeet-server backends (cpu, vulkan) plus the
/// model -- `install_parakeet` (Python) installs both backends
/// unconditionally rather than picking one, so this does too.
fn run_parakeet_install(
    journal: &Path,
    status_value: &mut status::InstallStatus,
) -> Result<Value, DispatchError> {
    let target: Value = serde_json::from_str(
        status_value
            .target_fingerprint_json
            .as_deref()
            .ok_or_else(|| {
                failure(
                    "state",
                    "fingerprint_missing",
                    "attempt fingerprint missing",
                    74,
                )
            })?,
    )
    .map_err(|error| failure("state", "fingerprint_malformed", error, 74))?;
    let key = target["artifact_key"]
        .as_str()
        .ok_or_else(|| failure("state", "fingerprint_malformed", "artifact_key missing", 74))?
        .to_owned();
    let total_bytes = parakeet_download_bytes(&key)?;
    let mut completed_bytes = 0;
    let mut binaries = Vec::new();
    let mut completed_archives = Vec::new();
    for backend in ["cpu", "vulkan"] {
        let (release, filename, _digest, binary_name) =
            pins::parakeet_backend_pin(&key, backend)
                .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
        let artifact = select_artifact(
            "parakeet-server",
            Some(artifact_platform(&key)?),
            Some(if backend == "cpu" {
                AssetBackend::Cpu
            } else {
                AssetBackend::Vulkan
            }),
            Some(&key),
            Some(filename),
        )?;
        let install_dir = pins::parakeet_cache_root(journal)
            .join("bin")
            .join(&key)
            .join(backend)
            .join(release);
        let staging = install_dir.parent().unwrap().join(format!(
            ".{}.staging",
            install_dir.file_name().unwrap().to_string_lossy()
        ));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)
            .map_err(|error| failure("io", "staging_create_failed", error, 74))?;
        let archive_path = pins::parakeet_cache_root(journal)
            .join(".fetch")
            .join(artifact.sha256)
            .join(filename);
        *status_value = status::write_status(
            journal,
            status::transition(status_value.clone(), "downloading", None, None)
                .map_err(|error| failure("state", "transition_failed", error, 74))?,
        )
        .map_err(|error| failure("state", "status_write_failed", error, 74))?;
        fetch_install_member(
            journal,
            status_value,
            artifact,
            &archive_path,
            completed_bytes,
            total_bytes,
            "download_failed",
        )?;
        completed_bytes += artifact.size_bytes;
        archive::verify_sha256(&archive_path, artifact.sha256)
            .map_err(|e| failure("archive", "digest_mismatch", e, 65))?;
        archive::extract_tar_gz(&archive_path, &staging)
            .map_err(|error| failure("archive", "extract_failed", error, 65))?;
        let binary = find_file(&staging, binary_name).ok_or_else(|| {
            failure(
                "archive",
                "binary_missing",
                format!("{binary_name} missing from archive"),
                65,
            )
        })?;
        let final_binary = staging.join(binary_name);
        if binary != final_binary {
            fs::rename(&binary, &final_binary)
                .map_err(|error| failure("io", "binary_move_failed", error, 74))?;
        }
        archive::make_executable(&final_binary)
            .map_err(|error| failure("io", "chmod_failed", error, 74))?;
        archive::clear_macos_quarantine(&staging)
            .map_err(|error| failure("io", "quarantine_clear_failed", error, 74))?;
        let pin_identity = pins::parakeet_backend_identity(&key, backend).unwrap();
        let manifest_value = manifest::build_manifest(
            "parakeet",
            "parakeet-server",
            status_value.target_fingerprint_sha256.as_deref().unwrap(),
            json!({"pin_identity": pin_identity}),
            manifest::runtime_inventory(&staging, &[filename.to_owned()])
                .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
            None,
            status_value.attempt_id.as_deref(),
        )
        .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
        manifest::write_manifest(&manifest::artifact_manifest_path(&staging), &manifest_value)
            .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
        publish_staged_tree(&staging, &install_dir)
            .map_err(|error| failure("io", "publish_failed", error, 74))?;
        completed_archives.push(archive_path);
        binaries.push(json!({"backend": backend, "binary_path": install_dir.join(binary_name)}));
    }
    let model_path = install_parakeet_model(journal, status_value, completed_bytes, total_bytes)?;
    for archive in completed_archives {
        let _ = fs::remove_file(archive);
    }
    Ok(json!({"artifact_key": key, "binaries": binaries, "model_path": model_path}))
}

fn install_parakeet_model(
    journal: &Path,
    status_value: &mut status::InstallStatus,
    completed_bytes: u64,
    total_bytes: u64,
) -> Result<PathBuf, DispatchError> {
    let (repo, filename, revision, ..) = pins::PARAKEET_MODEL;
    let artifact = select_artifact("parakeet-model", None, None, None, Some(filename))?;
    let model_dir = pins::parakeet_cache_root(journal)
        .join("models")
        .join(repo.replace('/', "__"))
        .join(revision);
    fs::create_dir_all(&model_dir)
        .map_err(|error| failure("io", "model_dir_create_failed", error, 74))?;
    let dest = model_dir.join(filename);
    *status_value = status::write_status(
        journal,
        status::transition(status_value.clone(), "downloading", None, None)
            .map_err(|error| failure("state", "transition_failed", error, 74))?,
    )
    .map_err(|error| failure("state", "status_write_failed", error, 74))?;
    fetch_install_member(
        journal,
        status_value,
        artifact,
        &dest,
        completed_bytes,
        total_bytes,
        "model_download_failed",
    )?;
    *status_value = status::write_status(
        journal,
        status::transition(status_value.clone(), "verifying", None, None)
            .map_err(|error| failure("state", "transition_failed", error, 74))?,
    )
    .map_err(|error| failure("state", "status_write_failed", error, 74))?;
    write_parakeet_model_manifest(&model_dir, status_value)?;
    Ok(dest)
}

pub(crate) fn write_parakeet_model_manifest(
    model_dir: &Path,
    status_value: &status::InstallStatus,
) -> Result<(), DispatchError> {
    let built = manifest::build_manifest(
        "parakeet",
        "parakeet-model",
        status_value
            .target_fingerprint_sha256
            .as_deref()
            .unwrap_or(""),
        json!({"pin_identity": pins::parakeet_model_identity()}),
        manifest::inventory_for_members(model_dir, &[pins::PARAKEET_MODEL.1], "model")
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        status_value.attempt_id.as_deref(),
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(model_dir), &built)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    Ok(())
}

/// A dropped connection is common over a multi-gigabyte model, so a member is
/// fetched again before the install fails. Cancel stops the installer process,
/// so the waits stay cancellable.
#[cfg(not(test))]
const MODEL_FETCH_RETRY_WAITS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(30)];
#[cfg(test)]
const MODEL_FETCH_RETRY_WAITS: [Duration; 2] = [Duration::ZERO, Duration::ZERO];

/// Only a network failure is worth another try; a size or digest mismatch, a
/// refused host or a full disk fails the same way again.
fn is_transient_download(error: &DispatchError) -> bool {
    error.envelope.error.as_ref().is_some_and(|error| {
        error.kind == "download"
            && matches!(
                error.reason_code.as_str(),
                "model_download_failed" | "download_origin_unreachable"
            )
    })
}

/// Run `attempt`, and after a transient download failure wait and run it
/// again, once per entry in `waits`. `before_retry` refuses a retry for an
/// attempt that has since been superseded.
fn retry_transient_fetch<S>(
    state: &mut S,
    mut attempt: impl FnMut(&mut S) -> Result<(), DispatchError>,
    mut before_retry: impl FnMut(&mut S) -> Result<(), DispatchError>,
    waits: &[Duration],
    mut sleep: impl FnMut(Duration),
) -> Result<(), DispatchError> {
    let mut waits = waits.iter();
    loop {
        match attempt(state) {
            Err(error) if is_transient_download(&error) => {
                let Some(wait) = waits.next() else {
                    return Err(error);
                };
                sleep(*wait);
                before_retry(state)?;
            }
            result => return result,
        }
    }
}

fn install_model(
    journal: &Path,
    model_id: &str,
    status_value: &mut status::InstallStatus,
    completed_bytes: u64,
    total_bytes: u64,
) -> Result<(), DispatchError> {
    #[cfg(any(test, feature = "test-hooks"))]
    TEST_INSTALL_MODEL_COUNT.with(|count| count.set(count.get() + 1));

    let identity = pins::model_identity(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    let root = pins::model_directory(journal, model_id);
    let generation = pins::model_generation(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    let fetch_root = pins::model_fetch_directory(journal, model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    fs::create_dir_all(&fetch_root).map_err(|e| failure("io", "model_dir_create_failed", e, 74))?;
    let _write = solstone_core_artifact_download::generation::hold_write(&root)
        .map_err(|e| failure("state", "model_in_use", e, 74))?;
    fs::create_dir_all(&root)
        .map_err(|error| failure("io", "model_dir_create_failed", error, 74))?;
    let mut files = vec![identity["filename"].as_str().unwrap()];
    if let (Some(name), Some(_hash)) = (
        identity["mmproj_filename"].as_str(),
        identity["mmproj_sha256"].as_str(),
    ) {
        files.push(name);
    }
    *status_value = status::write_status(
        journal,
        status::transition(status_value.clone(), "downloading", None, None)
            .map_err(|e| failure("state", "transition_failed", e, 74))?,
    )
    .map_err(|e| failure("state", "status_write_failed", e, 74))?;
    let mut cumulative_bytes = completed_bytes;

    for name in &files {
        status::assert_current(journal, status_value)
            .map_err(|error| failure("state", "attempt_superseded", error, 74))?;
        let artifact = select_artifact("local-model", None, None, None, Some(name))?;
        let dest = fetch_root.join(name);
        if solstone_core_artifact_download::verify_sha256(&root.join(name), artifact.sha256).is_ok()
        {
            cumulative_bytes += artifact.size_bytes;
            continue;
        }
        let member = generation
            .files
            .iter()
            .find(|m| m.file == *name)
            .expect("pinned model member");
        if let Some(source) = pins::reusable_model_member(journal, model_id, member)
            && fs::hard_link(source, root.join(name)).is_ok()
        {
            cumulative_bytes += artifact.size_bytes;
            continue;
        }
        // Admission may have credited a reusable member. If hardlinks are not
        // supported, recheck the allocation needed by the download fallback.
        let remaining = artifact.size_bytes.saturating_sub(
            solstone_core_artifact_download::retained_prefix_bytes(
                &dest,
                artifact.sha256,
                artifact.size_bytes,
            ),
        );
        if fit_report::free_bytes(&fetch_root).is_ok_and(|free| free < remaining) {
            return Err(failure(
                "disk",
                "disk_space_insufficient",
                "not enough space to finish downloading the model",
                74,
            ));
        }
        retry_transient_fetch(
            status_value,
            |status_value| {
                fetch_install_member(
                    journal,
                    status_value,
                    artifact,
                    &dest,
                    cumulative_bytes,
                    total_bytes,
                    "model_download_failed",
                )
            },
            |status_value| {
                status::assert_current(journal, status_value)
                    .map(|_| ())
                    .map_err(|error| failure("state", "attempt_superseded", error, 74))
            },
            &MODEL_FETCH_RETRY_WAITS,
            std::thread::sleep,
        )?;
        solstone_core_journal_io::install_retained_file(
            &dest,
            root.join(name),
            solstone_core_journal_io::AtomicWriteOptions::default(),
        )
        .map_err(|e| failure("io", "model_publish_failed", e, 74))?;
        cumulative_bytes += artifact.size_bytes;
    }

    // Verification begins only after every pinned file has completed its fetch.
    // Preserve the exact final byte count even if the last callback was coalesced.
    status_value.progress_bytes_received = Some(cumulative_bytes);
    status_value.progress_bytes_total = Some(total_bytes);

    status::assert_current(journal, status_value)
        .map_err(|error| failure("state", "attempt_superseded", error, 74))?;

    *status_value = status::write_status(
        journal,
        status::transition(status_value.clone(), "verifying", None, None)
            .map_err(|error| failure("state", "transition_failed", error, 74))?,
    )
    .map_err(|error| failure("state", "status_write_failed", error, 74))?;

    status::assert_current(journal, status_value)
        .map_err(|error| failure("state", "attempt_superseded", error, 74))?;

    if root == generation.directory(&pins::cache_root(journal).join("models")) {
        generation
            .seal(&root)
            .map_err(|e| failure("io", "model_publish_failed", e, 74))?;
    }
    let built = manifest::build_manifest(
        "local",
        "local-model",
        status_value
            .target_fingerprint_sha256
            .as_deref()
            .unwrap_or(""),
        json!({"pin_identity":identity}),
        manifest::inventory_for_members(&root, &files, "model")
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        status_value.attempt_id.as_deref(),
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(&root), &built)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    drop(_write);
    pins::collect_model_generations_under_install_lease(journal);
    Ok(())
}

/// Cancel only the named attempt. The composition layer owns process observation;
/// this domain owns the lease and status compare-and-write.
pub fn cancel_local_bootstrap(
    journal: &Path,
    provider: &str,
    expected_attempt_id: &str,
    stop_writer: impl FnOnce(&Value) -> Result<(), String>,
) -> Result<status::InstallStatus, DispatchError> {
    let check = |current: &status::InstallStatus| {
        if current.attempt_id.as_deref() == Some(expected_attempt_id) {
            Ok(())
        } else {
            Err(failure(
                "conflict",
                "attempt_mismatch",
                "install attempt changed",
                65,
            ))
        }
    };
    let current = status::read_status(journal, provider)
        .map_err(|error| failure("state", "read_status_failed", error, 74))?;
    check(&current)?;
    if !status::is_in_flight(&current.install_state) {
        return Ok(current);
    }
    if lease::is_held(journal, provider)
        .map_err(|error| failure("state", "lease_unavailable", error, 74))?
    {
        let owner = current.owner.as_ref().ok_or_else(|| {
            failure(
                "state",
                "owner_unavailable",
                "installer identity is unavailable",
                74,
            )
        })?;
        stop_writer(owner).map_err(|error| failure("state", "cancel_failed", error, 74))?;
    }
    // No terminal write while the installer can still publish progress or success.
    let _held = lease::acquire(journal, provider)
        .map_err(|error| failure("state", "lease_unavailable", error, 74))?
        .ok_or_else(|| failure("busy", "install_busy", "installer still owns its lease", 75))?;
    let latest = status::read_status(journal, provider)
        .map_err(|error| failure("state", "read_status_failed", error, 74))?;
    check(&latest)?;
    if !status::is_in_flight(&latest.install_state) {
        return Ok(latest);
    }
    let cancelled = status::transition(
        latest,
        "failed",
        Some("install_cancelled".into()),
        Some("install_cancelled".into()),
    )
    .map_err(|error| failure("state", "transition_failed", error, 74))?;
    status::write_status(journal, cancelled)
        .map_err(|error| failure("state", "status_write_failed", error, 74))
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|candidate| candidate == name) {
            return Some(path);
        }
    }
    None
}

fn hoist_binary(staging: &Path, binary: &Path, backend: &str) -> std::io::Result<()> {
    let final_binary = staging.join("llama-server");
    if binary == final_binary {
        return Ok(());
    }
    if backend == "metal" || backend == "vulkan" {
        flatten_binary_bundle(staging, binary)
    } else {
        fs::rename(binary, final_binary)
    }
}

fn flatten_binary_bundle(staging: &Path, binary: &Path) -> std::io::Result<()> {
    let parent = binary.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "binary has no parent")
    })?;
    if parent == staging {
        return Ok(());
    }
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let destination = staging.join(entry.file_name());
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "bundle destination already exists: {}",
                    destination.display()
                ),
            ));
        }
        fs::rename(entry.path(), destination)?;
    }
    fs::remove_dir(parent)
}

fn publish_staged_tree(staging: &Path, target: &Path) -> std::io::Result<()> {
    publish_staged_tree_with(staging, target, &mut |from, to| fs::rename(from, to))
}

fn publish_staged_tree_with(
    staging: &Path,
    target: &Path,
    rename: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no parent")
    })?;
    let name = target.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no file name")
    })?;
    let aside = loop {
        let candidate = parent.join(format!(
            ".{}.previous.{}.{}",
            name.to_string_lossy(),
            std::process::id(),
            PUBLISH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        if !candidate.exists() {
            break candidate;
        }
    };
    let had_target = target.exists();
    if had_target {
        rename(target, &aside)?;
    }
    if let Err(error) = rename(staging, target) {
        if had_target {
            let _ = rename(&aside, target);
        }
        let _ = fs::remove_dir_all(staging);
        return Err(error);
    }
    if had_target {
        let _ = fs::remove_dir_all(aside);
    }
    Ok(())
}

fn journal(object: &Map<String, Value>) -> Result<PathBuf, DispatchError> {
    Ok(PathBuf::from(required(object, "journal")?))
}
fn required(object: &Map<String, Value>, field: &str) -> Result<String, DispatchError> {
    string(object, field).ok_or_else(|| {
        failure(
            "input",
            "missing_field",
            format!("{field} must be a string"),
            65,
        )
    })
}
fn string(object: &Map<String, Value>, field: &str) -> Option<String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}
fn array_strings(object: &Map<String, Value>, field: &str) -> Result<Vec<String>, DispatchError> {
    object
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            failure(
                "input",
                "missing_field",
                format!("{field} must be an array"),
                65,
            )
        })?
        .iter()
        .map(|value| {
            value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                failure(
                    "input",
                    "invalid_field",
                    format!("{field} must contain strings"),
                    65,
                )
            })
        })
        .collect()
}
fn failure(kind: &str, reason: &str, message: impl ToString, exit_code: u8) -> DispatchError {
    DispatchError {
        envelope: InstallEnvelope::error(kind, reason, message),
        exit_code,
    }
}

#[cfg(all(test, feature = "test-fixture-pin"))]
fn windows_payload_test_keys() -> &'static minisign::KeyPair {
    static KEYS: std::sync::OnceLock<minisign::KeyPair> = std::sync::OnceLock::new();
    KEYS.get_or_init(|| {
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let pin_dir = tempfile::tempdir().unwrap();
        let pin = pin_dir.path().join("fixture.pub");
        std::fs::write(&pin, keys.pk.to_box().unwrap().to_bytes()).unwrap();
        solstone_core_distribution::manifest_verify::install_test_fixture_pin(&pin).unwrap();
        keys
    })
}

#[cfg(test)]
mod nvidia_probe_retry_tests {
    use super::live_nvidia_probe_is_transient_undetected;
    use crate::nvidia::NVIDIA_PROBE_SCHEMA;
    use serde_json::json;

    fn probe(detected: bool, error: Option<&str>) -> crate::NvidiaProbe {
        serde_json::from_value(json!({
            "schema": NVIDIA_PROBE_SCHEMA,
            "detected": detected,
            "gpu_index": detected.then_some(0),
            "gpu_name": detected.then_some("NVIDIA GeForce RTX 4090"),
            "compute_cap": detected.then_some("8.9"),
            "arch": detected.then_some("sm_89"),
            "driver_cuda_major": detected.then_some(13),
            "vram_mib": detected.then_some(24564),
            "unified_memory_mib": null,
            "probe_error": error,
        }))
        .unwrap()
    }

    #[test]
    fn retries_only_transient_undetected_nvidia_smi_failures() {
        let transient = [
            "NVIDIA GPU probe timed out after 10s",
            "NVIDIA GPU probe could not start: resource temporarily unavailable",
            "NVIDIA GPU probe could not wait for nvidia-smi: broken pipe",
            "NVIDIA GPU probe exited with status 255",
        ];
        let permanent = [
            "nvidia-smi binary not found",
            "NVIDIA GPU probe returned no rows",
            "NVIDIA GPU probe returned invalid CSV",
        ];
        for error in transient {
            assert!(
                live_nvidia_probe_is_transient_undetected(&probe(false, Some(error))),
                "{error}"
            );
        }
        for error in permanent {
            assert!(
                !live_nvidia_probe_is_transient_undetected(&probe(false, Some(error))),
                "{error}"
            );
        }
        assert!(!live_nvidia_probe_is_transient_undetected(&probe(
            true, None
        )));
    }
}
