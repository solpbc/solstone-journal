// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::test_hooks::{inspect_parakeet, stage_ready_parakeet};
use super::{
    InstallVerb, archive, cleanup_legacy_cuda_oci_dirs, dispatch, download_artifact, fingerprint,
    flatten_binary_bundle, hoist_binary, lease, local_backend_choice, manifest, metal_candidate,
    parakeet_target_for_install, pins, publish_staged_tree_with, readiness, status,
    write_parakeet_model_manifest,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_assets::{Artifact, Backend, Platform, catalog, resolve};

use crate::nvidia::NVIDIA_PROBE_SCHEMA;

const PARAKEET_TEST_KEY: &str = "x86_64-unknown-linux-gnu";

fn temp(name: &str) -> PathBuf {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "solstone-local-{name}-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn contacted_only_origin_host(contacted: &BTreeSet<String>, expected_host: &str) -> bool {
    contacted == &BTreeSet::from([expected_host.to_owned()])
}

fn assert_only_origin_host(contacted: BTreeSet<String>, expected_host: &str) {
    assert!(
        contacted_only_origin_host(&contacted, expected_host),
        "unexpected contacted hosts: {contacted:?}"
    );
}

fn flipped_origin_artifacts() -> Vec<&'static Artifact> {
    [
        ("llama-server-vulkan", Some(Platform::LinuxX64), None),
        ("llama-server-cuda", Some(Platform::LinuxX64), None),
        ("local-model", None, None),
        (
            "parakeet-server",
            Some(Platform::LinuxX64),
            Some(Backend::Cpu),
        ),
        ("parakeet-model", None, None),
    ]
    .into_iter()
    .map(|(unit, platform, backend)| resolve(unit, platform, backend).into_iter().next().unwrap())
    .collect()
}

#[test]
fn installer_modules_do_not_read_compile_time_host_platform() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let modules = [
        manifest_dir.join("src/install/ced_install.rs"),
        manifest_dir.join("src/install/rfdetr_install.rs"),
    ];
    assert!(modules.iter().all(|path| path.is_file()));
    let texts = modules
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>();
    assert!(texts.iter().all(|text| !text.is_empty()));
    assert!(texts.iter().all(|text| !text.contains("std::env::consts")));

    let orchestrator = manifest_dir
        .parent()
        .unwrap()
        .join("solstone-core/src/install_models.rs");
    assert!(
        fs::read_to_string(orchestrator)
            .unwrap()
            .contains("std::env::consts")
    );
}

#[test]
fn ced_does_not_construct_fit_reports() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let modules = [manifest_dir.join("src/install/ced_install.rs")];
    assert!(modules.iter().all(|path| path.is_file()));
    let texts = modules
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>();
    assert!(texts.iter().all(|text| !text.is_empty()));
    assert!(texts.iter().all(|text| !text.contains("fit_report")));

    let rfdetr = manifest_dir.join("src/install/rfdetr_install.rs");
    assert!(fs::read_to_string(rfdetr).unwrap().contains("fit_report"));
}

#[test]
fn download_artifact_reason_codes_cover_every_archive_error() {
    use archive::ArchiveError;
    let fallback = "download_failed";
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::HostRefused {
                host: "blocked.test".to_owned()
            },
            fallback
        ),
        "download_host_refused"
    );
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::InsecureScheme {
                scheme: "http".to_owned(),
                host: "example.test".to_owned()
            },
            fallback
        ),
        "download_insecure_scheme"
    );
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::UrlUserinfoRefused {
                authority: "user@host".to_owned()
            },
            fallback
        ),
        "download_url_userinfo_refused"
    );
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::SizeMismatch {
                expected: 1,
                actual: 2
            },
            fallback
        ),
        "download_size_mismatch"
    );
    assert_eq!(
        super::download_artifact_reason_code(&ArchiveError::DigestMismatch, fallback),
        "download_digest_mismatch"
    );
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::RedirectHopLimitExceeded { limit: 5 },
            fallback
        ),
        "download_redirect_hop_limit_exceeded"
    );
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::OriginUnavailable {
                host: "origin.test".to_owned(),
                message: "refused".to_owned()
            },
            fallback
        ),
        "download_origin_unreachable"
    );
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::Io(std::io::Error::other("io")),
            fallback
        ),
        fallback
    );
    assert_eq!(
        super::download_artifact_reason_code(
            &ArchiveError::Download("failed".to_owned()),
            fallback
        ),
        fallback
    );
    assert_eq!(
        super::download_artifact_reason_code(&ArchiveError::PathEscape("..".to_owned()), fallback),
        fallback
    );
}

#[test]
fn injected_dns_failure_maps_to_origin_unreachable_without_a_lookup() {
    let error = archive::ArchiveError::OriginUnavailable {
        host: "origin.test".to_owned(),
        message: "injected DNS resolution failure".to_owned(),
    };
    assert_eq!(
        super::download_artifact_reason_code(&error, "download_failed"),
        "download_origin_unreachable"
    );
}

fn fixture_artifact(url: String, filename: &'static str, body: &[u8]) -> Artifact {
    let sha256: String = Sha256::digest(body)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Artifact {
        unit: "test-artifact",
        version: "test",
        filename,
        sha256: Box::leak(sha256.into_boxed_str()),
        size_bytes: body.len() as u64,
        upstream_url: Box::leak(url.into_boxed_str()),
        origin_key: "test-origin",
        artifact_key: None,
        platform: None,
        backend: None,
        extracted_binary_sha256: None,
    }
}

fn candidate_request(root: &PathBuf) -> serde_json::Map<String, Value> {
    serde_json::from_value(json!({
        "journal": root,
        "backend": "metal",
        "metal_unified_memory_mib": 16000,
    }))
    .unwrap()
}

#[test]
fn local_backend_defaults_to_metal_on_apple_silicon() {
    let request = serde_json::Map::new();
    assert_eq!(
        super::local_backend_for_key(&request, "aarch64-apple-darwin").unwrap(),
        super::LocalBackend::Metal
    );
    assert_eq!(
        super::local_backend_for_key(&request, "x86_64-unknown-linux-gnu").unwrap(),
        super::LocalBackend::Existing
    );
}

#[test]
#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn metal_runtime_requires_the_supported_platform_without_ready_state() {
    let root = temp("metal-candidate-platform");
    let error = dispatch(
        InstallVerb::RunLocal,
        json!({"journal": root, "backend": "metal"}),
    )
    .unwrap_err();
    assert_eq!(error.exit_code, 65);
    assert_eq!(
        error.envelope.error.unwrap().reason_code,
        "unsupported_platform"
    );
    assert!(!status::status_path(&root, "local").exists());
    assert!(lease::lease_path(&root, "local").exists());
    assert!(!lease::is_held(&root, "local").unwrap());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metal_target_reuses_the_shared_4b_model_and_darwin_runtime_pin() {
    let root = temp("metal-target-4b");
    let target = super::local_target_for_key(
        &root,
        "local/qwen3.5-4b",
        super::LocalBackend::Metal,
        "aarch64-apple-darwin",
    )
    .unwrap();
    assert_eq!(target["backend"], "metal");
    assert_eq!(
        target["model_pin"],
        pins::model_identity("local/qwen3.5-4b").unwrap()
    );
    assert_eq!(
        target["runtime_pin"],
        pins::vulkan_identity("aarch64-apple-darwin").unwrap()
    );
    let error = super::local_target_for_key(
        &root,
        "local/qwen3.5-4b",
        super::LocalBackend::Metal,
        "x86_64-unknown-linux-gnu",
    )
    .unwrap_err();
    assert_eq!(
        error.envelope.error.unwrap().reason_code,
        "unsupported_platform"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metal_candidate_inspect_is_pure_and_reports_component_reasons_and_fit() {
    let root = temp("metal-candidate-inspect");
    let cache = pins::cache_root(&root);
    let runtime = cache.join("bin/aarch64-apple-darwin/b10068");
    let model = cache.join("models/local__qwen3.5-4b");
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&model).unwrap();
    fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").unwrap();
    archive::make_executable(&runtime.join("llama-server")).unwrap();
    fs::write(model.join("Qwen3.5-4B-Q4_K_M.gguf"), b"model").unwrap();
    fs::write(model.join("mmproj-F16.gguf"), b"projector").unwrap();
    let runtime_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        "target",
        json!({"pin_identity":pins::vulkan_identity("aarch64-apple-darwin").unwrap()}),
        manifest::runtime_inventory(&runtime, &[]).unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(
        &manifest::artifact_manifest_path(&runtime),
        &runtime_manifest,
    )
    .unwrap();
    let model_manifest = manifest::build_manifest(
        "local",
        "local-model",
        "target",
        json!({"pin_identity":pins::model_identity("local/qwen3.5-4b").unwrap()}),
        manifest::inventory_for_tree(&model, "model").unwrap(),
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(&manifest::artifact_manifest_path(&model), &model_manifest).unwrap();
    let before = archive::snapshot_tree(&pins::cache_root(&root)).unwrap();
    let ready =
        metal_candidate::inspect_with(&candidate_request(&root), "aarch64-apple-darwin").unwrap();
    assert_eq!(ready["ready"], true);
    assert_eq!(ready["artifacts"]["model_id"], "local/qwen3.5-4b");
    assert_eq!(ready["fit"]["model_bytes"], 2_740_937_888_u64);
    assert_eq!(ready["fit"]["measurement"], "unmeasured");
    assert_eq!(ready["fit"]["tier"]["source"], "supplied_measurement");
    assert_eq!(ready["fit"]["tier"]["unified_memory_mib"], 16000);
    assert!(ready["fit"].get("ram_requirement_mib").is_none());
    assert!(ready["fit"].get("threshold_mib").is_none());
    assert_eq!(
        archive::snapshot_tree(&pins::cache_root(&root)).unwrap(),
        before
    );

    status::begin(
        &root,
        r#"{"provider":"local","runtime":"mlx","model_pin":{"model_id":"qwen3.5:9b"}}"#.to_owned(),
        "legacy-mlx".to_owned(),
        None,
        "downloading",
    )
    .unwrap();
    let ignores_legacy_status =
        metal_candidate::inspect_with(&candidate_request(&root), "aarch64-apple-darwin").unwrap();
    assert_eq!(ignores_legacy_status["ready"], true);
    assert!(ignores_legacy_status["install"].is_null());

    fs::remove_file(status::status_path(&root, "local")).unwrap();
    status::begin(
        &root,
        r#"{"backend":"metal","model_pin":{"model_id":"local/qwen3.5-4b"},"runtime":"llama.cpp","runtime_pin":{"release_tag":"stale"}}"#.to_owned(),
        "stale-native-4b".to_owned(),
        None,
        "downloading",
    )
    .unwrap();
    let ignores_stale_native_status =
        metal_candidate::inspect_with(&candidate_request(&root), "aarch64-apple-darwin").unwrap();
    assert_eq!(ignores_stale_native_status["ready"], true);
    assert!(ignores_stale_native_status["install"].is_null());

    fs::remove_file(status::status_path(&root, "local")).unwrap();
    let current = super::resolved_fingerprint(
        super::local_target_for_key(
            &root,
            "local/qwen3.5-4b",
            super::LocalBackend::Metal,
            "aarch64-apple-darwin",
        )
        .unwrap(),
    )
    .unwrap();
    status::begin(
        &root,
        current["target_fingerprint_json"]
            .as_str()
            .unwrap()
            .to_owned(),
        current["target_fingerprint_sha256"]
            .as_str()
            .unwrap()
            .to_owned(),
        None,
        "downloading",
    )
    .unwrap();
    let reports_current_native_status =
        metal_candidate::inspect_with(&candidate_request(&root), "aarch64-apple-darwin").unwrap();
    assert_eq!(
        reports_current_native_status["install"]["install_state"],
        "downloading"
    );
    assert_eq!(
        reports_current_native_status["target"]["target_fingerprint_sha256"],
        current["target_fingerprint_sha256"]
    );

    fs::remove_file(model.join("Qwen3.5-4B-Q4_K_M.gguf")).unwrap();
    let missing =
        metal_candidate::inspect_with(&candidate_request(&root), "aarch64-apple-darwin").unwrap();
    assert_eq!(missing["failed_component"], "model_gguf");
    assert_eq!(missing["reason_code"], "inventory_member_missing");
    let _ = fs::remove_dir_all(root);
}

fn assert_manifest_proves_preflip_identity(root: &std::path::Path, unit: &str, identity: Value) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("payload"), b"fixture payload").unwrap();
    let manifest = manifest::build_manifest(
        "fixture",
        unit,
        "target",
        json!({"pin_identity": identity.clone()}),
        manifest::inventory_for_tree(root, "fixture").unwrap(),
        None,
        None,
    )
    .unwrap();
    let path = manifest::artifact_manifest_path(root);
    manifest::write_manifest(&path, &manifest).unwrap();
    assert_eq!(
        manifest::prove_manifest(&path, &identity),
        json!({"status":"ready","reason_code":"ready","cache_hit":false})
    );
}

