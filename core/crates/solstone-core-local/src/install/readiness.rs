// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use std::path::PathBuf;

use super::{lease, local_backend_choice, manifest, pins, status};

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
    let model_identity = pins::model_identity(model_id).unwrap_or(Value::Null);
    let model_file = model_identity
        .get("filename")
        .and_then(Value::as_str)
        .unwrap_or("");
    let projector_file = model_identity
        .get("mmproj_filename")
        .and_then(Value::as_str)
        .unwrap_or("");
    let binary_proof =
        manifest::prove_manifest(&manifest::artifact_manifest_path(&binary_root), &identity);
    let model_proof = manifest::prove_manifest(
        &manifest::artifact_manifest_path(&model_root),
        &model_identity,
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
    json!({"provider":"local","ready":state=="ready","status":state,"reason_code":reason,"target":{"model_id":model_id,"target_fingerprint_json":install["target_fingerprint_json"],"target_fingerprint_sha256":install["target_fingerprint_sha256"]},"install":install,"host":{"platform_supported":platform_supported,"backend":backend,"backend_reason":choice.reason,"vulkan_observation":input.get("vulkan_observation").cloned().unwrap_or(Value::Null)},"artifacts":{"model_id":model_id,"binary_installed":binary_proof["status"]=="ready","model_installed":model_proof["status"]=="ready","binary_path":binary_root.join("llama-server"),"model_path":model_root.join(model_file),"projector_path":model_root.join(projector_file)},"proof":{"binary":binary_proof,"model":model_proof}})
}

pub fn inspect_parakeet(input: Map<String, Value>) -> Value {
    let requested_artifact_key = match input.get("artifact_key") {
        Some(Value::String(key)) => Value::String(key.to_owned()),
        Some(_) => return parakeet_unavailable("artifact_key_invalid", Value::Null),
        None => Value::Null,
    };
    let journal = input
        .get("journal")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let Some(journal) = journal else {
        return parakeet_unavailable("journal_required", requested_artifact_key);
    };
    let artifact_key = match requested_artifact_key {
        Value::String(key) => key,
        Value::Null => match pins::parakeet_host_artifact_key() {
            Ok(key) => key,
            Err(_) => return parakeet_unavailable("unsupported_platform", Value::Null),
        },
        _ => unreachable!(),
    };
    let Some(cpu_identity) = pins::parakeet_backend_identity(&artifact_key, "cpu") else {
        return parakeet_unavailable("unsupported_platform", Value::String(artifact_key));
    };
    let Some(vulkan_identity) = pins::parakeet_backend_identity(&artifact_key, "vulkan") else {
        return parakeet_unavailable("unsupported_platform", Value::String(artifact_key));
    };
    let Some((cpu_release, _, _, binary_name)) = pins::parakeet_backend_pin(&artifact_key, "cpu")
    else {
        return parakeet_unavailable("unsupported_platform", Value::String(artifact_key));
    };
    let Some((vulkan_release, _, _, _)) = pins::parakeet_backend_pin(&artifact_key, "vulkan")
    else {
        return parakeet_unavailable("unsupported_platform", Value::String(artifact_key));
    };
    let cache_root = pins::parakeet_cache_root(&journal);
    let cpu_root = cache_root
        .join("bin")
        .join(&artifact_key)
        .join("cpu")
        .join(cpu_release);
    let vulkan_root = cache_root
        .join("bin")
        .join(&artifact_key)
        .join("vulkan")
        .join(vulkan_release);
    let (repo, filename, revision, ..) = pins::PARAKEET_MODEL;
    let model_root = cache_root
        .join("models")
        .join(repo.replace('/', "__"))
        .join(revision);
    let install = match status::read_status(&journal, "parakeet") {
        Ok(value) => serde_json::to_value(value).expect("install status serializes"),
        Err(_) => {
            return parakeet_unavailable("status_unavailable", Value::String(artifact_key));
        }
    };
    let in_flight = match lease::is_held(&journal, "parakeet") {
        Ok(held) => held,
        Err(_) => return parakeet_unavailable("lease_unavailable", Value::String(artifact_key)),
    };
    let cpu_proof =
        manifest::prove_manifest(&manifest::artifact_manifest_path(&cpu_root), &cpu_identity);
    let vulkan_proof = manifest::prove_manifest(
        &manifest::artifact_manifest_path(&vulkan_root),
        &vulkan_identity,
    );
    let model_proof = manifest::prove_manifest(
        &manifest::artifact_manifest_path(&model_root),
        &pins::parakeet_model_identity(),
    );
    let (mut state, mut reason) = combined_proof(&[&cpu_proof, &vulkan_proof, &model_proof]);
    let (binary_state, binary_reason) = combined_proof(&[&cpu_proof, &vulkan_proof]);
    let cpu_path = cpu_root.join(binary_name);
    let vulkan_path = vulkan_root.join(binary_name);
    let model_path = model_root.join(filename);
    let mut runnable = false;
    let mut host = Map::new();
    if state == "ready" {
        let mut probe_input = Map::new();
        probe_input.insert(
            "path".to_owned(),
            Value::String(cpu_path.display().to_string()),
        );
        let probe = probe_binary(&probe_input);
        runnable = probe["runnable"].as_bool().unwrap_or(false);
        let probe_reason = probe["reason_code"].as_str().unwrap_or("ready");
        let detail = probe.get("message").cloned().unwrap_or_else(|| {
            match probe.get("exit_code").and_then(Value::as_i64) {
                Some(code) => Value::String(format!("exited with status {code}")),
                None if probe.get("exit_code").is_some() => {
                    Value::String("terminated by signal".to_owned())
                }
                None => Value::Null,
            }
        });
        host.insert(
            "binary_runtime".to_owned(),
            json!({"backend":"cpu","runnable":runnable,"reason_code":probe_reason,"detail":detail}),
        );
        if !runnable {
            state = "host-ineligible".to_owned();
            reason = probe_reason.to_owned();
        }
    }
    json!({
        "provider":"parakeet",
        "ready":state == "ready",
        "status":state,
        "reason_code":reason,
        "in_flight":in_flight,
        "target":{"artifact_key":artifact_key},
        "install":install,
        "host":host,
        "artifacts":{
            "binary_installed":cpu_proof["status"] == "ready" && vulkan_proof["status"] == "ready",
            "binary_cpu_installed":cpu_proof["status"] == "ready",
            "binary_vulkan_installed":vulkan_proof["status"] == "ready",
            "binary_runnable":runnable,
            "model_installed":model_proof["status"] == "ready",
            "binary_path_cpu":cpu_path,
            "binary_path_vulkan":vulkan_path,
            "model_path":model_path,
        },
        "proof":{
            "binary":proof_payload(binary_state, binary_reason),
            "binary_cpu":proof_payload_value(&cpu_proof),
            "binary_vulkan":proof_payload_value(&vulkan_proof),
            "model":proof_payload_value(&model_proof),
        },
    })
}

