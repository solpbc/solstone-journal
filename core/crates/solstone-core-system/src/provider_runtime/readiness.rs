// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Parakeet readiness observations for doctor.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::Value;
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
    NotApplicable { detail: String },
    ArtifactsMissing { detail: String },
    BinaryUnstartable { detail: String },
    OpenMpRuntimeUnavailable { detail: String },
    ServerUnreachable { detail: String },
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
    let cache = home_dir.join("Library/Application Support/solstone/parakeet/models");
    let sentinel = cache.join(".install-complete");
    let payload = fs::read(&sentinel)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| {
            format!(
                "parakeet check failed: sentinel not ready at {}",
                sentinel.display()
            )
        })?;
    let platform = payload.get("platform").and_then(Value::as_object);
    let ready = payload.get("schema_version").and_then(Value::as_i64) == Some(1)
        && payload.get("backend").and_then(Value::as_str) == Some("parakeet")
        && payload.get("variant").and_then(Value::as_str) == Some("coreml")
        && payload.get("model_version").and_then(Value::as_str) == Some("v3")
        && payload.get("quantization").and_then(Value::as_str) == Some("fp32")
        && payload.get("fluidaudio_version").is_some()
        && platform.is_some_and(|platform| {
            platform.get("os").and_then(Value::as_str) == Some(os_name)
                && platform.get("arch").and_then(Value::as_str) == Some(arch)
        });
    let configured = payload
        .get("cache_dir")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| path.exists());
    let Some(configured) = configured.filter(|_| ready) else {
        return Err(format!(
            "parakeet check failed: sentinel not ready at {}",
            sentinel.display()
        ));
    };
    let model_root = configured
        .parent()
        .unwrap_or(&configured)
        .join("parakeet-tdt-0.6b-v3");
    let complete = [
        "Encoder.mlmodelc/weights/weight.bin",
        "Decoder.mlmodelc/weights/weight.bin",
        "JointDecision.mlmodelc/weights/weight.bin",
        "Preprocessor.mlmodelc/weights/weight.bin",
    ]
    .iter()
    .all(|relative| model_root.join(relative).is_file());
    complete.then_some(configured.clone()).ok_or_else(|| {
        format!(
            "parakeet check failed: cache verification failed at {}",
            configured.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