#[test]
fn preflip_origin_readiness_fixture_preserves_all_pin_identities_and_proofs() {
    // Captured pre-flip at commit d343a2899712fd666266cd7c76a648ec0cb48120.
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/local_origin_readiness_preflip.json"
    ))
    .unwrap();
    assert_eq!(
        fixture["capture_commit"],
        "d343a2899712fd666266cd7c76a648ec0cb48120"
    );
    let root = temp("preflip-origin-readiness");

    for row in fixture["llama_server_vulkan"].as_array().unwrap() {
        let identity = pins::vulkan_identity(row["arch_key"].as_str().unwrap()).unwrap();
        assert_eq!(identity, row["pin_identity"]);
        assert_manifest_proves_preflip_identity(
            &root.join(format!("vulkan-{}", row["arch_key"].as_str().unwrap())),
            "llama-server-vulkan",
            identity,
        );
    }
    for row in fixture["llama_server_cuda"].as_array().unwrap() {
        let identity = pins::cuda_identity(row["arch_key"].as_str().unwrap()).unwrap();
        assert_eq!(identity, row["pin_identity"]);
        assert_manifest_proves_preflip_identity(
            &root.join(format!("cuda-{}", row["arch_key"].as_str().unwrap())),
            "llama-server-cuda",
            identity,
        );
    }

    let local_identity = pins::model_identity("local/qwen3.5-4b").unwrap();
    assert_eq!(local_identity, fixture["local_model"]["pin_identity"]);
    assert_manifest_proves_preflip_identity(
        &root.join("local-model"),
        "local-model",
        local_identity,
    );

    for row in fixture["parakeet_server"].as_array().unwrap() {
        let identity = pins::parakeet_backend_identity(
            row["arch_key"].as_str().unwrap(),
            row["backend"].as_str().unwrap(),
        )
        .unwrap();
        assert_eq!(identity, row["pin_identity"]);
        assert_manifest_proves_preflip_identity(
            &root.join(format!(
                "parakeet-{}-{}",
                row["arch_key"].as_str().unwrap(),
                row["backend"].as_str().unwrap()
            )),
            "parakeet-server",
            identity,
        );
    }

    let parakeet_identity = pins::parakeet_model_identity();
    assert_eq!(parakeet_identity, fixture["parakeet_model"]["pin_identity"]);
    assert_manifest_proves_preflip_identity(
        &root.join("parakeet-model"),
        "parakeet-model",
        parakeet_identity,
    );
    assert_eq!(
        fixture["proof"],
        json!({"status":"ready","reason_code":"ready","cache_hit":false})
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn preflip_manifest_fixture_rejects_a_perturbed_pin_identity() {
    let root = temp("preflip-origin-readiness-mismatch");
    let identity = pins::model_identity("local/qwen3.5-4b").unwrap();
    fs::write(root.join("payload"), b"fixture payload").unwrap();
    let built = manifest::build_manifest(
        "fixture",
        "local-model",
        "target",
        json!({"pin_identity": identity.clone()}),
        manifest::inventory_for_tree(&root, "fixture").unwrap(),
        None,
        None,
    )
    .unwrap();
    let path = manifest::artifact_manifest_path(&root);
    manifest::write_manifest(&path, &built).unwrap();
    let mut perturbed = identity;
    perturbed["sha256"] = Value::String("00".repeat(32));
    assert_eq!(
        manifest::prove_manifest(&path, &perturbed)["reason_code"],
        "manifest_pin_mismatch"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn preflip_fixture_preserves_paths_and_native_pins_json_fields() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../fixtures/local_origin_readiness_preflip.json"
    ))
    .unwrap();
    let exported = pins::pins_json();

    for row in fixture["llama_server_vulkan"].as_array().unwrap() {
        let expected = &row["pin_identity"];
        let actual = exported["llama_server_pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["artifact_key"] == expected["artifact_key"])
            .unwrap();
        for field in [
            "artifact_key",
            "release_tag",
            "filename",
            "sha256",
            "binary_name",
        ] {
            assert_eq!(actual[field], expected[field], "{field}");
        }
        let paths = pins::paths(
            std::path::Path::new("/journal"),
            expected["artifact_key"].as_str().unwrap(),
            Some("local/qwen3.5-4b"),
        );
        assert_eq!(
            paths["binary_path"],
            format!(
                "/journal/cache/providers/local/bin/{}/{}/{}",
                expected["artifact_key"].as_str().unwrap(),
                expected["release_tag"].as_str().unwrap(),
                expected["binary_name"].as_str().unwrap(),
            )
        );
    }
    assert_eq!(
        exported["cuda_server_pin"]["artifacts"],
        Value::Array(
            fixture["llama_server_cuda"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["pin_identity"].clone())
                .collect(),
        )
    );
    for row in fixture["llama_server_cuda"].as_array().unwrap() {
        let identity = &row["pin_identity"];
        let paths = pins::paths(
            std::path::Path::new("/journal"),
            identity["artifact_key"].as_str().unwrap(),
            None,
        );
        assert_eq!(
            paths["cuda_binary_path"],
            format!(
                "/journal/cache/providers/local/cuda/{}/{}/llama-server",
                identity["artifact_key"].as_str().unwrap(),
                identity["sha256"].as_str().unwrap(),
            )
        );
    }
    assert_eq!(
        pins::paths(
            std::path::Path::new("/journal"),
            PARAKEET_TEST_KEY,
            Some("local/qwen3.5-4b"),
        )["model_dir"],
        fixture["paths"]["local_model_dir"]
    );
    for row in fixture["parakeet_server"].as_array().unwrap() {
        let identity = &row["pin_identity"];
        let paths = pins::parakeet_paths(
            std::path::Path::new("/journal"),
            identity["artifact_key"].as_str().unwrap(),
        );
        assert_eq!(
            paths[format!("binary_path_{}", identity["backend"].as_str().unwrap())],
            format!(
                "/journal/cache/providers/parakeet/bin/{}/{}/{}/{}",
                identity["artifact_key"].as_str().unwrap(),
                identity["backend"].as_str().unwrap(),
                identity["release_tag"].as_str().unwrap(),
                identity["binary_name"].as_str().unwrap(),
            )
        );
    }
    assert_eq!(
        pins::parakeet_paths(std::path::Path::new("/journal"), PARAKEET_TEST_KEY)["model_path"],
        fixture["paths"]["parakeet_model_path"]
    );
}

#[test]
fn origin_urls_follow_the_catalog_for_every_rust_download_unit() {
    let cases = [
        (
            "llama-server-vulkan",
            Some(Platform::LinuxX64),
            None,
            "https://updates.solstone.app/assets/llama-server-vulkan/b10068/llama-b10068-bin-ubuntu-vulkan-x64.tar.gz",
        ),
        (
            "llama-server-cuda",
            Some(Platform::LinuxX64),
            None,
            "https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz",
        ),
        (
            "local-model",
            None,
            None,
            "https://updates.solstone.app/assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        ),
        (
            "parakeet-server",
            Some(Platform::LinuxX64),
            Some(Backend::Cpu),
            "https://updates.solstone.app/assets/parakeet-server/v0.5.0/parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz",
        ),
        (
            "parakeet-model",
            None,
            None,
            "https://updates.solstone.app/assets/parakeet-model/bf0af9f425fa01809cadec671b3cb672709d13e9/tdt-0.6b-v3-q8_0.gguf",
        ),
    ];
    for (unit, platform, backend, expected) in cases {
        let artifact = resolve(unit, platform, backend).into_iter().next().unwrap();
        assert_eq!(
            archive::origin_url("https://updates.solstone.app", artifact.origin_key),
            expected
        );
    }
}

#[test]
fn origin_url_for_arch_key_is_host_independent_and_catalog_derived() {
    for (unit, key) in [
        ("llama-server-vulkan", "aarch64-apple-darwin"),
        ("llama-server-vulkan", "x86_64-unknown-linux-gnu"),
        ("llama-server-vulkan", "aarch64-unknown-linux-gnu"),
        ("llama-server-cuda", "x86_64-unknown-linux-gnu"),
        ("llama-server-cuda", "aarch64-unknown-linux-gnu"),
    ] {
        let expected = catalog()
            .iter()
            .find(|artifact| artifact.unit == unit && artifact.artifact_key == Some(key))
            .unwrap();
        assert_eq!(
            pins::origin_url_for_arch_key(unit, key),
            Some(format!(
                "https://updates.solstone.app/{}",
                expected.origin_key
            ))
        );
    }
    assert_eq!(
        pins::origin_url_for_arch_key("llama-server-vulkan", "unknown"),
        None
    );
}

#[test]
fn status_corpus_has_exact_case_count() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../fixtures/install_status.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 63);
}