pub fn probe_binary(input: &Map<String, Value>) -> Value {
    let Some(path) = input.get("path").and_then(Value::as_str) else {
        return json!({"runnable":false,"reason_code":"path_required"});
    };
    probe_binary_with_arg(path, "--version")
}

pub fn probe_binary_with_arg(path: &str, arg: &str) -> Value {
    match std::process::Command::new(path).arg(arg).output() {
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

fn parakeet_unavailable(reason: &str, artifact_key: Value) -> Value {
    json!({
        "provider":"parakeet",
        "ready":false,
        "status":"proof-unavailable",
        "reason_code":reason,
        "in_flight":false,
        "target":{"artifact_key":artifact_key},
        "host":{},
        "artifacts":{
            "binary_installed":false,
            "binary_cpu_installed":false,
            "binary_vulkan_installed":false,
            "binary_runnable":false,
            "model_installed":false,
        },
        "proof":{},
    })
}

fn combined_proof(proofs: &[&Value]) -> (String, String) {
    for proof in proofs {
        if proof["status"] == "proof-unavailable" {
            return proof_pair(proof);
        }
    }
    for proof in proofs {
        if proof["status"] == "missing-or-mismatched" {
            return proof_pair(proof);
        }
    }
    ("ready".to_owned(), "ready".to_owned())
}

fn proof_pair(proof: &Value) -> (String, String) {
    (
        proof["status"]
            .as_str()
            .unwrap_or("proof-unavailable")
            .to_owned(),
        proof["reason_code"]
            .as_str()
            .unwrap_or("proof_unavailable")
            .to_owned(),
    )
}

fn proof_payload(status: String, reason_code: String) -> Value {
    json!({"status":status,"reason_code":reason_code})
}

fn proof_payload_value(proof: &Value) -> Value {
    let (status, reason_code) = proof_pair(proof);
    proof_payload(status, reason_code)
}
