// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Frozen local-provider oracles: Vulkan child protocol and installer pin table.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use solstone_core_assets::{Artifact, Backend, Platform, catalog, resolve};
use solstone_core_local::install::ced_install::{ENGINE_VERSION, ced_artifact_key, ced_model_path};
use solstone_core_local::install::rfdetr_install::{
    ENGINE_VERSION as RFDETR_ENGINE_VERSION, RFDETR_ENGINE_LINUX_CPU_ARM64_BINARY_SHA256,
    RFDETR_ENGINE_LINUX_CPU_ARM64_TARBALL_SHA256, RFDETR_ENGINE_LINUX_CPU_X64_BINARY_SHA256,
    RFDETR_ENGINE_LINUX_CPU_X64_TARBALL_SHA256, RFDETR_ENGINE_MACOS_METAL_ARM64_BINARY_SHA256,
    RFDETR_ENGINE_MACOS_METAL_ARM64_TARBALL_SHA256, RFDETR_MODEL_SHA256, binary_path, model_path,
    rfdetr_artifact_key, rfdetr_platform_supported,
};
use solstone_core_local::{VulkanDevice, VulkanProbeConfig, VulkanProbeProgram, enumerate_gpus};

// Pin table read from solstone_core_assets::{catalog, resolve}, the CED
// installer, and RF-DETR's bundled-payload constants and public path helpers.
const CED_MODEL_SHA256: &str = "48bee4e2fc3cc85d7806e03471db24e77fda6c2a2e81ffe9ef67caebaf2bd674";
const CED_ENGINE_X64_SHA256: &str =
    "915e0573bc4e17197a7a893d0eb98e1a851abb64451b2e1a8ad51f5f99040360";
const CED_ENGINE_ARM64_SHA256: &str =
    "a87de0a8b086429aa5d6544a6f881a70e62726d07901734640ac85dbf146181e";
const CED_ENGINE_METAL_SHA256: &str =
    "4c913ba0ece1d06ba2210da9fcaee3d8199ca3c62697c331810f224444e4054b";

const KNOWN_DEVICE_TYPES: [u32; 4] = [1, 2, 3, 4];

fn catalog_row(sha256: &str) -> &'static Artifact {
    catalog()
        .iter()
        .find(|row| row.sha256 == sha256)
        .unwrap_or_else(|| panic!("catalog lost frozen pin {sha256}"))
}

#[test]
fn stub_probe_parses_a_three_device_payload() {
    let payload = concat!(
        "[",
        r#"{"index":0,"name":"GPU","device_type":2,"vram_mib":6144},"#,
        r#"{"index":1,"name":"iGPU","device_type":1,"vram_mib":23814},"#,
        r#"{"index":2,"name":"llvmpipe","device_type":4,"vram_mib":0}"#,
        "]"
    );
    let config = VulkanProbeConfig {
        program: VulkanProbeProgram::Explicit {
            executable: PathBuf::from("sh"),
            args: vec![
                OsString::from("-c"),
                OsString::from(format!("printf '{payload}'")),
            ],
            env: Vec::new(),
        },
        timeout: Duration::from_secs(1),
    };
    let (devices, probe_ok) = enumerate_gpus(&config);
    assert!(probe_ok);
    assert_eq!(
        devices,
        vec![
            VulkanDevice {
                index: 0,
                name: "GPU".into(),
                device_type: Some(2),
                vram_mib: 6144,
            },
            VulkanDevice {
                index: 1,
                name: "iGPU".into(),
                device_type: Some(1),
                vram_mib: 23_814,
            },
            VulkanDevice {
                index: 2,
                name: "llvmpipe".into(),
                device_type: Some(4),
                vram_mib: 0,
            },
        ]
    );
}

#[test]
fn sibling_helper_is_empty_failure_or_well_formed_success() {
    let (devices, probe_ok) = enumerate_gpus(&VulkanProbeConfig {
        program: VulkanProbeProgram::SiblingHelper,
        timeout: Duration::from_secs(5),
    });
    if !probe_ok {
        assert!(
            devices.is_empty(),
            "a failed probe must not return devices: {devices:?}"
        );
        return;
    }
    for (index, device) in devices.iter().enumerate() {
        assert_eq!(device.index, u32::try_from(index).expect("index fits u32"));
        assert!(!device.name.is_empty(), "device {index} has an empty name");
        let device_type = device
            .device_type
            .expect("successful probe devices carry device_type");
        assert!(
            KNOWN_DEVICE_TYPES.contains(&device_type),
            "device {index} has unknown device_type {device_type}"
        );
    }
}

