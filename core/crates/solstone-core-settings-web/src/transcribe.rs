// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use axum::response::Response;
use serde_json::json;
use solstone_core_sense::memory::{MemoryProbe, SystemMemoryProbe};
use solstone_core_system::stt_backend_choice::{SttRuntimeKind, stt_host_policy};

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
    let mut response = json!({
        "backends": backend_metadata(),
        "api_keys": {"parakeet": true, "parakeet-cpp": true},
        "config": transcribe,
        "runtime_label": runtime_label(std::env::consts::OS, std::env::consts::ARCH),
        "parakeet_uses_cpp": parakeet_uses_cpp(std::env::consts::OS, std::env::consts::ARCH),
        "resource": transcribe_resource(std::env::consts::OS, std::env::consts::ARCH, available),
    });
    // A mac cannot host the parakeet.cpp server, so the page stops offering that
    // backend there. On Windows the server and model come with the journal and
    // run on the CPU, so there is nothing to install and no device to pick.
    if !parakeet_uses_cpp(std::env::consts::OS, std::env::consts::ARCH) {
        response["parakeet_cpp_unavailable"] = json!(true);
    }
    if parakeet_uses_cpp(std::env::consts::OS, std::env::consts::ARCH)
        && parakeet_cpp_bundled_on(std::env::consts::OS)
    {
        response["parakeet_cpp_bundled"] = json!(true);
    }
    json_response(response)
}

/// Whether the parakeet.cpp server and its model ship inside the journal
/// package on this OS, rather than being installed into the provider cache.
pub fn parakeet_cpp_bundled_on(os: &str) -> bool {
    os == "windows"
}

fn transcribe_resource(os: &str, arch: &str, available: Option<u64>) -> serde_json::Value {
    let available_gb =
        available.map(|bytes| (bytes as f64 / 1024_f64.powi(3) * 10.0).round() / 10.0);
    let floor = stt_host_policy(os, arch).local_floor_bytes;
    if floor.is_none_or(|bytes| bytes == 0) {
        return json!({
            "min_ram_gb": floor,
            "available_memory_gb": available_gb,
            "requirement": "",
            "detected": "",
            "needs_setup": false,
            "notice": "",
        });
    }
    let floor_bytes = floor.expect("nonzero supported floor");
    let floor_gb = floor_bytes / 1024_u64.pow(3);
    json!({
        "min_ram_gb": floor_gb,
        "available_memory_gb": available_gb,
        "requirement": format!("local transcription needs about {floor_gb} GB of free memory."),
        "detected": available_gb.map(|value| format!("{value} GB of free memory detected on this machine.")).unwrap_or_else(|| "free memory on this machine could not be detected.".to_owned()),
        "needs_setup": available.is_some_and(|value| value < floor_bytes),
        "notice": "",
    })
}

fn backend_metadata() -> serde_json::Value {
    json!([
        {"name": "parakeet", "label": "Parakeet - local processing", "description": "On-device speech recognition via Parakeet TDT: a Core ML helper on a mac, the parakeet.cpp server on linux and windows.", "env_key": null, "settings": ["model_version", "device", "timeout_sec"]},
        {"name": "parakeet-cpp", "label": "parakeet.cpp - local processing (linux and windows)", "description": "On-device speech recognition via the parakeet.cpp server (mudler/parakeet.cpp), on linux and windows. `solstone journal install-provider parakeet` downloads it and its speech model on linux; on windows they come with the journal.", "env_key": null, "settings": ["device"]},
    ])
}

pub fn runtime_label(os: &str, arch: &str) -> &'static str {
    match stt_host_policy(os, arch).runtime_kind {
        Some(SttRuntimeKind::MacosCoreml) => "macOS CoreML helper",
        Some(SttRuntimeKind::LinuxCpp) => "Linux parakeet.cpp",
        Some(SttRuntimeKind::WindowsCpp) => "Windows parakeet.cpp",
        None => "unsupported",
    }
}

/// Whether the `parakeet` backend runs the parakeet.cpp server on this host:
/// Linux on both architectures, and Windows from its bundled package.
pub fn parakeet_uses_cpp(os: &str, arch: &str) -> bool {
    stt_host_policy(os, arch).uses_parakeet_cpp()
}

#[cfg(test)]
mod tests {
    use super::{parakeet_cpp_bundled_on, parakeet_uses_cpp, runtime_label, transcribe_resource};

