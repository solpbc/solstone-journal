// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::response::Response;
use serde_json::json;
use solstone_core_assets::{Platform, resolve_host_platform};
use solstone_core_sense::memory::{MemoryProbe, SystemMemoryProbe};

use crate::{config, http::json_response};

pub async fn get(journal_root: PathBuf) -> Response {
    let config_value = solstone_core_journal_config::read_journal_config(&journal_root)
        .expect("session gate handled corrupt config")
        .config
        .unwrap_or_default();
    let transcribe = config::project_transcribe(
        config_value.get("transcribe").cloned().unwrap_or(json!({})),
        true,
    );
    let available = SystemMemoryProbe.available_bytes();
    let available_gb =
        available.map(|bytes| (bytes as f64 / 1024_f64.powi(3) * 10.0).round() / 10.0);
    json_response(json!({
        "backends": backend_metadata(),
        "api_keys": {"parakeet": true, "parakeet-cpp": true},
        "config": transcribe,
        "runtime_label": runtime_label(std::env::consts::OS, std::env::consts::ARCH),
        "parakeet_uses_cpp": parakeet_uses_cpp(std::env::consts::OS, std::env::consts::ARCH),
        "resource": {"min_ram_gb": 6, "available_memory_gb": available_gb, "requirement": "local transcription needs about 6 GB of free memory for the on-device model (transcription, speaker labels, and overlap detection).", "detected": available_gb.map(|value| format!("{value} GB of free memory detected on this machine.")).unwrap_or_else(|| "free memory on this machine could not be detected.".to_owned()), "needs_setup": available.is_some_and(|value| value < 6 * 1024_u64.pow(3)), "notice": ""},
    }))
}

fn backend_metadata() -> serde_json::Value {
    json!([
        {"name": "parakeet", "label": "Parakeet - local processing (Apple Silicon CoreML or Linux parakeet.cpp)", "description": "On-device speech recognition via Parakeet TDT; macOS uses a FluidAudio/CoreML helper, Linux uses the supervised parakeet.cpp server. Requires `make install`.", "env_key": null, "settings": ["model_version", "device", "timeout_sec"]},
        {"name": "parakeet-cpp", "label": "Parakeet.cpp - local processing (Linux)", "description": "On-device speech recognition via a supervised parakeet.cpp server (mudler/parakeet.cpp). Linux only; install with `journal install-provider parakeet`.", "env_key": null, "settings": ["device"]},
    ])
}

pub fn runtime_label(os: &str, arch: &str) -> &'static str {
    match resolve_host_platform(os, arch) {
        Ok(Platform::MacosArm64) => "macOS CoreML helper",
        Ok(Platform::LinuxX64) => "Linux parakeet.cpp",
        Ok(Platform::LinuxArm64) | Err(_) => "unsupported",
    }
}

pub fn parakeet_uses_cpp(os: &str, arch: &str) -> bool {
    matches!(resolve_host_platform(os, arch), Ok(Platform::LinuxX64))
}

#[cfg(test)]
mod tests {
    use super::{backend_metadata, parakeet_uses_cpp, runtime_label};

    /// G3-20: option/backend labels are sentence case, not Title Case — the
    /// only capitals allowed are proper nouns/acronyms already present
    /// elsewhere in this dashboard (Parakeet, Apple Silicon, CoreML, Linux).
    #[test]
    fn g3_20_backend_labels_are_sentence_case_not_title_case() {
        let backends = backend_metadata();
        let labels: Vec<&str> = backends
            .as_array()
            .expect("backends is an array")
            .iter()
            .map(|entry| entry["label"].as_str().expect("label is a string"))
            .collect();
        assert!(
            labels
                .iter()
                .any(|label| label.contains("local processing")),
            "expected a lowercase 'local processing', got {labels:?}"
        );
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("Local processing")),
            "Title-case 'Local processing' should not survive G3-20: {labels:?}"
        );
    }

    #[test]
    fn ac12_runtime_label_has_all_three_branches() {
        assert_eq!(runtime_label("darwin", "arm64"), "macOS CoreML helper");
        assert_eq!(runtime_label("linux", "x86_64"), "Linux parakeet.cpp");
        assert_eq!(runtime_label("windows", "x86_64"), "unsupported");
    }

    #[test]
    fn runtime_label_for_macos_aarch64_is_coreml_helper() {
        assert_eq!(runtime_label("macos", "aarch64"), "macOS CoreML helper");
    }

    #[test]
    fn runtime_label_for_linux_aarch64_is_unsupported() {
        assert_eq!(runtime_label("linux", "aarch64"), "unsupported");
    }

    #[test]
    fn parakeet_uses_cpp_is_true_only_for_linux_x86_64() {
        assert!(parakeet_uses_cpp("linux", "x86_64"));
        assert!(!parakeet_uses_cpp("linux", "aarch64"));
        assert!(!parakeet_uses_cpp("macos", "aarch64"));
        assert!(!parakeet_uses_cpp("darwin", "arm64"));
        assert!(!parakeet_uses_cpp("windows", "x86_64"));
    }
}
