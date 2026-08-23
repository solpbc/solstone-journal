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

pub mod archive;
pub mod ced_fixture;
pub mod ced_install;
pub mod ced_readiness;
pub mod coreml_install;
pub mod fingerprint;
pub mod fit_report;
pub mod lease;
pub mod manifest;
pub mod metal_candidate;
pub mod migration;
pub mod pins;
pub mod readiness;
pub mod rerank_install;
pub mod rfdetr_install;
pub mod status;

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
    #[cfg(feature = "test-hooks")]
    use solstone_core_assets::Artifact;
    #[cfg(feature = "test-hooks")]
    use solstone_core_journal_config::{
        JournalConfigRead, parakeet_coreml::ParakeetCoremlSentinel,
    };

    #[cfg(feature = "test-hooks")]
    use super::archive::DownloadHostPolicy;
    #[cfg(feature = "test-hooks")]
    use super::coreml_install::{
        CoremlInstallError, install_with_rows_and_seams, install_with_rows_for_test,
    };
    #[cfg(feature = "test-hooks")]
    use super::rfdetr_install::{
        RfdetrInstallError, RfdetrInstallRecord, check_rfdetr_model_with_artifacts,
        install_rfdetr_with_artifacts,
    };
    use super::{InstallVerb, dispatch, manifest, pins};

    pub struct ParakeetFixture {
        pub cpu_path: PathBuf,
        pub vulkan_path: PathBuf,
        pub model_path: PathBuf,
    }

    fn write_parakeet_binary(path: &Path, executable: bool) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write Parakeet binary fixture");
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
        policy: &DownloadHostPolicy<'_>,
        rows: &[&Artifact],
    ) -> Result<PathBuf, CoremlInstallError> {
        install_with_rows_for_test(home_dir, config, force, policy, rows)
    }

    #[allow(clippy::too_many_arguments)] // Fixture-only write seams prove atomic ordering.
    #[cfg(feature = "test-hooks")]
    pub fn install_coreml_with_seams(
        home_dir: &Path,
        config: &JournalConfigRead,
        force: bool,
        policy: &DownloadHostPolicy<'_>,
        platform: (&str, &str),
        rows: &[&Artifact],
        publish: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
        write: &mut impl FnMut(&Path, &ParakeetCoremlSentinel) -> std::io::Result<()>,
    ) -> Result<PathBuf, CoremlInstallError> {
        install_with_rows_and_seams(
            home_dir, config, force, policy, platform, rows, publish, write,
        )
    }

    #[allow(clippy::too_many_arguments)] // Fixture rows exercise installer cleanup through its production path.
    #[cfg(feature = "test-hooks")]
    pub fn install_rfdetr_with_fixture_artifacts(
        journal: &Path,
        os_name: &str,
        arch: &str,
        force: bool,
        policy: &DownloadHostPolicy<'_>,
        engine: &Artifact,
        model: &Artifact,
    ) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
        install_rfdetr_with_artifacts(journal, os_name, arch, force, policy, engine, model)
    }

    #[cfg(feature = "test-hooks")]
    pub fn check_rfdetr_model_with_fixture_artifacts(
        journal: &Path,
        key: &str,
        engine: &Artifact,
        model: &Artifact,
    ) -> Result<RfdetrInstallRecord, RfdetrInstallError> {
        check_rfdetr_model_with_artifacts(journal, key, engine, model)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};

    pub fn leaked_temps(root: &Path) -> Vec<PathBuf> {
        let mut leaked = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap().filter_map(Result::ok) {
                let path = entry.path();
                let name = path.file_name().unwrap().to_string_lossy();
                if (name.starts_with('.') && name.ends_with(".part"))
                    || name.ends_with(".tmp")
                    || (name.starts_with('.') && name.ends_with(".extract"))
                    || (name.starts_with('.') && name.ends_with(".stage"))
                    || name.starts_with("tmp")
                {
                    leaked.push(path.clone());
                }
                if path.is_dir() {
                    pending.push(path);
                }
            }
        }
        leaked
    }

    pub fn prove_temp_sweep(root: &Path, filename: &str, key: &str) {
        let paths = [
            root.join(format!(".{filename}.part")),
            root.join(format!("{filename}.tmp")),
            root.join(format!(".{key}.extract")),
            root.join(format!(".{key}.stage")),
            root.join(".extract"),
            root.join("tmp-sidecar"),
        ];
        for path in &paths {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"deliberate temporary").unwrap();
        }
        assert_eq!(leaked_temps(root).len(), paths.len());
        for path in &paths {
            fs::remove_file(path).unwrap();
        }
        assert!(leaked_temps(root).is_empty());
    }
}

