// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Frozen local-provider oracles: Vulkan child protocol, installer pin table,
//! and the Qwen3.5-4B admission and b10068 wire oracles.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
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
const QWEN35_ORACLE_BYTES: &[u8] = include_bytes!("../../../fixtures/qwen35_admission_oracle.json");
const QWEN35_ORACLE_LEN: usize = 5424;
const QWEN35_ORACLE_SHA256: &str =
    "44e549fe43014e2e774bf6337a1e999c898b46d80ac9008cc57a05e19efaf838";
const QWEN35_B10068_WIRE_ORACLE_BYTES: &[u8] =
    include_bytes!("../../../fixtures/qwen35_b10068_wire_oracle_v1.json");
const QWEN35_B10068_WIRE_ORACLE_LEN: usize = 9234;
const QWEN35_B10068_WIRE_ORACLE_SHA256: &str =
    "ad96c3492f9cace1fb50ae0cf174f6489caf8158b1dccc715965776020ae995d";

const KNOWN_DEVICE_TYPES: [u32; 4] = [1, 2, 3, 4];

fn catalog_row(sha256: &str) -> &'static Artifact {
    catalog()
        .iter()
        .find(|row| row.sha256 == sha256)
        .unwrap_or_else(|| panic!("catalog lost frozen pin {sha256}"))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

// This oracle proves independent tokenizer rendering/token vectors for the named tokenizer and
// GGUF-header chat template, not model weights, multimodal image cost, a running provider, or
// llama.cpp.
#[test]
fn qwen35_admission_oracle_matches_pinned_digest() {
    assert_eq!(QWEN35_ORACLE_BYTES.len(), QWEN35_ORACLE_LEN);
    assert_eq!(
        format!("{:x}", Sha256::digest(QWEN35_ORACLE_BYTES)),
        QWEN35_ORACLE_SHA256
    );
}

#[test]
fn qwen35_admission_oracle_has_expected_shape() {
    let _fixture_text = std::str::from_utf8(QWEN35_ORACLE_BYTES).expect("oracle fixture is UTF-8");
    let fixture: Value =
        serde_json::from_slice(QWEN35_ORACLE_BYTES).expect("oracle fixture parses as JSON");
    let document = fixture.as_object().expect("oracle fixture is an object");
    assert_eq!(
        document.get("schema").and_then(Value::as_str),
        Some("solstone.qwen35-admission-oracle.v1")
    );

    let receipt = document
        .get("receipt")
        .and_then(Value::as_object)
        .expect("oracle fixture has a receipt object");
    for key in [
        "producer",
        "constructed_date",
        "construction",
        "sources",
        "claims",
    ] {
        assert!(receipt.contains_key(key), "receipt has {key}");
        assert!(!receipt[key].is_null(), "receipt {key} is non-null");
    }

    let sources = receipt
        .get("sources")
        .and_then(Value::as_object)
        .expect("receipt has sources object");
    let gguf_model = sources
        .get("gguf_model")
        .and_then(Value::as_object)
        .expect("receipt has GGUF model source");
    assert!(
        gguf_model
            .get("repository")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "GGUF model repository is non-empty"
    );
    assert!(
        gguf_model
            .get("revision")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "GGUF model revision is non-empty"
    );
    assert!(
        gguf_model
            .get("sha256")
            .and_then(Value::as_str)
            .is_some_and(is_sha256_hex),
        "GGUF model SHA-256 has lowercase hexadecimal syntax"
    );

    let template = sources
        .get("gguf_embedded_chat_template")
        .and_then(Value::as_object)
        .expect("receipt has embedded chat template source");
    assert!(
        template
            .get("utf8_bytes")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0),
        "embedded chat template UTF-8 byte length is positive"
    );
    assert!(
        template
            .get("sha256")
            .and_then(Value::as_str)
            .is_some_and(is_sha256_hex),
        "embedded chat template SHA-256 has lowercase hexadecimal syntax"
    );

    let tokenizer = sources
        .get("tokenizer")
        .and_then(Value::as_object)
        .expect("receipt has tokenizer source");
    assert!(
        tokenizer
            .get("repository")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "tokenizer repository is non-empty"
    );
    assert!(
        tokenizer
            .get("revision")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "tokenizer revision is non-empty"
    );
    assert!(
        tokenizer
            .get("tokenizer_json_sha256")
            .and_then(Value::as_str)
            .is_some_and(is_sha256_hex),
        "tokenizer JSON SHA-256 has lowercase hexadecimal syntax"
    );

    let claims = receipt
        .get("claims")
        .and_then(Value::as_object)
        .expect("receipt has claims object");
    assert!(
        claims
            .get("proves")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "receipt proves claim is non-empty"
    );
    let does_not_prove = claims
        .get("does_not_prove")
        .and_then(Value::as_array)
        .expect("receipt has does_not_prove array");
    assert_eq!(does_not_prove.len(), 4);
    assert!(
        does_not_prove.iter().all(|claim| claim.is_string()),
        "each does_not_prove claim is a string"
    );

    let cases = document
        .get("cases")
        .and_then(Value::as_array)
        .expect("oracle fixture has cases array");
    assert_eq!(cases.len(), 5);
    let mut names = Vec::with_capacity(cases.len());
    for case in cases {
        let case = case.as_object().expect("oracle case is an object");
        names.push(
            case.get("name")
                .and_then(Value::as_str)
                .expect("oracle case has a name"),
        );
        let token_count = case
            .get("token_count")
            .and_then(Value::as_u64)
            .expect("oracle case token count is an unsigned integer");
        let token_ids = case
            .get("token_ids")
            .and_then(Value::as_array)
            .expect("oracle case has token IDs");
        assert_eq!(
            token_count,
            u64::try_from(token_ids.len()).expect("token IDs length fits u64")
        );
        assert!(
            case.get("rendered_sha256")
                .and_then(Value::as_str)
                .is_some_and(is_sha256_hex),
            "oracle case rendered SHA-256 has lowercase hexadecimal syntax"
        );
        assert!(
            case.get("rendered_bytes")
                .and_then(Value::as_u64)
                .is_some_and(|value| value > 0),
            "oracle case rendered byte length is positive"
        );
    }
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "empty-user",
            "json-terminal",
            "plain",
            "tool-roundtrip",
            "unicode"
        ]
    );
}

