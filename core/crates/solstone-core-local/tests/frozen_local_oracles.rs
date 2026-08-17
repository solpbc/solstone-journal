// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Frozen local-provider oracles: Vulkan child protocol and installer pin table.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use solstone_core_assets::{catalog, resolve, Backend, Platform};
use solstone_core_local::install::ced_install::{ced_artifact_key, ced_model_path, ENGINE_VERSION};
use solstone_core_local::install::rfdetr_install::{binary_path, model_path};
use solstone_core_local::{enumerate_gpus, VulkanDevice, VulkanProbeConfig, VulkanProbeProgram};

// Pin table read from solstone_core_assets::ARTIFACTS and the install path
// helpers on 2026-08-16. Hashes are the catalog rows, not a live fetch.
const RERANK_REPO: &str = "Xenova/ms-marco-MiniLM-L-6-v2";
const RERANK_REVISION: &str = "a09144355adeed5f58c8ed011d209bf8ee5a1fec";
const RERANK_MODEL_SHA256: &str =
    "c623d0bcb99f4622beb413eaef00cfbe5db20df9f1dd982da4b4f26022881870";
const RERANK_MODEL_SIZE: u64 = 90_992_115;
const RERANK_TOKENIZER_SHA256: &str =
    "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66";
const RERANK_TOKENIZER_SIZE: u64 = 711_396;

const CED_MODEL_REPO: &str = "mudler/ced-gguf";
const CED_MODEL_REVISION: &str = "b5e9a4aad6438763c8da16079d77563fbed35c65";
const CED_MODEL_FILE: &str = "ced-tiny-q8_0.gguf";
const CED_MODEL_SHA256: &str = "48bee4e2fc3cc85d7806e03471db24e77fda6c2a2e81ffe9ef67caebaf2bd674";
const CED_MODEL_SIZE: u64 = 6_211_616;
const CED_ENGINE_X64_SHA256: &str =
    "915e0573bc4e17197a7a893d0eb98e1a851abb64451b2e1a8ad51f5f99040360";
const CED_ENGINE_ARM64_SHA256: &str =
    "a87de0a8b086429aa5d6544a6f881a70e62726d07901734640ac85dbf146181e";
const CED_ENGINE_METAL_SHA256: &str =
    "4c913ba0ece1d06ba2210da9fcaee3d8199ca3c62697c331810f224444e4054b";

const RFDETR_ENGINE_SHA256: &str =
    "74f3258a94c975444923be0cc451d90c1e8d9e2595d3cab6876a11086d8357dd";
const RFDETR_ENGINE_SIZE: u64 = 995_601;
const RFDETR_ENGINE_BINARY_SHA256: &str =
    "7c4fb4d499d53509d5099e768510a164c6647b84480c72170b865233504f367c";
const RFDETR_MODEL_SHA256: &str =
    "d798cc448faa53209b88fc905c91beb1dd104634b95f6948cc4877540a8fd3ee";
const RFDETR_MODEL_SIZE: u64 = 63_439_488;
const RFDETR_RELEASE_TAG: &str = "bin-65c0ffcc-1";
const RFDETR_ENGINE_REF: &str = "65c0ffcc";

const KNOWN_DEVICE_TYPES: [u32; 4] = [1, 2, 3, 4];

