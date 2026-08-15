// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Non-owner-facing macOS Metal candidate installer and readiness probe.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_assets::{Artifact, Platform, resolve};

use crate::plan::{MetalTierMetadata, metal_tier};

use super::{
    DispatchError, archive, download_artifact, failure, find_file, fingerprint, journal, manifest,
    pins, publish_staged_tree,
};

const PLATFORM_KEY: &str = "aarch64-apple-darwin";
const RUNTIME_UNIT: &str = "llama-server-vulkan";
const MODEL_UNIT: &str = "local-model";
const MODEL_FILENAME: &str = "Qwen3.5-9B-Q8_0.gguf";
const PROJECTOR_FILENAME: &str = "mmproj-F16.gguf";

pub(super) fn run(
    object: &Map<String, Value>,
    policy: &archive::DownloadHostPolicy<'_>,
) -> Result<Value, DispatchError> {
    let (runtime, model, projector) = artifacts()?;
    run_with(
        object,
        policy,
        &pins::platform_key(),
        runtime,
        model,
        projector,
    )
}

pub(super) fn inspect(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    inspect_with(object, &pins::platform_key())
}

pub(super) fn run_with(
    object: &Map<String, Value>,
    policy: &archive::DownloadHostPolicy<'_>,
    platform_key: &str,
    runtime: &Artifact,
    model: &Artifact,
    projector: &Artifact,
) -> Result<Value, DispatchError> {
    require_platform(platform_key)?;
    let journal = journal(object)?;
    let root = pins::cache_root(&journal);
    let (release, runtime_filename, _, _) = pins::vulkan_pin(PLATFORM_KEY)
        .ok_or_else(|| failure("platform", "unsupported_platform", PLATFORM_KEY, 65))?;
    let runtime_dir = root.join("bin").join(PLATFORM_KEY).join(release);
    let runtime_staging = staged_path(&runtime_dir);
    let model_dir = root.join("models").join(pins::METAL_CANDIDATE_MODEL_SLUG);
    let model_staging = staged_path(&model_dir);
    let target = json!({
        "provider":"local",
        "runtime":"llama.cpp",
        "backend":"metal",
        "runtime_pin":pins::vulkan_identity(PLATFORM_KEY).expect("Darwin runtime pin"),
        "model_pin":pins::metal_candidate_model_identity(),
    });
    let target_json = fingerprint::canonical(target)
        .map_err(|error| failure("input", "fingerprint_invalid", error, 65))?;
    let target_sha256 = fingerprint::sha256(&target_json);

    prepare_staging(&runtime_staging)?;
    let archive_path = runtime_staging.join(runtime_filename);
    download_artifact(runtime, &archive_path, policy, |_, _| {}, "download_failed")?;
    archive::extract_tar_gz(&archive_path, &runtime_staging)
        .map_err(|error| failure("archive", "extract_failed", error, 65))?;
    let binary = find_file(&runtime_staging, "llama-server").ok_or_else(|| {
        failure(
            "archive",
            "binary_missing",
            "llama-server missing from archive",
            65,
        )
    })?;
    let final_binary = runtime_staging.join("llama-server");
    if binary != final_binary {
        fs::rename(&binary, &final_binary)
            .map_err(|error| failure("io", "binary_move_failed", error, 74))?;
    }
    archive::make_executable(&final_binary)
        .map_err(|error| failure("io", "chmod_failed", error, 74))?;
    archive::clear_macos_quarantine(&runtime_staging)
        .map_err(|error| failure("io", "quarantine_clear_failed", error, 74))?;
    write_runtime_manifest(&runtime_staging, &target_sha256, runtime_filename)?;

    prepare_staging(&model_staging)?;
    download_artifact(
        model,
        &model_staging.join(MODEL_FILENAME),
        policy,
        |_, _| {},
        "model_download_failed",
    )?;
    download_artifact(
        projector,
        &model_staging.join(PROJECTOR_FILENAME),
        policy,
        |_, _| {},
        "model_download_failed",
    )?;
    write_model_manifest(&model_staging, &target_sha256)?;

    publish_staged_tree(&runtime_staging, &runtime_dir)
        .map_err(|error| failure("io", "publish_failed", error, 74))?;
    publish_staged_tree(&model_staging, &model_dir)
        .map_err(|error| failure("io", "publish_failed", error, 74))?;
    inspect_with(object, platform_key)
}

