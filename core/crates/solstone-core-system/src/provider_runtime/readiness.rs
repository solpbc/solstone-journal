// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Parakeet readiness observations for doctor.

#[cfg(unix)]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use solstone_core_local::install::pins;

use crate::process::{Disposition, LaunchError, launch};

pub const PARAKEET_CPP_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetCppArtifacts {
    pub cache_root: PathBuf,
    pub artifact_key: String,
    pub binary_cpu: PathBuf,
    pub binary_vulkan: PathBuf,
    pub model: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParakeetCppReadiness {
    BinaryUnstartable { detail: String },
    OpenMpRuntimeUnavailable { detail: String },
    Ready,
}

/// Resolve the durable cache layout without reading the host platform.
pub fn parakeet_cpp_artifacts(
    journal_path: &Path,
    os_name: &str,
    arch: &str,
) -> Result<ParakeetCppArtifacts, String> {
    let artifact_key =
        pins::parakeet_artifact_key(os_name, arch).map_err(|error| error.to_string())?;
    let paths = pins::parakeet_paths(journal_path, &artifact_key);
    let path = |key: &str| {
        paths
            .get(key)
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| format!("parakeet-cpp pin lacks {key}"))
    };
    Ok(ParakeetCppArtifacts {
        cache_root: pins::parakeet_cache_root(journal_path),
        artifact_key,
        binary_cpu: path("binary_path_cpu")?,
        binary_vulkan: path("binary_path_vulkan")?,
        model: path("model_path")?,
    })
}

/// Inspect the pinned Parakeet files without installing or changing them.
pub fn check_parakeet_cpp_files(artifacts: &ParakeetCppArtifacts) -> Result<(), String> {
    for (name, path) in [
        ("binary_cpu", &artifacts.binary_cpu),
        ("binary_vulkan", &artifacts.binary_vulkan),
    ] {
        if !path.is_file() {
            return Err(format!(
                "parakeet-cpp check failed: {name} missing at {}",
                path.display()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 == 0) {
                return Err(format!(
                    "parakeet-cpp check failed: {name} not executable at {}",
                    path.display()
                ));
            }
        }
    }
    if !artifacts.model.is_file() {
        return Err(format!(
            "parakeet-cpp check failed: model missing at {}",
            artifacts.model.display()
        ));
    }
    Ok(())
}

/// Run the pinned binary only to surface dynamic-loader failures.
pub fn probe_parakeet_cpp_binary(binary: &Path, timeout: Duration) -> ParakeetCppReadiness {
    let mut authority = match launch(
        Disposition::IndependentBoundedHelper { timeout },
        || {
            let mut command = Command::new(binary);
            #[cfg(unix)]
            crate::process::remove_launch_only_environment(&mut command);
            command
                .arg("--version")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        },
        Box::new(|child, _timeout| child.kill().map_err(LaunchError::Terminate)),
    ) {
        Ok(authority) => authority,
        Err(error) => {
            return ParakeetCppReadiness::BinaryUnstartable {
                detail: error.to_string(),
            };
        }
    };
    let start = std::time::Instant::now();
    loop {
        match authority.poll() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = authority.terminate(timeout);
                return ParakeetCppReadiness::BinaryUnstartable {
                    detail: format!("timed out after {}s", timeout.as_secs()),
                };
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                return ParakeetCppReadiness::BinaryUnstartable {
                    detail: error.to_string(),
                };
            }
        }
    }
    let output = authority.wait_with_output().ok();
    match output {
        Some(output) if output.status.success() => ParakeetCppReadiness::Ready,
        Some(output) => {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let detail = if !detail.is_empty() {
                detail
            } else {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if !stdout.is_empty() {
                    stdout
                } else {
                    format!("exited with status {}", output.status)
                }
            };
            classify_parakeet_cpp_probe_failure(detail)
        }
        None => classify_parakeet_cpp_probe_failure("exited without collected output".to_owned()),
    }
}

/// Classify a loader/probe failure using the Python-compatible OpenMP marker.
pub fn classify_parakeet_cpp_probe_failure(detail: String) -> ParakeetCppReadiness {
    if detail.contains("libgomp.so.1") {
        ParakeetCppReadiness::OpenMpRuntimeUnavailable { detail }
    } else {
        ParakeetCppReadiness::BinaryUnstartable { detail }
    }
}