fn hex64(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
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
fn downloading_installer_specs_match_the_catalog_pins() {
    let journal = Path::new("/synthetic/journal");

    let rerank = resolve("rerank-model", None, None);
    assert_eq!(
        rerank.len(),
        2,
        "rerank catalog must keep model + tokenizer"
    );
    let model = rerank
        .iter()
        .find(|row| row.filename == "onnx/model.onnx")
        .expect("rerank model row");
    let tokenizer = rerank
        .iter()
        .find(|row| row.filename == "tokenizer.json")
        .expect("rerank tokenizer row");
    assert_eq!(model.version, RERANK_REVISION);
    assert!(model.upstream_url.contains(RERANK_REPO));
    assert_eq!(model.sha256, RERANK_MODEL_SHA256);
    assert_eq!(model.size_bytes, RERANK_MODEL_SIZE);
    assert_eq!(tokenizer.sha256, RERANK_TOKENIZER_SHA256);
    assert_eq!(tokenizer.size_bytes, RERANK_TOKENIZER_SIZE);
    assert!(hex64(RERANK_MODEL_SHA256));
    assert!(hex64(RERANK_TOKENIZER_SHA256));
    assert_eq!(
        journal
            .join("cache/providers/rerank")
            .join(RERANK_REVISION)
            .join("onnx/model.onnx"),
        journal
            .join("cache/providers/rerank")
            .join(model.version)
            .join(model.filename)
    );

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
    assert_eq!(ced_model[0].filename, CED_MODEL_FILE);
    assert_eq!(ced_model[0].version, CED_MODEL_REVISION);
    assert!(ced_model[0].upstream_url.contains(CED_MODEL_REPO));
    assert_eq!(ced_model[0].sha256, CED_MODEL_SHA256);
    assert_eq!(ced_model[0].size_bytes, CED_MODEL_SIZE);
    for (key, platform, backend, sha) in [
        (
            "linux-cpu-x64",
            Platform::LinuxX64,
            Backend::Cpu,
            CED_ENGINE_X64_SHA256,
        ),
        (
            "linux-cpu-arm64",
            Platform::LinuxArm64,
            Backend::Cpu,
            CED_ENGINE_ARM64_SHA256,
        ),
        (
            "macos-metal-arm64",
            Platform::MacosArm64,
            Backend::Metal,
            CED_ENGINE_METAL_SHA256,
        ),
    ] {
        let rows = resolve("ced-engine", Some(platform), Some(backend));
        assert_eq!(rows.len(), 1, "{key}");
        assert_eq!(rows[0].artifact_key, Some(key));
        assert_eq!(rows[0].sha256, sha, "{key}");
        assert!(hex64(sha), "{key}");
    }
    assert_eq!(
        ced_model_path(journal),
        journal
            .join("cache/providers/ced")
            .join(ENGINE_VERSION)
            .join("models/mudler__ced-gguf")
            .join(CED_MODEL_REVISION)
            .join(CED_MODEL_FILE)
    );
    let expected_file_keys = [
        format!("engine/linux-cpu-arm64/ced-v0.1.0-lib-linux-cpu-arm64.tar.gz"),
        format!("engine/linux-cpu-x64/ced-v0.1.0-lib-linux-cpu-x64.tar.gz"),
        format!("engine/macos-metal-arm64/ced-v0.1.0-lib-macos-metal-arm64.tar.gz"),
        format!("models/{CED_MODEL_REPO}/{CED_MODEL_REVISION}/{CED_MODEL_FILE}"),
        format!("models/{CED_MODEL_REPO}/{CED_MODEL_REVISION}/{CED_MODEL_FILE}"),
        format!("models/{CED_MODEL_REPO}/{CED_MODEL_REVISION}/{CED_MODEL_FILE}"),
    ];
    let mut sorted = expected_file_keys;
    sorted.sort();
    assert!(sorted.iter().any(|key| key
        == "models/mudler/ced-gguf/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf"));

    let engine = resolve(
        "rfdetr-engine",
        Some(Platform::LinuxX64),
        Some(Backend::Cpu),
    );
    assert_eq!(engine.len(), 1);
    assert_eq!(engine[0].version, RFDETR_RELEASE_TAG);
    assert_eq!(engine[0].sha256, RFDETR_ENGINE_SHA256);
    assert_eq!(engine[0].size_bytes, RFDETR_ENGINE_SIZE);
    assert_eq!(
        engine[0].extracted_binary_sha256,
        Some(RFDETR_ENGINE_BINARY_SHA256)
    );
    let rfdetr_model = resolve("rfdetr-model", None, None);
    assert_eq!(rfdetr_model.len(), 1);
    assert_eq!(rfdetr_model[0].sha256, RFDETR_MODEL_SHA256);
    assert_eq!(rfdetr_model[0].size_bytes, RFDETR_MODEL_SIZE);
    assert_eq!(
        binary_path(journal),
        journal
            .join("cache/providers/rfdetr/engine")
            .join(RFDETR_ENGINE_REF)
            .join("rfdetr-cli")
    );
    assert_eq!(
        model_path(journal),
        journal
            .join("cache/providers/rfdetr/model")
            .join(rfdetr_model[0].version)
            .join(rfdetr_model[0].filename)
    );

    let _ = catalog();
}