pub(super) fn inspect_with(
    object: &Map<String, Value>,
    platform_key: &str,
) -> Result<Value, DispatchError> {
    require_platform(platform_key)?;
    let journal = journal(object)?;
    let root = pins::cache_root(&journal);
    let (release, _, _, _) = pins::vulkan_pin(PLATFORM_KEY)
        .ok_or_else(|| failure("platform", "unsupported_platform", PLATFORM_KEY, 65))?;
    let runtime_dir = root.join("bin").join(PLATFORM_KEY).join(release);
    let model_dir = root.join("models").join(pins::METAL_CANDIDATE_MODEL_SLUG);
    let runtime_proof = manifest::prove_manifest(
        &manifest::artifact_manifest_path(&runtime_dir),
        &pins::vulkan_identity(PLATFORM_KEY).expect("Darwin runtime pin"),
    );
    let model_manifest = manifest::artifact_manifest_path(&model_dir);
    let model_proof = manifest::prove_manifest_member(
        &model_manifest,
        &pins::metal_candidate_model_identity(),
        MODEL_FILENAME,
    );
    let projector_proof = manifest::prove_manifest_member(
        &model_manifest,
        &pins::metal_candidate_model_identity(),
        PROJECTOR_FILENAME,
    );
    let binary_path = runtime_dir.join("llama-server");
    let probe = readiness_probe(&binary_path);
    let (tier, tier_metadata) = metal_tier(unified_memory_mib(object)?);
    let (status, reason_code, failed_component) =
        readiness_status(&runtime_proof, &model_proof, &projector_proof, &probe);
    Ok(json!({
        "provider":"local",
        "backend":"metal",
        "ready":status == "ready",
        "status":status,
        "reason_code":reason_code,
        "failed_component":failed_component,
        "artifacts":{
            "model_id":pins::METAL_CANDIDATE_MODEL_ID,
            "binary_path":binary_path,
            "model_path":model_dir.join(MODEL_FILENAME),
            "projector_path":model_dir.join(PROJECTOR_FILENAME),
        },
        "proof":{
            "server_binary":runtime_proof,
            "model_gguf":model_proof,
            "projector":projector_proof,
            "binary_probe":probe,
        },
        "fit":fit(&tier_metadata, tier.context_tokens, tier.parallel_slots, tier.prompt_cache_mib),
    }))
}

fn artifacts() -> Result<(&'static Artifact, &'static Artifact, &'static Artifact), DispatchError> {
    let models = resolve(MODEL_UNIT, Some(Platform::MacosArm64), None);
    let model = models
        .iter()
        .find(|artifact| artifact.filename == MODEL_FILENAME)
        .copied()
        .ok_or_else(|| {
            failure(
                "registry",
                "artifact_registry_mismatch",
                "candidate GGUF missing",
                65,
            )
        })?;
    let projector = models
        .iter()
        .find(|artifact| artifact.filename == PROJECTOR_FILENAME)
        .copied()
        .ok_or_else(|| {
            failure(
                "registry",
                "artifact_registry_mismatch",
                "candidate projector missing",
                65,
            )
        })?;
    let runtime = resolve(RUNTIME_UNIT, Some(Platform::MacosArm64), None)
        .into_iter()
        .find(|artifact| artifact.artifact_key == Some(PLATFORM_KEY))
        .ok_or_else(|| {
            failure(
                "registry",
                "artifact_registry_mismatch",
                "candidate runtime missing",
                65,
            )
        })?;
    Ok((runtime, model, projector))
}

