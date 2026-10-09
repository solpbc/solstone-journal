// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

use super::{lease, local_backend_choice, local_backend_choice_present, manifest, pins, status};

pub fn inspect_local(input: Map<String, Value>) -> Value {
    inspect_local_with(
        input,
        manifest::prove_manifest_required,
        local_backend_choice,
        super::windows_engine::verified_windows_llama_package,
    )
}

pub fn inspect_local_present_with_package(
    input: Map<String, Value>,
    package: Option<super::windows_engine::WindowsLlamaPackage>,
) -> Value {
    inspect_local_with(
        input,
        manifest::inspect_manifest_required,
        local_backend_choice_present,
        || match package {
            Some(pkg) => Ok(pkg),
            None => super::windows_engine::verified_windows_llama_package(),
        },
    )
}

/// Cheap installation candidate for read paths. Launch and installation use
/// `inspect_local`, which verifies the artifact bytes.
pub fn inspect_local_present(input: Map<String, Value>) -> Value {
    inspect_local_present_with_package(input, None)
}

fn inspect_local_with(
    input: Map<String, Value>,
    check_manifest: fn(&Path, &Value, &[&str]) -> Value,
    choose_backend: fn(&Path, Option<crate::NvidiaProbe>) -> crate::nvidia::BackendSelection,
    resolve_package: impl FnOnce() -> Result<
        super::windows_engine::WindowsLlamaPackage,
        super::windows_engine::WindowsLlamaPackageError,
    >,
) -> Value {
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
    let selection = if key == "x86_64-windows" {
        crate::nvidia::BackendSelection::Selected(crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: "Windows packaged Vulkan runtime".into(),
        })
    } else {
        choose_backend(&journal, nvidia_probe)
    };
    let choice = match selection {
        crate::nvidia::BackendSelection::Selected(choice) => choice,
        crate::nvidia::BackendSelection::IntegrityBlocked => {
            let root = pins::cache_root(&journal);
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
            let model_proof = check_manifest(
                &manifest::artifact_manifest_path(&model_root),
                &model_identity,
                &[model_file, projector_file],
            );
            let (binary_root, identity) = (
                pins::cuda_pin(key).map(|(_, digest, _)| root.join("cuda").join(key).join(digest)),
                pins::cuda_identity(key),
            );
            let platform_supported = identity.is_some();
            let binary_root = binary_root.unwrap_or_else(|| root.join("missing"));
            let binary_path = binary_root.join("llama-server");
            let binary_proof = json!({
                "status": "missing-or-mismatched",
                "reason_code": "cuda_runtime_integrity",
                "cache_hit": false
            });
            let install = status::read_observed_status(&journal, "local")
                .map(|value| serde_json::to_value(value).unwrap())
                .unwrap_or(Value::Null);
            return json!({
                "provider": "local",
                "ready": false,
                "status": "missing-or-mismatched",
                "reason_code": "cuda_runtime_integrity",
                "target": {
                    "model_id": model_id,
                    "target_fingerprint_json": install["target_fingerprint_json"],
                    "target_fingerprint_sha256": install["target_fingerprint_sha256"]
                },
                "install": install,
                "host": {
                    "platform_supported": platform_supported,
                    "backend": "cuda",
                    "backend_reason": "cuda runtime integrity failure",
                    "vulkan_observation": input.get("vulkan_observation").cloned().unwrap_or(Value::Null)
                },
                "artifacts": {
                    "model_id": model_id,
                    "binary_installed": false,
                    "model_installed": model_proof["status"] == "ready",
                    "binary_path": binary_path,
                    "model_path": model_root.join(model_file),
                    "projector_path": model_root.join(projector_file)
                },
                "proof": {
                    "binary": binary_proof,
                    "model": model_proof
                }
            });
        }
    };
    let backend = match choice.backend {
        crate::Backend::Cuda => "cuda",
        crate::Backend::Vulkan => "vulkan",
    };
    let root = pins::cache_root(&journal);
    let mut current_target = Value::Null;
    let (platform_supported, _binary_root, binary_proof, binary_path) = if key == "x86_64-windows" {
        let pkg_result = resolve_package();
        let (proof, path) = match pkg_result {
            Ok(pkg) => {
                current_target = super::local_target_for_windows_package(&pkg, model_id)
                    .and_then(super::resolved_fingerprint)
                    .unwrap_or(Value::Null);
                (
                    json!({"status": "ready", "reason_code": "ready"}),
                    pkg.engine,
                )
            }
            Err(super::windows_engine::WindowsLlamaPackageError::Missing(msg)) => (
                json!({
                    "status": "missing-or-mismatched",
                    "reason_code": "package_unavailable",
                    "message": msg,
                }),
                PathBuf::from("bin/llama-server.exe"),
            ),
            Err(super::windows_engine::WindowsLlamaPackageError::Invalid(msg)) => (
                json!({
                    "status": "missing-or-mismatched",
                    "reason_code": "package_invalid",
                    "message": msg,
                }),
                PathBuf::from("bin/llama-server.exe"),
            ),
        };
        (true, root.join("bin"), proof, path)
    } else {
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
        let proof = check_manifest(
            &manifest::artifact_manifest_path(&binary_root),
            &identity,
            &["llama-server"],
        );
        let path = binary_root.join("llama-server");
        (platform_supported, binary_root, proof, path)
    };
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
    let model_proof = check_manifest(
        &manifest::artifact_manifest_path(&model_root),
        &model_identity,
        &[model_file, projector_file],
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
    let install = status::read_observed_status(&journal, "local")
        .map(|value| serde_json::to_value(value).unwrap())
        .unwrap_or(Value::Null);
    let target = if key == "x86_64-windows" {
        &current_target
    } else {
        &install
    };
    json!({"provider":"local","ready":state=="ready","status":state,"reason_code":reason,"target":{"model_id":model_id,"target_fingerprint_json":target["target_fingerprint_json"],"target_fingerprint_sha256":target["target_fingerprint_sha256"]},"install":install,"host":{"platform_supported":platform_supported,"backend":backend,"backend_reason":choice.reason,"vulkan_observation":input.get("vulkan_observation").cloned().unwrap_or(Value::Null)},"artifacts":{"model_id":model_id,"binary_installed":binary_proof["status"]=="ready","model_installed":model_proof["status"]=="ready","binary_path":binary_path,"model_path":model_root.join(model_file),"projector_path":model_root.join(projector_file)},"proof":{"binary":binary_proof,"model":model_proof}})
}

