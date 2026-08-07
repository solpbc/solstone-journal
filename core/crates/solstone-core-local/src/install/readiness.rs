// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use std::path::PathBuf;

use super::{local_backend_choice, manifest, pins, status};

pub fn inspect_local(input: Map<String, Value>) -> Value {
    let journal = input
        .get("journal")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let model_id = input
        .get("model_id")
        .and_then(Value::as_str)
        .unwrap_or("local/qwen3.5-4b");
    let Some(journal) = journal else {
        return unavailable("journal_required", model_id);
    };
    let owned_key = input
        .get("artifact_key")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(pins::platform_key);
    let key = owned_key.as_str();
    let nvidia_probe = input
        .get("nvidia_probe")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .ok()
        .flatten();
    let choice = local_backend_choice(&journal, nvidia_probe);
    let backend = match choice.backend {
        crate::Backend::Cuda => "cuda",
        crate::Backend::Vulkan => "vulkan",
    };
    let root = pins::cache_root(&journal);
    let (binary_root, identity) = if backend == "cuda" {
        (
            pins::cuda_pin(key).map(|(_, digest, _)| root.join("cuda").join(key).join(digest)),
            pins::cuda_identity(key),
        )
    } else {
        (
            pins::vulkan_pin(key)
                .map(|(release, _, _, _)| root.join("bin").join(key).join(release)),
            pins::vulkan_identity(key),
        )
    };
    let platform_supported = identity.is_some();
    let binary_root = binary_root.unwrap_or_else(|| root.join("missing"));
    let identity = identity.unwrap_or(Value::Null);
    let model_root = root.join("models").join(model_id.replace('/', "__"));
    let binary_proof =
        manifest::prove_manifest(&manifest::artifact_manifest_path(&binary_root), &identity);
    let model_proof = manifest::prove_manifest(
        &manifest::artifact_manifest_path(&model_root),
        &pins::model_identity(model_id).unwrap_or(Value::Null),
    );
    let proofs = [binary_proof.clone(), model_proof.clone()];
    let proof_unavailable = proofs
        .iter()
        .find(|proof| proof["status"] == "proof-unavailable");
    let missing = proofs
        .iter()
        .find(|proof| proof["status"] == "missing-or-mismatched");
    let (state, reason) = if let Some(proof) = proof_unavailable {
        (
            "proof-unavailable",
            proof["reason_code"].as_str().unwrap_or("proof_unavailable"),
        )
    } else if let Some(proof) = missing {
        (
            "missing-or-mismatched",
            proof["reason_code"].as_str().unwrap_or("artifact_missing"),
        )
    } else {
        ("ready", "ready")
    };
    let install = status::read_status(&journal, "local")
        .map(|value| serde_json::to_value(value).unwrap())
        .unwrap_or(Value::Null);
    json!({"provider":"local","ready":state=="ready","status":state,"reason_code":reason,"target":{"model_id":model_id,"target_fingerprint_json":install["target_fingerprint_json"],"target_fingerprint_sha256":install["target_fingerprint_sha256"]},"install":install,"host":{"platform_supported":platform_supported,"backend":backend,"backend_reason":choice.reason,"vulkan_observation":input.get("vulkan_observation").cloned().unwrap_or(Value::Null)},"artifacts":{"model_id":model_id,"binary_installed":binary_proof["status"]=="ready","model_installed":model_proof["status"]=="ready","binary_path":binary_root.join("llama-server"),"model_path":model_root},"proof":{"binary":binary_proof,"model":model_proof}})
}

pub fn inspect_mlx(input: Map<String, Value>) -> Value {
    let journal = input
        .get("journal")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let model_id = input
        .get("model_id")
        .and_then(Value::as_str)
        .unwrap_or("Qwen3.5-9B");
    let Some(journal) = journal else {
        return unavailable("journal_required", model_id);
    };
    let Some(model) = pins::MLX_MODELS.iter().find(|model| model.0 == model_id) else {
        return unavailable("unsupported_model", model_id);
    };
    let base = pins::cache_root(&journal)
        .join("mlx")
        .join(model.1.replace('/', "--"))
        .join(model.2);
    let snapshot = base.join("snapshot");
    let identity = json!({"unit":"mlx-snapshot","model_id":model.0,"repo":model.1,"revision":model.2,"size_bytes":model.3});
    let proof = manifest::prove_manifest(&base.join("snapshot.manifest.json"), &identity);
    let package_available = input
        .get("mlx_vlm_importable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let platform_supported = input
        .get("platform_supported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (state, reason) = if proof["status"] != "ready" {
        (
            proof["status"].as_str().unwrap_or("proof-unavailable"),
            proof["reason_code"].as_str().unwrap_or("proof_unavailable"),
        )
    } else if !platform_supported {
        ("host-ineligible", "platform_unsupported")
    } else if !package_available {
        ("host-ineligible", "package_unavailable")
    } else {
        ("ready", "ready")
    };
    json!({"provider":"local","ready":state=="ready","status":state,"reason_code":reason,"target":{"model_id":model_id},"host":{"platform_supported":platform_supported,"package_available":package_available},"artifacts":{"model_id":model_id,"model_installed":proof["status"]=="ready","snapshot_installed":proof["status"]=="ready","snapshot_dir":snapshot},"proof":{"snapshot":proof,"variant":Value::Null}})
}

pub fn probe_binary(input: &Map<String, Value>) -> Value {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return json!({"runnable":false,"reason_code":"path_required"});
    };
    match std::process::Command::new(path).arg("--version").output() {
        Ok(output) if output.status.success() => json!({"runnable":true,"reason_code":Value::Null}),
        Ok(output) => {
            json!({"runnable":false,"reason_code":"binary_exit","exit_code":output.status.code()})
        }
        Err(error) => {
            json!({"runnable":false,"reason_code":"binary_unavailable","message":error.to_string()})
        }
    }
}
fn unavailable(reason: &str, model_id: &str) -> Value {
    json!({"provider":"local","ready":false,"status":"proof-unavailable","reason_code":reason,"target":{"model_id":model_id},"host":{"platform_supported":false},"artifacts":{"model_installed":false},"proof":{}})
}