fn require_platform(platform_key: &str) -> Result<(), DispatchError> {
    if platform_key == PLATFORM_KEY {
        Ok(())
    } else {
        Err(failure(
            "platform",
            "unsupported_platform",
            platform_key,
            65,
        ))
    }
}

fn prepare_staging(path: &Path) -> Result<(), DispatchError> {
    let _ = fs::remove_dir_all(path);
    fs::create_dir_all(path).map_err(|error| failure("io", "staging_create_failed", error, 74))
}

fn staged_path(target: &Path) -> PathBuf {
    target
        .parent()
        .expect("candidate target parent")
        .join(format!(
            ".{}.staging",
            target
                .file_name()
                .expect("candidate target name")
                .to_string_lossy()
        ))
}

fn write_runtime_manifest(
    root: &Path,
    target_sha256: &str,
    archive_filename: &str,
) -> Result<(), DispatchError> {
    let manifest_value = manifest::build_manifest(
        "local",
        RUNTIME_UNIT,
        target_sha256,
        json!({"pin_identity":pins::vulkan_identity(PLATFORM_KEY).expect("Darwin runtime pin")} ),
        manifest::runtime_inventory(root, &[archive_filename.to_owned()])
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        None,
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(root), &manifest_value)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    Ok(())
}

fn write_model_manifest(root: &Path, target_sha256: &str) -> Result<(), DispatchError> {
    let manifest_value = manifest::build_manifest(
        "local",
        MODEL_UNIT,
        target_sha256,
        json!({"pin_identity":pins::metal_candidate_model_identity()}),
        manifest::inventory_for_tree(root, "model")
            .map_err(|error| failure("io", "manifest_inventory_failed", error, 74))?,
        None,
        None,
    )
    .map_err(|error| failure("io", "manifest_build_failed", error, 74))?;
    manifest::write_manifest(&manifest::artifact_manifest_path(root), &manifest_value)
        .map_err(|error| failure("io", "manifest_write_failed", error, 74))?;
    Ok(())
}

fn readiness_probe(binary_path: &Path) -> Value {
    let mut input = Map::new();
    input.insert(
        "path".into(),
        Value::String(binary_path.display().to_string()),
    );
    super::readiness::probe_binary(&input)
}

fn readiness_status(
    binary: &Value,
    model: &Value,
    projector: &Value,
    probe: &Value,
) -> (String, String, Option<&'static str>) {
    for (component, proof) in [
        ("server_binary", binary),
        ("model_gguf", model),
        ("projector", projector),
    ] {
        if proof["status"] != "ready" {
            return (
                proof["status"]
                    .as_str()
                    .unwrap_or("proof-unavailable")
                    .to_owned(),
                proof["reason_code"]
                    .as_str()
                    .unwrap_or("proof_unavailable")
                    .to_owned(),
                Some(component),
            );
        }
    }
    if probe["runnable"] != true {
        return (
            "host-ineligible".into(),
            probe["reason_code"]
                .as_str()
                .unwrap_or("binary_unavailable")
                .to_owned(),
            Some("binary_probe"),
        );
    }
    ("ready".into(), "ready".into(), None)
}

fn unified_memory_mib(object: &Map<String, Value>) -> Result<Option<u64>, DispatchError> {
    match object.get("metal_unified_memory_mib") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number.as_u64().map(Some).ok_or_else(|| {
            failure(
                "input",
                "invalid_request",
                "metal_unified_memory_mib must be an unsigned integer",
                65,
            )
        }),
        Some(_) => Err(failure(
            "input",
            "invalid_request",
            "metal_unified_memory_mib must be an unsigned integer",
            65,
        )),
    }
}

fn fit(
    tier: &MetalTierMetadata,
    context_tokens: u32,
    parallel_slots: u32,
    prompt_cache_mib: u32,
) -> Value {
    json!({
        "measurement":"unmeasured",
        "model_bytes":9527502048_u64,
        "projector_bytes":918166080_u64,
        "context_tokens":context_tokens,
        "parallel_slots":parallel_slots,
        "prompt_cache_mib":prompt_cache_mib,
        "tier":tier,
    })
}
