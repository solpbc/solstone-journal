// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! macOS MLX local provider candidate inspection and proof.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::pins::{MLX_QWEN_MODEL_ID, MLX_QWEN_PINS, MLX_QWEN_REVISION, MlxFilePin};

pub const HELPER_ENV_KEY: &str = "SOLSTONE_MLX_HELPER";
pub const MODEL_DIR_ENV_KEY: &str = "SOLSTONE_MLX_MODEL_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosLocalBackend {
    Metal,
    Mlx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosLocalBackendError(pub String);

impl std::fmt::Display for MacosLocalBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MacosLocalBackendError {}

pub fn select_macos_local_backend(
    value: Option<&str>,
) -> Result<MacosLocalBackend, MacosLocalBackendError> {
    match value {
        None | Some("metal") => Ok(MacosLocalBackend::Metal),
        Some("mlx") => Ok(MacosLocalBackend::Mlx),
        Some(other) => Err(MacosLocalBackendError(format!(
            "invalid macOS local backend: {other:?}"
        ))),
    }
}

pub fn read_macos_local_backend_env() -> Result<MacosLocalBackend, MacosLocalBackendError> {
    match std::env::var("SOLSTONE_MACOS_LOCAL_BACKEND") {
        Ok(val) => select_macos_local_backend(Some(&val)),
        Err(std::env::VarError::NotPresent) => select_macos_local_backend(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(MacosLocalBackendError(
            "SOLSTONE_MACOS_LOCAL_BACKEND is not valid unicode".into(),
        )),
    }
}

pub fn resolve_override_path(path: PathBuf) -> PathBuf {
    let expanded = match path.to_str().and_then(|value| value.strip_prefix("~/")) {
        Some(remainder) => std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(remainder))
            .unwrap_or(path),
        None => path,
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(&expanded))
            .unwrap_or(expanded)
    }
}

fn is_valid_helper_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = path.metadata() {
            if meta.permissions().mode() & 0o111 == 0 {
                return false;
            }
        } else {
            return false;
        }
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let metallib = parent
        .join("mlx-swift_Cmlx.bundle")
        .join("Contents")
        .join("Resources")
        .join("default.metallib");
    metallib.is_file()
}

pub fn resolve_mlx_helper(override_opt: Option<&str>) -> Result<PathBuf, String> {
    if let Some(explicit) = override_opt {
        let path = resolve_override_path(PathBuf::from(explicit));
        if is_valid_helper_executable(&path) {
            return path.canonicalize().map_err(|error| error.to_string());
        }
        return Err(format!(
            "explicit MLX helper is not valid or missing metallib: {}",
            path.display()
        ));
    }
    if let Some(env_val) = std::env::var_os(HELPER_ENV_KEY) {
        let path = resolve_override_path(PathBuf::from(env_val));
        if is_valid_helper_executable(&path) {
            return path.canonicalize().map_err(|error| error.to_string());
        }
        return Err(format!(
            "SOLSTONE_MLX_HELPER is not valid or missing metallib: {}",
            path.display()
        ));
    }
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(parent) = current_exe.parent()
    {
        let sibling_path = parent.join("mlx-helper");
        if is_valid_helper_executable(&sibling_path) {
            return sibling_path
                .canonicalize()
                .map_err(|error| error.to_string());
        }
    }
    let dev_path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/mlx-helper/DerivedData/Build/Products/Release/mlx-helper"
    ));
    if is_valid_helper_executable(&dev_path) {
        return dev_path.canonicalize().map_err(|error| error.to_string());
    }
    Err("mlx-helper executable with metallib resource bundle not found".to_owned())
}

pub fn resolve_mlx_model_dir(override_opt: Option<&str>) -> Result<PathBuf, String> {
    if let Some(explicit) = override_opt {
        let path = resolve_override_path(PathBuf::from(explicit));
        if path.is_dir() {
            return path.canonicalize().map_err(|error| error.to_string());
        }
        return Err(format!(
            "explicit MLX model directory does not exist: {}",
            path.display()
        ));
    }
    if let Some(env_val) = std::env::var_os(MODEL_DIR_ENV_KEY) {
        let path = resolve_override_path(PathBuf::from(env_val));
        if path.is_dir() {
            return path.canonicalize().map_err(|error| error.to_string());
        }
        return Err(format!(
            "SOLSTONE_MLX_MODEL_DIR does not exist: {}",
            path.display()
        ));
    }
    Err("SOLSTONE_MLX_MODEL_DIR is unset or not a directory".to_owned())
}