/// Verify the CoreML install sentinel and its required model files without loading them.
pub fn check_parakeet_coreml_cache(
    home_dir: &Path,
    config: &solstone_core_journal_config::JournalConfigRead,
    os_name: &str,
    arch: &str,
) -> Result<PathBuf, String> {
    solstone_core_local::install::coreml_install::check_parakeet_coreml_cache_doctor(
        home_dir, config, os_name, arch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use solstone_core_assets::catalog;
    use solstone_core_journal_config::parakeet_coreml::{
        ParakeetCoremlFile, ParakeetCoremlSentinel, default_parakeet_coreml_cache_dir,
        parakeet_coreml_model_root, parakeet_coreml_sentinel_path,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn resolves_and_reports_missing_pinned_artifacts() {
        let journal = tempdir().unwrap();
        let artifacts = parakeet_cpp_artifacts(journal.path(), "linux", "x86_64").unwrap();
        assert_eq!(artifacts.artifact_key, "x86_64-unknown-linux-gnu");
        assert!(check_parakeet_cpp_files(&artifacts).is_err());
    }

    #[test]
    fn coreml_readiness_rejects_weight_only_bundles() {
        let home = tempdir().unwrap();
        let cache = home.path().join("configured/cache");
        let model_root = parakeet_coreml_model_root(&cache);
        for artifact in catalog().iter().filter(|artifact| {
            artifact.unit == "parakeet-coreml" && artifact.filename.ends_with("weights/weight.bin")
        }) {
            let path = model_root.join(artifact.filename);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"weight").unwrap();
        }
        fs::create_dir_all(&cache).unwrap();
        let sentinel = default_parakeet_coreml_cache_dir(home.path()).join(".install-complete");
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(
            sentinel,
            serde_json::json!({
                "schema_version": 1,
                "backend": "parakeet",
                "variant": "coreml",
                "model_version": "v3",
                "quantization": "fp32",
                "fluidaudio_version": "0.14.0",
                "platform": {"os": "darwin", "arch": "arm64"},
                "cache_dir": cache,
            })
            .to_string(),
        )
        .unwrap();

        assert!(
            check_parakeet_coreml_cache(home.path(), &config(&cache), "darwin", "arm64").is_err()
        );
    }

    #[test]
    fn coreml_override_uses_default_sentinel_and_configured_complete_tree() {
        let home = tempdir().unwrap();
        let cache = home.path().join("configured/cache");
        let model_root = parakeet_coreml_model_root(&cache);
        for artifact in catalog()
            .iter()
            .filter(|artifact| artifact.unit == "parakeet-coreml")
        {
            let path = model_root.join(artifact.filename);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"fixture").unwrap();
        }
        fs::create_dir_all(&cache).unwrap();
        let sentinel = default_parakeet_coreml_cache_dir(home.path()).join(".install-complete");
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(
            &sentinel,
            serde_json::json!({
                "schema_version": 1,
                "backend": "parakeet",
                "variant": "coreml",
                "model_version": "v3",
                "quantization": "fp32",
                "fluidaudio_version": "0.14.0",
                "platform": {"os": "darwin", "arch": "arm64"},
                "cache_dir": cache,
            })
            .to_string(),
        )
        .unwrap();

        assert!(sentinel.is_file());
        assert_ne!(sentinel.parent().unwrap(), cache.parent().unwrap());
        let payload: Value = serde_json::from_slice(&fs::read(&sentinel).unwrap()).unwrap();
        assert_eq!(payload["cache_dir"], cache.display().to_string());
        assert_eq!(
            model_root,
            cache.parent().unwrap().join("parakeet-tdt-0.6b-v3")
        );

        assert_eq!(
            check_parakeet_coreml_cache(home.path(), &config(&cache), "darwin", "arm64").unwrap(),
            cache
        );
    }

    #[test]
    fn coreml_readiness_rejects_a_sentinel_for_a_missing_cache_directory() {
        let home = tempdir().unwrap();
        let missing = home.path().join("removed/cache");
        let sentinel = default_parakeet_coreml_cache_dir(home.path()).join(".install-complete");
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(
            sentinel,
            serde_json::json!({
                "schema_version": 1,
                "backend": "parakeet",
                "variant": "coreml",
                "model_version": "v3",
                "quantization": "fp32",
                "fluidaudio_version": "0.14.0",
                "platform": {"os": "darwin", "arch": "arm64"},
                "cache_dir": &missing,
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            check_parakeet_coreml_cache(home.path(), &config(&missing), "darwin", "arm64").is_err()
        );
    }

    fn config(cache: &Path) -> solstone_core_journal_config::JournalConfigRead {
        solstone_core_journal_config::JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(
                serde_json::json!({"transcribe": {"parakeet": {"cache_dir": cache}}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        }
    }

    fn installed_fixture(home: &Path, cache: &Path, bytes: &[u8]) -> Vec<ParakeetCoremlFile> {
        fs::create_dir_all(cache).unwrap();
        let root = parakeet_coreml_model_root(cache);
        let files: Vec<_> = catalog()
            .iter()
            .filter(|row| row.unit == "parakeet-coreml")
            .map(|row| {
                let path = root.join(row.filename);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, bytes).unwrap();
                ParakeetCoremlFile {
                    file: row.filename.to_owned(),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                    size_bytes: bytes.len() as u64,
                }
            })
            .collect();
        assert!(
            files
                .iter()
                .any(|row| row.file == "Encoder.mlmodelc/weights/weight.bin")
        );
        let sentinel = ParakeetCoremlSentinel::new(
            cache.to_path_buf(),
            "darwin",
            "arm64",
            "0.14.0",
            "incumbent-pin",
            files.clone(),
        );
        let path = parakeet_coreml_sentinel_path(home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&sentinel).unwrap()).unwrap();
        files
    }

    #[test]
    fn coreml_readiness_rejects_missing_or_mismatched_recorded_files() {
        let home = tempdir().unwrap();
        let cache = default_parakeet_coreml_cache_dir(home.path());
        installed_fixture(home.path(), &cache, b"expected-content");
        let check = || check_parakeet_coreml_cache(home.path(), &config(&cache), "darwin", "arm64");
        assert_eq!(check().unwrap(), cache);
        let required =
            parakeet_coreml_model_root(&cache).join("Encoder.mlmodelc/weights/weight.bin");
        fs::write(&required, b"corrupt--content").unwrap();
        assert!(check().is_err());
        fs::write(&required, b"expected-content").unwrap();
        assert!(check().is_ok());
        fs::remove_file(required).unwrap();
        assert!(check().is_err());
    }

    #[test]
    fn coreml_readiness_rejects_a_valid_sentinel_for_a_different_configured_tree() {
        let home = tempdir().unwrap();
        let first = home.path().join("first/cache");
        let second = home.path().join("second/cache");
        assert_ne!(
            parakeet_coreml_model_root(&first),
            parakeet_coreml_model_root(&second)
        );
        installed_fixture(home.path(), &first, b"first-valid-tree");
        let first_receipt = fs::read(parakeet_coreml_sentinel_path(home.path())).unwrap();
        assert_eq!(
            check_parakeet_coreml_cache(home.path(), &config(&first), "darwin", "arm64").unwrap(),
            first
        );
        installed_fixture(home.path(), &second, b"other-valid-tree");
        assert_eq!(
            check_parakeet_coreml_cache(home.path(), &config(&second), "darwin", "arm64").unwrap(),
            second
        );
        fs::write(parakeet_coreml_sentinel_path(home.path()), first_receipt).unwrap();
        assert!(
            check_parakeet_coreml_cache(home.path(), &config(&second), "darwin", "arm64").is_err()
        );
        assert_eq!(
            check_parakeet_coreml_cache(home.path(), &config(&first), "darwin", "arm64").unwrap(),
            first
        );
    }

    #[test]
    fn coreml_readiness_recovers_pending_marker_to_complete_backup() {
        let home = tempdir().unwrap();
        let cache = default_parakeet_coreml_cache_dir(home.path());
        let files = installed_fixture(home.path(), &cache, b"recorded-backup-bytes");
        let root = parakeet_coreml_model_root(&cache);
        let backup = root.with_extension("backup");
        fs::rename(&root, &backup).unwrap();
        for file in &files {
            let path = root.join(&file.file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"uncommitted-canonical").unwrap();
        }
        let marker = cache.join(".publication.json");
        fs::write(
            &marker,
            serde_json::to_vec(&serde_json::json!({
                "phase": "pending", "tree_canonical": root, "tree_backup": backup,
                "published_catalog_version": "replacement-pin", "published_files": files
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            check_parakeet_coreml_cache(home.path(), &config(&cache), "darwin", "arm64").unwrap(),
            cache
        );
        assert_eq!(
            fs::read(root.join("Encoder.mlmodelc/weights/weight.bin")).unwrap(),
            b"recorded-backup-bytes"
        );
        assert!(!marker.exists());
        assert!(
            check_parakeet_coreml_cache(home.path(), &config(&cache), "darwin", "arm64").is_ok()
        );
    }
}