    #[test]
    fn parakeet_cpp_comes_with_the_journal_only_on_windows() {
        assert!(parakeet_cpp_bundled_on("windows"));
        assert!(!parakeet_cpp_bundled_on("linux"));
        assert!(!parakeet_cpp_bundled_on("macos"));
    }

    #[test]
    fn ac12_runtime_label_has_all_three_branches() {
        assert_eq!(runtime_label("darwin", "arm64"), "macOS CoreML helper");
        assert_eq!(runtime_label("linux", "x86_64"), "Linux parakeet.cpp");
        assert_eq!(runtime_label("windows", "x86_64"), "Windows parakeet.cpp");
        assert_eq!(runtime_label("windows", "aarch64"), "Windows parakeet.cpp");
        assert_eq!(runtime_label("windows", "armv7l"), "unsupported");
    }

    #[test]
    fn runtime_label_for_macos_aarch64_is_coreml_helper() {
        assert_eq!(runtime_label("macos", "aarch64"), "macOS CoreML helper");
    }

    #[test]
    fn runtime_label_for_linux_aarch64_is_parakeet_cpp() {
        assert_eq!(runtime_label("linux", "aarch64"), "Linux parakeet.cpp");
    }

    #[test]
    fn parakeet_uses_cpp_on_linux_and_windows() {
        assert!(parakeet_uses_cpp("linux", "x86_64"));
        assert!(parakeet_uses_cpp("linux", "aarch64"));
        assert!(parakeet_uses_cpp("windows", "x86_64"));
        assert!(parakeet_uses_cpp("windows", "aarch64"));
        assert!(!parakeet_uses_cpp("windows", "armv7l"));
        assert!(!parakeet_uses_cpp("macos", "aarch64"));
        assert!(!parakeet_uses_cpp("darwin", "arm64"));
    }

    #[test]
    fn linux_and_windows_ask_for_setup_only_below_the_admission_floor() {
        const GIB: u64 = 1024 * 1024 * 1024;
        for os in ["linux", "windows"] {
            assert_eq!(
                transcribe_resource(os, "x86_64", Some(3 * GIB))["needs_setup"],
                true
            );
            assert_eq!(
                transcribe_resource(os, "x86_64", Some(5 * GIB))["needs_setup"],
                false
            );
        }
    }

    #[test]
    fn apple_silicon_does_not_require_free_ram_setup() {
        let resource = transcribe_resource("darwin", "arm64", Some(1));
        assert_eq!(resource["min_ram_gb"], 0);
        assert_eq!(resource["needs_setup"], false);
        assert_eq!(resource["requirement"], "");
        let resource = transcribe_resource("macos", "aarch64", Some(1));
        assert_eq!(resource["needs_setup"], false);
    }

    #[test]
    fn host_projection_keeps_support_distinct_from_memory_warnings() {
        const GIB: u64 = 1024 * 1024 * 1024;
        for (os, arch, min_ram, cpp, label) in [
            ("darwin", "arm64", Some(0), false, "macOS CoreML helper"),
            ("macos", "aarch64", Some(0), false, "macOS CoreML helper"),
            ("linux", "amd64", Some(4), true, "Linux parakeet.cpp"),
            ("linux", "arm64", Some(4), true, "Linux parakeet.cpp"),
            ("windows", "x86_64", Some(4), true, "Windows parakeet.cpp"),
            ("windows", "aarch64", Some(4), true, "Windows parakeet.cpp"),
            ("windows", "armv7l", None, false, "unsupported"),
            ("darwin", "x86_64", None, false, "unsupported"),
            ("linux", "riscv64", None, false, "unsupported"),
            ("unknown", "x86_64", None, false, "unsupported"),
        ] {
            assert_eq!(runtime_label(os, arch), label, "{os}/{arch}");
            assert_eq!(parakeet_uses_cpp(os, arch), cpp, "{os}/{arch}");
            for available in [None, Some(0), Some(4 * GIB - 1), Some(4 * GIB)] {
                let resource = transcribe_resource(os, arch, available);
                assert_eq!(resource["min_ram_gb"], serde_json::json!(min_ram));
                assert_eq!(
                    resource["needs_setup"],
                    min_ram == Some(4) && available.is_some_and(|bytes| bytes < 4 * GIB),
                    "{os}/{arch}, {available:?}"
                );
            }
        }
    }
}