#[test]
fn manifest_model_rewrite_excludes_the_previous_manifest_from_its_inventory() {
    let root = temp("manifest-model-rewrite");
    let manifest_path = manifest::artifact_manifest_path(&root);
    fs::write(root.join("model.gguf"), b"model bytes").unwrap();
    fs::write(&manifest_path, b"old manifest\n").unwrap();
    let pin_identity = json!({"unit": "test-model"});

    dispatch(
        InstallVerb::ManifestModel,
        json!({
            "root": root,
            "manifest_path": manifest_path,
            "target_fingerprint_sha256": "target",
            "pin_identity": pin_identity,
        }),
    )
    .unwrap();

    assert_eq!(
        manifest::prove_manifest(&manifest_path, &pin_identity)["status"],
        "ready"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn manifest_model_rewrite_excludes_a_leftover_writer_temp_from_its_inventory() {
    let root = temp("manifest-model-temp-rewrite");
    let manifest_path = manifest::artifact_manifest_path(&root);
    fs::write(root.join("model.gguf"), b"model bytes").unwrap();
    fs::write(
        root.join(format!(".{}.tmp", manifest::MANIFEST_NAME)),
        b"interrupted manifest write",
    )
    .unwrap();
    let pin_identity = json!({"unit": "test-model"});

    dispatch(
        InstallVerb::ManifestModel,
        json!({
            "root": root,
            "manifest_path": manifest_path,
            "target_fingerprint_sha256": "target",
            "pin_identity": pin_identity,
        }),
    )
    .unwrap();

    assert_eq!(
        manifest::prove_manifest(&manifest_path, &pin_identity)["status"],
        "ready"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn parakeet_model_manifest_still_proves_after_a_second_write() {
    let root = temp("parakeet-model-manifest-rewrite");
    fs::write(root.join("tdt-0.6b-v3-q8_0.gguf"), b"model bytes").unwrap();
    let mut attempt = status::idle_status("parakeet");
    attempt.attempt_id = Some("attempt".to_owned());
    attempt.target_fingerprint_sha256 = Some("target".to_owned());

    write_parakeet_model_manifest(&root, &attempt).unwrap();
    write_parakeet_model_manifest(&root, &attempt).unwrap();

    assert_eq!(
        manifest::prove_manifest(
            &manifest::artifact_manifest_path(&root),
            &pins::parakeet_model_identity(),
        )["status"],
        "ready"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn status_corpus_replays_the_full_transition_matrix() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../fixtures/install_status.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 63);
    let target = fixture["targets"]["primary"].clone();
    let other_target = fixture["targets"]["other"].clone();
    let mut visited = BTreeSet::new();
    for case in cases.iter().filter(|case| {
        case["name"].as_str().unwrap().starts_with("transition_")
            && case["name"].as_str().unwrap().contains("_to_")
    }) {
        let name = case["name"].as_str().unwrap();
        let parts: Vec<_> = name
            .trim_start_matches("transition_")
            .split("_to_")
            .collect();
        if parts.len() != 2 || parts[0].is_empty() {
            continue;
        }
        visited.insert(name.to_owned());
        let root = temp(name);
        let mut current = begin_for_test(&root, target.clone());
        if parts[0] != "resolving" {
            match status::transition(current, parts[0], None, None)
                .and_then(|value| status::write_status(&root, value))
            {
                Ok(value) => current = value,
                Err(_) => {
                    assert!(case["refused"].is_string(), "{name}");
                    let _ = fs::remove_dir_all(root);
                    continue;
                }
            }
        }
        let actual = status::transition(current, parts[1], None, None)
            .and_then(|value| status::write_status(&root, value));
        let expected_refusal = case["refused"].is_string();
        assert_eq!(actual.is_err(), expected_refusal, "{name}");
        if let Ok(actual) = actual {
            assert_eq!(
                redact_status(&serde_json::to_value(actual).unwrap()),
                case["status"],
                "{name}"
            );
        }
        assert_durable_case(&root, case);
        let _ = fs::remove_dir_all(root);
    }
    replay_status_special_cases(cases, target, other_target, &mut visited);
    assert_eq!(
        visited,
        cases
            .iter()
            .map(|case| case["name"].as_str().unwrap().to_owned())
            .collect()
    );
}

fn replay_status_special_cases(
    cases: &[Value],
    target: Value,
    other_target: Value,
    visited: &mut BTreeSet<String>,
) {
    for case in cases {
        let name = case["name"].as_str().unwrap();
        if name.starts_with("transition_")
            && name.contains("_to_")
            && !name.starts_with("transition_to_")
        {
            continue;
        }
        let root = temp(name);
        let outcome = match name {
            "read_before_any_write" => Ok(status::read_status(&root, "local").unwrap()),
            "begin_from_idle" => Ok(begin_for_test(&root, target.clone())),
            "begin_twice_same_target" => {
                let _ = begin_for_test(&root, target.clone());
                begin_for_test_result(&root, target.clone())
            }
            "begin_twice_different_target" => {
                let _ = begin_for_test(&root, target.clone());
                begin_for_test_result(&root, other_target.clone())
            }
            "begin_or_replace_takes_over" => {
                let _ = begin_for_test(&root, target.clone());
                let other = fingerprint::canonical(other_target.clone()).unwrap();
                status::begin_or_replace(
                    &root,
                    "local",
                    other.clone(),
                    fingerprint::sha256(&other),
                    None,
                    "resolving",
                )
            }
            "transition_to_failed_carries_error" => {
                let current = begin_for_test(&root, target.clone());
                status::transition(
                    current,
                    "failed",
                    Some("download timed out".to_owned()),
                    Some("network_unreachable".to_owned()),
                )
                .and_then(|value| status::write_status(&root, value))
            }
            "progress_bump" => {
                let current = status::write_status(
                    &root,
                    status::transition(
                        begin_for_test(&root, target.clone()),
                        "downloading",
                        None,
                        None,
                    )
                    .unwrap(),
                )
                .unwrap();
                let mut clock = Instant::now() - Duration::from_secs(2);
                Ok(
                    status::bump_progress(current, Some(1024), Some(4096), &mut clock)
                        .unwrap()
                        .unwrap(),
                )
            }
            "progress_bump_without_total" => {
                let current = status::write_status(
                    &root,
                    status::transition(
                        begin_for_test(&root, target.clone()),
                        "downloading",
                        None,
                        None,
                    )
                    .unwrap(),
                )
                .unwrap();
                let mut clock = Instant::now() - Duration::from_secs(2);
                Ok(status::bump_progress(current, Some(1024), None, &mut clock)
                    .unwrap()
                    .unwrap())
            }
            "stale_attempt_write_is_refused" => {
                let stale = begin_for_test(&root, target.clone());
                let other = fingerprint::canonical(other_target.clone()).unwrap();
                let _ = status::begin_or_replace(
                    &root,
                    "local",
                    other.clone(),
                    fingerprint::sha256(&other),
                    None,
                    "resolving",
                );
                status::transition(stale, "installed", None, None)
                    .and_then(|value| status::write_status(&root, value))
            }
            "assert_current_after_replacement" => {
                let stale = begin_for_test(&root, target.clone());
                let other = fingerprint::canonical(other_target.clone()).unwrap();
                let _ = status::begin_or_replace(
                    &root,
                    "local",
                    other.clone(),
                    fingerprint::sha256(&other),
                    None,
                    "resolving",
                );
                status::assert_current(&root, &stale)
            }
            "record_interrupted" => {
                let started = begin_for_test(&root, target.clone());
                status::record_interrupted(
                    &root,
                    started.attempt_id.as_deref().unwrap(),
                    started.target_fingerprint_sha256.as_deref(),
                )
            }
            "record_interrupted_wrong_attempt" => {
                let _ = begin_for_test(&root, target.clone());
                status::record_interrupted(&root, "00000000-0000-0000-0000-000000000000", None)
            }
            "malformed_record_is_refused_not_replaced" => {
                let path = status::status_path(&root, "local");
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, "{{").unwrap();
                status::read_status(&root, "local")
            }
            "unknown_provider_is_refused" => status::read_status(&root, "not-a-provider"),
            _ => continue,
        };
        visited.insert(name.to_owned());
        assert_eq!(outcome.is_err(), case["refused"].is_string(), "{name}");
        if let Ok(status) = outcome {
            assert_eq!(
                redact_status(&serde_json::to_value(status).unwrap()),
                case["status"],
                "{name}"
            );
        }
        assert_durable_case(&root, case);
        let _ = fs::remove_dir_all(root);
    }
}

fn assert_durable_case(root: &std::path::Path, case: &Value) {
    let path = status::status_path(root, "local");
    if let Some(raw) = case.get("on_disk_raw").and_then(Value::as_str) {
        assert_eq!(fs::read_to_string(path).unwrap(), raw, "{}", case["name"]);
        return;
    }
    let expected = &case["on_disk"];
    if expected.is_null() {
        assert!(!path.exists(), "{}", case["name"]);
    } else {
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(redact_status(&value), *expected, "{}", case["name"]);
    }
}

fn begin_for_test(root: &std::path::Path, target: Value) -> status::InstallStatus {
    begin_for_test_result(root, target).unwrap()
}

fn begin_for_test_result(
    root: &std::path::Path,
    target: Value,
) -> Result<status::InstallStatus, status::StatusError> {
    let canonical = fingerprint::canonical(target).unwrap();
    status::begin(
        root,
        canonical.clone(),
        fingerprint::sha256(&canonical),
        None,
        "resolving",
    )
}

fn redact_status(value: &Value) -> Value {
    let mut value = value.clone();
    if value["attempt_id"].is_string() {
        value["attempt_id"] = Value::String("<attempt-id>".to_owned());
    }
    for key in [
        "started_at",
        "last_transition_at",
        "last_progress_at",
        "completed_at",
    ] {
        if value[key].is_string() {
            value[key] = Value::String("<timestamp>".to_owned());
        }
    }
    value
}

#[test]
fn unknown_provider_message_matches_the_accepted_set() {
    // The rejection message is built from `status::PROVIDERS` at the call
    // site, not hardcoded here, so this fails the moment the message and
    // the accepted set could ever disagree again.
    let mut sorted = status::PROVIDERS.to_vec();
    sorted.sort_unstable();
    let listed = sorted
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let expected =
        format!("malformed install status: provider install status must be one of: [{listed}]");

    let root = temp("unknown-provider-read");
    let error = status::read_status(&root, "not-a-provider").unwrap_err();
    assert_eq!(error.to_string(), expected);

    let root = temp("unknown-provider-write");
    let error = status::write_status(&root, status::idle_status("not-a-provider")).unwrap_err();
    assert_eq!(error.to_string(), expected);
}

#[test]
fn read_status_accepts_every_provider_in_the_allowlist() {
    for provider in status::PROVIDERS {
        let root = temp(&format!("accepts-{provider}"));
        let idle = status::read_status(&root, provider).unwrap();
        assert_eq!(idle.provider, *provider);
        assert_eq!(idle.install_state, "idle");
    }
}

#[test]
fn canonical_fingerprint_vectors_match_fixture() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../fixtures/local_contract.json")).unwrap();
    let vectors = fixture["canonical_fingerprint"]["vectors"]
        .as_array()
        .unwrap();
    assert_eq!(vectors.len(), 18);
    for vector in vectors {
        let input = serde_json::from_str(vector["input_json"].as_str().unwrap()).unwrap();
        assert_eq!(
            fingerprint::canonical(input).unwrap(),
            vector["canonical_json"].as_str().unwrap(),
            "{}",
            vector["name"]
        );
    }
    let digests = fixture["canonical_fingerprint"]["canonical_digest_vectors"]
        .as_array()
        .unwrap();
    assert_eq!(digests.len(), 4);
    for vector in digests {
        let text = vector["wrapped_canonical_json"].as_str().unwrap();
        assert_eq!(
            fingerprint::hmac_sha256(&(0_u8..32).collect::<Vec<_>>(), text),
            vector["hmac_sha256"].as_str().unwrap(),
            "{}",
            vector["name"]
        );
    }
}

#[test]
fn local_target_fingerprint_matches_python_vulkan_reference() {
    // Captured with:
    // python3 -c 'from solstone.think.providers import local_install,local_cuda;\
    // local_cuda.resolve_local_backend=lambda p: local_cuda.BackendChoice("vulkan","no NVIDIA GPU detected");\
    // print(local_install.target_fingerprint())'
    // on the current reference tree, for x86_64-unknown-linux-gnu and LOCAL_MODEL.
    let expected_json = r#"{"backend":"vulkan","backend_reason":"no NVIDIA GPU detected","model_pin":{"filename":"Qwen3.5-4B-Q4_K_M.gguf","mmproj_filename":"mmproj-F16.gguf","mmproj_sha256":"cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864","model_id":"local/qwen3.5-4b","repo":"unsloth/Qwen3.5-4B-GGUF","revision":"main","sha256":"00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4","unit":"local-model"},"provider":"local","runtime":"llama.cpp","runtime_pin":{"artifact_key":"x86_64-unknown-linux-gnu","binary_name":"llama-server","filename":"llama-b10068-bin-ubuntu-vulkan-x64.tar.gz","release_tag":"b10068","sha256":"713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3","unit":"llama-server-vulkan"}}"#;
    let mut input = serde_json::from_str::<serde_json::Map<String, Value>>(expected_json).unwrap();
    input.remove("provider");
    let actual = fingerprint::local_fingerprint(input).unwrap();
    assert_eq!(actual["target_fingerprint_json"], expected_json);
    assert_eq!(
        actual["target_fingerprint_sha256"],
        "73b5c7de3b796917a5b8cc80b00ba0eef57daef790f1aafb7e64abcd4c9770e2"
    );
}

#[test]
fn fingerprint_transport_resolves_targets_without_writing_status() {
    let root = temp("fingerprint-transport");
    let local = dispatch(
        InstallVerb::FingerprintLocal,
        json!({"journal":root,"model_id":"local/qwen3.5-4b"}),
    )
    .unwrap();
    let local = local.result.unwrap();
    let local_target: Value =
        serde_json::from_str(local["target_fingerprint_json"].as_str().unwrap()).unwrap();
    assert_eq!(local_target["runtime"], "llama.cpp");
    assert!(local_target["runtime_pin"].is_object());
    assert!(local_target["model_pin"].is_object());
    assert!(!status::status_path(&root, "local").exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn dispatch_pins_parakeet_matches_the_pins_table() {
    let result = dispatch(InstallVerb::PinsParakeet, json!({})).unwrap();
    let pins = result.result.unwrap();
    assert_eq!(
        pins["parakeet_vulkan_pins"].as_array().unwrap().len(),
        pins::PARAKEET_VULKAN_PINS.len()
    );
    assert_eq!(
        pins["parakeet_cpu_pins"].as_array().unwrap().len(),
        pins::PARAKEET_CPU_PINS.len()
    );
    assert_eq!(pins["parakeet_model"]["repo"], pins::PARAKEET_MODEL.0);
}

#[test]
fn dispatch_paths_parakeet_with_explicit_artifact_key_is_host_independent() {
    // artifact_key is supplied explicitly, so this never touches the real
    // host's OS/arch -- it must pass on every CI runner, not just Linux.
    let root = temp("paths-parakeet");
    let result = dispatch(
        InstallVerb::PathsParakeet,
        json!({"journal": root, "artifact_key": "aarch64-unknown-linux-gnu"}),
    )
    .unwrap();
    let paths = result.result.unwrap();
    assert_eq!(
        paths["binary_path_cpu"],
        format!(
            "{}/cache/providers/parakeet/bin/aarch64-unknown-linux-gnu/cpu/v0.5.0/parakeet-server",
            root.display()
        )
    );
    assert_eq!(
        paths["binary_path_vulkan"],
        format!(
            "{}/cache/providers/parakeet/bin/aarch64-unknown-linux-gnu/vulkan/v0.5.0/parakeet-server",
            root.display()
        )
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn fingerprint_parakeet_matches_host_support() {
    // Deliberately host-conditional rather than host-independent: this is
    // the one test that exercises `parakeet_host_artifact_key`'s real-host
    // path, so it asserts whatever is actually correct for the machine it
    // runs on (both branches are exercised host-independently already by
    // `parakeet_artifact_key_matches_every_python_alias` /
    // `_refuses_non_linux_and_unrecognized_arch` above).
    let root = temp("fingerprint-parakeet");
    let result = dispatch(InstallVerb::FingerprintParakeet, json!({"journal": root}));
    let host_supported =
        std::env::consts::OS == "linux" && matches!(std::env::consts::ARCH, "x86_64" | "aarch64");
    if host_supported {
        let ok = result.unwrap().result.unwrap();
        let target: Value =
            serde_json::from_str(ok["target_fingerprint_json"].as_str().unwrap()).unwrap();
        assert_eq!(target["provider"], "parakeet");
        assert_eq!(target["runtime"], "parakeet.cpp");
        assert_eq!(target["binary_pins"].as_array().unwrap().len(), 2);
        assert!(target["model_pin"].is_object());
    } else {
        let error = result.unwrap_err();
        assert_eq!(error.envelope.error.as_ref().unwrap().kind, "platform");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn parakeet_model_identity_matches_pinned_model_tuple() {
    let (repo, filename, revision, sha256, _size_bytes) = pins::PARAKEET_MODEL;
    assert_eq!(
        pins::parakeet_model_identity(),
        json!({"unit":"parakeet-model","repo":repo,"filename":filename,"revision":revision,"sha256":sha256})
    );
}

/// The regression that matters to an owner: a manifest carrying the identity
/// the SHIPPED reference writes must prove here, or upgrading re-fetches a
/// model that is already on disk and correct.
///
/// ⚠ The expected identity is transcribed from the reference's
/// `_model_pin_identity()` rather than built from `pins`, deliberately. Deriving
/// it from the thing under test is what let the drift live: every existing
/// assertion compared `parakeet_model_identity()` against itself and passed.
#[test]
fn a_manifest_written_with_the_reference_identity_still_proves() {
    let reference_identity = json!({
        "unit": "parakeet-model",
        "repo": "mudler/parakeet-cpp-gguf",
        "filename": "tdt-0.6b-v3-q8_0.gguf",
        "revision": "bf0af9f425fa01809cadec671b3cb672709d13e9",
        "sha256": "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757",
    });
    let root = temp("reference-parakeet-model-identity");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("payload"), b"fixture payload").unwrap();
    let manifest = manifest::build_manifest(
        "parakeet",
        "parakeet-model",
        "target",
        json!({"pin_identity": reference_identity}),
        manifest::inventory_for_tree(&root, "model").unwrap(),
        None,
        None,
    )
    .unwrap();
    let path = manifest::artifact_manifest_path(&root);
    manifest::write_manifest(&path, &manifest).unwrap();
    assert_eq!(
        manifest::prove_manifest(&path, &pins::parakeet_model_identity()),
        json!({"status":"ready","reason_code":"ready","cache_hit":false}),
        "native readiness rejects a manifest the shipped reference wrote"
    );
    let _ = fs::remove_dir_all(&root);
}

/// The key SET is the contract, not the values alone: `prove_manifest` compares
/// canonicalized JSON for exact equality, so one extra key invalidates every
/// manifest an owner already has on disk. An assertion phrased "carries these
/// five" passes on a six-key identity -- which is how the size field got here
/// and stayed.
#[test]
fn parakeet_model_identity_carries_exactly_the_reference_key_set() {
    let identity = pins::parakeet_model_identity();
    let keys = identity
        .as_object()
        .expect("identity is an object")
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        keys,
        ["filename", "repo", "revision", "sha256", "unit"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "parakeet model identity drifted from the shape the reference records"
    );
}

#[test]
fn run_parakeet_returns_exit_75_when_the_lease_is_held() {
    // The lease check runs before any platform lookup, so this is
    // host-independent even though parakeet itself is Linux-only.
    let root = temp("run-parakeet-busy-lease");
    let _held = lease::acquire(&root, "parakeet").unwrap().unwrap();
    let error = dispatch(InstallVerb::RunParakeet, json!({"journal": root})).unwrap_err();
    assert_eq!(error.exit_code, lease::BUSY_EXIT_CODE);
    let error_body = error.envelope.error.as_ref().unwrap();
    assert_eq!(error_body.kind, "busy");
    assert_eq!(error_body.reason_code, "install_busy");
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn is_held_reports_read_only_held_lease() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp("read-only-held-lease");
    let held = lease::acquire(&root, "parakeet").unwrap().unwrap();
    let path = lease::lease_path(&root, "parakeet");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();

    let observed = lease::is_held(&root, "parakeet");

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(observed.unwrap());
    drop(held);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn inspect_local_resolves_backend_and_exposes_supervisor_host_fields() {
    let root = temp("inspect-local-host");
    let probe = json!({
        "schema": NVIDIA_PROBE_SCHEMA,
        "detected": false,
        "gpu_index": null,
        "gpu_name": null,
        "compute_cap": null,
        "arch": null,
        "driver_cuda_major": null,
        "vram_mib": null,
        "unified_memory_mib": null,
        "probe_error": "test override"
    });
    let result = readiness::inspect_local(
        serde_json::from_value(json!({
            "journal": root,
            "model_id": "local/qwen3.5-4b",
            "backend": "cuda",
            "nvidia_probe": probe,
        }))
        .unwrap(),
    );
    assert_eq!(result["host"]["backend"], "vulkan");
    assert_eq!(result["host"]["backend_reason"], "no NVIDIA GPU detected");
    assert_eq!(result["host"]["platform_supported"], true);

    let unsupported = readiness::inspect_local(
        serde_json::from_value(json!({
            "journal": temp("inspect-local-unsupported"),
            "artifact_key": "unsupported-platform",
            "nvidia_probe": probe,
        }))
        .unwrap(),
    );
    assert_eq!(unsupported["host"]["platform_supported"], false);
}

#[test]
fn inspect_parakeet_isolates_each_corrupt_artifact_proof() {
    for name in ["binary_cpu", "binary_vulkan", "model"] {
        let root = temp(&format!("inspect-parakeet-isolation-{name}"));
        let fixture = stage_ready_parakeet(&root, PARAKEET_TEST_KEY, true);
        let corrupt = match name {
            "binary_cpu" => fixture.cpu_path,
            "binary_vulkan" => fixture.vulkan_path,
            "model" => fixture.model_path,
            _ => unreachable!(),
        };
        fs::write(corrupt, b"corrupt").unwrap();

        let result = inspect_parakeet(&root, PARAKEET_TEST_KEY);
        assert_eq!(result["status"], "missing-or-mismatched", "{name}");
        assert_eq!(
            result["proof"][name]["reason_code"], "sha256_mismatch",
            "{name}"
        );
        for other in ["binary_cpu", "binary_vulkan", "model"] {
            if other != name {
                assert_eq!(result["proof"][other]["status"], "ready", "{name}/{other}");
            }
        }
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn inspect_parakeet_distinguishes_missing_manifest_from_corrupt_artifact() {
    let missing_root = temp("inspect-parakeet-missing-manifest");
    let fixture = stage_ready_parakeet(&missing_root, PARAKEET_TEST_KEY, true);
    fs::remove_file(manifest::artifact_manifest_path(
        fixture.cpu_path.parent().unwrap(),
    ))
    .unwrap();
    let missing = inspect_parakeet(&missing_root, PARAKEET_TEST_KEY);
    assert_eq!(
        missing["proof"]["binary_cpu"]["reason_code"],
        "manifest_missing"
    );

    let corrupt_root = temp("inspect-parakeet-corrupt-artifact");
    let fixture = stage_ready_parakeet(&corrupt_root, PARAKEET_TEST_KEY, true);
    fs::write(fixture.model_path, b"corrupt").unwrap();
    let corrupt = inspect_parakeet(&corrupt_root, PARAKEET_TEST_KEY);
    assert_eq!(corrupt["proof"]["model"]["reason_code"], "sha256_mismatch");

    let _ = fs::remove_dir_all(missing_root);
    let _ = fs::remove_dir_all(corrupt_root);
}

#[test]
fn inspect_parakeet_reduces_invalid_inputs() {
    let root = temp("inspect-parakeet-invalid-input");
    let missing_journal = dispatch(
        InstallVerb::InspectParakeet,
        json!({"artifact_key":PARAKEET_TEST_KEY}),
    )
    .unwrap()
    .result
    .unwrap();
    assert_eq!(missing_journal["status"], "proof-unavailable");
    assert_eq!(missing_journal["reason_code"], "journal_required");
    assert_eq!(missing_journal["target"]["artifact_key"], PARAKEET_TEST_KEY);

    let unsupported = dispatch(
        InstallVerb::InspectParakeet,
        json!({"journal":root,"artifact_key":"unsupported-key"}),
    )
    .unwrap()
    .result
    .unwrap();
    assert_eq!(unsupported["status"], "proof-unavailable");
    assert_eq!(unsupported["reason_code"], "unsupported_platform");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extraction_refuses_relative_and_absolute_escapes_without_parent_changes() {
    for (name, member) in [("relative", "../../etc/passwd"), ("absolute", "/tmp/evil")] {
        let root = temp(name);
        let archive_path = root.join("bad.tar.gz");
        write_unsafe_tar(&archive_path, member);
        let before = archive::snapshot_tree(&root).unwrap();
        assert!(matches!(
            archive::extract_tar_gz(&archive_path, &root.join("dest")),
            Err(archive::ArchiveError::PathEscape(_))
        ));
        assert_eq!(archive::snapshot_tree(&root).unwrap(), before);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn extraction_refuses_symlink_and_hardlink_escapes_without_parent_changes() {
    for (name, kind) in [
        ("symlink", tar::EntryType::Symlink),
        ("hardlink", tar::EntryType::Link),
    ] {
        let root = temp(name);
        let archive_path = root.join("bad-link.tar.gz");
        write_link_escape_tar(&archive_path, kind);
        let before = archive::snapshot_tree(&root).unwrap();
        assert!(matches!(
            archive::extract_tar_gz(&archive_path, &root.join("dest")),
            Err(archive::ArchiveError::PathEscape(_))
        ));
        assert_eq!(archive::snapshot_tree(&root).unwrap(), before);
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn extraction_creates_missing_parent_for_regular_file_entry() {
    let root = temp("missing-dirent-file");
    let archive_path = root.join("missing-dirent-file.tar.gz");
    let contents = b"NVIDIA CUDA EULA";
    let file = fs::File::create(&archive_path).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(
            &mut header,
            "licenses/NVIDIA-CUDA-EULA-13.3.txt",
            contents.as_slice(),
        )
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();

    let destination = root.join("dest");
    let extraction = archive::extract_tar_gz(&archive_path, &destination);
    assert!(
        extraction.is_ok(),
        "failed to extract fixture: {extraction:?}"
    );
    assert_eq!(
        fs::read(destination.join("licenses/NVIDIA-CUDA-EULA-13.3.txt")).unwrap(),
        contents
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn extraction_creates_missing_parent_for_symlink_entry() {
    let root = temp("missing-dirent-symlink");
    let archive_path = root.join("missing-dirent-symlink.tar.gz");
    let contents = b"NVIDIA CUDA EULA";
    let file = fs::File::create(&archive_path).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let mut link_header = tar::Header::new_gnu();
    link_header.set_entry_type(tar::EntryType::Symlink);
    link_header.set_size(0);
    link_header.set_mode(0o777);
    link_header
        .set_link_name("NVIDIA-CUDA-EULA-13.3.txt")
        .unwrap();
    link_header.set_cksum();
    builder
        .append_data(&mut link_header, "licenses/CUDA-EULA.txt", std::io::empty())
        .unwrap();

    let mut file_header = tar::Header::new_gnu();
    file_header.set_size(contents.len() as u64);
    file_header.set_mode(0o644);
    file_header.set_cksum();
    builder
        .append_data(
            &mut file_header,
            "licenses/NVIDIA-CUDA-EULA-13.3.txt",
            contents.as_slice(),
        )
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();

    let destination = root.join("dest");
    let extraction = archive::extract_tar_gz(&archive_path, &destination);
    assert!(
        extraction.is_ok(),
        "failed to extract fixture: {extraction:?}"
    );
    let link = destination.join("licenses/CUDA-EULA.txt");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(link).unwrap(), contents);
    let _ = fs::remove_dir_all(root);
}

fn write_nested_vulkan_tar(path: &std::path::Path) {
    let file = fs::File::create(path).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut dir_header = tar::Header::new_gnu();
    dir_header.set_entry_type(tar::EntryType::Directory);
    dir_header.set_size(0);
    dir_header.set_mode(0o755);
    dir_header.set_cksum();
    builder
        .append_data(&mut dir_header, "llama-b10068", std::io::empty())
        .unwrap();
    for (name, bytes) in [
        ("llama-b10068/llama-server", b"binary".as_slice()),
        (
            "llama-b10068/libllama-server-impl.so",
            b"library".as_slice(),
        ),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, bytes).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn ac6_vulkan_nested_extract_is_flattened_by_run_local_install_hoist() {
    let root = temp("vulkan-nested-hoist");
    let staging = root.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let archive_path = staging.join("archive.tar.gz");
    write_nested_vulkan_tar(&archive_path);
    archive::extract_tar_gz(&archive_path, &staging).unwrap();
    let binary = staging.join("llama-b10068").join("llama-server");
    assert!(binary.is_file());
    assert!(
        staging
            .join("llama-b10068")
            .join("libllama-server-impl.so")
            .is_file()
    );

    hoist_binary(&staging, &binary, "vulkan").unwrap();
    assert_eq!(fs::read(staging.join("llama-server")).unwrap(), b"binary");
    assert_eq!(
        fs::read(staging.join("libllama-server-impl.so")).unwrap(),
        b"library"
    );
    assert!(!fs::read(staging.join("archive.tar.gz")).unwrap().is_empty());
    assert!(
        !staging.join("llama-b10068").exists(),
        "run_local_install vulkan hoist must remove the nested llama-b10068/ dir"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metal_bundle_flatten_keeps_runtime_siblings_beside_binary() {
    let root = temp("metal-bundle-flatten");
    let staging = root.join("staging");
    let bundle = staging.join("llama-b10068");
    fs::create_dir_all(&bundle).unwrap();
    fs::write(staging.join("archive.tar.gz"), b"archive").unwrap();
    fs::write(bundle.join("llama-server"), b"binary").unwrap();
    fs::write(bundle.join("libllama-server-impl.dylib"), b"library").unwrap();
    fs::write(bundle.join("LICENSE"), b"license").unwrap();

    flatten_binary_bundle(&staging, &bundle.join("llama-server")).unwrap();

    assert_eq!(fs::read(staging.join("llama-server")).unwrap(), b"binary");
    assert_eq!(
        fs::read(staging.join("libllama-server-impl.dylib")).unwrap(),
        b"library"
    );
    assert_eq!(fs::read(staging.join("LICENSE")).unwrap(), b"license");
    assert_eq!(
        fs::read(staging.join("archive.tar.gz")).unwrap(),
        b"archive"
    );
    assert!(!bundle.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_local_publish_restores_the_existing_tree() {
    assert_failed_publish_restores_the_existing_tree("local-publish-rollback");
}

fn assert_failed_publish_restores_the_existing_tree(name: &str) {
    let root = temp(name);
    let target = root.join("target");
    let staging = root.join("staging");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("old"), b"old").unwrap();
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("new"), b"new").unwrap();
    let mut calls = 0;
    let error = publish_staged_tree_with(&staging, &target, &mut |from, to| {
        calls += 1;
        if calls == 2 {
            return Err(std::io::Error::other("injected publish failure"));
        }
        fs::rename(from, to)
    })
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert_eq!(fs::read(target.join("old")).unwrap(), b"old");
    assert!(!staging.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn backend_choice_selects_cuda_when_hardware_qualifies_and_a_pin_exists() {
    let root = temp("backend-trust");
    let key = pins::platform_key();
    let Some((_, digest, _)) = pins::cuda_pin(&key) else {
        return;
    };
    let probe: crate::NvidiaProbe = serde_json::from_value(json!({
        "schema": NVIDIA_PROBE_SCHEMA,
        "detected": true,
        "gpu_index": 0,
        "gpu_name": "test GPU",
        "compute_cap": "8.6",
        "arch": "sm_86",
        "driver_cuda_major": 13,
        "vram_mib": 1024,
        "unified_memory_mib": null,
        "probe_error": null,
    }))
    .unwrap();
    // First install: pin exists, artifact not yet downloaded. Install must
    // still select CUDA so the published runtime is fetched.
    let unpublished_locally = local_backend_choice(&root, Some(probe.clone()));
    assert_eq!(unpublished_locally.backend, crate::Backend::Cuda);
    assert_eq!(
        unpublished_locally.reason,
        "compute_cap sm_86 covered; driver CUDA 13 >= 13"
    );
    let artifact = pins::cache_root(&root)
        .join("cuda")
        .join(&key)
        .join(digest)
        .join("llama-server");
    fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    fs::write(&artifact, b"sm_86 sm_89 sm_120a sm_121a").unwrap();
    let trusted = local_backend_choice(&root, Some(probe.clone()));
    assert_eq!(trusted.backend, crate::Backend::Cuda);
    fs::write(&artifact, b"sm_90 only").unwrap();
    let uncovered = local_backend_choice(&root, Some(probe));
    assert_eq!(uncovered.backend, crate::Backend::Vulkan);
    assert!(
        uncovered
            .reason
            .contains("CUDA runtime artifact does not cover this GPU"),
        "{}",
        uncovered.reason
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn legacy_cuda_cleanup_requires_the_original_validated_sidecar_shape() {
    let root = temp("legacy-cleanup");
    let keep = root.join("keep");
    fs::create_dir_all(&keep).unwrap();
    let valid = root.join("a".repeat(64));
    fs::create_dir_all(&valid).unwrap();
    fs::write(
        valid.join(".oci-install.json"),
        json!({
            "image_ref": format!("ghcr.io/example@sha256:{}", valid.file_name().unwrap().to_string_lossy()),
            "arch": "amd64",
            "files": {"llama-server": "b".repeat(64)},
        })
        .to_string(),
    )
    .unwrap();
    let invalid = root.join("c".repeat(64));
    fs::create_dir_all(&invalid).unwrap();
    fs::write(invalid.join(".oci-install.json"), "{}").unwrap();
    cleanup_legacy_cuda_oci_dirs(&root, &keep);
    assert!(!valid.exists());
    assert!(invalid.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn manifest_proof_rejects_malformed_json_and_escaping_inventory_paths() {
    let root = temp("manifest-proof");
    let path = root.join("manifest.json");
    fs::write(&path, "{").unwrap();
    assert_eq!(
        manifest::prove_manifest(&path, &json!({}))["reason_code"],
        "manifest_malformed"
    );
    fs::write(
        &path,
        json!({"source":{"pin_identity":{}},"inventory":[{"relative_path":"../escape","sha256":"00"}]}).to_string(),
    )
    .unwrap();
    assert_eq!(
        manifest::prove_manifest(&path, &json!({}))["reason_code"],
        "inventory_malformed"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn inventory_for_tree_excludes_provider_manifest() {
    let root = temp("inventory-excludes-manifest");
    fs::write(root.join("payload.bin"), b"payload").unwrap();
    fs::write(
        root.join(manifest::MANIFEST_NAME),
        b"existing provider manifest",
    )
    .unwrap();

    let inventory = manifest::inventory_for_tree(&root, "model").unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0]["relative_path"], "payload.bin");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn manifest_model_rewrite_proves_after_existing_manifest() {
    let root = temp("manifest-model-rewrite");
    let manifest_path = manifest::artifact_manifest_path(&root);
    let identity = json!({"unit":"local-model","model_id":"test"});
    fs::write(root.join("payload.bin"), b"payload").unwrap();
    let request = || {
        json!({
            "root": root,
            "manifest_path": manifest_path,
            "target_fingerprint_sha256": "target",
            "pin_identity": identity,
        })
    };

    dispatch(InstallVerb::ManifestModel, request()).unwrap();
    dispatch(InstallVerb::ManifestModel, request()).unwrap();

    assert_eq!(
        manifest::prove_manifest(&manifest_path, &identity)["reason_code"],
        "ready"
    );
    let _ = fs::remove_dir_all(root);
}

fn write_unsafe_tar(path: &std::path::Path, member: &str) {
    let file = fs::File::create(path).unwrap();
    let mut encoder = GzEncoder::new(file, Compression::default());
    let mut header = [0_u8; 512];
    header[..member.len()].copy_from_slice(member.as_bytes());
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..136].copy_from_slice(b"00000000003\0");
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    let checksum_text = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(checksum_text.as_bytes());
    encoder.write_all(&header).unwrap();
    encoder.write_all(b"bad").unwrap();
    encoder.write_all(&[0_u8; 509]).unwrap();
    encoder.write_all(&[0_u8; 1024]).unwrap();
    encoder.finish().unwrap();
}

fn write_link_escape_tar(path: &std::path::Path, kind: tar::EntryType) {
    let file = fs::File::create(path).unwrap();
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_link_name("../../outside").unwrap();
    header.set_cksum();
    builder
        .append_data(&mut header, "safe/link", std::io::empty())
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn digest_mismatch_leaves_destination_unchanged() {
    let root = temp("digest");
    let artifact = root.join("artifact");
    fs::write(&artifact, b"known bytes").unwrap();
    let before = fs::read(&artifact).unwrap();
    assert!(matches!(
        archive::verify_sha256(&artifact, "00"),
        Err(archive::ArchiveError::DigestMismatch)
    ));
    assert_eq!(fs::read(&artifact).unwrap(), before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn every_flipped_catalog_unit_contacts_only_its_origin_for_each_failure_class() {
    const ORIGIN_BASE: &str = "https://updates.solstone.app";
    const ORIGIN_HOST: &str = "updates.solstone.app";
    let artifacts = flipped_origin_artifacts();
    assert!(!artifacts.is_empty());
    for artifact in &artifacts {
        let origin = archive::origin_url(ORIGIN_BASE, artifact.origin_key);
        assert_eq!(origin, format!("{ORIGIN_BASE}/{}", artifact.origin_key));
        assert!(
            origin.contains(ORIGIN_HOST),
            "origin url {} must retain host {ORIGIN_HOST}",
            origin
        );
    }
    assert_only_origin_host(BTreeSet::from([ORIGIN_HOST.to_owned()]), ORIGIN_HOST);
    assert!(!contacted_only_origin_host(
        &BTreeSet::from([ORIGIN_HOST.to_owned(), "localhost".to_owned()]),
        ORIGIN_HOST,
    ));
}

#[test]
fn origin_contact_assertion_rejects_a_test_double_upstream_fallback() {
    let contacted = BTreeSet::from(["127.0.0.1".to_owned(), "localhost".to_owned()]);
    assert!(
        !contacted_only_origin_host(&contacted, "127.0.0.1"),
        "the host-set assertion must reject fallback"
    );
}

#[test]
fn download_artifact_refuses_userinfo_url_with_distinct_envelope_reason() {
    let artifact = fixture_artifact("https://github.com/upstream".to_owned(), "artifact", b"");
    let policy = archive::DownloadHostPolicy {
        allowed_hosts: &["blocked.test"],
        allow_http: true,
        origin_base_url: "http://127.0.0.1:1@blocked.test",
    };
    let root = temp("download-userinfo");
    let destination = root.join("artifact");
    let error = download_artifact(
        &artifact,
        &destination,
        &policy,
        |_received, _total| {},
        "download_failed",
    )
    .unwrap_err();
    let error = error.envelope.error.unwrap();
    assert_eq!(error.reason_code, "download_url_userinfo_refused");
    assert!(error.message.contains("userinfo"));
    assert!(!destination.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cuda_trust_handles_matching_missing_and_unreadable() {
    let root = temp("trust");
    let artifact = root.join("runtime");
    fs::write(&artifact, b"sm_86 sm_89 sm_120a sm_121a").unwrap();
    assert_eq!(
        manifest::cuda_trust(&artifact, &["sm_86".to_owned()])["trust"],
        "trusted"
    );
    assert_eq!(
        manifest::cuda_trust(&artifact, &["sm_90".to_owned()])["trust"],
        "absent"
    );
    assert_eq!(
        manifest::cuda_trust(&root.join("missing"), &["sm_86".to_owned()])["trust"],
        "unavailable"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pin_tables_cover_every_pinned_platform() {
    assert_eq!(pins::LLAMA_SERVER_PINS.len(), 3);
    assert_eq!(pins::CUDA_ARTIFACTS.len(), 2);
    let root = std::path::Path::new("/journal");
    for (key, release, _filename, digest, binary) in pins::LLAMA_SERVER_PINS {
        let paths = pins::paths(root, key, Some("local/qwen3.5-4b"));
        assert_eq!(
            paths["binary_path"],
            format!("/journal/cache/providers/local/bin/{key}/{release}/{binary}")
        );
        assert_eq!(
            paths["model_dir"],
            "/journal/cache/providers/local/models/local__qwen3.5-4b"
        );
        assert_eq!(pins::vulkan_identity(key).unwrap()["sha256"], *digest);
    }
    for (key, _url, digest, _size) in pins::CUDA_ARTIFACTS {
        let paths = pins::paths(root, key, None);
        assert_eq!(
            paths["cuda_binary_path"],
            format!("/journal/cache/providers/local/cuda/{key}/{digest}/llama-server")
        );
        assert_eq!(pins::cuda_identity(key).unwrap()["sha256"], *digest);
    }
}

#[test]
fn parakeet_pin_tables_cover_every_pinned_platform_and_backend() {
    assert_eq!(pins::PARAKEET_VULKAN_PINS.len(), 2);
    assert_eq!(pins::PARAKEET_CPU_PINS.len(), 2);
    let root = std::path::Path::new("/journal");
    for (backend, table) in [
        ("vulkan", pins::PARAKEET_VULKAN_PINS),
        ("cpu", pins::PARAKEET_CPU_PINS),
    ] {
        for (key, release, _filename, digest, binary) in table {
            let paths = pins::parakeet_paths(root, key);
            assert_eq!(
                paths[format!("binary_path_{backend}")],
                format!("/journal/cache/providers/parakeet/bin/{key}/{backend}/{release}/{binary}")
            );
            assert_eq!(
                pins::parakeet_backend_identity(key, backend).unwrap()["sha256"],
                *digest
            );
        }
    }
    let (repo, filename, revision, sha256, size_bytes) = pins::PARAKEET_MODEL;
    assert_eq!(
        pins::parakeet_paths(root, "x86_64-unknown-linux-gnu")["model_path"],
        format!(
            "/journal/cache/providers/parakeet/models/{}/{revision}/{filename}",
            repo.replace('/', "__")
        )
    );
    let model = pins::parakeet_model_identity();
    assert_eq!(model["repo"], repo);
    assert_eq!(model["sha256"], sha256);
    // The size stays PINNED -- it is what the fetch primitive refuses a length
    // mismatch against -- it is just not part of the RECORDED identity, because
    // the reference's manifests do not carry it.
    assert_eq!(size_bytes, 940_663_680);
    assert!(model.get("size_bytes").is_none());
}

#[test]
fn parakeet_backend_pin_is_none_for_an_unknown_backend_or_key() {
    assert!(pins::parakeet_backend_pin("x86_64-unknown-linux-gnu", "cuda").is_none());
    assert!(pins::parakeet_backend_pin("aarch64-apple-darwin", "vulkan").is_none());
    assert!(pins::parakeet_backend_identity("x86_64-unknown-linux-gnu", "cuda").is_none());
}

#[test]
fn registry_binds_existing_pins_and_the_parakeet_model_pin() {
    for (key, release, filename, sha256, _) in pins::LLAMA_SERVER_PINS {
        let row = resolve(
            "llama-server-vulkan",
            match *key {
                "aarch64-apple-darwin" => Some(Platform::MacosArm64),
                "x86_64-unknown-linux-gnu" => Some(Platform::LinuxX64),
                "aarch64-unknown-linux-gnu" => Some(Platform::LinuxArm64),
                _ => unreachable!(),
            },
            None,
        );
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].artifact_key, Some(*key));
        assert_eq!(row[0].version, *release);
        assert_eq!(row[0].filename, *filename);
        assert_eq!(row[0].sha256, *sha256);
    }
    for (key, url, sha256, size_bytes) in pins::CUDA_ARTIFACTS {
        let row = resolve(
            "llama-server-cuda",
            if key.starts_with("x86_64") {
                Some(Platform::LinuxX64)
            } else {
                Some(Platform::LinuxArm64)
            },
            None,
        );
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].artifact_key, Some(*key));
        assert_eq!(row[0].upstream_url, *url);
        assert_eq!(row[0].sha256, *sha256);
        assert_eq!(row[0].size_bytes, *size_bytes);
    }
    for (backend, table) in [
        (Backend::Vulkan, pins::PARAKEET_VULKAN_PINS),
        (Backend::Cpu, pins::PARAKEET_CPU_PINS),
    ] {
        for (key, release, filename, sha256, _) in table {
            let platform = if key.starts_with("x86_64") {
                Platform::LinuxX64
            } else {
                Platform::LinuxArm64
            };
            let row = resolve("parakeet-server", Some(platform), Some(backend));
            assert_eq!(row.len(), 1);
            assert_eq!(row[0].artifact_key, Some(*key));
            assert_eq!(row[0].version, *release);
            assert_eq!(row[0].filename, *filename);
            assert_eq!(row[0].sha256, *sha256);
        }
    }

    let expected = "4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757";
    let row = resolve("parakeet-model", None, None);
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].sha256, expected);
    assert_eq!(pins::PARAKEET_MODEL.3, expected);

    for (key, _, _, _) in pins::CUDA_ARTIFACTS {
        assert_eq!(
            super::select_artifact(
                "llama-server-cuda",
                Some(super::artifact_platform(key).unwrap()),
                None,
                Some(key),
                None,
            )
            .unwrap()
            .artifact_key,
            Some(*key)
        );
    }
    for (key, _, _, _, _) in pins::LLAMA_SERVER_PINS {
        assert_eq!(
            super::select_artifact(
                "llama-server-vulkan",
                Some(super::artifact_platform(key).unwrap()),
                None,
                Some(key),
                None,
            )
            .unwrap()
            .artifact_key,
            Some(*key)
        );
    }
    for (backend, table) in [
        (Backend::Vulkan, pins::PARAKEET_VULKAN_PINS),
        (Backend::Cpu, pins::PARAKEET_CPU_PINS),
    ] {
        for (key, _, filename, _, _) in table {
            assert_eq!(
                super::select_artifact(
                    "parakeet-server",
                    Some(super::artifact_platform(key).unwrap()),
                    Some(backend),
                    Some(key),
                    Some(filename),
                )
                .unwrap()
                .filename,
                *filename
            );
        }
    }
    assert_eq!(
        super::select_artifact(
            "parakeet-model",
            None,
            None,
            None,
            Some(pins::PARAKEET_MODEL.1),
        )
        .unwrap()
        .filename,
        pins::PARAKEET_MODEL.1
    );
    let local_identity = pins::model_identity("local/qwen3.5-4b").unwrap();
    for filename in [
        local_identity["filename"].as_str().unwrap(),
        local_identity["mmproj_filename"].as_str().unwrap(),
    ] {
        let artifact =
            super::select_artifact("local-model", None, None, None, Some(filename)).unwrap();
        assert!(
            artifact
                .upstream_url
                .contains("e87f176479d0855a907a41277aca2f8ee7a09523")
        );
        assert!(!artifact.upstream_url.contains("/main/"));
    }
}

#[test]
fn registry_preserves_prechange_identity_literals() {
    let literals = [
        (
            fingerprint::canonical(pins::vulkan_identity("x86_64-unknown-linux-gnu").unwrap())
                .unwrap(),
            "{\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"filename\":\"llama-b10068-bin-ubuntu-vulkan-x64.tar.gz\",\"release_tag\":\"b10068\",\"sha256\":\"713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3\",\"unit\":\"llama-server-vulkan\"}",
        ),
        (
            fingerprint::canonical(pins::cuda_identity("x86_64-unknown-linux-gnu").unwrap())
                .unwrap(),
            "{\"arch\":\"amd64\",\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"llama_cpp_revision\":\"571d0d540df04f25298d0e159e520d9fc62ed121\",\"release_tag\":\"b10068\",\"repack_revision\":\"sol1\",\"sha256\":\"3727630e6ac79953f5c652fddcfd7100da98c55d773c0aec115a55f40f3aafea\",\"size_bytes\":550238443,\"unit\":\"llama-server-cuda\",\"upstream_image_digest\":\"sha256:5bd5290bd35cfde893d0dcbd9811723c16d89575927d537b5f21becbfbab2f63\",\"url\":\"https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz\",\"wanted_files\":[\"libcublas.so.13\",\"libcublasLt.so.13\",\"libcudart.so.13\",\"libggml-base.so.0\",\"libggml-cpu-alderlake.so\",\"libggml-cpu-cannonlake.so\",\"libggml-cpu-cascadelake.so\",\"libggml-cpu-cooperlake.so\",\"libggml-cpu-haswell.so\",\"libggml-cpu-icelake.so\",\"libggml-cpu-ivybridge.so\",\"libggml-cpu-piledriver.so\",\"libggml-cpu-sandybridge.so\",\"libggml-cpu-sapphirerapids.so\",\"libggml-cpu-skylakex.so\",\"libggml-cpu-sse42.so\",\"libggml-cpu-x64.so\",\"libggml-cpu-zen4.so\",\"libggml-cuda.so\",\"libggml.so.0\",\"libllama-common.so.0\",\"libllama-server-impl.so\",\"libllama.so.0\",\"libmtmd.so.0\",\"llama-server\"]}",
        ),
        (
            fingerprint::canonical(pins::model_identity("local/qwen3.5-4b").unwrap()).unwrap(),
            "{\"filename\":\"Qwen3.5-4B-Q4_K_M.gguf\",\"mmproj_filename\":\"mmproj-F16.gguf\",\"mmproj_sha256\":\"cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864\",\"model_id\":\"local/qwen3.5-4b\",\"repo\":\"unsloth/Qwen3.5-4B-GGUF\",\"revision\":\"main\",\"sha256\":\"00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4\",\"unit\":\"local-model\"}",
        ),
        (
            fingerprint::canonical(
                pins::parakeet_backend_identity("x86_64-unknown-linux-gnu", "cpu").unwrap(),
            )
            .unwrap(),
            "{\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"backend\":\"cpu\",\"binary_name\":\"parakeet-server\",\"filename\":\"parakeet-v0.5.0-bin-linux-cpu-x64.tar.gz\",\"release_tag\":\"v0.5.0\",\"sha256\":\"636a9fc48ac023096037790f9b77d7e5043b200dd6399ec0438bd648c35d79b9\",\"unit\":\"parakeet-server\"}",
        ),
        (
            fingerprint::canonical(pins::parakeet_model_identity()).unwrap(),
            "{\"filename\":\"tdt-0.6b-v3-q8_0.gguf\",\"repo\":\"mudler/parakeet-cpp-gguf\",\"revision\":\"bf0af9f425fa01809cadec671b3cb672709d13e9\",\"sha256\":\"4d69a4a6683f4f2d952bad794c1357ca6eb628027695b4699c5a9ad4cd07d757\",\"unit\":\"parakeet-model\"}",
        ),
    ];
    for (actual, expected) in literals {
        assert_eq!(actual, expected);
    }
}

#[test]
fn registry_path_fixtures_keep_directory_and_manifest_filename_distinct() {
    let journal = std::path::Path::new("/journal");
    let key = "x86_64-unknown-linux-gnu";
    let local_paths = pins::paths(journal, key, Some("local/qwen3.5-4b"));
    assert_eq!(
        local_paths["binary_path"],
        "/journal/cache/providers/local/bin/x86_64-unknown-linux-gnu/b10068/llama-server"
    );
    assert_eq!(
        local_paths["cuda_binary_path"],
        "/journal/cache/providers/local/cuda/x86_64-unknown-linux-gnu/3727630e6ac79953f5c652fddcfd7100da98c55d773c0aec115a55f40f3aafea/llama-server"
    );
    let model_dir = PathBuf::from(local_paths["model_dir"].as_str().unwrap());
    assert_eq!(
        model_dir,
        PathBuf::from("/journal/cache/providers/local/models/local__qwen3.5-4b")
    );
    let local_readiness = readiness::inspect_local(
        serde_json::from_value(json!({
            "journal": journal,
            "model_id": "local/qwen3.5-4b",
            "artifact_key": key,
            "nvidia_probe": {
                "schema": NVIDIA_PROBE_SCHEMA,
                "detected": false,
                "gpu_index": null,
                "gpu_name": null,
                "compute_cap": null,
                "arch": null,
                "driver_cuda_major": null,
                "vram_mib": null,
                "unified_memory_mib": null,
                "probe_error": "test override"
            }
        }))
        .unwrap(),
    );
    assert_eq!(
        local_readiness["artifacts"]["binary_path"],
        "/journal/cache/providers/local/bin/x86_64-unknown-linux-gnu/b10068/llama-server"
    );
    assert_eq!(
        local_readiness["artifacts"]["model_path"],
        "/journal/cache/providers/local/models/local__qwen3.5-4b/Qwen3.5-4B-Q4_K_M.gguf"
    );
    assert_eq!(
        local_readiness["artifacts"]["projector_path"],
        "/journal/cache/providers/local/models/local__qwen3.5-4b/mmproj-F16.gguf"
    );
    // `install_model` joins each pin filename inline immediately before its
    // network fetch; `pins::paths` above is its production root derivation.
    let local_identity = pins::model_identity("local/qwen3.5-4b").unwrap();
    assert_eq!(
        model_dir.join(local_identity["filename"].as_str().unwrap()),
        PathBuf::from(
            "/journal/cache/providers/local/models/local__qwen3.5-4b/Qwen3.5-4B-Q4_K_M.gguf"
        )
    );
    assert_eq!(
        model_dir.join(local_identity["mmproj_filename"].as_str().unwrap()),
        PathBuf::from("/journal/cache/providers/local/models/local__qwen3.5-4b/mmproj-F16.gguf")
    );
    let parakeet = pins::parakeet_paths(journal, key);
    assert_eq!(
        parakeet["binary_path_cpu"],
        "/journal/cache/providers/parakeet/bin/x86_64-unknown-linux-gnu/cpu/v0.5.0/parakeet-server"
    );
    assert_eq!(
        parakeet["model_path"],
        "/journal/cache/providers/parakeet/models/mudler__parakeet-cpp-gguf/bf0af9f425fa01809cadec671b3cb672709d13e9/tdt-0.6b-v3-q8_0.gguf"
    );
}

#[test]
fn registry_binds_local_model_artifacts_without_mutating_manifest_identity() {
    let identity = pins::model_identity("local/qwen3.5-4b").unwrap();
    let rows = resolve("local-model", None, None);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| {
        identity["filename"].as_str() == Some(row.filename)
            && identity["sha256"].as_str() == Some(row.sha256)
    }));
    assert!(rows.iter().any(|row| {
        identity["mmproj_filename"].as_str() == Some(row.filename)
            && identity["mmproj_sha256"].as_str() == Some(row.sha256)
    }));
    assert_eq!(identity["revision"], "main");
    assert_eq!(
        rows.iter()
            .map(|row| row.size_bytes)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([672423616, 2740937888])
    );
    assert_eq!(
        catalog()
            .iter()
            .filter(|row| row.unit == "local-model")
            .count(),
        2
    );
}

#[test]
fn prechange_local_model_manifest_still_proves_ready() {
    let root = temp("prechange-local-model-manifest");
    fs::write(root.join("Qwen3.5-4B-Q4_K_M.gguf"), b"fixture model").unwrap();
    let identity = pins::model_identity("local/qwen3.5-4b").unwrap();
    let built = manifest::build_manifest(
        "local",
        "local-model",
        "target",
        json!({"pin_identity": identity}),
        manifest::inventory_for_tree(&root, "model").unwrap(),
        None,
        None,
    )
    .unwrap();
    let path = manifest::artifact_manifest_path(&root);
    manifest::write_manifest(&path, &built).unwrap();
    assert_eq!(
        manifest::prove_manifest(&path, &identity)["status"],
        "ready"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn parakeet_artifact_key_matches_every_python_alias() {
    for (arch, expected) in [
        ("amd64", "x86_64-unknown-linux-gnu"),
        ("x64", "x86_64-unknown-linux-gnu"),
        ("x86_64", "x86_64-unknown-linux-gnu"),
        ("AMD64", "x86_64-unknown-linux-gnu"),
        ("arm64", "aarch64-unknown-linux-gnu"),
        ("aarch64", "aarch64-unknown-linux-gnu"),
        ("ARM64", "aarch64-unknown-linux-gnu"),
    ] {
        assert_eq!(
            pins::parakeet_artifact_key("linux", arch).unwrap(),
            expected,
            "arch={arch}"
        );
    }
}

#[test]
fn delegated_parakeet_target_uses_the_supplied_platform() {
    let root = temp("delegated-parakeet-platform");
    let target = parakeet_target_for_install(&root, Some(("linux", "arm64"))).unwrap();
    assert_eq!(target["artifact_key"], "aarch64-unknown-linux-gnu");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn parakeet_artifact_key_refuses_non_linux_and_unrecognized_arch() {
    for (os_name, arch) in [
        ("macos", "arm64"),
        ("windows", "x86_64"),
        ("darwin", "amd64"),
    ] {
        let error = pins::parakeet_artifact_key(os_name, arch).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("parakeet-cpp is unsupported on {os_name}/{arch}")
        );
    }
    let error = pins::parakeet_artifact_key("linux", "riscv64").unwrap_err();
    assert_eq!(
        error.to_string(),
        "parakeet-cpp is unsupported on linux/riscv64"
    );
}

#[test]
fn progress_writes_are_coalesced_until_the_window_elapses() {
    let root = temp("progress");
    let mut state = status::idle_status("local");
    state.target_fingerprint_json = Some("{}".to_owned());
    state.target_fingerprint_sha256 = Some("x".to_owned());
    let state = status::write_status(
        &root,
        status::transition(state, "downloading", None, None).unwrap(),
    )
    .unwrap();
    let mut clock = Instant::now();
    assert!(
        status::bump_progress(state.clone(), Some(1), None, &mut clock)
            .unwrap()
            .is_none()
    );
    assert!(
        status::bump_progress(state.clone(), Some(2), None, &mut clock)
            .unwrap()
            .is_none()
    );
    clock = Instant::now() - Duration::from_secs(2);
    assert!(
        status::bump_progress(state, Some(3), None, &mut clock)
            .unwrap()
            .is_some()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn status_write_is_atomic_and_revisioned() {
    let root = temp("status");
    let mut state = status::idle_status("local");
    state.target_fingerprint_json = Some("{}".to_owned());
    state.target_fingerprint_sha256 = Some("x".to_owned());
    let state = status::transition(state, "downloading", None, None).unwrap();
    let written = status::write_status(&root, state).unwrap();
    assert_eq!(written.revision, 1);
    let on_disk: Value =
        serde_json::from_slice(&fs::read(status::status_path(&root, "local")).unwrap()).unwrap();
    assert_eq!(on_disk["revision"], 1);
    let _ = fs::remove_dir_all(root);
}
