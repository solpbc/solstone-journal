// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;

use solstone_core_convey_http::window_catalog::{resolve_table, strings, text};

use axum::response::Response;
use serde_json::json;
use solstone_core_sense::memory::{MemoryProbe, SystemMemoryProbe};
use solstone_core_system::stt_backend_choice::{SttRuntimeKind, stt_host_policy};

use crate::{config, http::json_response};

pub async fn get(locale: &str, journal_root: PathBuf) -> Response {
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
        "backends": backend_metadata(locale, std::env::consts::OS, std::env::consts::ARCH),
        "api_keys": {"parakeet": true, "parakeet-cpp": true},
        "config": transcribe,
        "runtime_label": runtime_label(locale, std::env::consts::OS, std::env::consts::ARCH),
        "parakeet_uses_cpp": parakeet_uses_cpp(std::env::consts::OS, std::env::consts::ARCH),
        "resource": transcribe_resource(locale, std::env::consts::OS, std::env::consts::ARCH, available),
    });
    if parakeet_embedded(std::env::consts::OS, std::env::consts::ARCH) {
        response["parakeet_embedded"] = json!(true);
    }
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

fn transcribe_resource(
    locale: &str,
    os: &str,
    arch: &str,
    available: Option<u64>,
) -> serde_json::Value {
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
        "requirement": strings(locale, "journal.window.settings.transcribe.memory.requirement", [("floor", &floor_gb.to_string())]),
        "detected": available_gb.map(|value| strings(locale, "journal.window.settings.transcribe.memory.detected", [("available", &value.to_string())])).unwrap_or_else(|| text(locale, "journal.window.settings.transcribe.memory.unknown")),
        "needs_setup": available.is_some_and(|value| value < floor_bytes),
        "notice": "",
    })
}

fn parakeet_embedded(os: &str, arch: &str) -> bool {
    stt_host_policy(os, arch).runtime_kind == Some(SttRuntimeKind::MobileEmbeddedCpp)
}

fn backend_metadata(locale: &str, os: &str, arch: &str) -> serde_json::Value {
    let mut backends = if parakeet_embedded(os, arch) {
        // Keep the legacy explicit C++ choice visible only when already saved;
        // both names reach the same embedded recognizer on mobile.
        json!([
            {"name": "parakeet", "label": "journal.window.settings.transcribe.backend.label.parakeet.local.processing", "description": "journal.window.settings.transcribe.backend.description.parakeet.tdt.runs.on.this.device.with", "env_key": null, "settings": []},
            {"name": "parakeet-cpp", "label": "journal.window.settings.transcribe.backend.label.parakeet.cpp.local.processing", "description": "journal.window.settings.transcribe.backend.description.parakeet.tdt.runs.on.this.device.with", "env_key": null, "settings": []},
        ])
    } else {
        json!([
        {"name": "parakeet", "label": "journal.window.settings.transcribe.backend.label.parakeet.local.processing", "description": "journal.window.settings.transcribe.backend.description.on.device.speech.recognition.via.parakeet.tdt", "env_key": null, "settings": ["model_version", "device", "timeout_sec"]},
        {"name": "parakeet-cpp", "label": "journal.window.settings.transcribe.backend.label.parakeet.cpp.local.processing.linux.and.windows", "description": "journal.window.settings.transcribe.backend.description.on.device.speech.recognition.via.the.parakeet", "env_key": null, "settings": ["device"]},
        ])
    };
    resolve_table(locale, &mut backends);
    backends
}

fn runtime_label(locale: &str, os: &str, arch: &str) -> String {
    let id = match stt_host_policy(os, arch).runtime_kind {
        Some(SttRuntimeKind::MacosCoreml) => {
            "journal.window.settings.transcribe.runtime.macos.coreml.helper"
        }
        Some(SttRuntimeKind::LinuxCpp) => {
            "journal.window.settings.transcribe.runtime.linux.parakeet.cpp"
        }
        Some(SttRuntimeKind::WindowsCpp) => {
            "journal.window.settings.transcribe.runtime.windows.parakeet.cpp"
        }
        Some(SttRuntimeKind::MobileEmbeddedCpp) => {
            "journal.window.settings.transcribe.runtime.on.device.parakeet.tdt"
        }
        None => "journal.window.settings.transcribe.runtime.unsupported",
    };
    text(locale, id)
}