static PUBLISH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_assets::{Artifact, Backend as AssetBackend, Platform as AssetPlatform, resolve};

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
    dispatch_with_download_policy(verb, request, &archive::PRODUCTION_DOWNLOAD_POLICY)
}

fn dispatch_with_download_policy(
    verb: InstallVerb,
    request: Value,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<InstallEnvelope, DispatchError> {
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
            run_local(&object, policy)?
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
        InstallVerb::RunParakeet => run_parakeet(&object, policy)?,
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
        archive::ArchiveError::DigestMismatch => "download_digest_mismatch",
        archive::ArchiveError::RedirectHopLimitExceeded { .. } => {
            "download_redirect_hop_limit_exceeded"
        }
        archive::ArchiveError::OriginUnavailable { .. } => "download_origin_unreachable",
        archive::ArchiveError::Io(_) | archive::ArchiveError::Download(_) => fallback_reason_code,
        archive::ArchiveError::PathEscape(_) => fallback_reason_code,
    }
}

pub(crate) fn download_artifact(
    artifact: &Artifact,
    destination: &Path,
    policy: &archive::DownloadHostPolicy<'_>,
    progress: impl FnMut(u64, Option<u64>),
    fallback_reason_code: &str,
) -> Result<(), DispatchError> {
    archive::download_verified(artifact, destination, policy, progress).map_err(|error| {
        failure(
            "download",
            download_artifact_reason_code(&error, fallback_reason_code),
            error,
            74,
        )
    })
}

