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
pub mod fingerprint;
pub mod lease;
pub mod manifest;
pub mod mlx;
pub mod pins;
pub mod readiness;
pub mod status;

static PUBLISH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallVerb {
    PinsLocal,
    PathsLocal,
    FingerprintLocal,
    FingerprintMlx,
    VerifySha256,
    CudaTrust,
    ManifestVulkan,
    ManifestCuda,
    ManifestModel,
    InspectLocal,
    InspectMlx,
    ProbeBinary,
    RunLocal,
    RunMlx,
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
            let target = local_target(&journal, &required(&object, "model_id")?)?;
            resolved_fingerprint(target)?
        }
        InstallVerb::FingerprintMlx => {
            let target = mlx_target(&required(&object, "model_id")?)?;
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
        InstallVerb::InspectLocal => readiness::inspect_local(object),
        InstallVerb::InspectMlx => readiness::inspect_mlx(object),
        InstallVerb::ProbeBinary => readiness::probe_binary(&object),
        InstallVerb::RunLocal => run_local(&object)?,
        InstallVerb::RunMlx => run_mlx(&object)?,
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

fn parakeet_key(object: &Map<String, Value>) -> Result<String, DispatchError> {
    match string(object, "artifact_key") {
        Some(key) => Ok(key),
        None => pins::parakeet_host_artifact_key()
            .map_err(|error| failure("platform", "unsupported_platform", error, 65)),
    }
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
    Mlx,
    Parakeet,
}
impl RunKind {
    fn status_provider(self) -> &'static str {
        match self {
            Self::Local | Self::Mlx => "local",
            Self::Parakeet => "parakeet",
        }
    }
}

fn run_local(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    run(object, RunKind::Local)
}
fn run_mlx(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    run(object, RunKind::Mlx)
}
fn run_parakeet(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    run(object, RunKind::Parakeet)
}