#[test]
fn installer_specs_match_the_pinned_sources() {
    let journal = Path::new("/synthetic/journal");

    assert_eq!(ENGINE_VERSION, "v0.1.0");
    assert_eq!(ced_artifact_key("linux", "x86_64"), Some("linux-cpu-x64"));
    assert_eq!(
        ced_artifact_key("linux", "aarch64"),
        Some("linux-cpu-arm64")
    );
    assert_eq!(
        ced_artifact_key("darwin", "arm64"),
        Some("macos-metal-arm64")
    );
    let ced_model = resolve("ced-model", None, None);
    assert_eq!(ced_model.len(), 1);
    assert_eq!(ced_model[0].filename, "ced-tiny-q8_0.gguf");
    assert_eq!(
        ced_model[0].version,
        "b5e9a4aad6438763c8da16079d77563fbed35c65"
    );
    assert_eq!(
        ced_model[0].upstream_url,
        "https://huggingface.co/mudler/ced-gguf/resolve/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf"
    );
    assert_eq!(ced_model[0].sha256, CED_MODEL_SHA256);
    assert_eq!(ced_model[0].size_bytes, 6_211_616);
    assert_eq!(catalog_row(CED_MODEL_SHA256).filename, "ced-tiny-q8_0.gguf");
    for (key, platform, backend, sha, size) in [
        (
            "linux-cpu-x64",
            Platform::LinuxX64,
            Backend::Cpu,
            CED_ENGINE_X64_SHA256,
            788_651_u64,
        ),
        (
            "linux-cpu-arm64",
            Platform::LinuxArm64,
            Backend::Cpu,
            CED_ENGINE_ARM64_SHA256,
            720_034,
        ),
        (
            "macos-metal-arm64",
            Platform::MacosArm64,
            Backend::Metal,
            CED_ENGINE_METAL_SHA256,
            686_952,
        ),
    ] {
        let rows = resolve("ced-engine", Some(platform), Some(backend));
        assert_eq!(rows.len(), 1, "{key}");
        assert_eq!(rows[0].artifact_key, Some(key));
        assert_eq!(rows[0].sha256, sha, "{key}");
        assert_eq!(rows[0].size_bytes, size, "{key}");
        assert_eq!(catalog_row(sha).artifact_key, Some(key));
    }
    assert_eq!(
        ced_model_path(journal),
        PathBuf::from(
            "/synthetic/journal/cache/providers/ced/v0.1.0/models/mudler__ced-gguf/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf"
        )
    );

    assert_eq!(RFDETR_ENGINE_VERSION, "v0.1.0-solpbc.5");
    assert!(rfdetr_platform_supported("linux", "x86_64"));
    assert!(rfdetr_platform_supported("linux", "amd64"));
    assert!(rfdetr_platform_supported("linux", "aarch64"));
    assert!(rfdetr_platform_supported("darwin", "arm64"));
    for (os, arch, key) in [
        ("linux", "x86_64", "linux-cpu-x64"),
        ("linux", "aarch64", "linux-cpu-arm64"),
        ("darwin", "arm64", "macos-metal-arm64"),
    ] {
        assert_eq!(rfdetr_artifact_key(os, arch), Some(key));
    }
    assert_eq!(
        RFDETR_ENGINE_LINUX_CPU_X64_TARBALL_SHA256,
        "56231d6675395ed790dba882e0335e4c79616427af558b1820975951cd9d14a7"
    );
    assert_eq!(
        RFDETR_ENGINE_LINUX_CPU_X64_BINARY_SHA256,
        "6f225708e4b9dafc39a085f1323bc426ca037b746b3be9c7c571d9be494306af"
    );
    assert_eq!(
        RFDETR_ENGINE_LINUX_CPU_ARM64_TARBALL_SHA256,
        "2c11e1af6986571d4d9f4d2cf377018973095b10c234a9da40a3edf45cf11f9d"
    );
    assert_eq!(
        RFDETR_ENGINE_LINUX_CPU_ARM64_BINARY_SHA256,
        "14c47251ffd61a3ef0dc358c4b6a88d8718c5c3f266f4d79db9ae1440e3b6ecc"
    );
    assert_eq!(
        RFDETR_ENGINE_MACOS_METAL_ARM64_TARBALL_SHA256,
        "46b497950c7a73000007abdb9ef54bc8b46ba0a46dcf26f6c0ae51fccd21ad71"
    );
    assert_eq!(
        RFDETR_ENGINE_MACOS_METAL_ARM64_BINARY_SHA256,
        "f15d89e24d44245e2288e0d9839e54d4495d6ebf1071e1f906805f2989d18c9e"
    );
    assert_eq!(
        RFDETR_MODEL_SHA256,
        "d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee"
    );
    assert_eq!(
        binary_path(journal, "linux-cpu-x64"),
        PathBuf::from(
            "/synthetic/journal/cache/providers/rfdetr/v0.1.0-solpbc.5/engine/linux-cpu-x64/rfdetr-cli"
        )
    );
    assert_eq!(
        model_path(journal),
        PathBuf::from(
            "/synthetic/journal/cache/providers/rfdetr/model/c3dc0c037df499f5503545247df6618415fca643/rfdetr-nano-f16.gguf"
        )
    );
}