// This hybrid oracle freezes product-derived wire bodies together with three agreeing
// observations made by the pinned b10068 provider process. It does not assert that the historical
// source hashes still match the repository's current HEAD.
#[test]
fn qwen35_b10068_wire_oracle_matches_pinned_digest() {
    assert_eq!(
        QWEN35_B10068_WIRE_ORACLE_BYTES.len(),
        QWEN35_B10068_WIRE_ORACLE_LEN
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(QWEN35_B10068_WIRE_ORACLE_BYTES)),
        QWEN35_B10068_WIRE_ORACLE_SHA256
    );
}

#[test]
fn qwen35_b10068_wire_oracle_has_expected_shape_and_observations() {
    let document: Value = serde_json::from_slice(QWEN35_B10068_WIRE_ORACLE_BYTES)
        .expect("b10068 wire oracle parses as JSON");
    assert_eq!(
        document.get("schema").and_then(Value::as_str),
        Some("solstone.qwen35-b10068-wire-oracle.v1")
    );

    let receipt = document["receipt"]
        .as_object()
        .expect("b10068 wire oracle has a receipt");
    let runtime = receipt["runtime"]
        .as_object()
        .expect("receipt has runtime provenance");
    assert_eq!(runtime["tag"], "b10068");
    assert_eq!(
        runtime["commit"],
        "571d0d540df04f25298d0e159e520d9fc62ed121"
    );
    let product_wire = receipt["product_wire"]
        .as_object()
        .expect("receipt has product-wire provenance");
    assert_eq!(
        product_wire["commit"],
        "1dbb1ccf68210d0a9a128fb1a63fd2aa8c20481e"
    );
    for key in [
        "generate_builder_source_sha256",
        "converse_builder_source_sha256",
        "converse_body_builder_source_sha256",
        "fixed_local_wrapper_source_sha256",
    ] {
        assert!(
            product_wire
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(is_sha256_hex),
            "product-wire receipt has {key}"
        );
    }

    let cases = document["cases"]
        .as_array()
        .expect("b10068 wire oracle has cases");
    assert_eq!(cases.len(), 5);
    let mut names = Vec::with_capacity(cases.len());
    for case in cases {
        let name = case["name"].as_str().expect("wire case has a name");
        names.push(name);
        let body = serde_json::to_vec(&case["body"]).expect("wire body compact-serializes");
        assert_eq!(
            u64::try_from(body.len()).expect("wire body length fits u64"),
            case["wire_bytes"].as_u64().expect("wire_bytes is u64"),
            "case={name}"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(&body)),
            case["wire_sha256"].as_str().expect("wire_sha256 is text"),
            "case={name}"
        );
        let count = case["native_input_tokens"]
            .as_u64()
            .expect("native input count is u64");
        assert_eq!(case["apply_template_tokenize_tokens"], count);
        assert_eq!(case["completion_usage_prompt_tokens"], count);
        assert_eq!(
            u64::try_from(
                case["token_ids"]
                    .as_array()
                    .expect("case has token IDs")
                    .len()
            )
            .expect("token count fits u64"),
            count,
            "case={name}"
        );
        assert!(
            case["rendered_sha256"].as_str().is_some_and(is_sha256_hex),
            "case={name}"
        );
    }
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "empty-user",
            "json-terminal-schema",
            "plain",
            "tool-roundtrip-wire",
            "unicode"
        ]
    );

    let named = |name: &str| {
        cases
            .iter()
            .find(|case| case["name"] == name)
            .unwrap_or_else(|| panic!("b10068 wire oracle lost case {name}"))
    };
    let unicode = named("unicode");
    let upstream: Value =
        serde_json::from_slice(QWEN35_ORACLE_BYTES).expect("upstream oracle parses");
    let upstream_unicode = upstream["cases"]
        .as_array()
        .expect("upstream oracle has cases")
        .iter()
        .find(|case| case["name"] == "unicode")
        .expect("upstream oracle has unicode case");
    assert_eq!(unicode["rendered_bytes"], 126);
    assert_eq!(
        unicode["rendered_sha256"],
        upstream_unicode["rendered_sha256"]
    );
    assert_eq!(upstream_unicode["token_count"], 30);
    assert_eq!(unicode["native_input_tokens"], 31);

    let tool = &named("tool-roundtrip-wire")["body"];
    assert!(tool["messages"][2]["content"].is_null());
    assert_eq!(
        tool["messages"][2]["tool_calls"][0]["function"]["arguments"],
        r#"{"city":"Denver"}"#
    );
    assert_eq!(tool["messages"][2]["tool_calls"][0]["id"], "call-1");
    assert_eq!(tool["messages"][3]["tool_call_id"], "call-1");

    let schema = &named("json-terminal-schema")["body"];
    assert_eq!(schema["response_format"]["type"], "json_schema");
    assert_eq!(
        schema["response_format"]["json_schema"]["schema"]["additionalProperties"],
        false
    );
    assert_eq!(
        schema["messages"][1]["content"],
        r#"{"pane":"%1","text":"\u001b[31mRED\u001b[0m\n$ git status"}"#
    );
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