fn run(object: &Map<String, Value>, kind: RunKind) -> Result<Value, DispatchError> {
    let journal = journal(object)?;
    let provider = kind.status_provider();
    let Some(_lease) = lease::acquire(&journal, provider)
        .map_err(|error| failure("io", "lease_error", error, 74))?
    else {
        return Err(failure(
            "busy",
            "install_busy",
            format!("{provider} install lease is held"),
            lease::BUSY_EXIT_CODE,
        ));
    };
    let fingerprint = match kind {
        RunKind::Local => local_target(
            &journal,
            &string(object, "model_id").unwrap_or_else(|| "local/qwen3.5-4b".to_owned()),
        )?,
        RunKind::Mlx => mlx_target(
            &string(object, "model_id").unwrap_or_else(|| "local/qwen3.5-4b".to_owned()),
        )?,
        RunKind::Parakeet => parakeet_target(&journal)?,
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
        RunKind::Mlx => run_mlx_install(object),
        RunKind::Local => run_local_install(object, &mut state, start),
        RunKind::Parakeet => run_parakeet_install(&journal, &mut state),
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

fn local_target(journal: &Path, model_id: &str) -> Result<Value, DispatchError> {
    let key = pins::platform_key();
    let choice = local_backend_choice(journal, None);
    let runtime_pin = match choice.backend {
        crate::Backend::Cuda => pins::cuda_identity(&key),
        crate::Backend::Vulkan => pins::vulkan_identity(&key),
    }
    .ok_or_else(|| {
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
        json!({"provider":"local","runtime":"llama.cpp","backend":match choice.backend { crate::Backend::Cuda => "cuda", crate::Backend::Vulkan => "vulkan" },"backend_reason":choice.reason,"runtime_pin":runtime_pin,"model_pin":model_pin}),
    )
}

pub(crate) fn local_backend_choice(
    journal: &Path,
    nvidia_probe: Option<crate::NvidiaProbe>,
) -> crate::BackendChoice {
    let probe = nvidia_probe.unwrap_or_else(crate::probe_nvidia_gpu);
    let key = pins::platform_key();
    let trust = pins::cuda_pin(&key)
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
    crate::select_local_backend(
        &probe,
        &crate::CUDA_EMBEDDED_ARCH_SET,
        crate::CUDA_MIN_DRIVER_VERSION,
        trust,
        status::read_status(journal, "local")
            .map(|value| {
                value.install_state == "installed"
                    && value
                        .target_fingerprint_json
                        .as_deref()
                        .is_some_and(|target| {
                            serde_json::from_str::<Value>(target)
                                .ok()
                                .and_then(|target| {
                                    target
                                        .get("backend")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                })
                                .as_deref()
                                == Some("cuda")
                        })
            })
            .unwrap_or(false),
    )
}

fn mlx_target(model_id: &str) -> Result<Value, DispatchError> {
    let model = pins::MLX_MODELS
        .iter()
        .find(|model| model.0 == model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    Ok(
        json!({"provider":"local","runtime":"mlx","model_pin":{"unit":"mlx-snapshot","model_id":model.0,"repo":model.1,"revision":model.2,"soft_token_budget":if model.0 == "gemma-4-26b-a4b-it-mlx-4bit" { Value::from(1120) } else { Value::Null }}}),
    )
}

/// Mirrors Python's `parakeet_install.target_fingerprint` field-for-field:
/// `provider`, `runtime`, `artifact_key`, `binary_pins` (cpu then vulkan,
/// matching `PARAKEET_CPP_BINARY_BACKENDS`'s order), `model_pin`, `cache_root`.
fn parakeet_target(journal: &Path) -> Result<Value, DispatchError> {
    let key = pins::parakeet_host_artifact_key()
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
) -> Result<Value, DispatchError> {
    let journal = journal(object)?;
    let model_id = string(object, "model_id").unwrap_or_else(|| "local/qwen3.5-4b".to_owned());
    let key = pins::platform_key();
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
    let root = pins::cache_root(&journal);
    let (url, digest, filename, install_dir, pin_identity, exclude_names, cuda) =
        if backend == "cuda" {
            let (url, digest, _) = pins::cuda_pin(&key)
                .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
            let install_dir = root.join("cuda").join(&key).join(digest);
            (
                url,
                digest,
                format!("llama-{digest}.tar.gz"),
                install_dir,
                pins::cuda_identity(&key).unwrap(),
                Vec::new(),
                true,
            )
        } else {
            let (release, filename, digest, _) = pins::vulkan_pin(&key)
                .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
            (
            Box::leak(
                format!(
                    "https://github.com/ggml-org/llama.cpp/releases/download/{release}/{filename}"
                )
                .into_boxed_str(),
            ) as &str,
            digest,
            filename.to_owned(),
            root.join("bin").join(&key).join(release),
            pins::vulkan_identity(&key).unwrap(),
            vec![filename.to_owned()],
            false,
        )
        };
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
    archive::download(url, &archive_path, digest, |received, total| {
        if let Ok(Some(next)) = status::bump_progress(
            status_value.clone(),
            Some(received),
            total,
            &mut progress_at,
        ) && let Ok(written) = status::write_status(&journal, next)
        {
            *status_value = written;
        }
    })
    .map_err(|error| failure("download", "download_failed", error, 74))?;
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
    if binary != final_binary {
        fs::rename(&binary, &final_binary)
            .map_err(|error| failure("io", "binary_move_failed", error, 74))?;
    }
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
    install_model(&journal, &model_id, status_value)?;
    Ok(
        json!({"backend":backend,"binary_path":install_dir.join("llama-server"),"model_id":model_id}),
    )
}

fn run_mlx_install(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    let journal = journal(object)?;
    let model_id = required(object, "model_id")?;
    let model = pins::MLX_MODELS
        .iter()
        .find(|model| model.0 == model_id)
        .ok_or_else(|| failure("model", "unsupported_model", &model_id, 65))?;
    let source = PathBuf::from(required(object, "source_snapshot")?);
    let destination = pins::cache_root(&journal)
        .join("mlx")
        .join(model.1.replace('/', "--"))
        .join(model.2)
        .join("snapshot");
    if !source.is_dir() {
        return Err(failure(
            "input",
            "snapshot_source_missing",
            "source_snapshot must be a directory",
            65,
        ));
    }
    let staging = destination.parent().unwrap().join(format!(
        ".{}.staging",
        destination.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| failure("io", "snapshot_create_failed", error, 74))?;
    copy_tree(&source, &staging)?;
    let hashes = object
        .get("lfs_sha256")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| {
            failure(
                "input",
                "missing_field",
                "lfs_sha256 must describe the source snapshot",
                65,
            )
        })?;
    mlx::validate_snapshot_sha256(&staging, &hashes)
        .map_err(|error| failure("verification", "snapshot_sha256_mismatch", error, 65))?;
    publish_staged_tree(&staging, &destination)
        .map_err(|error| failure("io", "snapshot_replace_failed", error, 74))?;
    let target = mlx_target(&model_id)?;
    let target_json = fingerprint::canonical(target)
        .map_err(|error| failure("input", "fingerprint_invalid", error, 65))?;
    let target_sha = fingerprint::sha256(&target_json);
    let identity = json!({"unit":"mlx-snapshot","model_id":model.0,"repo":model.1,"revision":model.2,"size_bytes":model.3});
    let manifest_dir = destination.parent().unwrap();
    let built = manifest::build_manifest(
        "local",
        "mlx-snapshot",
        &target_sha,
        json!({"pin_identity":identity}),
        manifest::inventory_for_tree(&destination, "snapshot_file")
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        Some(&destination),
        None,
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest_dir.join("snapshot.manifest.json"), &built)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    let variant = if model.0 == "gemma-4-26b-a4b-it-mlx-4bit" {
        let path = manifest_dir.join("variant-solstone-budget1120");
        mlx::create_gemma4_variant(&destination, &path)
            .map_err(|error| failure("io", "variant_create_failed", error, 74))?;
        let built = manifest::build_manifest("local", "mlx-variant", &target_sha, json!({"pin_identity":{"unit":"mlx-variant","model_id":model.0,"repo":model.1,"revision":model.2,"size_bytes":model.3,"variant":"solstone-budget1120"}}), manifest::inventory_for_tree(&path, "variant_file").map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?, Some(&path), None).map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
        manifest::write_manifest(
            &manifest_dir.join("variant-solstone-budget1120.manifest.json"),
            &built,
        )
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
        Some(path)
    } else {
        None
    };
    Ok(json!({"snapshot_path":destination,"variant_path":variant,"source":"local_snapshot_seam"}))
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
    let mut binaries = Vec::new();
    for backend in ["cpu", "vulkan"] {
        let (release, filename, digest, binary_name) = pins::parakeet_backend_pin(&key, backend)
            .ok_or_else(|| failure("platform", "unsupported_platform", &key, 65))?;
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
        let url = format!(
            "https://github.com/mudler/parakeet.cpp/releases/download/{release}/{filename}"
        );
        archive::download(&url, &archive_path, digest, |received, total| {
            if let Ok(Some(next)) = status::bump_progress(
                status_value.clone(),
                Some(received),
                total,
                &mut progress_at,
            ) && let Ok(written) = status::write_status(journal, next)
            {
                *status_value = written;
            }
        })
        .map_err(|error| failure("download", "download_failed", error, 74))?;
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
    let model_path = install_parakeet_model(journal, status_value)?;
    Ok(json!({"artifact_key": key, "binaries": binaries, "model_path": model_path}))
}

fn install_parakeet_model(
    journal: &Path,
    status_value: &mut status::InstallStatus,
) -> Result<PathBuf, DispatchError> {
    let (repo, filename, revision, sha256, ..) = pins::PARAKEET_MODEL;
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
    archive::download(
        &format!("https://huggingface.co/{repo}/resolve/{revision}/{filename}"),
        &dest,
        sha256,
        |_received, _total| {},
    )
    .map_err(|error| failure("download", "model_download_failed", error, 74))?;
    *status_value = status::write_status(
        journal,
        status::transition(status_value.clone(), "verifying", None, None)
            .map_err(|error| failure("state", "transition_failed", error, 74))?,
    )
    .map_err(|error| failure("state", "status_write_failed", error, 74))?;
    let built = manifest::build_manifest(
        "parakeet",
        "parakeet-model",
        status_value
            .target_fingerprint_sha256
            .as_deref()
            .unwrap_or(""),
        json!({"pin_identity": pins::parakeet_model_identity()}),
        manifest::inventory_for_tree(&model_dir, "model")
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        status_value.attempt_id.as_deref(),
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(&model_dir), &built)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    Ok(dest)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), DispatchError> {
    for entry in std::fs::read_dir(source)
        .map_err(|error| failure("io", "snapshot_read_failed", error, 74))?
    {
        let entry = entry.map_err(|error| failure("io", "snapshot_read_failed", error, 74))?;
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| failure("io", "snapshot_read_failed", error, 74))?
            .is_dir()
        {
            fs::create_dir_all(&target)
                .map_err(|error| failure("io", "snapshot_create_failed", error, 74))?;
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)
                .map_err(|error| failure("io", "snapshot_copy_failed", error, 74))?;
        }
    }
    Ok(())
}

fn install_model(
    journal: &Path,
    model_id: &str,
    status_value: &mut status::InstallStatus,
) -> Result<(), DispatchError> {
    let identity = pins::model_identity(model_id)
        .ok_or_else(|| failure("model", "unsupported_model", model_id, 65))?;
    let root = pins::cache_root(journal)
        .join("models")
        .join(model_id.replace('/', "__"));
    fs::create_dir_all(&root)
        .map_err(|error| failure("io", "model_dir_create_failed", error, 74))?;
    let repo = identity["repo"].as_str().unwrap();
    let revision = identity["revision"].as_str().unwrap();
    let mut files = vec![(
        identity["filename"].as_str().unwrap(),
        identity["sha256"].as_str().unwrap(),
    )];
    if let (Some(name), Some(hash)) = (
        identity["mmproj_filename"].as_str(),
        identity["mmproj_sha256"].as_str(),
    ) {
        files.push((name, hash));
    }
    for (name, hash) in files {
        archive::download(
            &format!("https://huggingface.co/{repo}/resolve/{revision}/{name}"),
            &root.join(name),
            hash,
            |_received, _total| {},
        )
        .map_err(|error| failure("download", "model_download_failed", error, 74))?;
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
