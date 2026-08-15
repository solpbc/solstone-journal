// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! macOS Metal readiness over the shared native Qwen 3.5 4B install.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::plan::{MetalTierMetadata, metal_tier};

use super::{DispatchError, failure, journal, manifest, pins, status};

const PLATFORM_KEY: &str = "aarch64-apple-darwin";
const MODEL_ID: &str = "local/qwen3.5-4b";
const MODEL_FILENAME: &str = "Qwen3.5-4B-Q4_K_M.gguf";
const PROJECTOR_FILENAME: &str = "mmproj-F16.gguf";

pub fn inspect(object: &Map<String, Value>) -> Result<Value, DispatchError> {
    inspect_with(object, &pins::platform_key())
}

pub fn inspect_with(
    object: &Map<String, Value>,
    platform_key: &str,
) -> Result<Value, DispatchError> {
    require_platform(platform_key)?;
    let journal = journal(object)?;
    let root = pins::cache_root(&journal);
    let (release, _, _, _) = pins::vulkan_pin(PLATFORM_KEY)
        .ok_or_else(|| failure("platform", "unsupported_platform", PLATFORM_KEY, 65))?;
    let runtime_dir = root.join("bin").join(PLATFORM_KEY).join(release);
    let model_dir = root.join("models").join(MODEL_ID.replace('/', "__"));
    let runtime_identity = pins::vulkan_identity(PLATFORM_KEY).expect("Darwin runtime pin");
    let model_identity = pins::model_identity(MODEL_ID).expect("Qwen 3.5 4B model pin");
    let runtime_proof = manifest::prove_manifest(
        &manifest::artifact_manifest_path(&runtime_dir),
        &runtime_identity,
    );
    let model_manifest = manifest::artifact_manifest_path(&model_dir);
    let model_proof =
        manifest::prove_manifest_member(&model_manifest, &model_identity, MODEL_FILENAME);
    let projector_proof =
        manifest::prove_manifest_member(&model_manifest, &model_identity, PROJECTOR_FILENAME);
    let binary_path = runtime_dir.join("llama-server");
    let probe = readiness_probe(&binary_path);
    let (tier, tier_metadata) = metal_tier(unified_memory_mib(object)?);
    let (readiness, reason_code, failed_component) =
        readiness_status(&runtime_proof, &model_proof, &projector_proof, &probe);
    let target = current_target(&journal)?;
    let install = status::read_status(&journal, "local")
        .ok()
        .filter(status_targets_native)
        .map(|value| serde_json::to_value(value).expect("install status serializes"))
        .unwrap_or(Value::Null);
    Ok(json!({
        "provider":"local",
        "backend":"metal",
        "ready":readiness == "ready",
        "status":readiness,
        "reason_code":reason_code,
        "failed_component":failed_component,
        "target":{
            "model_id":MODEL_ID,
            "target_fingerprint_json":target["target_fingerprint_json"],
            "target_fingerprint_sha256":target["target_fingerprint_sha256"],
        },
        "install":install,
        "host":{
            "platform_supported":true,
            "backend":"metal",
            "backend_reason":"Darwin Metal runtime",
        },
        "artifacts":{
            "model_id":MODEL_ID,
            "binary_installed":runtime_proof["status"] == "ready",
            "model_installed":model_proof["status"] == "ready" && projector_proof["status"] == "ready",
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

fn current_target(journal: &Path) -> Result<Value, DispatchError> {
    let target =
        super::local_target_for_key(journal, MODEL_ID, super::LocalBackend::Metal, PLATFORM_KEY)?;
    super::resolved_fingerprint(target)
}

pub fn status_targets_native(value: &status::InstallStatus) -> bool {
    let Ok(target) = current_target(Path::new("")) else {
        return false;
    };
    value.target_fingerprint_json.as_deref() == target["target_fingerprint_json"].as_str()
        && value.target_fingerprint_sha256.as_deref()
            == target["target_fingerprint_sha256"].as_str()
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
        "model_bytes":2_740_937_888_u64,
        "projector_bytes":672_423_616_u64,
        "context_tokens":context_tokens,
        "parallel_slots":parallel_slots,
        "prompt_cache_mib":prompt_cache_mib,
        "tier":tier,
    })
}