/// Operator paths take precedence; ordinary installs are journal-scoped generations.
pub fn model_directory(journal: &Path) -> PathBuf {
    std::env::var_os(MODEL_DIR_ENV_KEY)
        .map(|value| resolve_override_path(PathBuf::from(value)))
        .unwrap_or_else(|| super::pins::mlx_model_directory(journal))
}

pub fn artifact_target_json(pins: &[MlxFilePin]) -> (Value, String) {
    let mut sorted_pins = pins.to_vec();
    sorted_pins.sort_by_key(|p| p.name);
    let target = json!({
        "provider": "local",
        "backend": "mlx",
        "model_id": "local/qwen3.5-4b",
        "repo": MLX_QWEN_MODEL_ID,
        "revision": MLX_QWEN_REVISION,
        "pins": sorted_pins.iter().map(|p| json!({
            "name": p.name,
            "size": p.size,
            "sha256": p.sha256,
        })).collect::<Vec<_>>()
    });
    let canonical =
        super::fingerprint::canonical(target.clone()).expect("MLX identity is valid JSON");
    let sha256 = super::fingerprint::sha256(&canonical);
    (target, sha256)
}

fn check_extra_loadable_names(model_dir: &Path, pins: &[MlxFilePin]) -> Result<(), String> {
    let read_dir =
        fs::read_dir(model_dir).map_err(|e| format!("cannot read model directory: {e}"))?;
    for entry in read_dir {
        let entry = entry.map_err(|e| format!("failed reading entry: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("failed file type: {e}"))?;
        let name = entry.file_name();
        // The download client's metadata directory is never a model input.
        if (file_type.is_dir() && name == ".cache")
            || (file_type.is_file() && name == ".generation.json")
        {
            continue;
        }
        if !pins.iter().any(|pin| name == pin.name) {
            return Err(format!(
                "extra unverified model input: {}",
                name.to_string_lossy()
            ));
        }
    }
    Ok(())
}

fn inspect_present_with_overrides(
    helper_override: Option<&str>,
    model_dir_override: Option<&str>,
    pins: &[MlxFilePin],
) -> Value {
    if pins.is_empty() {
        return failure_readiness("empty_pins", "no pins supplied");
    }
    let helper_path = match resolve_mlx_helper(helper_override) {
        Ok(path) => path,
        Err(err) => return failure_readiness("package_unavailable", &err),
    };
    let model_dir = match resolve_mlx_model_dir(model_dir_override) {
        Ok(dir) => dir,
        Err(err) => return failure_readiness("artifact_missing", &err),
    };
    if let Err(err) = check_extra_loadable_names(&model_dir, pins) {
        return failure_readiness("artifact_stale", &err);
    }
    for pin in pins {
        let file_path = model_dir.join(pin.name);
        let meta = match fs::metadata(&file_path) {
            Ok(m) => m,
            Err(_) => {
                return failure_readiness(
                    "artifact_missing",
                    &format!("missing required file: {}", pin.name),
                );
            }
        };
        if !meta.is_file() {
            return failure_readiness(
                "artifact_missing",
                &format!("not a regular file: {}", pin.name),
            );
        }
        if meta.len() != pin.size {
            return failure_readiness(
                "inventory_size_mismatch",
                &format!(
                    "size mismatch for {}: expected {} got {}",
                    pin.name,
                    pin.size,
                    meta.len()
                ),
            );
        }
    }
    let (target_json, target_sha256) = artifact_target_json(pins);
    json!({
        "provider": "local",
        "ready": true,
        "status": "ready",
        "reason_code": "ready",
        "target": {
            "model_id": "local/qwen3.5-4b",
            "target_fingerprint_json": target_json,
            "target_fingerprint_sha256": target_sha256,
        },
        "host": {
            "platform_supported": true,
            "backend": "mlx",
            "backend_reason": "Darwin MLX runtime",
        },
        "artifacts": {
            "model_id": "local/qwen3.5-4b",
            "binary_installed": true,
            "model_installed": true,
            "binary_path": helper_path.display().to_string(),
            "model_path": model_dir.display().to_string(),
            "projector_path": Value::Null,
        },
    })
}

fn prove_artifacts_with_pins(
    model_dir: &Path,
    helper_path: &Path,
    pins: &[MlxFilePin],
) -> Result<Value, String> {
    if pins.is_empty() {
        return Err("empty pins list".to_owned());
    }
    if !is_valid_helper_executable(helper_path) {
        return Err(format!(
            "helper binary is invalid: {}",
            helper_path.display()
        ));
    }
    if !model_dir.is_dir() {
        return Err(format!(
            "model dir is not a directory: {}",
            model_dir.display()
        ));
    }
    check_extra_loadable_names(model_dir, pins)?;
    for pin in pins {
        let file_path = model_dir.join(pin.name);
        let meta = fs::metadata(&file_path)
            .map_err(|e| format!("cannot read metadata for {}: {e}", pin.name))?;
        if !meta.is_file() {
            return Err(format!("not a regular file: {}", pin.name));
        }
        if meta.len() != pin.size {
            return Err(format!(
                "size mismatch for {}: expected {} got {}",
                pin.name,
                pin.size,
                meta.len()
            ));
        }
        let digest = super::manifest::sha256_file(&file_path)
            .map_err(|e| format!("sha256_file failed for {}: {e}", pin.name))?;
        if digest != pin.sha256 {
            return Err(format!(
                "sha256 mismatch for {}: expected {} got {}",
                pin.name, pin.sha256, digest
            ));
        }
    }
    let (target_json, target_sha256) = artifact_target_json(pins);
    Ok(json!({
        "provider": "local",
        "ready": true,
        "status": "ready",
        "reason_code": "ready",
        "target": {
            "model_id": "local/qwen3.5-4b",
            "target_fingerprint_json": target_json,
            "target_fingerprint_sha256": target_sha256,
        },
        "host": {
            "platform_supported": true,
            "backend": "mlx",
            "backend_reason": "Darwin MLX runtime",
        },
        "artifacts": {
            "model_id": "local/qwen3.5-4b",
            "binary_installed": true,
            "model_installed": true,
            "binary_path": helper_path.display().to_string(),
            "model_path": model_dir.display().to_string(),
            "projector_path": Value::Null,
        },
    }))
}

pub fn inspect_verified_default(journal: &Path) -> Value {
    let mut readiness = inspect_present_default(journal);
    if readiness["ready"] != true {
        return readiness;
    }
    let helper = Path::new(
        readiness["artifacts"]["binary_path"]
            .as_str()
            .expect("ready helper path"),
    );
    let model = Path::new(
        readiness["artifacts"]["model_path"]
            .as_str()
            .expect("ready model path"),
    );
    match prove_artifacts_with_pins(model, helper, &MLX_QWEN_PINS) {
        Ok(proof) => proof,
        Err(error) => {
            readiness["ready"] = json!(false);
            readiness["status"] = json!("missing-or-mismatched");
            readiness["reason_code"] = json!("sha256_mismatch");
            readiness["detail"] = json!(error);
            readiness
        }
    }
}

/// Fixture-only inspection with small model inputs; absent from production builds.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn inspect_fixture(helper: &Path, model: &Path, pins: &[MlxFilePin], verify: bool) -> Value {
    if verify {
        prove_artifacts_with_pins(model, helper, pins)
            .unwrap_or_else(|error| failure_readiness("sha256_mismatch", &error))
    } else {
        inspect_present_with_overrides(helper.to_str(), model.to_str(), pins)
    }
}

pub fn inspect_present_default(journal: &Path) -> Value {
    let directory = model_directory(journal);
    let mut readiness = inspect_present_with_overrides(None, directory.to_str(), &MLX_QWEN_PINS);
    let (target, hash) = artifact_target_json(&MLX_QWEN_PINS);
    let canonical = super::fingerprint::canonical(target).expect("valid MLX target");
    readiness["target"]["target_fingerprint_json"] = json!(canonical);
    readiness["target"]["target_fingerprint_sha256"] = json!(hash);
    readiness["install"] = super::status::read_observed_status(journal, "local")
        .ok()
        .filter(|state| state.target_fingerprint_sha256.as_deref() == Some(&hash))
        .and_then(|state| serde_json::to_value(state).ok())
        .unwrap_or(Value::Null);
    // A managed generation is unavailable until every member has been verified
    // and its exact control record published. Presence checks never hash weights.
    if std::env::var_os(MODEL_DIR_ENV_KEY).is_none()
        && readiness["reason_code"] != "package_unavailable"
    {
        let generation = super::pins::mlx_model_generation();
        let sealed = fs::read(directory.join(".generation.json"))
            .ok()
            .and_then(|bytes| {
                serde_json::from_slice::<solstone_core_artifact_download::generation::Generation>(
                    &bytes,
                )
                .ok()
            })
            .as_ref()
            == Some(&generation);
        if !sealed {
            readiness["ready"] = json!(false);
            readiness["status"] = json!("missing-or-mismatched");
            readiness["reason_code"] = json!("manifest_missing");
            readiness["proof"] = json!({"model": {"status":"missing-or-mismatched", "reason_code":"manifest_missing"}});
        }
    }
    readiness
}

fn failure_readiness(reason_code: &str, detail: &str) -> Value {
    json!({
        "provider": "local",
        "ready": false,
        "status": "proof-unavailable",
        "reason_code": reason_code,
        "detail": detail,
        "target": {
            "model_id": "local/qwen3.5-4b",
            "target_fingerprint_json": Value::Null,
            "target_fingerprint_sha256": "",
        },
        "host": {
            "platform_supported": true,
            "backend": "mlx",
            "backend_reason": "Darwin MLX runtime",
        },
        "artifacts": {
            "model_id": "local/qwen3.5-4b",
            "binary_installed": false,
            "model_installed": false,
            "binary_path": Value::Null,
            "model_path": Value::Null,
            "projector_path": Value::Null,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{fingerprint, manifest, pins};

    #[test]
    fn test_select_macos_local_backend_matrix() {
        assert_eq!(
            select_macos_local_backend(None).unwrap(),
            MacosLocalBackend::Metal
        );
        assert_eq!(
            select_macos_local_backend(Some("metal")).unwrap(),
            MacosLocalBackend::Metal
        );
        assert_eq!(
            select_macos_local_backend(Some("mlx")).unwrap(),
            MacosLocalBackend::Mlx
        );
        assert!(select_macos_local_backend(Some("")).is_err());
        assert!(select_macos_local_backend(Some("MLX")).is_err());
        assert!(select_macos_local_backend(Some("vulkan")).is_err());
        assert!(select_macos_local_backend(Some("cuda")).is_err());
    }

    #[test]
    fn test_mlx_artifact_target_sha256_and_properties() {
        let (target, sha256) = artifact_target_json(&MLX_QWEN_PINS);
        assert_eq!(target["provider"], "local");
        assert_eq!(target["backend"], "mlx");
        assert_eq!(target["model_id"], "local/qwen3.5-4b");
        assert_eq!(target["repo"], MLX_QWEN_MODEL_ID);
        assert_eq!(target["revision"], MLX_QWEN_REVISION);
        assert!(!sha256.is_empty());

        // Target JSON has no llama.cpp pin or GGUF filenames
        let target_str = target.to_string();
        assert!(!target_str.contains("llama"));
        assert!(!target_str.contains(".gguf"));
        assert!(!target_str.contains("mmproj"));

        // Compare against GGUF model identity hash
        let gguf_identity = pins::model_identity("local/qwen3.5-4b").unwrap();
        let gguf_canonical = fingerprint::canonical(gguf_identity).unwrap();
        let gguf_sha256 = fingerprint::sha256(&gguf_canonical);
        assert_ne!(sha256, gguf_sha256);
    }

    #[cfg(all(unix, feature = "full-tests"))]
    #[test]
    fn test_mlx_candidate_full_lifecycle_and_rejections() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::Builder::new()
            .prefix("solstone-mlx-test-")
            .tempdir_in("/var/tmp")
            .expect("tempdir in /var/tmp");

        let helper_dir = temp_dir.path().join("helper");
        fs::create_dir_all(&helper_dir).unwrap();
        let helper_bin = helper_dir.join("mlx-helper");
        fs::write(&helper_bin, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(&helper_bin).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&helper_bin, perms).unwrap();

        let bundle_res = helper_dir
            .join("mlx-swift_Cmlx.bundle")
            .join("Contents")
            .join("Resources");
        fs::create_dir_all(&bundle_res).unwrap();
        let metallib = bundle_res.join("default.metallib");
        fs::write(&metallib, b"metallib-dummy").unwrap();

        let model_dir = temp_dir.path().join("model");
        fs::create_dir_all(&model_dir).unwrap();

        // 1. Empty pins slice fails closed
        let empty_res = inspect_present_with_overrides(
            Some(helper_bin.to_str().unwrap()),
            Some(model_dir.to_str().unwrap()),
            &[],
        );
        assert_eq!(empty_res["ready"], false);
        assert_eq!(empty_res["reason_code"], "empty_pins");

        // Fixture pin
        let dummy_bytes = b"sample-config-contents";
        let dummy_sha256 = fingerprint::sha256(std::str::from_utf8(dummy_bytes).unwrap());
        let test_pins = [MlxFilePin {
            name: "config.json",
            size: dummy_bytes.len() as u64,
            sha256: Box::leak(dummy_sha256.into_boxed_str()),
        }];

        // 2. Missing required file
        let missing_res = inspect_present_with_overrides(
            Some(helper_bin.to_str().unwrap()),
            Some(model_dir.to_str().unwrap()),
            &test_pins,
        );
        assert_eq!(missing_res["ready"], false);
        assert_eq!(missing_res["reason_code"], "artifact_missing");

        // Write corrupt file (right size, wrong bytes)
        let config_file = model_dir.join("config.json");
        let wrong_bytes = vec![b'x'; dummy_bytes.len()];
        fs::write(&config_file, &wrong_bytes).unwrap();

        // 3. Presence passes because size matches, but prove fails with sha256 mismatch
        let corrupt_presence = inspect_present_with_overrides(
            Some(helper_bin.to_str().unwrap()),
            Some(model_dir.to_str().unwrap()),
            &test_pins,
        );
        assert_eq!(corrupt_presence["ready"], true);

        let corrupt_proof = prove_artifacts_with_pins(&model_dir, &helper_bin, &test_pins);
        assert!(corrupt_proof.is_err());
        assert!(corrupt_proof.unwrap_err().contains("sha256 mismatch"));

        // Write correct file bytes
        fs::write(&config_file, dummy_bytes).unwrap();

        // Prove succeeds and matches sha256_file
        let valid_proof = prove_artifacts_with_pins(&model_dir, &helper_bin, &test_pins).unwrap();
        assert_eq!(valid_proof["ready"], true);
        assert_eq!(
            manifest::sha256_file(&config_file).unwrap(),
            test_pins[0].sha256
        );

        // 4. Deliberately wrong pin sha256: presence still passes (does not hash), prove fails
        let wrong_hash_pins = [MlxFilePin {
            name: "config.json",
            size: dummy_bytes.len() as u64,
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        }];
        let wrong_hash_presence = inspect_present_with_overrides(
            Some(helper_bin.to_str().unwrap()),
            Some(model_dir.to_str().unwrap()),
            &wrong_hash_pins,
        );
        assert_eq!(wrong_hash_presence["ready"], true);

        let wrong_hash_proof = prove_artifacts_with_pins(&model_dir, &helper_bin, &wrong_hash_pins);
        assert!(wrong_hash_proof.is_err());

        // 5. Extra generation_config.json fails presence
        let extra_gen = model_dir.join("generation_config.json");
        fs::write(&extra_gen, b"{}").unwrap();
        let extra_gen_res = inspect_present_with_overrides(
            Some(helper_bin.to_str().unwrap()),
            Some(model_dir.to_str().unwrap()),
            &test_pins,
        );
        assert_eq!(extra_gen_res["ready"], false);
        assert_eq!(extra_gen_res["reason_code"], "artifact_stale");
        fs::remove_file(&extra_gen).unwrap();

        // 6. Extra .safetensors file fails presence
        let extra_safe = model_dir.join("other.safetensors");
        fs::write(&extra_safe, b"weights").unwrap();
        let extra_safe_res = inspect_present_with_overrides(
            Some(helper_bin.to_str().unwrap()),
            Some(model_dir.to_str().unwrap()),
            &test_pins,
        );
        assert_eq!(extra_safe_res["ready"], false);
        assert_eq!(extra_safe_res["reason_code"], "artifact_stale");
        fs::remove_file(&extra_safe).unwrap();

        // 7. Missing metallib fails helper validation
        fs::remove_file(&metallib).unwrap();
        let no_metallib = inspect_present_with_overrides(
            Some(helper_bin.to_str().unwrap()),
            Some(model_dir.to_str().unwrap()),
            &test_pins,
        );
        assert_eq!(no_metallib["ready"], false);
        assert_eq!(no_metallib["reason_code"], "package_unavailable");

        // 8. Invalid helper override with nonexistent path
        let bad_helper = inspect_present_with_overrides(
            Some("/nonexistent/helper"),
            Some(model_dir.to_str().unwrap()),
            &test_pins,
        );
        assert_eq!(bad_helper["ready"], false);
        assert_eq!(bad_helper["reason_code"], "package_unavailable");
    }
}
