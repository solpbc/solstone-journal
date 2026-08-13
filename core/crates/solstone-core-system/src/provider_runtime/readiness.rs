// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Parakeet readiness observations for doctor.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use solstone_core_assets::catalog;
use solstone_core_journal_config::parakeet_coreml::{
    default_parakeet_coreml_cache_dir, parakeet_coreml_model_root,
    read_valid_parakeet_coreml_sentinel,
};
use solstone_core_local::install::pins;

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
    let mut child = match Command::new(binary)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return ParakeetCppReadiness::BinaryUnstartable {
                detail: error.to_string(),
            };
        }
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return ParakeetCppReadiness::Ready,
            Ok(Some(status)) => {
                let output = child.wait_with_output().ok();
                let detail = output
                    .as_ref()
                    .map(|output| String::from_utf8_lossy(&output.stderr).trim().to_owned())
                    .filter(|detail| !detail.is_empty())
                    .or_else(|| {
                        output
                            .as_ref()
                            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                            .filter(|detail| !detail.is_empty())
                    })
                    .unwrap_or_else(|| format!("exited with status {status}"));
                return classify_parakeet_cpp_probe_failure(detail);
            }
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
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
    os_name: &str,
    arch: &str,
) -> Result<PathBuf, String> {
    let cache = default_parakeet_coreml_cache_dir(home_dir);
    let sentinel = read_valid_parakeet_coreml_sentinel(home_dir, os_name, arch);
    let Some(sentinel) = sentinel else {
        return Err(format!(
            "parakeet check failed: sentinel not ready at {}",
            cache.join(".install-complete").display()
        ));
    };
    let configured = sentinel.cache_dir();
    let model_root = parakeet_coreml_model_root(configured);
    let complete = catalog()
        .iter()
        .filter(|artifact| artifact.unit == "parakeet-coreml")
        .all(|artifact| model_root.join(artifact.filename).is_file());
    complete.then_some(configured.to_path_buf()).ok_or_else(|| {
        format!(
            "parakeet check failed: cache verification failed at {}",
            configured.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_journal_config::parakeet_coreml::{
        default_parakeet_coreml_cache_dir, parakeet_coreml_model_root,
    };
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
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

        assert!(check_parakeet_coreml_cache(home.path(), "darwin", "arm64").is_err());
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
            check_parakeet_coreml_cache(home.path(), "darwin", "arm64").unwrap(),
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
                "cache_dir": missing,
            })
            .to_string(),
        )
        .unwrap();
        assert!(check_parakeet_coreml_cache(home.path(), "darwin", "arm64").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn binary_probe_classifies_openmp_loader_failure() {
        let root = tempdir().unwrap();
        let binary = root.path().join("parakeet-server");
        fs::write(
            &binary,
            "#!/bin/sh\necho 'libgomp.so.1 missing' >&2\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();
        assert!(matches!(
            probe_parakeet_cpp_binary(&binary, Duration::from_secs(1)),
            ParakeetCppReadiness::OpenMpRuntimeUnavailable { .. }
        ));
    }

    #[test]
    #[cfg(unix)]
    fn binary_probe_reaps_a_timed_out_child() {
        let root = tempdir().unwrap();
        let binary = root.path().join("parakeet-server");
        let pid = root.path().join("pid");
        fs::write(
            &binary,
            format!("#!/bin/sh\necho $$ > {}\nsleep 60\n", pid.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions).unwrap();

        assert!(matches!(
            probe_parakeet_cpp_binary(&binary, Duration::from_millis(100)),
            ParakeetCppReadiness::BinaryUnstartable { .. }
        ));
        let child_pid = fs::read_to_string(&pid).unwrap();
        let status = Command::new("sh")
            .args(["-c", &format!("kill -0 {} 2>/dev/null", child_pid.trim())])
            .status()
            .unwrap();
        assert!(!status.success(), "timed-out child must be reaped");
    }
}