/// Whether the `parakeet` backend runs the parakeet.cpp server on this host:
/// Linux on both architectures, and Windows from its bundled package.
pub fn parakeet_uses_cpp(os: &str, arch: &str) -> bool {
    stt_host_policy(os, arch).uses_parakeet_cpp()
}

#[cfg(test)]
mod tests {
    use super::{
        backend_metadata, parakeet_cpp_bundled_on, parakeet_embedded, parakeet_uses_cpp,
        runtime_label, transcribe_resource,
    };
    use solstone_core_convey_http::window_catalog::ENGLISH;

    #[test]
    fn mobile_projects_the_bundled_recognizer_without_desktop_controls() {
        for os in ["android", "ios"] {
            assert!(parakeet_embedded(os, "aarch64"));
            assert_eq!(
                runtime_label(ENGLISH, os, "aarch64"),
                "on-device Parakeet TDT"
            );
            assert!(!parakeet_uses_cpp(os, "aarch64"));
            assert_eq!(
                transcribe_resource(ENGLISH, os, "aarch64", None)["needs_setup"],
                false
            );
            for entry in backend_metadata(ENGLISH, os, "aarch64").as_array().unwrap() {
                assert_eq!(entry["settings"], serde_json::json!([]));
                assert!(matches!(
                    entry["name"].as_str(),
                    Some("parakeet" | "parakeet-cpp")
                ));
            }
        }
        assert!(!parakeet_embedded("android", "x86_64"));
    }

    #[test]
    fn parakeet_cpp_comes_with_the_journal_only_on_windows() {
        assert!(parakeet_cpp_bundled_on("windows"));
        assert!(!parakeet_cpp_bundled_on("linux"));
        assert!(!parakeet_cpp_bundled_on("macos"));
    }

    #[test]
    fn ac12_runtime_label_has_all_three_branches() {
        assert_eq!(
            runtime_label(ENGLISH, "darwin", "arm64"),
            "macOS CoreML helper"
        );
        assert_eq!(
            runtime_label(ENGLISH, "linux", "x86_64"),
            "Linux parakeet.cpp"
        );
        assert_eq!(
            runtime_label(ENGLISH, "windows", "x86_64"),
            "Windows parakeet.cpp"
        );
        assert_eq!(
            runtime_label(ENGLISH, "windows", "aarch64"),
            "Windows parakeet.cpp"
        );
        assert_eq!(runtime_label(ENGLISH, "windows", "armv7l"), "unsupported");
    }

    #[test]
    fn runtime_label_for_macos_aarch64_is_coreml_helper() {
        assert_eq!(
            runtime_label(ENGLISH, "macos", "aarch64"),
            "macOS CoreML helper"
        );
    }

    #[test]
    fn runtime_label_for_linux_aarch64_is_parakeet_cpp() {
        assert_eq!(
            runtime_label(ENGLISH, "linux", "aarch64"),
            "Linux parakeet.cpp"
        );
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
                transcribe_resource(ENGLISH, os, "x86_64", Some(3 * GIB))["needs_setup"],
                true
            );
            assert_eq!(
                transcribe_resource(ENGLISH, os, "x86_64", Some(5 * GIB))["needs_setup"],
                false
            );
        }
    }

    #[test]
    fn apple_silicon_does_not_require_free_ram_setup() {
        let resource = transcribe_resource(ENGLISH, "darwin", "arm64", Some(1));
        assert_eq!(resource["min_ram_gb"], 0);
        assert_eq!(resource["needs_setup"], false);
        assert_eq!(resource["requirement"], "");
        let resource = transcribe_resource(ENGLISH, "macos", "aarch64", Some(1));
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
            assert_eq!(runtime_label(ENGLISH, os, arch), label, "{os}/{arch}");
            assert_eq!(parakeet_uses_cpp(os, arch), cpp, "{os}/{arch}");
            for available in [None, Some(0), Some(4 * GIB - 1), Some(4 * GIB)] {
                let resource = transcribe_resource(ENGLISH, os, arch, available);
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
