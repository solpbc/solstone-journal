// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::response::Response;
use serde_json::json;
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
        "backends": [
            {"name": "parakeet", "label": "Parakeet - Local processing (Apple Silicon CoreML or Linux parakeet.cpp)", "description": "On-device speech recognition via Parakeet TDT; macOS uses a FluidAudio/CoreML helper, Linux uses the supervised parakeet.cpp server. Requires `make install`.", "env_key": null, "settings": ["model_version", "device", "timeout_sec"]},
            {"name": "parakeet-cpp", "label": "Parakeet.cpp - Local processing (Linux)", "description": "On-device speech recognition via a supervised parakeet.cpp server (mudler/parakeet.cpp). Linux only; install with `journal install-provider parakeet`.", "env_key": null, "settings": ["device"]},
        ],
        "api_keys": {"parakeet": true, "parakeet-cpp": true},
        "config": transcribe,
        "runtime_label": runtime_label(std::env::consts::OS, std::env::consts::ARCH),
        "parakeet_uses_cpp": std::env::consts::OS == "linux" && std::env::consts::ARCH == "x86_64",
        "resource": {"min_ram_gb": 6, "available_memory_gb": available_gb, "requirement": "local transcription needs about 6 GB of free memory for the on-device model (transcription, speaker labels, and overlap detection).", "detected": available_gb.map(|value| format!("{value} GB of free memory detected on this machine.")).unwrap_or_else(|| "free memory on this machine could not be detected.".to_owned()), "needs_setup": available.is_some_and(|value| value < 6 * 1024_u64.pow(3)), "notice": ""},
    }))
}

pub fn runtime_label(os: &str, arch: &str) -> &'static str {
    if os == "darwin" && arch == "arm64" {
        "macOS CoreML helper"
    } else if os != "linux" || arch != "x86_64" {
        "unsupported"
    } else {
        "Linux parakeet.cpp"
    }
}

#[cfg(test)]
mod tests {
    use super::runtime_label;

    #[test]
    fn ac12_runtime_label_has_all_three_branches() {
        assert_eq!(runtime_label("darwin", "arm64"), "macOS CoreML helper");
        assert_eq!(runtime_label("linux", "x86_64"), "Linux parakeet.cpp");
        assert_eq!(runtime_label("windows", "x86_64"), "unsupported");
    }
}