/// Read the installed local artifacts selected by the current install target
/// without probing host hardware.
pub fn inspect_local_installed(journal: &Path, model_id: &str) -> Value {
    let root = pins::cache_root(journal);
    let key = pins::platform_key();
    let (platform_supported, binary_installed) = if key == "x86_64-windows" {
        (
            true,
            super::windows_engine::verified_windows_llama_package().is_ok(),
        )
    } else {
        let installed = status::read_status(journal, "local")
            .ok()
            .and_then(|install| install.target_fingerprint_json)
            .and_then(|fingerprint| serde_json::from_str::<Value>(&fingerprint).ok())
            .and_then(
                |target| match target.get("backend").and_then(Value::as_str) {
                    Some("cuda") => {
                        let (_, digest, _) = pins::cuda_pin(&key)?;
                        Some((
                            root.join("cuda").join(&key).join(digest),
                            pins::cuda_identity(&key)?,
                        ))
                    }
                    Some("vulkan") => {
                        let (release, _, _, _) = pins::vulkan_pin(&key)?;
                        Some((
                            root.join("bin").join(&key).join(release),
                            pins::vulkan_identity(&key)?,
                        ))
                    }
                    Some("metal") => {
                        let (release, _, _, _) = pins::vulkan_pin(&key)?;
                        Some((
                            root.join("bin").join(&key).join(release),
                            pins::vulkan_identity(&key)?,
                        ))
                    }
                    _ => None,
                },
            )
            .is_some_and(|(binary_root, identity)| {
                manifest::inspect_manifest_required(
                    &manifest::artifact_manifest_path(&binary_root),
                    &identity,
                    &["llama-server"],
                )["status"]
                    == "ready"
            });
        (pins::vulkan_pin(&key).is_some(), installed)
    };
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
    let model_installed = manifest::inspect_manifest_required(
        &manifest::artifact_manifest_path(&model_root),
        &model_identity,
        &[model_file, projector_file],
    )["status"]
        == "ready";

    json!({
        "host":{"platform_supported":platform_supported},
        "artifacts":{"binary_installed":binary_installed,"model_installed":model_installed},
    })
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
    let install = match status::read_observed_status(&journal, "parakeet") {
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

#[cfg(test)]
mod windows_identity_tests {
    use super::super::windows_engine::WindowsLlamaPackage;
    use super::*;

    #[test]
    fn current_package_identity_survives_missing_status_and_model_proof_detects_same_size_corruption()
     {
        let root = tempfile::tempdir().unwrap();
        let model_id = "local/qwen3.5-4b";
        let model_root = pins::cache_root(root.path()).join("models/local__qwen3.5-4b");
        std::fs::create_dir_all(&model_root).unwrap();
        let identity = pins::model_identity(model_id).unwrap();
        let model = identity["filename"].as_str().unwrap();
        let projector = identity["mmproj_filename"].as_str().unwrap();
        std::fs::write(model_root.join(model), b"model").unwrap();
        std::fs::write(model_root.join(projector), b"projector").unwrap();
        let inventory = manifest::inventory_for_tree(&model_root, "model").unwrap();
        let proof = manifest::build_manifest(
            "local",
            "local-model",
            "fixture",
            json!({"pin_identity":identity}),
            inventory,
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(&manifest::artifact_manifest_path(&model_root), &proof).unwrap();
        let input = Map::from_iter([
            ("journal".into(), json!(root.path())),
            ("model_id".into(), json!(model_id)),
            ("artifact_key".into(), json!("x86_64-windows")),
        ]);
        let inspect = |pkg| {
            inspect_local_with(
                input.clone(),
                manifest::prove_manifest_required,
                local_backend_choice,
                || Ok(pkg),
            )
        };
        let package = WindowsLlamaPackage::mock();
        let initial = inspect(package.clone());
        assert_eq!(initial["ready"], true);
        assert!(
            !initial["target"]["target_fingerprint_sha256"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        let mut replacement = package.clone();
        replacement.engine_sha256 = "d".repeat(64);
        assert_ne!(initial["target"], inspect(replacement)["target"]);
        std::fs::write(model_root.join(model), b"other").unwrap();
        let corrupt = inspect(package);
        assert_eq!(corrupt["ready"], false);
        assert_eq!(corrupt["reason_code"], "sha256_mismatch");
    }
}