pub(crate) fn ensure_verified(
    artifact: &Artifact,
    destination: &Path,
    policy: &archive::DownloadHostPolicy<'_>,
    progress: impl FnMut(u64, Option<u64>),
    fallback_reason_code: &str,
) -> Result<(), DispatchError> {
    let origin = archive::origin_url(policy.origin_base_url, artifact.origin_key);
    archive::ensure_verified_url(
        &origin,
        artifact.sha256,
        Some(artifact.size_bytes),
        destination,
        policy,
        progress,
    )
    .map(|_| ())
    .map_err(|error| {
        failure(
            "download",
            download_artifact_reason_code(&error, fallback_reason_code),
            error,
            74,
        )
    })
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

fn run_local(
    object: &Map<String, Value>,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<Value, DispatchError> {
    run(object, RunKind::Local, policy)
}
fn run_parakeet(
    object: &Map<String, Value>,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<Value, DispatchError> {
    run(object, RunKind::Parakeet, policy)
}

pub fn install_parakeet_with_lease(
    journal: &Path,
    os_name: &str,
    arch: &str,
    lease: lease::InstallLease,
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
        &archive::PRODUCTION_DOWNLOAD_POLICY,
    )
}

fn run(
    object: &Map<String, Value>,
    kind: RunKind,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<Value, DispatchError> {
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
    run_with_lease(object, kind, journal, lease, None, policy)
}

fn run_with_lease(
    object: &Map<String, Value>,
    kind: RunKind,
    journal: PathBuf,
    _lease: lease::InstallLease,
    parakeet_platform: Option<(&str, &str)>,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<Value, DispatchError> {
    let provider = kind.status_provider();
    let fingerprint = match kind {
        RunKind::Local => local_target(
            &journal,
            &string(object, "model_id").unwrap_or_else(|| "local/qwen3.5-4b".to_owned()),
            local_backend(object)?,
        )?,
        RunKind::Parakeet => parakeet_target_for_install(&journal, parakeet_platform)?,
    };
    let resolved = resolved_fingerprint(fingerprint)?;
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
    )
    .map_err(|error| failure("state", "begin_failed", error, 74))?;
    let start = Instant::now();
    let result = match kind {
        RunKind::Local => run_local_install(object, &mut state, start, policy),
        RunKind::Parakeet => run_parakeet_install(&journal, &mut state, policy),
    };
    match result {
        Ok(result) => {
            if kind == RunKind::Local && result["backend"] == "cuda" {
                let key = pins::platform_key();
                if let Some((_, digest, _)) = pins::cuda_pin(&key) {
                    let root = pins::cache_root(&journal).join("cuda").join(&key);
                    cleanup_legacy_cuda_oci_dirs(&root, &root.join(digest));
                }
            }
            let terminal = status::transition(state, "installed", None, None)
                .and_then(|value| status::write_status(&journal, value))
                .map_err(|error| failure("state", "terminal_write_failed", error, 74))?;
            Ok(json!({"status":terminal,"install":result}))
        }
        Err(error) => {
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
                Some("install_failed".to_owned()),
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
    let key = pins::platform_key();
    local_target_for_key(journal, model_id, backend, &key)
}

fn local_target_for_key(
    journal: &Path,
    model_id: &str,
    backend: LocalBackend,
    key: &str,
) -> Result<Value, DispatchError> {
    let (runtime_pin, backend_name, backend_reason) = match backend {
        LocalBackend::Metal => {
            if key != "aarch64-apple-darwin" {
                return Err(failure(
                    "platform",
                    "unsupported_platform",
                    format!("Metal local inference is unsupported on {key}"),
                    65,
                ));
            }
            (
                pins::vulkan_identity(key),
                "metal",
                "Darwin Metal runtime".to_owned(),
            )
        }
        LocalBackend::Existing => {
            let choice = local_backend_choice(journal, None);
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

pub fn local_backend_choice(
    journal: &Path,
    nvidia_probe: Option<crate::NvidiaProbe>,
) -> crate::BackendChoice {
    let probe = nvidia_probe.unwrap_or_else(crate::probe_nvidia_gpu);
    let key = pins::platform_key();
    let pin = pins::cuda_pin(&key);
    let trust = pin
        .as_ref()
        .map(|(_, digest, _)| {
            let artifact = pins::cache_root(journal)
                .join("cuda")
                .join(&key)
                .join(digest)
                .join("llama-server");
            let declared = crate::CUDA_EMBEDDED_ARCH_SET
                .iter()
                .map(|arch| (*arch).to_owned())
                .collect::<Vec<_>>();
            match manifest::cuda_trust(&artifact, &declared)["trust"].as_str() {
                Some("trusted") => crate::ArtifactTrust::Trusted,
                Some("absent") => crate::ArtifactTrust::Absent,
                _ => crate::ArtifactTrust::Unavailable,
            }
        })
        .unwrap_or(crate::ArtifactTrust::Unavailable);
    if let Some(rejection) = crate::hardware_backend_rejection(
        &probe,
        &crate::CUDA_EMBEDDED_ARCH_SET,
        crate::CUDA_MIN_DRIVER_VERSION,
    ) {
        return rejection;
    }
    if pin.is_none() {
        return crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: "CUDA runtime is not published for this platform".to_owned(),
        };
    }
    if trust == crate::ArtifactTrust::Absent {
        let arch = probe
            .arch
            .as_deref()
            .expect("hardware rejection checked arch");
        let driver_cuda_major = probe
            .driver_cuda_major
            .expect("hardware rejection checked driver CUDA major");
        return crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: format!(
                "compute_cap {arch} covered; driver CUDA {driver_cuda_major} >= {}; CUDA runtime artifact does not cover this GPU",
                crate::CUDA_MIN_DRIVER_VERSION
            ),
        };
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
    crate::BackendChoice {
        backend: crate::Backend::Cuda,
        reason: format!(
            "compute_cap {arch} covered; driver CUDA {driver_cuda_major} >= {}",
            crate::CUDA_MIN_DRIVER_VERSION
        ),
    }
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

fn run_local_install(
    object: &Map<String, Value>,
    status_value: &mut status::InstallStatus,
    _start: Instant,
    policy: &archive::DownloadHostPolicy<'_>,
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
    let root = pins::cache_root(&journal);
    let (filename, install_dir, pin_identity, exclude_names, cuda) = if backend == "cuda" {
        let (_, digest, _) = pins::cuda_pin(&key)
            .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
        let install_dir = root.join("cuda").join(&key).join(digest);
        (
            format!("llama-{digest}.tar.gz"),
            install_dir,
            pins::cuda_identity(&key).unwrap(),
            Vec::new(),
            true,
        )
    } else {
        let (release, filename, _digest, _) = pins::vulkan_pin(&key)
            .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
        (
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
    let archive_path = staging.join(&filename);
    let mut progress_at = Instant::now();
    *status_value = status::write_status(
        &journal,
        status::transition(status_value.clone(), "downloading", None, None)
            .map_err(|error| failure("state", "transition_failed", error, 74))?,
    )
    .map_err(|error| failure("state", "status_write_failed", error, 74))?;
    download_artifact(
        artifact,
        &archive_path,
        policy,
        |received, total| {
            if let Ok(Some(next)) = status::bump_progress(
                status_value.clone(),
                Some(received),
                total,
                &mut progress_at,
            ) && let Ok(written) = status::write_status(&journal, next)
            {
                *status_value = written;
            }
        },
        "download_failed",
    )?;
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
    let manifest_value = manifest::build_manifest(
        "local",
        if cuda {
            "llama-server-cuda"
        } else {
            "llama-server-vulkan"
        },
        status_value.target_fingerprint_sha256.as_deref().unwrap(),
        json!({"pin_identity":pin_identity}),
        manifest::runtime_inventory(&staging, &exclude_names)
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        status_value.attempt_id.as_deref(),
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(&staging), &manifest_value)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    if cuda {
        let declared = crate::CUDA_EMBEDDED_ARCH_SET
            .iter()
            .map(|arch| (*arch).to_owned())
            .collect::<Vec<_>>();
        let trust = manifest::cuda_trust(&final_binary, &declared);
        if trust["trust"] != "trusted" {
            return Err(failure("verification", "cuda_runtime_untrusted", trust, 65));
        }
    }
    publish_staged_tree(&staging, &install_dir)
        .map_err(|error| failure("io", "publish_failed", error, 74))?;
    install_model(&journal, &model_id, status_value, policy)?;
    Ok(
        json!({"backend":backend,"binary_path":install_dir.join("llama-server"),"model_id":model_id}),
    )
}

/// Mirrors `run_local_install`'s download-extract-chmod-manifest-publish
/// shape, but for both parakeet-server backends (cpu, vulkan) plus the
/// model -- `install_parakeet` (Python) installs both backends
/// unconditionally rather than picking one, so this does too.
fn run_parakeet_install(
    journal: &Path,
    status_value: &mut status::InstallStatus,
    policy: &archive::DownloadHostPolicy<'_>,
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
    let mut binaries = Vec::new();
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
        let archive_path = staging.join(filename);
        let mut progress_at = Instant::now();
        *status_value = status::write_status(
            journal,
            status::transition(status_value.clone(), "downloading", None, None)
                .map_err(|error| failure("state", "transition_failed", error, 74))?,
        )
        .map_err(|error| failure("state", "status_write_failed", error, 74))?;
        download_artifact(
            artifact,
            &archive_path,
            policy,
            |received, total| {
                if let Ok(Some(next)) = status::bump_progress(
                    status_value.clone(),
                    Some(received),
                    total,
                    &mut progress_at,
                ) && let Ok(written) = status::write_status(journal, next)
                {
                    *status_value = written;
                }
            },
            "download_failed",
        )?;
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
        binaries.push(json!({"backend": backend, "binary_path": install_dir.join(binary_name)}));
    }
    let model_path = install_parakeet_model(journal, status_value, policy)?;
    Ok(json!({"artifact_key": key, "binaries": binaries, "model_path": model_path}))
}

fn install_parakeet_model(
    journal: &Path,
    status_value: &mut status::InstallStatus,
    policy: &archive::DownloadHostPolicy<'_>,
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
    download_artifact(
        artifact,
        &dest,
        policy,
        |_received, _total| {},
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
        manifest::inventory_for_tree(model_dir, "model")
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        status_value.attempt_id.as_deref(),
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(model_dir), &built)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    Ok(())
}

fn install_model(
    journal: &Path,
    model_id: &str,
    status_value: &mut status::InstallStatus,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<(), DispatchError> {
    let identity = pins::model_identity(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    let root = pins::cache_root(journal)
        .join("models")
        .join(model_id.replace('/', "__"));
    fs::create_dir_all(&root)
        .map_err(|error| failure("io", "model_dir_create_failed", error, 74))?;
    let mut files = vec![identity["filename"].as_str().unwrap()];
    if let (Some(name), Some(_hash)) = (
        identity["mmproj_filename"].as_str(),
        identity["mmproj_sha256"].as_str(),
    ) {
        files.push(name);
    }
    for name in files {
        let artifact = select_artifact("local-model", None, None, None, Some(name))?;
        download_artifact(
            artifact,
            &root.join(name),
            policy,
            |_received, _total| {},
            "model_download_failed",
        )?;
    }
    *status_value = status::write_status(
        journal,
        status::transition(status_value.clone(), "verifying", None, None)
            .map_err(|error| failure("state", "transition_failed", error, 74))?,
    )
    .map_err(|error| failure("state", "status_write_failed", error, 74))?;
    let built = manifest::build_manifest(
        "local",
        "local-model",
        status_value
            .target_fingerprint_sha256
            .as_deref()
            .unwrap_or(""),
        json!({"pin_identity":identity}),
        manifest::inventory_for_tree(&root, "model")
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        status_value.attempt_id.as_deref(),
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(&root), &built)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    Ok(())
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

fn cleanup_legacy_cuda_oci_dirs(root: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let keep_resolved = keep.canonicalize().unwrap_or_else(|_| keep.to_path_buf());
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if path.canonicalize().ok().as_ref() != Some(&keep_resolved)
            && is_legacy_cuda_oci_tree(&path, name)
        {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn is_legacy_cuda_oci_tree(path: &Path, name: &str) -> bool {
    if !path.is_dir()
        || fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        || name.len() != 64
        || !name.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return false;
    }
    let Ok(sidecar) = fs::read(path.join(".oci-install.json")) else {
        return false;
    };
    let Ok(record) = serde_json::from_slice::<Value>(&sidecar) else {
        return false;
    };
    let Some(record) = record.as_object() else {
        return false;
    };
    let Some(image_ref) = record.get("image_ref").and_then(Value::as_str) else {
        return false;
    };
    if !image_ref.ends_with(&format!("@sha256:{name}")) {
        return false;
    }
    let Some(arch) = record.get("arch").and_then(Value::as_str) else {
        return false;
    };
    if pins::cuda_wanted_files(arch).is_none() {
        return false;
    }
    let Some(files) = record.get("files").and_then(Value::as_object) else {
        return false;
    };
    files.values().all(|digest| {
        digest.as_str().is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    })
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
