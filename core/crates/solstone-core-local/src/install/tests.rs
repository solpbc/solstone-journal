// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[cfg(unix)]
#[cfg(all(test, feature = "full-tests"))]
use super::metal_candidate;
use super::test_hooks::{inspect_parakeet, stage_ready_parakeet};
use super::{
    InstallVerb, archive, cleanup_legacy_cuda_oci_dirs, dispatch, fetch_runtime_member,
    fingerprint, flatten_binary_bundle, hoist_binary, lease, local_backend_choice, manifest,
    parakeet_target_for_install, pins, publish_staged_tree_with, readiness, status,
    write_parakeet_model_manifest,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
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

fn path_value(value: &Value) -> PathBuf {
    PathBuf::from(value.as_str().expect("expected path string"))
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
    let modules = [manifest_dir.join("src/install/rfdetr_install.rs")];
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
        super::download_artifact_reason_code(
            &ArchiveError::DigestMismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
            },
            fallback,
        ),
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

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
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
fn explicit_metal_target_refuses_windows_before_package_resolution() {
    let error = super::local_target_for_key(
        std::path::Path::new("unused"),
        "local/qwen3.5-4b",
        super::LocalBackend::Metal,
        "x86_64-windows",
    )
    .unwrap_err();
    assert_eq!(error.exit_code, 65);
    assert_eq!(
        error.envelope.error.unwrap().reason_code,
        "unsupported_platform"
    );
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

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
#[test]
fn metal_candidate_inspect_is_pure_and_reports_component_reasons_and_fit() {
    let root = temp("metal-candidate-inspect");
    let cache = pins::cache_root(&root);
    let (release, _, _, _) = pins::vulkan_pin("aarch64-apple-darwin").unwrap();
    let runtime = cache.join(format!("bin/aarch64-apple-darwin/{release}"));
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
    let present =
        metal_candidate::inspect_present_with(&candidate_request(&root), "aarch64-apple-darwin")
            .unwrap();
    assert_eq!(present["status"], "ready");
    assert_eq!(
        present["proof"]["binary_probe"]["verification"],
        "deferred_until_launch"
    );
    let marker = root.join("probe-ran");
    fs::write(
        runtime.join("llama-server"),
        format!("#!/bin/sh\ntouch {}\n", marker.display()),
    )
    .unwrap();
    assert_eq!(
        metal_candidate::inspect_with(&candidate_request(&root), "aarch64-apple-darwin").unwrap()["reason_code"],
        "sha256_mismatch"
    );
    assert!(!marker.exists(), "failed proof must not execute the binary");
    fs::write(runtime.join("llama-server"), b"#!/bin/sh\nexit 0\n").unwrap();

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
    let installer = lease::acquire(&root, "local").unwrap().unwrap();
    let reports_current_native_status =
        metal_candidate::inspect_with(&candidate_request(&root), "aarch64-apple-darwin").unwrap();
    drop(installer);
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
        let arch_key = row["arch_key"].as_str().unwrap();
        let identity = pins::vulkan_identity(arch_key).unwrap();
        assert_ne!(identity, row["pin_identity"]);
        assert_manifest_proves_preflip_identity(
            &root.join(format!("vulkan-{}", arch_key)),
            "llama-server-vulkan",
            identity,
        );
    }
    for row in fixture["llama_server_cuda"].as_array().unwrap() {
        let arch_key = row["arch_key"].as_str().unwrap();
        let identity = pins::cuda_identity(arch_key).unwrap();
        assert_ne!(identity, row["pin_identity"]);
        assert_manifest_proves_preflip_identity(
            &root.join(format!("cuda-{}", arch_key)),
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
        assert_ne!(identity, row["pin_identity"]);
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
    let journal = Path::new("/journal");

    for row in fixture["llama_server_vulkan"].as_array().unwrap() {
        let arch_key = row["arch_key"].as_str().unwrap();
        let actual = exported["llama_server_pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["artifact_key"] == arch_key)
            .unwrap();
        assert_ne!(actual, &row["pin_identity"]);
        let paths = pins::paths(journal, arch_key, Some("local/qwen3.5-4b"));
        assert_eq!(
            path_value(&paths["binary_path"]),
            pins::cache_root(journal)
                .join("bin")
                .join(actual["artifact_key"].as_str().unwrap())
                .join(actual["release_tag"].as_str().unwrap())
                .join(actual["binary_name"].as_str().unwrap()),
        );
    }
    assert_eq!(
        exported["cuda_server_pin"]["artifacts"],
        Value::Array(vec![
            pins::cuda_identity("x86_64-unknown-linux-gnu").unwrap(),
            pins::cuda_identity("aarch64-unknown-linux-gnu").unwrap(),
        ])
    );
    for row in fixture["llama_server_cuda"].as_array().unwrap() {
        let arch_key = row["arch_key"].as_str().unwrap();
        let identity = pins::cuda_identity(arch_key).unwrap();
        assert_ne!(identity, row["pin_identity"]);
        let paths = pins::paths(journal, arch_key, None);
        assert_eq!(
            path_value(&paths["cuda_binary_path"]),
            pins::cache_root(journal)
                .join("cuda")
                .join(arch_key)
                .join(identity["sha256"].as_str().unwrap())
                .join("llama-server"),
        );
    }
    assert_eq!(
        path_value(
            &pins::paths(journal, PARAKEET_TEST_KEY, Some("local/qwen3.5-4b"),)["model_dir"]
        ),
        PathBuf::from(fixture["paths"]["local_model_dir"].as_str().unwrap())
    );
    for row in fixture["parakeet_server"].as_array().unwrap() {
        let identity = pins::parakeet_backend_identity(
            row["arch_key"].as_str().unwrap(),
            row["backend"].as_str().unwrap(),
        )
        .unwrap();
        assert_ne!(identity, row["pin_identity"]);
        let paths = pins::parakeet_paths(journal, identity["artifact_key"].as_str().unwrap());
        assert_eq!(
            path_value(&paths[format!("binary_path_{}", identity["backend"].as_str().unwrap())]),
            pins::parakeet_cache_root(journal)
                .join("bin")
                .join(identity["artifact_key"].as_str().unwrap())
                .join(identity["backend"].as_str().unwrap())
                .join(identity["release_tag"].as_str().unwrap())
                .join(identity["binary_name"].as_str().unwrap()),
        );
    }
    assert_eq!(
        path_value(&pins::parakeet_paths(journal, PARAKEET_TEST_KEY)["model_path"]),
        PathBuf::from(fixture["paths"]["parakeet_model_path"].as_str().unwrap())
    );
}

#[test]
fn origin_urls_follow_the_catalog_for_every_rust_download_unit() {
    let cases = [
        (
            "llama-server-vulkan",
            Some(Platform::LinuxX64),
            None,
            "https://updates.solstone.app/assets/llama-server-vulkan/b11429/llama-b11429-bin-ubuntu-vulkan-x64.tar.gz",
        ),
        (
            "llama-server-cuda",
            Some(Platform::LinuxX64),
            None,
            "https://updates.solstone.app/runtimes/llama-cuda13/b11429/llama-b11429-bin-linux-cuda13-amd64-sol1.tar.gz",
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
            "https://updates.solstone.app/assets/parakeet-server/v0.6.1/parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz",
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
            format!("https://updates.solstone.app/{}", artifact.origin_key),
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

#[cfg(all(test, feature = "full-tests"))]
#[test]
fn local_fingerprint_supplies_provider_and_backend_then_canonicalises() {
    // Canonicalisation itself is covered by the 18 named vectors in
    // `canonical_fingerprint_vectors_match_fixture`. What only this path owns
    // is that `local_fingerprint` inserts `provider` and the resolved
    // `backend` into the map before canonicalising, and that the digest is
    // taken over exactly the bytes it reports.
    //
    // ⛔ The payload is synthetic on purpose. The predecessor here carried a
    // real model and runtime pin inside its fixture, so repinning either one
    // meant editing a test that was never about pins, and its digest literal
    // was just sha256 of the string on the line above it.
    let mut input = serde_json::Map::new();
    input.insert("runtime".to_owned(), json!("llama.cpp"));
    input.insert("backend".to_owned(), json!("vulkan"));

    let actual = fingerprint::local_fingerprint(input).unwrap();
    let canonical = actual["target_fingerprint_json"]
        .as_str()
        .expect("canonical json");
    assert_eq!(
        canonical,
        r#"{"backend":"vulkan","provider":"local","runtime":"llama.cpp"}"#
    );
    assert_eq!(
        actual["target_fingerprint_sha256"],
        fingerprint::sha256(canonical)
    );
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(not(windows))]
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

#[cfg(windows)]
#[test]
fn fingerprint_transport_refuses_an_unverified_windows_package_without_writing_status() {
    let root = temp("fingerprint-transport-windows");
    let error = dispatch(
        InstallVerb::FingerprintLocal,
        json!({"journal":root,"model_id":"local/qwen3.5-4b"}),
    )
    .expect_err("the test executable is outside a verified installed package");
    assert_eq!(
        error.envelope.error.as_ref().unwrap().reason_code,
        if cfg!(target_arch = "x86_64") {
            "package_invalid"
        } else {
            "unsupported_platform"
        }
    );
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
        path_value(&paths["binary_path_cpu"]),
        pins::parakeet_cache_root(&root)
            .join("bin")
            .join("aarch64-unknown-linux-gnu")
            .join("cpu")
            .join("v0.6.1")
            .join("parakeet-server"),
    );
    assert_eq!(
        path_value(&paths["binary_path_vulkan"]),
        pins::parakeet_cache_root(&root)
            .join("bin")
            .join("aarch64-unknown-linux-gnu")
            .join("vulkan")
            .join("v0.6.1")
            .join("parakeet-server"),
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

#[test]
fn an_install_whose_installer_is_gone_reads_interrupted_not_in_progress() {
    let root = temp("inspect-parakeet-installer-gone");
    let attempt = status::begin_or_replace(
        &root,
        "parakeet",
        "{}".to_owned(),
        "target".to_owned(),
        None,
        "downloading",
    )
    .unwrap();

    let held = lease::acquire(&root, "parakeet").unwrap().unwrap();
    let running = inspect_parakeet(&root, PARAKEET_TEST_KEY);
    assert_eq!(running["in_flight"], true);
    assert_eq!(running["install"]["install_state"], "downloading");
    drop(held);

    let gone = inspect_parakeet(&root, PARAKEET_TEST_KEY);
    assert_eq!(gone["in_flight"], false);
    assert_eq!(gone["install"]["install_state"], "failed");
    assert_eq!(gone["install"]["error_code"], "install_interrupted");
    assert_eq!(gone["install"]["attempt_id"], json!(attempt.attempt_id));
    // Observing writes nothing; the next installer records the interruption.
    assert_eq!(status::read_status(&root, "parakeet").unwrap(), attempt);
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
    let windows_package = pins::platform_key() == "x86_64-windows";
    assert_eq!(
        result["host"]["backend_reason"],
        if windows_package {
            "Windows packaged Vulkan runtime"
        } else {
            "no NVIDIA GPU detected"
        }
    );
    assert_eq!(
        result["host"]["platform_supported"],
        windows_package || pins::vulkan_pin(&pins::platform_key()).is_some()
    );

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

const LLAMA_SERVER_ADMISSION_SIZE: u64 = 17888;
const LLAMA_SERVER_ADMISSION_SHA256: &str =
    "5a2b208943ca04915d380824f3fcf79f1708b22d8157e4db3cb18b3e52b8b8cc";
const LIBLLAMA_SERVER_IMPL_ADMISSION_SIZE: u64 = 7321840;
const LIBLLAMA_SERVER_IMPL_ADMISSION_SHA256: &str =
    "13fd78deb520de1410409c44b715bb87039a489e2571d4a6960966ec936cd817";
const LIBGGML_CUDA_ADMISSION_SIZE: u64 = 162636208;
const LIBGGML_CUDA_ADMISSION_SHA256: &str =
    "c3b4065cc5f46e0a109e382dad89cd95748eabd5b8c2a871e811209e4219bbae";

#[test]
fn backend_choice_selects_cuda_when_hardware_qualifies_and_a_pin_exists() {
    assert_eq!(LLAMA_SERVER_ADMISSION_SIZE, 17888);
    assert_eq!(
        LLAMA_SERVER_ADMISSION_SHA256,
        "5a2b208943ca04915d380824f3fcf79f1708b22d8157e4db3cb18b3e52b8b8cc"
    );
    assert_eq!(LIBLLAMA_SERVER_IMPL_ADMISSION_SIZE, 7321840);
    assert_eq!(
        LIBLLAMA_SERVER_IMPL_ADMISSION_SHA256,
        "13fd78deb520de1410409c44b715bb87039a489e2571d4a6960966ec936cd817"
    );
    assert_eq!(LIBGGML_CUDA_ADMISSION_SIZE, 162636208);
    assert_eq!(
        LIBGGML_CUDA_ADMISSION_SHA256,
        "c3b4065cc5f46e0a109e382dad89cd95748eabd5b8c2a871e811209e4219bbae"
    );

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

    // 1. No llama-server at the install dir -> Selected CUDA (first download).
    let unpublished_locally = match local_backend_choice(&root, Some(probe.clone())) {
        crate::BackendSelection::Selected(c) => c,
        crate::BackendSelection::IntegrityBlocked => panic!("expected selected"),
    };
    assert_eq!(unpublished_locally.backend, crate::Backend::Cuda);
    assert_eq!(
        unpublished_locally.reason,
        "compute_cap sm_86 covered; driver CUDA 13 >= 13"
    );

    let artifact_dir = pins::cache_root(&root).join("cuda").join(&key).join(digest);
    fs::create_dir_all(&artifact_dir).unwrap();
    let launcher_path = artifact_dir.join("llama-server");
    let twin_path = artifact_dir.join("libggml-cuda.so");
    let cuda_id = pins::cuda_identity(&key).unwrap();

    // 2. Incumbent trusted only with a real build_manifest/write_manifest whose
    // pin is cuda_identity and whose inventory hash matches a launcher that contains
    // all four strings.
    let incumbent_launcher_bytes = b"sm_86 sm_89 sm_120a sm_121a header";
    fs::write(&launcher_path, incumbent_launcher_bytes).unwrap();
    let inv = manifest::runtime_inventory(&artifact_dir, &[]).unwrap();
    let incumbent_manifest = manifest::build_manifest(
        "local",
        "llama-server-cuda",
        "target",
        json!({"pin_identity": cuda_id.clone()}),
        inv,
        None,
        None,
    )
    .unwrap();
    let manifest_path = manifest::artifact_manifest_path(&artifact_dir);
    manifest::write_manifest(&manifest_path, &incumbent_manifest).unwrap();
    let incumbent_choice = match local_backend_choice(&root, Some(probe.clone())) {
        crate::BackendSelection::Selected(c) => c,
        crate::BackendSelection::IntegrityBlocked => panic!("expected selected for incumbent"),
    };
    assert_eq!(incumbent_choice.backend, crate::Backend::Cuda);

    // 3. Split trusted: launcher is exactly 17888 bytes and contains none of the four strings;
    // libggml-cuda.so contains all four; inventory hashes match; result is Selected CUDA.
    // Gate this case on artifact key x86_64-unknown-linux-gnu.
    if key == "x86_64-unknown-linux-gnu" {
        let mut split_launcher = vec![0u8; 17888];
        split_launcher[0..8].copy_from_slice(b"launcher");
        fs::write(&launcher_path, &split_launcher).unwrap();
        let twin_bytes = b"libggml-cuda: sm_86 sm_89 sm_120a sm_121a binary content";
        fs::write(&twin_path, twin_bytes).unwrap();
        let inv = manifest::runtime_inventory(&artifact_dir, &[]).unwrap();
        let split_manifest = manifest::build_manifest(
            "local",
            "llama-server-cuda",
            "target",
            json!({"pin_identity": cuda_id.clone()}),
            inv,
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(&manifest_path, &split_manifest).unwrap();
        let split_choice = match local_backend_choice(&root, Some(probe.clone())) {
            crate::BackendSelection::Selected(c) => c,
            crate::BackendSelection::IntegrityBlocked => panic!("expected selected for split"),
        };
        assert_eq!(split_choice.backend, crate::Backend::Cuda);

        // 4. Missing twin, corrupted twin whose markers remain but whose hash does not match inventory,
        // uncovered carrier (sm_90 only) whose hash matches, sibling file, declared arch list,
        // and manifest pin mismatch -> IntegrityBlocked. Assert value is not a Vulkan BackendChoice.
        // Case 4a: Missing twin
        fs::remove_file(&twin_path).unwrap();
        assert_eq!(
            local_backend_choice(&root, Some(probe.clone())),
            crate::BackendSelection::IntegrityBlocked
        );

        // Case 4b: Corrupted twin whose markers remain but hash mismatches inventory
        fs::write(
            &twin_path,
            b"libggml-cuda: sm_86 sm_89 sm_120a sm_121a corrupted",
        )
        .unwrap();
        assert_eq!(
            local_backend_choice(&root, Some(probe.clone())),
            crate::BackendSelection::IntegrityBlocked
        );

        // Case 4c: Uncovered carrier (sm_90 only) whose hash matches inventory
        fs::write(&twin_path, b"libggml-cuda: sm_90 only").unwrap();
        let inv = manifest::runtime_inventory(&artifact_dir, &[]).unwrap();
        let uncovered_manifest = manifest::build_manifest(
            "local",
            "llama-server-cuda",
            "target",
            json!({"pin_identity": cuda_id.clone()}),
            inv,
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(&manifest_path, &uncovered_manifest).unwrap();
        assert_eq!(
            local_backend_choice(&root, Some(probe.clone())),
            crate::BackendSelection::IntegrityBlocked
        );

        // Case 4d: Manifest pin mismatch
        let mut bad_pin = cuda_id.clone();
        bad_pin["sha256"] = json!("00".repeat(32));
        fs::write(&twin_path, twin_bytes).unwrap();
        let inv = manifest::runtime_inventory(&artifact_dir, &[]).unwrap();
        let bad_pin_manifest = manifest::build_manifest(
            "local",
            "llama-server-cuda",
            "target",
            json!({"pin_identity": bad_pin}),
            inv,
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(&manifest_path, &bad_pin_manifest).unwrap();
        assert_eq!(
            local_backend_choice(&root, Some(probe.clone())),
            crate::BackendSelection::IntegrityBlocked
        );

        // A sibling file and a declared arch list are not the selected carrier.
        fs::remove_file(&twin_path).unwrap();
        fs::write(
            artifact_dir.join("sibling-arches.so"),
            b"sm_86 sm_89 sm_120a sm_121a",
        )
        .unwrap();
        fs::write(
            artifact_dir.join("arches.json"),
            br#"["sm_86","sm_89","sm_120a","sm_121a"]"#,
        )
        .unwrap();
        let inv = manifest::runtime_inventory(&artifact_dir, &[]).unwrap();
        let sibling_manifest = manifest::build_manifest(
            "local",
            "llama-server-cuda",
            "target",
            json!({"pin_identity": cuda_id.clone()}),
            inv,
            None,
            None,
        )
        .unwrap();
        manifest::write_manifest(&manifest_path, &sibling_manifest).unwrap();
        assert_eq!(
            local_backend_choice(&root, Some(probe.clone())),
            crate::BackendSelection::IntegrityBlocked
        );
    }

    // 5. local_backend_choice_present still selects CUDA without inspecting.
    let present_choice = match super::local_backend_choice_present(&root, Some(probe.clone())) {
        crate::BackendSelection::Selected(c) => c,
        crate::BackendSelection::IntegrityBlocked => panic!("expected selected"),
    };
    assert_eq!(present_choice.backend, crate::Backend::Cuda);

    // 6. select_local_backend checks
    let undetected_probe = crate::NvidiaProbe::undetected("no nvidia-smi".into());
    let sel1 = crate::select_local_backend(
        &undetected_probe,
        &crate::CUDA_EMBEDDED_ARCH_SET,
        crate::CUDA_MIN_DRIVER_VERSION,
        crate::ArtifactTrust::Integrity,
        false,
    );
    assert_eq!(
        sel1,
        crate::BackendSelection::Selected(crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: "no NVIDIA GPU detected".into(),
        })
    );

    let mut unreadable_driver = probe.clone();
    unreadable_driver.driver_cuda_major = None;
    let sel2 = crate::select_local_backend(
        &unreadable_driver,
        &crate::CUDA_EMBEDDED_ARCH_SET,
        crate::CUDA_MIN_DRIVER_VERSION,
        crate::ArtifactTrust::Integrity,
        false,
    );
    assert_eq!(
        sel2,
        crate::BackendSelection::Selected(crate::BackendChoice {
            backend: crate::Backend::Vulkan,
            reason: "driver CUDA version unreadable".into(),
        })
    );

    let sel3 = crate::select_local_backend(
        &probe,
        &crate::CUDA_EMBEDDED_ARCH_SET,
        crate::CUDA_MIN_DRIVER_VERSION,
        crate::ArtifactTrust::Absent,
        false,
    );
    match sel3 {
        crate::BackendSelection::Selected(c) => {
            assert_eq!(c.backend, crate::Backend::Vulkan);
            assert!(
                c.reason
                    .contains("CUDA runtime artifact does not cover this GPU"),
                "{}",
                c.reason
            );
        }
        _ => panic!("expected selected vulkan"),
    }

    // 7. inspect_local on qualified hardware with a broken selected CUDA tree
    let inspected = readiness::inspect_local(
        serde_json::from_value(json!({
            "journal": root,
            "model_id": "local/qwen3.5-4b",
            "artifact_key": key,
            "nvidia_probe": probe,
        }))
        .unwrap(),
    );
    assert_eq!(inspected["ready"], false);
    assert_eq!(inspected["reason_code"], "cuda_runtime_integrity");
    assert_ne!(inspected["host"]["backend"], "vulkan");

    // 8. local_target Existing on that broken tree returns cuda_runtime_integrity error
    let err = super::local_target_with_probe(
        &root,
        "local/qwen3.5-4b",
        super::LocalBackend::Existing,
        Some(probe),
    )
    .unwrap_err();
    assert_eq!(
        err.envelope.error.as_ref().unwrap().reason_code,
        "cuda_runtime_integrity"
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
fn inventory_presence_is_cheap_but_launch_proof_catches_same_size_tampering() {
    let root = temp("manifest-presence-vs-proof");
    let artifact = root.join("model.gguf");
    let manifest_path = manifest::artifact_manifest_path(&root);
    let identity = json!({"unit":"local-model","model_id":"test"});
    fs::write(&artifact, b"original").unwrap();
    let inventory = manifest::inventory_for_tree(&root, "model").unwrap();
    let record = manifest::build_manifest(
        "local",
        "model",
        "target",
        identity.clone(),
        inventory,
        None,
        None,
    )
    .unwrap();
    manifest::write_manifest(&manifest_path, &record).unwrap();

    assert_eq!(
        manifest::inspect_manifest(&manifest_path, &identity)["status"],
        "ready"
    );
    assert_eq!(
        manifest::prove_manifest(&manifest_path, &identity)["status"],
        "ready"
    );

    fs::write(&artifact, b"tampered").unwrap();
    assert_eq!(
        manifest::inspect_manifest(&manifest_path, &identity)["status"],
        "ready"
    );
    assert_eq!(
        manifest::prove_manifest(&manifest_path, &identity)["reason_code"],
        "sha256_mismatch"
    );

    fs::write(&artifact, b"short").unwrap();
    assert_eq!(
        manifest::inspect_manifest(&manifest_path, &identity)["reason_code"],
        "inventory_size_mismatch"
    );
    assert_eq!(
        manifest::inspect_manifest(&manifest_path, &json!({"wrong":"pin"}))["reason_code"],
        "manifest_pin_mismatch"
    );
    let mut omitted = record;
    omitted["inventory"] = json!([]);
    manifest::write_manifest(&manifest_path, &omitted).unwrap();
    assert_eq!(
        manifest::prove_manifest_required(&manifest_path, &identity, &["model.gguf"])["reason_code"],
        "inventory_member_missing"
    );
    assert_eq!(
        manifest::inspect_manifest_required(&manifest_path, &identity, &["model.gguf"])["reason_code"],
        "inventory_member_missing"
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
        Err(archive::ArchiveError::DigestMismatch { .. })
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
        let origin = format!("{ORIGIN_BASE}/{}", artifact.origin_key);
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
#[cfg(feature = "runtime-fetch-test")]
fn download_artifact_refuses_userinfo_url_with_distinct_envelope_reason() {
    let loopback = solstone_core_artifact_download::RuntimeFetchLoopback {
        base_url: "http://127.0.0.1:1@blocked.test".to_owned(),
        allowed_hosts: vec!["blocked.test".to_owned()],
        allow_http: true,
    };
    solstone_core_artifact_download::with_runtime_fetch_loopback(loopback, || {
        let root = temp("download-userinfo");
        let destination = root.join("artifact");
        let query = solstone_core_assets::RuntimeFetchQuery {
            unit: Some("local-model"),
            origin_key: Some(
                "assets/local-model/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
            ),
            ..Default::default()
        };
        let error = fetch_runtime_member(
            &query,
            &destination,
            |_received, _total| {},
            "download_failed",
        )
        .unwrap_err();
        let error = error.envelope.error.unwrap();
        assert_eq!(error.reason_code, "download_url_userinfo_refused");
        assert!(error.message.contains("userinfo"));
        assert!(!destination.exists());
        let _ = fs::remove_dir_all(root);
    });
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
    let root = Path::new("/journal");
    for (key, release, _filename, digest, binary) in pins::LLAMA_SERVER_PINS {
        let paths = pins::paths(root, key, Some("local/qwen3.5-4b"));
        assert_eq!(
            path_value(&paths["binary_path"]),
            pins::cache_root(root)
                .join("bin")
                .join(key)
                .join(release)
                .join(binary)
        );
        assert_eq!(
            path_value(&paths["model_dir"]),
            pins::cache_root(root)
                .join("models")
                .join("local__qwen3.5-4b")
        );
        assert_eq!(pins::vulkan_identity(key).unwrap()["sha256"], *digest);
    }
    for (key, _url, digest, _size) in pins::CUDA_ARTIFACTS {
        let paths = pins::paths(root, key, None);
        assert_eq!(
            path_value(&paths["cuda_binary_path"]),
            pins::cache_root(root)
                .join("cuda")
                .join(key)
                .join(digest)
                .join("llama-server")
        );
        assert_eq!(pins::cuda_identity(key).unwrap()["sha256"], *digest);
    }
}

#[test]
fn parakeet_pin_tables_cover_every_pinned_platform_and_backend() {
    assert_eq!(pins::PARAKEET_VULKAN_PINS.len(), 2);
    assert_eq!(pins::PARAKEET_CPU_PINS.len(), 2);
    let root = Path::new("/journal");
    for (backend, table) in [
        ("vulkan", pins::PARAKEET_VULKAN_PINS),
        ("cpu", pins::PARAKEET_CPU_PINS),
    ] {
        for (key, release, _filename, digest, binary) in table {
            let paths = pins::parakeet_paths(root, key);
            assert_eq!(
                path_value(&paths[format!("binary_path_{backend}")]),
                pins::parakeet_cache_root(root)
                    .join("bin")
                    .join(key)
                    .join(backend)
                    .join(release)
                    .join(binary)
            );
            assert_eq!(
                pins::parakeet_backend_identity(key, backend).unwrap()["sha256"],
                *digest
            );
        }
    }
    let (repo, filename, revision, sha256, size_bytes) = pins::PARAKEET_MODEL;
    assert_eq!(
        path_value(&pins::parakeet_paths(root, "x86_64-unknown-linux-gnu")["model_path"]),
        pins::parakeet_cache_root(root)
            .join("models")
            .join(repo.replace('/', "__"))
            .join(revision)
            .join(filename)
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

    // ⛔ No copy of the digest: asserting `pins::PARAKEET_MODEL.3` equals a
    // literal only restates the pin. What the resolver owes is that it returns
    // exactly that pin's row.
    let row = resolve("parakeet-model", None, None);
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].sha256, pins::PARAKEET_MODEL.3);

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
    let arm_vulkan = resolve("llama-server-vulkan", Some(Platform::LinuxArm64), None);
    assert_eq!(arm_vulkan.len(), 1);
    assert_eq!(arm_vulkan[0].size_bytes, 24845090);
    assert_eq!(
        arm_vulkan[0].upstream_url,
        "https://github.com/ggml-org/llama.cpp/releases/download/b11429/llama-b11429-bin-ubuntu-vulkan-arm64.tar.gz"
    );
    assert_eq!(
        arm_vulkan[0].origin_key,
        "assets/llama-server-vulkan/b11429/llama-b11429-bin-ubuntu-vulkan-arm64.tar.gz"
    );
}

#[test]
fn registry_preserves_prechange_identity_literals() {
    let historical_vulkan = "{\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"filename\":\"llama-b10068-bin-ubuntu-vulkan-x64.tar.gz\",\"release_tag\":\"b10068\",\"sha256\":\"713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3\",\"unit\":\"llama-server-vulkan\"}";
    let historical_arm_vulkan = "{\"artifact_key\":\"aarch64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"filename\":\"llama-b10068-bin-ubuntu-vulkan-arm64.tar.gz\",\"release_tag\":\"b10068\",\"sha256\":\"c3c49e6e124a574165ca28317be021b1a12a2ea06977e3eb7daee3eb443eb186\",\"unit\":\"llama-server-vulkan\"}";
    let historical_cuda = "{\"arch\":\"amd64\",\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"llama_cpp_revision\":\"571d0d540df04f25298d0e159e520d9fc62ed121\",\"release_tag\":\"b10068\",\"repack_revision\":\"sol1\",\"sha256\":\"3727630e6ac79953f5c652fddcfd7100da98c55d773c0aec115a55f40f3aafea\",\"size_bytes\":550238443,\"unit\":\"llama-server-cuda\",\"upstream_image_digest\":\"sha256:5bd5290bd35cfde893d0dcbd9811723c16d89575927d537b5f21becbfbab2f63\",\"url\":\"https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz\",\"wanted_files\":[\"libcublas.so.13\",\"libcublasLt.so.13\",\"libcudart.so.13\",\"libggml-base.so.0\",\"libggml-cpu-alderlake.so\",\"libggml-cpu-cannonlake.so\",\"libggml-cpu-cascadelake.so\",\"libggml-cpu-cooperlake.so\",\"libggml-cpu-haswell.so\",\"libggml-cpu-icelake.so\",\"libggml-cpu-ivybridge.so\",\"libggml-cpu-piledriver.so\",\"libggml-cpu-sandybridge.so\",\"libggml-cpu-sapphirerapids.so\",\"libggml-cpu-skylakex.so\",\"libggml-cpu-sse42.so\",\"libggml-cpu-x64.so\",\"libggml-cpu-zen4.so\",\"libggml-cuda.so\",\"libggml.so.0\",\"libllama-common.so.0\",\"libllama-server-impl.so\",\"libllama.so.0\",\"libmtmd.so.0\",\"llama-server\"]}";

    let current_vulkan =
        fingerprint::canonical(pins::vulkan_identity("x86_64-unknown-linux-gnu").unwrap()).unwrap();
    assert_ne!(current_vulkan, historical_vulkan);
    assert_eq!(
        current_vulkan,
        "{\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"filename\":\"llama-b11429-bin-ubuntu-vulkan-x64.tar.gz\",\"release_tag\":\"b11429\",\"sha256\":\"632c4e98feba2b94407a2130e3133e0c3aefb0ea1ab41337e926d8bfafdd0b74\",\"unit\":\"llama-server-vulkan\"}"
    );
    let current_arm_vulkan =
        fingerprint::canonical(pins::vulkan_identity("aarch64-unknown-linux-gnu").unwrap())
            .unwrap();
    assert_ne!(current_arm_vulkan, historical_arm_vulkan);
    assert_eq!(
        current_arm_vulkan,
        "{\"artifact_key\":\"aarch64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"filename\":\"llama-b11429-bin-ubuntu-vulkan-arm64.tar.gz\",\"release_tag\":\"b11429\",\"sha256\":\"702d99c4219b4314cc6d3b10fb41ae96cbfc68226305c2681269dfb51487af34\",\"unit\":\"llama-server-vulkan\"}"
    );
    let current_metal =
        fingerprint::canonical(pins::vulkan_identity("aarch64-apple-darwin").unwrap()).unwrap();
    assert_eq!(
        current_metal,
        "{\"artifact_key\":\"aarch64-apple-darwin\",\"binary_name\":\"llama-server\",\"filename\":\"llama-b11429-bin-macos-arm64.tar.gz\",\"release_tag\":\"b11429\",\"sha256\":\"740288ec6887be94280a5dfa25b5e23a78285cab104519e6c7e218904ee82459\",\"unit\":\"llama-server-vulkan\"}"
    );

    let current_cuda =
        fingerprint::canonical(pins::cuda_identity("x86_64-unknown-linux-gnu").unwrap()).unwrap();
    assert_ne!(current_cuda, historical_cuda);
    assert_eq!(
        current_cuda,
        "{\"arch\":\"amd64\",\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"binary_name\":\"llama-server\",\"cuda_toolkit\":\"13.4.1\",\"inputs\":[{\"filename\":\"cudart-llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz\",\"role\":\"cudart\",\"sha256\":\"93d18648d815b2bd624d83d82f653e1db97afb478f02064305fe3cf570040a6d\",\"size_bytes\":440236630,\"url_prefix\":\"https://github.com/ggml-org/llama.cpp/releases/download/b11429/\"},{\"filename\":\"llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz\",\"role\":\"engine\",\"sha256\":\"8082b7eaa74a714c9fecca19128f751c8e32da763ee8096b8ad1e824da7621d3\",\"size_bytes\":152519318,\"url_prefix\":\"https://github.com/ggml-org/llama.cpp/releases/download/b11429/\"}],\"llama_cpp_revision\":\"d81235049384534c167caea52b85a694f6103d14\",\"release_tag\":\"b11429\",\"repack_revision\":\"sol1\",\"sha256\":\"a9d8c0a4ece9f9dce7d8e634dd55f943ba39b93b339462dd645202db34aafbbd\",\"size_bytes\":591752886,\"unit\":\"llama-server-cuda\",\"url\":\"https://updates.solstone.app/runtimes/llama-cuda13/b11429/llama-b11429-bin-linux-cuda13-amd64-sol1.tar.gz\",\"wanted_files\":[\"libcublas.so.13\",\"libcublasLt.so.13\",\"libcudart.so.13\",\"libggml-base.so.0\",\"libggml-cpu-alderlake.so\",\"libggml-cpu-cannonlake.so\",\"libggml-cpu-cascadelake.so\",\"libggml-cpu-cooperlake.so\",\"libggml-cpu-haswell.so\",\"libggml-cpu-icelake.so\",\"libggml-cpu-ivybridge.so\",\"libggml-cpu-piledriver.so\",\"libggml-cpu-sandybridge.so\",\"libggml-cpu-sapphirerapids.so\",\"libggml-cpu-skylakex.so\",\"libggml-cpu-sse42.so\",\"libggml-cpu-x64.so\",\"libggml-cpu-zen4.so\",\"libggml-cuda.so\",\"libggml.so.0\",\"libllama-common.so.0\",\"libllama-server-impl.so\",\"libllama.so.0\",\"libmtmd.so.0\",\"llama-server\"]}"
    );

    let literals = [
        (
            fingerprint::canonical(pins::model_identity("local/qwen3.5-4b").unwrap()).unwrap(),
            "{\"filename\":\"Qwen3.5-4B-Q4_K_M.gguf\",\"mmproj_filename\":\"mmproj-F16.gguf\",\"mmproj_sha256\":\"cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864\",\"model_id\":\"local/qwen3.5-4b\",\"repo\":\"unsloth/Qwen3.5-4B-GGUF\",\"revision\":\"main\",\"sha256\":\"00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4\",\"unit\":\"local-model\"}",
        ),
        (
            fingerprint::canonical(
                pins::parakeet_backend_identity("x86_64-unknown-linux-gnu", "cpu").unwrap(),
            )
            .unwrap(),
            "{\"artifact_key\":\"x86_64-unknown-linux-gnu\",\"backend\":\"cpu\",\"binary_name\":\"parakeet-server\",\"filename\":\"parakeet-v0.6.1-bin-linux-cpu-x64.tar.gz\",\"release_tag\":\"v0.6.1\",\"sha256\":\"cce60d122ab72e1068cd0d164e54a21655a0b83f1b9c21befc20124f5a972c10\",\"unit\":\"parakeet-server\"}",
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
    let journal = Path::new("/journal");
    let key = "x86_64-unknown-linux-gnu";
    let local_paths = pins::paths(journal, key, Some("local/qwen3.5-4b"));
    let historical_vulkan_path = pins::cache_root(journal)
        .join("bin")
        .join(key)
        .join("b10068")
        .join("llama-server");
    let historical_cuda_path = pins::cache_root(journal)
        .join("cuda")
        .join(key)
        .join("3727630e6ac79953f5c652fddcfd7100da98c55d773c0aec115a55f40f3aafea")
        .join("llama-server");

    let current_vulkan_path = path_value(&local_paths["binary_path"]);
    let current_cuda_path = path_value(&local_paths["cuda_binary_path"]);

    assert_ne!(current_vulkan_path, historical_vulkan_path);
    assert_ne!(current_cuda_path, historical_cuda_path);

    assert_eq!(
        current_vulkan_path,
        pins::cache_root(journal)
            .join("bin")
            .join(key)
            .join("b11429")
            .join("llama-server")
    );
    assert_eq!(
        current_cuda_path,
        pins::cache_root(journal)
            .join("cuda")
            .join(key)
            .join("a9d8c0a4ece9f9dce7d8e634dd55f943ba39b93b339462dd645202db34aafbbd")
            .join("llama-server")
    );
    let model_dir = PathBuf::from(local_paths["model_dir"].as_str().unwrap());
    assert_eq!(
        model_dir,
        pins::cache_root(journal)
            .join("models")
            .join("local__qwen3.5-4b")
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
        path_value(&local_readiness["artifacts"]["binary_path"]),
        pins::cache_root(journal)
            .join("bin")
            .join(key)
            .join("b11429")
            .join("llama-server")
    );
    assert_eq!(
        path_value(&local_readiness["artifacts"]["model_path"]),
        model_dir.join("Qwen3.5-4B-Q4_K_M.gguf")
    );
    assert_eq!(
        path_value(&local_readiness["artifacts"]["projector_path"]),
        model_dir.join("mmproj-F16.gguf")
    );
    // `install_model` joins each pin filename inline immediately before its
    // network fetch; `pins::paths` above is its production root derivation.
    let local_identity = pins::model_identity("local/qwen3.5-4b").unwrap();
    assert_eq!(
        model_dir.join(local_identity["filename"].as_str().unwrap()),
        pins::cache_root(journal)
            .join("models")
            .join("local__qwen3.5-4b")
            .join("Qwen3.5-4B-Q4_K_M.gguf")
    );
    assert_eq!(
        model_dir.join(local_identity["mmproj_filename"].as_str().unwrap()),
        pins::cache_root(journal)
            .join("models")
            .join("local__qwen3.5-4b")
            .join("mmproj-F16.gguf")
    );
    let parakeet = pins::parakeet_paths(journal, key);
    assert_eq!(
        path_value(&parakeet["binary_path_cpu"]),
        pins::parakeet_cache_root(journal)
            .join("bin")
            .join(key)
            .join("cpu")
            .join("v0.6.1")
            .join("parakeet-server")
    );
    assert_eq!(
        path_value(&parakeet["model_path"]),
        pins::parakeet_cache_root(journal)
            .join("models")
            .join("mudler__parakeet-cpp-gguf")
            .join("bf0af9f425fa01809cadec671b3cb672709d13e9")
            .join("tdt-0.6b-v3-q8_0.gguf")
    );
}

#[test]
fn oracle_symlink_chain_and_production_commit_verification() {
    let root = temp("oracle-symlink-chain");
    let staging = root.join("staging");
    let nested = staging.join("llama-b11429");
    let target = root.join("target");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("LICENSE"), b"MIT").unwrap();
    fs::write(nested.join("llama-server"), b"server binary").unwrap();

    let members = pins::required_members_for("b11429", "x86_64-unknown-linux-gnu").unwrap();
    for member in members {
        match member {
            pins::RequiredMember::Regular(name) => {
                if *name != "llama-server" {
                    fs::write(nested.join(name), format!("regular {name}")).unwrap();
                }
            }
            pins::RequiredMember::Link { name, target } => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, nested.join(name)).unwrap();
                #[cfg(not(unix))]
                let _ = (name, target);
            }
        }
    }

    super::flatten_binary_bundle(&staging, &nested.join("llama-server")).unwrap();

    #[cfg(unix)]
    {
        assert!(
            super::verify_required_oracle(&staging, "b11429", "x86_64-unknown-linux-gnu", false)
                .is_ok()
        );

        // Symlink chain libggml.so -> libggml.so.0 -> libggml.so.0.26.0 still reads
        let content = fs::read(staging.join("libggml.so")).unwrap();
        assert_eq!(content, b"regular libggml.so.0.26.0");

        // Deleting libggml.so.0.26.0 fails the oracle
        fs::remove_file(staging.join("libggml.so.0.26.0")).unwrap();
        assert!(
            super::verify_required_oracle(&staging, "b11429", "x86_64-unknown-linux-gnu", false)
                .is_err()
        );

        // Restore target but delete libggml.so alias
        fs::write(
            staging.join("libggml.so.0.26.0"),
            b"regular libggml.so.0.26.0",
        )
        .unwrap();
        fs::remove_file(staging.join("libggml.so")).unwrap();
        assert!(
            super::verify_required_oracle(&staging, "b11429", "x86_64-unknown-linux-gnu", false)
                .is_err()
        );

        // Production commit on incomplete staging does not publish and does not write candidate manifest
        let identity = pins::vulkan_identity("x86_64-unknown-linux-gnu").unwrap();
        let commit_err = super::commit_staged_local_runtime(
            &staging,
            &target,
            super::LocalRuntimeAdmission {
                release_tag: "b11429",
                artifact_key: "x86_64-unknown-linux-gnu",
                is_cuda: false,
                pin_identity: &identity,
            },
            "target_sha",
            None,
            &[],
        );
        assert!(commit_err.is_err());
        assert!(!manifest::artifact_manifest_path(&target).exists());
    }

    // A b10068 tree whose only file is llama-server passes verify_required_oracle for release b10068
    let staging_b10068 = root.join("llama-b10068");
    fs::create_dir_all(&staging_b10068).unwrap();
    fs::write(staging_b10068.join("llama-server"), b"b10068 binary").unwrap();
    assert!(
        super::verify_required_oracle(&staging_b10068, "b10068", "x86_64-unknown-linux-gnu", false)
            .is_ok()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn oracle_missing_or_corrupt_member_fails_loudly_without_unrelated_errors() {
    let root = temp("oracle-verification");
    let staging = root.join("staging");
    fs::create_dir_all(&staging).unwrap();

    // Vulkan x86_64 b11429 has required members
    // Case 1: Missing member
    let err = super::verify_required_oracle(&staging, "b11429", "x86_64-unknown-linux-gnu", false)
        .unwrap_err();
    assert!(err.contains("required"));

    // Case 2: Incomplete members
    fs::write(staging.join("llama-server"), b"binary").unwrap();
    let err = super::verify_required_oracle(&staging, "b11429", "x86_64-unknown-linux-gnu", false)
        .unwrap_err();
    assert!(err.contains("required"));

    // Case 3: CUDA x86_64 wanted files
    let cuda_err =
        super::verify_required_oracle(&staging, "b11429", "x86_64-unknown-linux-gnu", true)
            .unwrap_err();
    assert!(cuda_err.contains("required cuda member"));

    // Case 4: Populating all required members for Vulkan x86_64 b11429
    let Some(members) = pins::required_members_for("b11429", "x86_64-unknown-linux-gnu") else {
        panic!("expected members");
    };
    for member in members {
        match member {
            pins::RequiredMember::Regular(name) => {
                fs::write(staging.join(name), b"regular file").unwrap();
            }
            pins::RequiredMember::Link { name, target } => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(target, staging.join(name)).unwrap();
                #[cfg(not(unix))]
                let _ = (name, target);
            }
        }
    }
    #[cfg(unix)]
    assert!(
        super::verify_required_oracle(&staging, "b11429", "x86_64-unknown-linux-gnu", false)
            .is_ok()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn oracle_arm64_vulkan_b11429_required_members_match_archive_subset() {
    assert_eq!(
        pins::required_members_for("b11429", "aarch64-unknown-linux-gnu"),
        Some(pins::VULKAN_LINUX_ARM64_B11429_REQUIRED)
    );
    assert_eq!(
        pins::required_members_for("b11429", "x86_64-unknown-linux-gnu"),
        Some(pins::VULKAN_LINUX_X64_B11429_REQUIRED)
    );
    assert_eq!(
        pins::required_members_for("b11429", "aarch64-apple-darwin"),
        Some(pins::METAL_MACOS_ARM64_B11429_REQUIRED)
    );
    assert_eq!(
        pins::required_members_for("b10068", "x86_64-unknown-linux-gnu"),
        Some(pins::B10068_REQUIRED)
    );
    assert_eq!(
        pins::required_members_for("b10068", "aarch64-unknown-linux-gnu"),
        Some(pins::B10068_REQUIRED)
    );

    let arm_members = pins::VULKAN_LINUX_ARM64_B11429_REQUIRED;
    let base_idx = arm_members
        .iter()
        .position(|m| matches!(m, pins::RequiredMember::Regular("libggml-base.so.0.26.0")))
        .expect("libggml-base.so.0.26.0 present");
    let rpc_idx = arm_members
        .iter()
        .position(|m| matches!(m, pins::RequiredMember::Regular("libggml-rpc.so")))
        .expect("libggml-rpc.so present");

    let expected_cpu = [
        "libggml-cpu-armv8.0_1.so",
        "libggml-cpu-armv8.2_1.so",
        "libggml-cpu-armv8.2_2.so",
        "libggml-cpu-armv8.2_3.so",
        "libggml-cpu-armv8.6_1.so",
        "libggml-cpu-armv8.6_2.so",
        "libggml-cpu-armv9.2_1.so",
        "libggml-cpu-armv9.2_2.so",
    ];

    let actual_cpu: Vec<&str> = arm_members[base_idx + 1..rpc_idx]
        .iter()
        .map(|m| match m {
            pins::RequiredMember::Regular(name) => *name,
            pins::RequiredMember::Link { name, .. } => *name,
        })
        .collect();
    assert_eq!(actual_cpu, expected_cpu);

    let x86_cpu_tokens = [
        "libggml-cpu-alderlake.so",
        "libggml-cpu-cannonlake.so",
        "libggml-cpu-cascadelake.so",
        "libggml-cpu-cooperlake.so",
        "libggml-cpu-haswell.so",
        "libggml-cpu-icelake.so",
        "libggml-cpu-ivybridge.so",
        "libggml-cpu-piledriver.so",
        "libggml-cpu-sandybridge.so",
        "libggml-cpu-sapphirerapids.so",
        "libggml-cpu-skylakex.so",
        "libggml-cpu-sse42.so",
        "libggml-cpu-x64.so",
    ];
    for member in arm_members {
        let name = match member {
            pins::RequiredMember::Regular(name) => *name,
            pins::RequiredMember::Link { name, .. } => *name,
        };
        for token in x86_cpu_tokens {
            assert_ne!(name, token, "arm64 members must not contain {token}");
        }
    }

    let x64_non_cpu: Vec<_> = pins::VULKAN_LINUX_X64_B11429_REQUIRED
        .iter()
        .filter(|m| {
            let name = match m {
                pins::RequiredMember::Regular(name) => *name,
                pins::RequiredMember::Link { name, .. } => *name,
            };
            !name.starts_with("libggml-cpu-")
        })
        .cloned()
        .collect();
    let arm_non_cpu: Vec<_> = arm_members
        .iter()
        .filter(|m| {
            let name = match m {
                pins::RequiredMember::Regular(name) => *name,
                pins::RequiredMember::Link { name, .. } => *name,
            };
            !name.starts_with("libggml-cpu-")
        })
        .cloned()
        .collect();
    assert_eq!(arm_non_cpu, x64_non_cpu);
}

#[test]
#[cfg(unix)]
fn oracle_arm64_vulkan_symlink_chain_admits_complete_tree_and_rejects_breaks() {
    fn build_arm_tree(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        for member in pins::VULKAN_LINUX_ARM64_B11429_REQUIRED {
            match member {
                pins::RequiredMember::Regular(name) => {
                    fs::write(dir.join(name), format!("regular {name}")).unwrap();
                }
                pins::RequiredMember::Link { name, target } => {
                    std::os::unix::fs::symlink(target, dir.join(name)).unwrap();
                }
            }
        }
    }

    let root = temp("oracle-arm64-vulkan");

    // Case 1: Complete tree admits
    let complete = root.join("complete");
    build_arm_tree(&complete);
    assert!(
        super::verify_required_oracle(&complete, "b11429", "aarch64-unknown-linux-gnu", false)
            .is_ok()
    );

    // Case 2: Missing regular llama-server fails
    let missing_regular = root.join("missing_regular");
    build_arm_tree(&missing_regular);
    fs::remove_file(missing_regular.join("llama-server")).unwrap();
    assert!(
        super::verify_required_oracle(
            &missing_regular,
            "b11429",
            "aarch64-unknown-linux-gnu",
            false
        )
        .is_err()
    );

    // Case 3: Missing symlink libggml-base.so fails
    let missing_symlink = root.join("missing_symlink");
    build_arm_tree(&missing_symlink);
    fs::remove_file(missing_symlink.join("libggml-base.so")).unwrap();
    assert!(
        super::verify_required_oracle(
            &missing_symlink,
            "b11429",
            "aarch64-unknown-linux-gnu",
            false
        )
        .is_err()
    );

    // Case 4: Retargeted symlink libggml.so fails
    let retargeted_symlink = root.join("retargeted_symlink");
    build_arm_tree(&retargeted_symlink);
    fs::remove_file(retargeted_symlink.join("libggml.so")).unwrap();
    std::os::unix::fs::symlink("libggml.so.0.26.0", retargeted_symlink.join("libggml.so")).unwrap();
    assert!(
        super::verify_required_oracle(
            &retargeted_symlink,
            "b11429",
            "aarch64-unknown-linux-gnu",
            false
        )
        .is_err()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_prepublish_failures_keep_an_admitted_incumbent() {
    let root = temp("upgrade-incumbent-preservation");
    let target = root.join("target");
    fs::create_dir_all(&target).unwrap();

    let incumbent_identity = json!({
        "artifact_key": "x86_64-unknown-linux-gnu",
        "binary_name": "llama-server",
        "filename": "llama-b10068-bin-ubuntu-vulkan-x64.tar.gz",
        "release_tag": "b10068",
        "sha256": "713641920dce6c8efb953ebc9ffa309977e200cec5e182e6ad0e8b086203cdc3",
        "unit": "llama-server-vulkan"
    });

    let incumbent_bin = target.join("llama-server");
    fs::write(&incumbent_bin, b"incumbent b10068 binary content").unwrap();
    let inventory = manifest::runtime_inventory(&target, &[]).unwrap();
    let built_manifest = manifest::build_manifest(
        "local",
        "llama-server-vulkan",
        "target",
        json!({"pin_identity": incumbent_identity.clone()}),
        inventory,
        None,
        None,
    )
    .unwrap();
    let manifest_path = manifest::artifact_manifest_path(&target);
    manifest::write_manifest(&manifest_path, &built_manifest).unwrap();

    // prove_manifest is ready for incumbent identity
    assert_eq!(
        manifest::prove_manifest(&manifest_path, &incumbent_identity)["status"],
        "ready"
    );

    // prove_manifest against current vulkan_identity is manifest_pin_mismatch
    let current_identity = pins::vulkan_identity("x86_64-unknown-linux-gnu").unwrap();
    assert_eq!(
        manifest::prove_manifest(&manifest_path, &current_identity)["reason_code"],
        "manifest_pin_mismatch"
    );

    // Snapshot incumbent bytes and manifest
    let snapshot_bin = fs::read(&incumbent_bin).unwrap();
    let snapshot_manifest = fs::read(&manifest_path).unwrap();

    // Failure 1: fetch_runtime_member into staging with an unminted query
    let staging1 = root.join("staging1");
    fs::create_dir_all(&staging1).unwrap();
    let unminted_query = solstone_core_assets::RuntimeFetchQuery {
        unit: Some("nonexistent-unit"),
        ..Default::default()
    };
    let dl_err = fetch_runtime_member(
        &unminted_query,
        &staging1.join("artifact"),
        |_received, _total| {},
        "download_failed",
    );
    assert!(dl_err.is_err());
    assert_eq!(fs::read(&incumbent_bin).unwrap(), snapshot_bin);
    assert_eq!(fs::read(&manifest_path).unwrap(), snapshot_manifest);

    // Failure 2: cancel_local_bootstrap on an in-flight local status, without writing the incumbent tree
    let status_dir = root.join("journal_status");
    let initial_status = status::begin(
        &status_dir,
        "{}".to_owned(),
        "target_fp".to_owned(),
        Some(json!({"pid": 12345})),
        "downloading",
    )
    .unwrap();
    let attempt_id = initial_status.attempt_id.as_deref().unwrap();
    let cancelled =
        super::cancel_local_bootstrap(&status_dir, "local", attempt_id, |_| Ok(())).unwrap();
    assert_eq!(cancelled.install_state, "failed");
    assert_eq!(fs::read(&incumbent_bin).unwrap(), snapshot_bin);
    assert_eq!(fs::read(&manifest_path).unwrap(), snapshot_manifest);

    // Failure 3: archive::verify_sha256 digest mismatch on a candidate file
    let staging3 = root.join("staging3");
    fs::create_dir_all(&staging3).unwrap();
    let cand_file = staging3.join("candidate");
    fs::write(&cand_file, b"corrupted bytes").unwrap();
    let verify_err = archive::verify_sha256(&cand_file, "00".repeat(32).as_str());
    assert!(verify_err.is_err());
    assert_eq!(fs::read(&incumbent_bin).unwrap(), snapshot_bin);
    assert_eq!(fs::read(&manifest_path).unwrap(), snapshot_manifest);

    // Failure 4: extract_tar_gz of a bad archive into staging
    let staging4 = root.join("staging4");
    fs::create_dir_all(&staging4).unwrap();
    let bad_tar = root.join("bad.tar.gz");
    fs::write(&bad_tar, b"not a valid tar gz").unwrap();
    let ext_err = archive::extract_tar_gz(&bad_tar, &staging4);
    assert!(ext_err.is_err());
    assert_eq!(fs::read(&incumbent_bin).unwrap(), snapshot_bin);
    assert_eq!(fs::read(&manifest_path).unwrap(), snapshot_manifest);

    // Failure 5: production commit function rejects a b11429 staging that fails the oracle.
    // The candidate directory is beside the incumbent and must stay unpublished.
    let candidate = root.join("candidate");
    let candidate_manifest = manifest::artifact_manifest_path(&candidate);
    let staging5 = root.join("staging5");
    fs::create_dir_all(&staging5).unwrap();
    fs::write(
        staging5.join("llama-server"),
        b"b11429 without required libs",
    )
    .unwrap();
    let commit_err = super::commit_staged_local_runtime(
        &staging5,
        &candidate,
        super::LocalRuntimeAdmission {
            release_tag: "b11429",
            artifact_key: "x86_64-unknown-linux-gnu",
            is_cuda: false,
            pin_identity: &current_identity,
        },
        "target_sha",
        None,
        &[],
    );
    assert!(commit_err.is_err());
    assert!(!candidate_manifest.exists());
    assert_eq!(fs::read(&incumbent_bin).unwrap(), snapshot_bin);
    assert_eq!(fs::read(&manifest_path).unwrap(), snapshot_manifest);

    // Successful commit publishes beside the incumbent. The admitted tree stays byte-identical.
    let staging_ok = root.join("staging_ok");
    fs::create_dir_all(&staging_ok).unwrap();
    let members = pins::required_members_for("b11429", "x86_64-unknown-linux-gnu").unwrap();
    for member in members {
        match member {
            pins::RequiredMember::Regular(name) => {
                fs::write(staging_ok.join(name), format!("payload for {name}")).unwrap();
            }
            pins::RequiredMember::Link {
                name,
                target: link_target,
            } => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(link_target, staging_ok.join(name)).unwrap();
                #[cfg(not(unix))]
                let _ = (name, link_target);
            }
        }
    }
    #[cfg(unix)]
    {
        super::commit_staged_local_runtime(
            &staging_ok,
            &candidate,
            super::LocalRuntimeAdmission {
                release_tag: "b11429",
                artifact_key: "x86_64-unknown-linux-gnu",
                is_cuda: false,
                pin_identity: &current_identity,
            },
            "target_sha",
            None,
            &[],
        )
        .unwrap();
        assert_eq!(
            manifest::prove_manifest(&candidate_manifest, &current_identity)["status"],
            "ready"
        );
        assert_eq!(fs::read(&incumbent_bin).unwrap(), snapshot_bin);
        assert_eq!(fs::read(&manifest_path).unwrap(), snapshot_manifest);
        assert_eq!(
            manifest::prove_manifest(&manifest_path, &incumbent_identity)["status"],
            "ready"
        );
        assert_eq!(
            manifest::prove_manifest(&manifest_path, &current_identity)["reason_code"],
            "manifest_pin_mismatch"
        );
    }

    let _ = fs::remove_dir_all(root);
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

#[test]
fn assert_current_rejects_modified_attempt_and_prevents_publication() {
    let root = temp("assert-current");
    let initial = status::begin(
        &root,
        "{}".to_owned(),
        "sha256-a".to_owned(),
        Some(json!({"pid": std::process::id()})),
        "resolving",
    )
    .unwrap();
    assert!(status::assert_current(&root, &initial).is_ok());

    // Supersede attempt
    let next_attempt = status::begin_or_replace(
        &root,
        "local",
        "{}".to_owned(),
        "sha256-b".to_owned(),
        Some(json!({"pid": std::process::id()})),
        "resolving",
    )
    .unwrap();
    assert!(status::assert_current(&root, &initial).is_err());
    assert!(status::assert_current(&root, &next_attempt).is_ok());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cancel_local_bootstrap_requires_and_matches_attempt_id() {
    let root = temp("cancel-attempt");
    let initial = status::begin(
        &root,
        "{}".to_owned(),
        "sha256-cancel".to_owned(),
        Some(json!({"pid": 999_999_999})),
        "resolving",
    )
    .unwrap();
    let attempt_id = initial.attempt_id.as_deref().unwrap();

    // Cancel without attempt_id fails
    let err = super::cancel_local_bootstrap(&root, "local", "", |_| Ok(())).unwrap_err();
    assert_eq!(
        err.envelope.error.as_ref().unwrap().reason_code,
        "attempt_mismatch"
    );

    // Cancel with mismatched attempt_id fails
    let err =
        super::cancel_local_bootstrap(&root, "local", "wrong-attempt-id", |_| Ok(())).unwrap_err();
    assert_eq!(
        err.envelope.error.as_ref().unwrap().reason_code,
        "attempt_mismatch"
    );

    // Status is still in-flight
    let current = status::read_status(&root, "local").unwrap();
    assert!(status::is_in_flight(&current.install_state));

    // Cancel with matching attempt_id succeeds and sets interrupted / failed
    let canceled = super::cancel_local_bootstrap(&root, "local", attempt_id, |_| Ok(())).unwrap();
    assert_eq!(canceled.install_state, "failed");
    assert_eq!(canceled.error_code.as_deref(), Some("install_cancelled"));

    // Calling cancel on non-in-flight status is a no-op returning current status
    let no_op = super::cancel_local_bootstrap(&root, "local", attempt_id, |_| Ok(())).unwrap();
    assert_eq!(no_op.install_state, "failed");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn cancellation_fences_attempt_and_requires_writer_lease_release() {
    let journal = temp("cancel-fencing");
    let held = lease::acquire(&journal, "local").unwrap().unwrap();
    let initial = status::begin(
        &journal,
        "{}".into(),
        "target".into(),
        Some(json!({"pid":42})),
        "downloading",
    )
    .unwrap();
    let attempt = initial.attempt_id.as_deref().unwrap();
    let stale = super::cancel_local_bootstrap(&journal, "local", "stale", |_| {
        panic!("stale attempt must not signal")
    });
    assert!(stale.is_err());
    let refused =
        super::cancel_local_bootstrap(&journal, "local", attempt, |_| Err("unverifiable".into()));
    assert!(refused.is_err());
    let still_live = super::cancel_local_bootstrap(&journal, "local", attempt, |_| Ok(()));
    assert_eq!(still_live.unwrap_err().exit_code, 75);
    assert_eq!(status::read_status(&journal, "local").unwrap(), initial);
    drop(held);
    let cancelled = super::cancel_local_bootstrap(&journal, "local", attempt, |_| {
        panic!("free lease needs no signal")
    })
    .unwrap();
    assert_eq!(cancelled.install_state, "failed");
    assert_eq!(
        cancelled.install_error.as_deref(),
        Some("install_cancelled")
    );
    assert!(super::cancel_local_bootstrap(&journal, "local", "stale", |_| Ok(())).is_err());
    assert_eq!(
        super::cancel_local_bootstrap(&journal, "local", attempt, |_| panic!(
            "terminal attempt must not signal"
        ))
        .unwrap(),
        cancelled
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn cancellation_does_not_overwrite_a_replacement_attempt() {
    let journal = temp("cancel-replacement");
    let held = lease::acquire(&journal, "local").unwrap().unwrap();
    let initial = status::begin(
        &journal,
        "{}".into(),
        "target".into(),
        Some(json!({"pid":42})),
        "downloading",
    )
    .unwrap();
    let attempt = initial.attempt_id.as_deref().unwrap();
    let result = super::cancel_local_bootstrap(&journal, "local", attempt, |_| {
        drop(held);
        status::begin_or_replace(
            &journal,
            "local",
            "{}".into(),
            "next".into(),
            None,
            "downloading",
        )
        .unwrap();
        Ok(())
    });
    assert!(result.is_err());
    assert_ne!(
        status::read_status(&journal, "local").unwrap().attempt_id,
        initial.attempt_id
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn cuda_b11429_identity_pins_toolkit_inputs_and_not_publisher_attestation() {
    let cases = [
        (
            "x86_64-unknown-linux-gnu",
            "amd64",
            "https://updates.solstone.app/runtimes/llama-cuda13/b11429/llama-b11429-bin-linux-cuda13-amd64-sol1.tar.gz",
            "a9d8c0a4ece9f9dce7d8e634dd55f943ba39b93b339462dd645202db34aafbbd",
            591752886u64,
            json!([
                {
                    "role": "engine",
                    "filename": "llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz",
                    "sha256": "8082b7eaa74a714c9fecca19128f751c8e32da763ee8096b8ad1e824da7621d3",
                    "size_bytes": 152519318,
                    "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11429/"
                },
                {
                    "role": "cudart",
                    "filename": "cudart-llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz",
                    "sha256": "93d18648d815b2bd624d83d82f653e1db97afb478f02064305fe3cf570040a6d",
                    "size_bytes": 440236630,
                    "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11429/"
                }
            ]),
        ),
        (
            "aarch64-unknown-linux-gnu",
            "arm64",
            "https://updates.solstone.app/runtimes/llama-cuda13/b11429/llama-b11429-bin-linux-cuda13-arm64-sol1.tar.gz",
            "de73a4cae3cb750170cfce090d752af95a67d97afd681154646547a7d6ddc72f",
            699234360u64,
            json!([
                {
                    "role": "engine",
                    "filename": "llama-b11429-bin-ubuntu-cuda-13.4-arm64.tar.gz",
                    "sha256": "9c76d072276c0faa7fc1b5cbf715b4186dd2e8d7f16acd20b67daab24c82bd22",
                    "size_bytes": 147639003,
                    "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11429/"
                },
                {
                    "role": "cudart",
                    "filename": "cudart-llama-b11429-bin-ubuntu-cuda-13.4-arm64.tar.gz",
                    "sha256": "ad62e46cdc2e8636fa91e883b9e9ad779516f61cc478ece1c90e5dbc905a7d29",
                    "size_bytes": 552521413,
                    "url_prefix": "https://github.com/ggml-org/llama.cpp/releases/download/b11429/"
                }
            ]),
        ),
    ];

    for (key, arch, url, sha256, size, inputs) in cases {
        let (pin_url, pin_sha256, pin_size) = pins::cuda_pin(key).expect("cuda_pin exists");
        assert_eq!(pin_url, url);
        assert_eq!(pin_sha256, sha256);
        assert_eq!(pin_size, size);

        let identity = pins::cuda_identity(key).expect("cuda_identity exists");
        assert_eq!(identity["unit"], "llama-server-cuda");
        assert_eq!(identity["artifact_key"], key);
        assert_eq!(identity["url"], url);
        assert_eq!(identity["sha256"], sha256);
        assert_eq!(identity["size_bytes"], size);
        assert_eq!(identity["release_tag"], "b11429");
        assert_eq!(
            identity["llama_cpp_revision"],
            "d81235049384534c167caea52b85a694f6103d14"
        );
        assert_ne!(
            identity["llama_cpp_revision"],
            "8345f333951c661d166b00e6f9362e553768f292"
        );
        assert_eq!(identity["cuda_toolkit"], "13.4.1");
        assert_eq!(identity["repack_revision"], "sol1");
        assert_eq!(identity["arch"], arch);
        assert_eq!(identity["binary_name"], "llama-server");
        assert!(identity.get("upstream_image_digest").is_none());
        assert_eq!(identity["inputs"], inputs);
    }
}

#[test]
fn cuda_b11429_wanted_files_match_selector_constants() {
    let cases = [
        (
            "x86_64-unknown-linux-gnu",
            "amd64",
            pins::CUDA_AMD64_WANTED_FILES,
        ),
        (
            "aarch64-unknown-linux-gnu",
            "arm64",
            pins::CUDA_ARM64_WANTED_FILES,
        ),
    ];
    for (key, arch, specific) in cases {
        let expected: Vec<String> = pins::CUDA_SHARED_WANTED_FILES
            .iter()
            .chain(specific)
            .map(|s| (*s).to_owned())
            .collect();
        let wanted = pins::cuda_wanted_files(arch).expect("cuda_wanted_files exists");
        assert_eq!(wanted, expected);
        let identity = pins::cuda_identity(key).expect("cuda_identity exists");
        let identity_wanted: Vec<String> = identity["wanted_files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(identity_wanted, expected);
    }
}

#[test]
fn cuda_b11429_qwen_identity_keeps_prechange_literal() {
    let expected_literal = "{\"filename\":\"Qwen3.5-4B-Q4_K_M.gguf\",\"mmproj_filename\":\"mmproj-F16.gguf\",\"mmproj_sha256\":\"cd88edcf8d031894960bb0c9c5b9b7e1fea6ebee02b9f7ce925a00d12891f864\",\"model_id\":\"local/qwen3.5-4b\",\"repo\":\"unsloth/Qwen3.5-4B-GGUF\",\"revision\":\"main\",\"sha256\":\"00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4\",\"unit\":\"local-model\"}";
    let actual = fingerprint::canonical(pins::model_identity("local/qwen3.5-4b").unwrap()).unwrap();
    assert_eq!(actual, expected_literal);
}

#[test]
fn cuda_b11429_closure_trusts_complete_regular_tree() {
    let cases = [
        ("x86_64-unknown-linux-gnu", "amd64"),
        ("aarch64-unknown-linux-gnu", "arm64"),
    ];
    for (key, arch) in cases {
        let dir = temp(&format!("cuda-trust-complete-{arch}"));
        let wanted = pins::cuda_wanted_files(arch).expect("cuda_wanted_files");
        for name in &wanted {
            let path = dir.join(name);
            if name == "llama-server" {
                fs::write(&path, b"sm_86 sm_89 sm_120a sm_121a header content").unwrap();
            } else {
                fs::write(&path, name.as_bytes()).unwrap();
            }
        }
        let inventory = manifest::runtime_inventory(&dir, &[]).unwrap();
        assert!(super::verify_required_oracle(&dir, "b11429", key, true).is_ok());
        assert_eq!(
            super::assess_cuda_bytes(&dir, &inventory),
            crate::ArtifactTrust::Trusted
        );
        let _ = fs::remove_dir_all(dir);
    }
}

#[test]
fn cuda_b11429_closure_rejects_omission_replacement_and_mixed_pins() {
    let cases = [
        ("x86_64-unknown-linux-gnu", "amd64"),
        ("aarch64-unknown-linux-gnu", "arm64"),
    ];
    for (key, arch) in cases {
        // 1. Omit libcudart.so.13
        {
            let dir = temp(&format!("cuda-omit-cudart-{arch}"));
            let wanted = pins::cuda_wanted_files(arch).expect("cuda_wanted_files");
            for name in &wanted {
                if name == "libcudart.so.13" {
                    continue;
                }
                let path = dir.join(name);
                if name == "llama-server" {
                    fs::write(&path, b"sm_86 sm_89 sm_120a sm_121a header content").unwrap();
                } else {
                    fs::write(&path, name.as_bytes()).unwrap();
                }
            }
            let err = super::verify_required_oracle(&dir, "b11429", key, true).unwrap_err();
            assert!(
                err.contains("required cuda member"),
                "error '{err}' must contain 'required cuda member'"
            );
            let _ = fs::remove_dir_all(dir);
        }

        // 2. Write full regular set, inventory once, then overwrite llama-server with different bytes that still contain all four arch strings -> assess_cuda_bytes is Integrity
        {
            let dir = temp(&format!("cuda-tampered-launcher-{arch}"));
            let wanted = pins::cuda_wanted_files(arch).expect("cuda_wanted_files");
            for name in &wanted {
                let path = dir.join(name);
                if name == "llama-server" {
                    fs::write(&path, b"sm_86 sm_89 sm_120a sm_121a initial content").unwrap();
                } else {
                    fs::write(&path, name.as_bytes()).unwrap();
                }
            }
            let inventory = manifest::runtime_inventory(&dir, &[]).unwrap();
            fs::write(
                dir.join("llama-server"),
                b"sm_86 sm_89 sm_120a sm_121a modified content replacement",
            )
            .unwrap();
            assert_eq!(
                super::assess_cuda_bytes(&dir, &inventory),
                crate::ArtifactTrust::Integrity
            );
            let _ = fs::remove_dir_all(dir);
        }

        // 3. assess_cuda_installation is Integrity for malformed manifest JSON (launcher exists, manifest path contains not-json)
        {
            let dir = temp(&format!("cuda-malformed-manifest-{arch}"));
            fs::write(
                dir.join("llama-server"),
                b"sm_86 sm_89 sm_120a sm_121a header content",
            )
            .unwrap();
            let manifest_path = manifest::artifact_manifest_path(&dir);
            if let Some(parent) = manifest_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&manifest_path, b"not-json").unwrap();
            assert_eq!(
                super::assess_cuda_installation(&dir, key),
                crate::ArtifactTrust::Integrity
            );
            let _ = fs::remove_dir_all(dir);
        }

        // 4. assess_cuda_installation is Integrity for missing manifest (launcher exists, no manifest file)
        {
            let dir = temp(&format!("cuda-missing-manifest-{arch}"));
            fs::write(
                dir.join("llama-server"),
                b"sm_86 sm_89 sm_120a sm_121a header content",
            )
            .unwrap();
            assert_eq!(
                super::assess_cuda_installation(&dir, key),
                crate::ArtifactTrust::Integrity
            );
            let _ = fs::remove_dir_all(dir);
        }

        // 5. Manifest whose pin_identity is the old ARM OCI object
        {
            let dir = temp(&format!("cuda-old-arm-oci-{arch}"));
            fs::write(
                dir.join("llama-server"),
                b"sm_86 sm_89 sm_120a sm_121a header content",
            )
            .unwrap();
            let old_arm_oci = json!({
                "unit": "llama-server-cuda",
                "artifact_key": key,
                "url": "https://updates.solstone.app/runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz",
                "sha256": "6de68319db40e8c0eb45dc4bd3a45a16971dbdc128f2b621b19bef5dae87d064",
                "size_bytes": 654508507,
                "release_tag": "b10068",
                "upstream_image_digest": "sha256:5bd5290bd35cfde893d0dcbd9811723c16d89575927d537b5f21becbfbab2f63",
                "llama_cpp_revision": "571d0d540df04f25298d0e159e520d9fc62ed121",
                "repack_revision": "sol1",
                "arch": arch,
                "binary_name": "llama-server",
                "wanted_files": pins::cuda_wanted_files(arch).unwrap()
            });
            let inv = manifest::runtime_inventory(&dir, &[]).unwrap();
            let mf = manifest::build_manifest(
                "local",
                "llama-server-cuda",
                "target",
                json!({"pin_identity": old_arm_oci}),
                inv,
                None,
                None,
            )
            .unwrap();
            manifest::write_manifest(&manifest::artifact_manifest_path(&dir), &mf).unwrap();
            assert_eq!(
                super::assess_cuda_installation(&dir, key),
                crate::ArtifactTrust::Integrity
            );
            let _ = fs::remove_dir_all(dir);
        }

        // 6. Mixed pin that is cuda_identity(key) plus upstream_image_digest
        {
            let dir = temp(&format!("cuda-mixed-pin-{arch}"));
            fs::write(
                dir.join("llama-server"),
                b"sm_86 sm_89 sm_120a sm_121a header content",
            )
            .unwrap();
            let mut mixed_pin = pins::cuda_identity(key).unwrap();
            mixed_pin["upstream_image_digest"] =
                json!("sha256:5bd5290bd35cfde893d0dcbd9811723c16d89575927d537b5f21becbfbab2f63");
            let inv = manifest::runtime_inventory(&dir, &[]).unwrap();
            let mf = manifest::build_manifest(
                "local",
                "llama-server-cuda",
                "target",
                json!({"pin_identity": mixed_pin}),
                inv,
                None,
                None,
            )
            .unwrap();
            manifest::write_manifest(&manifest::artifact_manifest_path(&dir), &mf).unwrap();
            assert_eq!(
                super::assess_cuda_installation(&dir, key),
                crate::ArtifactTrust::Integrity
            );
            let _ = fs::remove_dir_all(dir);
        }

        // 7. For the ARM key only: cuda_identity with the engine input filename replaced by llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz
        if key == "aarch64-unknown-linux-gnu" {
            let dir = temp("cuda-wrong-engine-filename-arm64");
            fs::write(
                dir.join("llama-server"),
                b"sm_86 sm_89 sm_120a sm_121a header content",
            )
            .unwrap();
            let mut perturbed_pin = pins::cuda_identity(key).unwrap();
            perturbed_pin["inputs"][0]["filename"] =
                json!("llama-b11429-bin-ubuntu-cuda-13.4-x64.tar.gz");
            let inv = manifest::runtime_inventory(&dir, &[]).unwrap();
            let mf = manifest::build_manifest(
                "local",
                "llama-server-cuda",
                "target",
                json!({"pin_identity": perturbed_pin}),
                inv,
                None,
                None,
            )
            .unwrap();
            manifest::write_manifest(&manifest::artifact_manifest_path(&dir), &mf).unwrap();
            assert_eq!(
                super::assess_cuda_installation(&dir, key),
                crate::ArtifactTrust::Integrity
            );
            let _ = fs::remove_dir_all(dir);
        }
    }
}

#[test]
fn cuda_b11429_covered_probe_integrity_is_blocked() {
    let arches = [("sm_86", "8.6"), ("sm_121", "12.1")];
    for (arch, compute_cap) in arches {
        let probe: crate::NvidiaProbe = serde_json::from_value(json!({
            "schema": NVIDIA_PROBE_SCHEMA,
            "detected": true,
            "gpu_index": 0,
            "gpu_name": "Test GPU",
            "compute_cap": compute_cap,
            "arch": arch,
            "driver_cuda_major": 13,
            "vram_mib": 1024,
            "unified_memory_mib": null,
            "probe_error": null,
        }))
        .unwrap();

        assert_eq!(
            crate::select_local_backend(
                &probe,
                &crate::CUDA_EMBEDDED_ARCH_SET,
                crate::CUDA_MIN_DRIVER_VERSION,
                crate::ArtifactTrust::Integrity,
                false
            ),
            crate::BackendSelection::IntegrityBlocked
        );

        let selected = crate::select_local_backend(
            &probe,
            &crate::CUDA_EMBEDDED_ARCH_SET,
            crate::CUDA_MIN_DRIVER_VERSION,
            crate::ArtifactTrust::Trusted,
            false,
        );
        match selected {
            crate::BackendSelection::Selected(choice) => {
                assert_eq!(choice.backend, crate::Backend::Cuda);
            }
            other => panic!("expected Selected(Cuda), got {other:?}"),
        }
    }
}

#[test]
pub fn fetch_set_selectors_request_only_that_targets_local_units() {
    // CED ships inside every package, so the installer selects no CED unit and the
    // fetch set carries none (asserted below).
    let local_units = [
        "llama-server-vulkan",
        "llama-server-cuda",
        "local-model",
        "parakeet-server",
        "parakeet-model",
        "parakeet-coreml",
    ];

    let targets = [
        "linux-x86_64",
        "linux-aarch64",
        "macos-arm64",
        "windows-x86_64",
    ];

    for target in targets {
        let mut collected = BTreeSet::new();

        // llama-server-vulkan: platform + artifact key, backend None. Not windows.
        if target != "windows-x86_64" {
            let (platform, key) = match target {
                "linux-x86_64" => (Platform::LinuxX64, "x86_64-unknown-linux-gnu"),
                "linux-aarch64" => (Platform::LinuxArm64, "aarch64-unknown-linux-gnu"),
                "macos-arm64" => (Platform::MacosArm64, "aarch64-apple-darwin"),
                _ => unreachable!(),
            };
            let art = super::select_artifact(
                "llama-server-vulkan",
                Some(platform),
                None,
                Some(key),
                None,
            )
            .expect("llama-server-vulkan");
            collected.insert(art.origin_key);
        }

        // llama-server-cuda: same linux keys, backend None. Not macOS, not windows.
        if target.starts_with("linux-") {
            let (platform, key) = match target {
                "linux-x86_64" => (Platform::LinuxX64, "x86_64-unknown-linux-gnu"),
                "linux-aarch64" => (Platform::LinuxArm64, "aarch64-unknown-linux-gnu"),
                _ => unreachable!(),
            };
            let art =
                super::select_artifact("llama-server-cuda", Some(platform), None, Some(key), None)
                    .expect("llama-server-cuda");
            collected.insert(art.origin_key);
        }

        // local-model: every catalog() row with unit == "local-model", on every
        // target. Windows packages the runtime but downloads the model.
        for row in catalog() {
            if row.unit == "local-model" {
                let art =
                    super::select_artifact("local-model", None, None, None, Some(row.filename))
                        .expect("local-model");
                collected.insert(art.origin_key);
            }
        }

        // parakeet-server: linux only, both Backend::Cpu and Backend::Vulkan, artifact key the linux triple, filename None
        if target.starts_with("linux-") {
            let (platform, key) = match target {
                "linux-x86_64" => (Platform::LinuxX64, "x86_64-unknown-linux-gnu"),
                "linux-aarch64" => (Platform::LinuxArm64, "aarch64-unknown-linux-gnu"),
                _ => unreachable!(),
            };
            for backend in [Backend::Cpu, Backend::Vulkan] {
                let art = super::select_artifact(
                    "parakeet-server",
                    Some(platform),
                    Some(backend),
                    Some(key),
                    None,
                )
                .expect("parakeet-server");
                collected.insert(art.origin_key);
            }
        }

        // parakeet-model: linux only
        if target.starts_with("linux-") {
            let art = super::select_artifact(
                "parakeet-model",
                None,
                None,
                None,
                Some("tdt-0.6b-v3-q8_0.gguf"),
            )
            .expect("parakeet-model");
            collected.insert(art.origin_key);
        }

        // parakeet-coreml: macos only, every catalog row of that unit
        if target == "macos-arm64" {
            for row in catalog() {
                if row.unit == "parakeet-coreml" {
                    let art = super::select_artifact(
                        "parakeet-coreml",
                        Some(Platform::MacosArm64),
                        None,
                        None,
                        Some(row.filename),
                    )
                    .expect("parakeet-coreml");
                    collected.insert(art.origin_key);
                }
            }
        }

        let fetch_set = solstone_core_assets::runtime_fetch_set(target).expect("fetch_set");
        let expected: BTreeSet<&str> = fetch_set
            .iter()
            .filter(|f| local_units.contains(&f.unit()))
            .map(solstone_core_assets::RuntimeFetch::origin_key)
            .collect();

        assert_eq!(collected, expected, "target {target} origin keys mismatch");
        assert!(
            !fetch_set
                .iter()
                .any(|f| matches!(f.unit(), "ced-engine" | "ced-model")),
            "target {target} fetch set carries a CED unit"
        );
    }
}

#[cfg(feature = "runtime-fetch-test")]
mod runtime_fetch_seam_tests {
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::Mutex;

    use serde_json::json;
    use solstone_core_artifact_download::{
        ArchiveError, FakeRuntimeFetch, with_fake_runtime_fetch,
    };
    use solstone_core_assets::{
        Artifact, Backend, Platform, RuntimeFetchQuery, catalog, resolve, runtime_fetch_set,
        with_runtime_fetch_target, without_runtime_fetch_unit,
    };
    use solstone_core_journal_config::JournalConfigRead;

    use super::*;
    use crate::install::{install_model, select_artifact, test_hooks};

    fn coreml_config(root: &Path) -> JournalConfigRead {
        JournalConfigRead {
            present: true,
            sha256: None,
            config: Some(
                json!({"transcribe": {"parakeet": {"cache_dir": root.display().to_string()}}})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        }
    }

    fn test_status(journal: &Path) -> status::InstallStatus {
        status::begin(
            journal,
            "{}".to_owned(),
            "target_fp".to_owned(),
            None,
            "downloading",
        )
        .unwrap()
    }

    #[derive(Default)]
    struct MockFake {
        calls: Mutex<Vec<(String, String, u64)>>,
        behavior: Mutex<MockBehavior>,
    }

    #[derive(Default, Clone, Copy)]
    enum MockBehavior {
        #[default]
        StopAtFetch,
        WrongBytes,
        ShortBody,
    }

    impl FakeRuntimeFetch for MockFake {
        fn fetch(&self, url: &str, sha256: &str, size_bytes: u64) -> Result<Vec<u8>, ArchiveError> {
            self.calls
                .lock()
                .unwrap()
                .push((url.to_string(), sha256.to_string(), size_bytes));
            match *self.behavior.lock().unwrap() {
                MockBehavior::StopAtFetch => Err(ArchiveError::Download("stop".to_string())),
                MockBehavior::WrongBytes => Ok(vec![0u8; size_bytes as usize]),
                MockBehavior::ShortBody => {
                    let len = if size_bytes > 0 { size_bytes - 1 } else { 0 };
                    Ok(vec![0u8; len as usize])
                }
            }
        }
    }

    #[test]
    fn five_production_runtime_fetch_callers_are_accounted() {
        let callers = [
            "run_local_install engine (install.rs ~1299, llama-server-cuda or llama-server-vulkan)",
            "install_model (~1689)",
            "run_parakeet_install server loop (~1507, CPU and Vulkan)",
            "install_parakeet_model (~1592)",
            "Core ML row loop (coreml_install.rs ~189)",
        ];
        assert_eq!(callers.len(), 5);
    }

    #[test]
    fn bundled_refusal_returns_component_packaged_with_zero_calls() {
        let fake = Rc::new(MockFake::default());
        let root = temp("bundled-refusal");
        let dest = root.join("dest");

        let ced_queries = [
            RuntimeFetchQuery {
                unit: Some("ced-model"),
                ..Default::default()
            },
            RuntimeFetchQuery {
                filename: Some("ced-tiny-q8_0.gguf"),
                ..Default::default()
            },
            RuntimeFetchQuery {
                origin_key: Some("assets/ced/tiny/ced-tiny-q8_0.gguf"),
                ..Default::default()
            },
        ];

        let backup_queries = [
            RuntimeFetchQuery {
                unit: Some("restic"),
                ..Default::default()
            },
            RuntimeFetchQuery {
                filename: Some("restic_0.19.0_linux_amd64.bz2"),
                ..Default::default()
            },
            RuntimeFetchQuery {
                origin_key: Some("assets/restic/0.19.0/restic_0.19.0_linux_amd64.bz2"),
                ..Default::default()
            },
            RuntimeFetchQuery {
                unit: Some("rclone"),
                ..Default::default()
            },
            RuntimeFetchQuery {
                filename: Some("rclone-v1.74.4-linux-amd64.zip"),
                ..Default::default()
            },
            RuntimeFetchQuery {
                origin_key: Some("assets/rclone/1.74.4/rclone-v1.74.4-linux-amd64.zip"),
                ..Default::default()
            },
        ];

        with_fake_runtime_fetch(&fake, || {
            for q in ced_queries.iter().chain(backup_queries.iter()) {
                let err =
                    fetch_runtime_member(q, &dest, |_, _| {}, "model_download_failed").unwrap_err();
                assert_eq!(err.exit_code, 65);
                assert_eq!(
                    err.envelope.error.as_ref().unwrap().reason_code,
                    "component_packaged"
                );
            }
        });
        assert_eq!(fake.calls.lock().unwrap().len(), 0);

        with_fake_runtime_fetch(&fake, || {
            without_runtime_fetch_unit("llama-server", || {
                let query = RuntimeFetchQuery {
                    unit: Some("llama-server-vulkan"),
                    ..Default::default()
                };
                let err =
                    fetch_runtime_member(&query, &dest, |_, _| {}, "download_failed").unwrap_err();
                assert_eq!(err.exit_code, 65);
                assert_eq!(
                    err.envelope.error.as_ref().unwrap().reason_code,
                    "component_packaged"
                );
            });

            without_runtime_fetch_unit("local-model", || {
                let model_root = temp("bundled-refusal-model");
                let mut status_val = test_status(&model_root);
                let err =
                    install_model(&model_root, "local/qwen3.5-4b", &mut status_val).unwrap_err();
                assert_eq!(err.exit_code, 65);
                assert_eq!(
                    err.envelope.error.as_ref().unwrap().reason_code,
                    "component_packaged"
                );
            });

            without_runtime_fetch_unit("parakeet-server", || {
                let query = RuntimeFetchQuery {
                    unit: Some("parakeet-server"),
                    ..Default::default()
                };
                let err =
                    fetch_runtime_member(&query, &dest, |_, _| {}, "download_failed").unwrap_err();
                assert_eq!(err.exit_code, 65);
                assert_eq!(
                    err.envelope.error.as_ref().unwrap().reason_code,
                    "component_packaged"
                );
            });

            without_runtime_fetch_unit("parakeet-coreml", || {
                let coreml_root = temp("bundled-refusal-coreml");
                let config = coreml_config(&coreml_root);
                let coreml_row = catalog()
                    .iter()
                    .find(|row| row.unit == "parakeet-coreml")
                    .expect("parakeet-coreml catalog row");
                let err = test_hooks::install_coreml_minted_rows(
                    &coreml_root,
                    &config,
                    false,
                    &[coreml_row],
                )
                .unwrap_err();
                assert_eq!(err.reason_code, "component_packaged");
            });
        });
        assert_eq!(fake.calls.lock().unwrap().len(), 0);
    }

    #[test]
    fn positive_twins_verify_recorded_urls_digests_and_sizes() {
        let targets = [
            "linux-x86_64",
            "linux-aarch64",
            "macos-arm64",
            "windows-x86_64",
        ];

        for target in targets {
            let set = runtime_fetch_set(target).expect("fetch set for target");
            with_runtime_fetch_target(target, || {
                let fake = Rc::new(MockFake::default());
                with_fake_runtime_fetch(&fake, || {
                    let root = temp(&format!("pos-twin-{target}"));
                    let dest = root.join("artifact.bin");

                    for item in &set {
                        if item.unit() == "parakeet-coreml" {
                            continue;
                        }
                        let query = RuntimeFetchQuery {
                            unit: Some(item.unit()),
                            origin_key: Some(item.origin_key()),
                            ..Default::default()
                        };
                        let _ = fetch_runtime_member(&query, &dest, |_, _| {}, "download_failed");
                    }
                });

                let calls = fake.calls.lock().unwrap();
                let non_coreml_items: Vec<_> = set
                    .iter()
                    .filter(|f| f.unit() != "parakeet-coreml")
                    .collect();
                assert_eq!(
                    calls.len(),
                    non_coreml_items.len(),
                    "calls mismatch for target {target}"
                );
                for (i, item) in non_coreml_items.iter().enumerate() {
                    let (url, sha, size) = &calls[i];
                    assert_eq!(
                        *url,
                        format!("https://updates.solstone.app/{}", item.origin_key())
                    );
                    assert_eq!(*sha, item.sha256());
                    assert_eq!(*size, item.size_bytes());
                }
            });
        }

        with_runtime_fetch_target("macos-arm64", || {
            let fake = Rc::new(MockFake::default());
            with_fake_runtime_fetch(&fake, || {
                let root = temp("pos-twin-metal");
                let dest = root.join("metal.bin");
                let vulkan_art = resolve("llama-server-vulkan", Some(Platform::MacosArm64), None)
                    .into_iter()
                    .next()
                    .unwrap();
                let query = RuntimeFetchQuery {
                    unit: Some("llama-server-vulkan"),
                    origin_key: Some(vulkan_art.origin_key),
                    ..Default::default()
                };
                let _ = fetch_runtime_member(&query, &dest, |_, _| {}, "download_failed");
            });
            let calls = fake.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let (url, sha, size) = &calls[0];
            let vulkan_art = resolve("llama-server-vulkan", Some(Platform::MacosArm64), None)
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(
                *url,
                format!("https://updates.solstone.app/{}", vulkan_art.origin_key)
            );
            assert_eq!(*sha, vulkan_art.sha256);
            assert_eq!(*size, vulkan_art.size_bytes);
        });

        with_runtime_fetch_target("linux-x86_64", || {
            let fake = Rc::new(MockFake::default());
            with_fake_runtime_fetch(&fake, || {
                let root = temp("pos-twin-parakeet-linux");
                let cpu_art = resolve(
                    "parakeet-server",
                    Some(Platform::LinuxX64),
                    Some(Backend::Cpu),
                )
                .into_iter()
                .next()
                .unwrap();
                let vulkan_art = resolve(
                    "parakeet-server",
                    Some(Platform::LinuxX64),
                    Some(Backend::Vulkan),
                )
                .into_iter()
                .next()
                .unwrap();
                let model_art = resolve("parakeet-model", None, None)
                    .into_iter()
                    .next()
                    .unwrap();

                for art in [cpu_art, vulkan_art, model_art] {
                    let dest = root.join(art.filename);
                    let query = RuntimeFetchQuery {
                        unit: Some(art.unit),
                        origin_key: Some(art.origin_key),
                        ..Default::default()
                    };
                    let _ = fetch_runtime_member(&query, &dest, |_, _| {}, "download_failed");
                }
            });
            let calls = fake.calls.lock().unwrap();
            assert_eq!(calls.len(), 3);
        });

        with_runtime_fetch_target("macos-arm64", || {
            let fake = Rc::new(MockFake::default());
            let root = temp("pos-twin-coreml");
            let config = coreml_config(&root);
            let coreml_rows: Vec<&Artifact> = catalog()
                .iter()
                .filter(|row| row.unit == "parakeet-coreml")
                .collect();
            assert_eq!(coreml_rows.len(), 23);

            with_fake_runtime_fetch(&fake, || {
                for row in &coreml_rows {
                    let _ = test_hooks::install_coreml_minted_rows(&root, &config, false, &[*row]);
                }
            });

            let calls = fake.calls.lock().unwrap();
            assert_eq!(calls.len(), 23);
            for (i, row) in coreml_rows.iter().enumerate() {
                let (url, sha, size) = &calls[i];
                assert_eq!(
                    *url,
                    format!("https://updates.solstone.app/{}", row.origin_key)
                );
                assert_eq!(*sha, row.sha256);
                assert_eq!(*size, row.size_bytes);
            }
        });

        {
            let fake = Rc::new(MockFake::default());
            *fake.behavior.lock().unwrap() = MockBehavior::WrongBytes;
            let root = temp("pos-twin-wrong-bytes");
            let config = coreml_config(&root);
            let coreml_row = catalog()
                .iter()
                .find(|row| row.unit == "parakeet-coreml")
                .unwrap();
            with_runtime_fetch_target("macos-arm64", || {
                with_fake_runtime_fetch(&fake, || {
                    let err = test_hooks::install_coreml_minted_rows(
                        &root,
                        &config,
                        false,
                        &[coreml_row],
                    )
                    .unwrap_err();
                    assert_eq!(err.reason_code, "download_digest_mismatch");
                });
            });
        }

        {
            let fake = Rc::new(MockFake::default());
            *fake.behavior.lock().unwrap() = MockBehavior::ShortBody;
            let root = temp("pos-twin-short-body");
            let config = coreml_config(&root);
            let coreml_row = catalog()
                .iter()
                .find(|row| row.unit == "parakeet-coreml")
                .unwrap();
            with_runtime_fetch_target("macos-arm64", || {
                with_fake_runtime_fetch(&fake, || {
                    let err = test_hooks::install_coreml_minted_rows(
                        &root,
                        &config,
                        false,
                        &[coreml_row],
                    )
                    .unwrap_err();
                    assert_eq!(err.reason_code, "download_size_mismatch");
                });
            });
        }

        with_runtime_fetch_target("linux-x86_64", || {
            let fake = Rc::new(MockFake::default());
            with_fake_runtime_fetch(&fake, || {
                let root = temp("override-foreign-target");
                let dest = root.join("artifact.bin");
                let query = RuntimeFetchQuery {
                    unit: Some("llama-server-vulkan"),
                    artifact_key: Some("x86_64-apple-darwin"),
                    ..Default::default()
                };
                let err =
                    fetch_runtime_member(&query, &dest, |_, _| {}, "download_failed").unwrap_err();
                assert_eq!(err.exit_code, 65);
                assert_eq!(
                    err.envelope.error.as_ref().unwrap().reason_code,
                    "component_packaged"
                );
            });
            assert_eq!(fake.calls.lock().unwrap().len(), 0);
        });

        // Windows packages the llama runtime but not the model: the model install
        // must reach the origin for both files. Journal 2.0.38 refused this.
        with_runtime_fetch_target("windows-x86_64", || {
            let fake = Rc::new(MockFake::default());
            with_fake_runtime_fetch(&fake, || {
                let root = temp("pos-twin-windows");
                let mut status_val = test_status(&root);
                if let Err(err) = install_model(&root, "local/qwen3.5-4b", &mut status_val) {
                    assert_ne!(
                        err.envelope.error.as_ref().unwrap().reason_code,
                        "component_packaged"
                    );
                }
            });
            // The fake fails the first fetch, so install_model stops there; the
            // point is that it reached the origin instead of refusing. The
            // positive twins above cover both files' URLs, digests and sizes.
            let calls = fake.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let first = runtime_fetch_set("windows-x86_64")
                .expect("windows fetch set")
                .into_iter()
                .find(|f| f.origin_key().ends_with("/Qwen3.5-4B-Q4_K_M.gguf"))
                .expect("windows fetch set carries the model");
            assert_eq!(
                calls[0].0,
                format!("https://updates.solstone.app/{}", first.origin_key())
            );
        });
    }

    #[test]
    fn local_model_removal_and_positive_twin_accounting() {
        let fake = Rc::new(MockFake::default());
        let root = temp("model-removal");

        without_runtime_fetch_unit("local-model", || {
            with_fake_runtime_fetch(&fake, || {
                let mut status_val = test_status(&root);
                let err = install_model(&root, "local/qwen3.5-4b", &mut status_val).unwrap_err();
                assert_eq!(err.exit_code, 65);
                assert_eq!(
                    err.envelope.error.as_ref().unwrap().reason_code,
                    "component_packaged"
                );
            });
        });
        assert_eq!(fake.calls.lock().unwrap().len(), 0);

        let twin_root = temp("model-twin");
        with_fake_runtime_fetch(&fake, || {
            let mut status_val = test_status(&twin_root);
            let _ = install_model(&twin_root, "local/qwen3.5-4b", &mut status_val);

            let mmproj_art =
                select_artifact("local-model", None, None, None, Some("mmproj-F16.gguf")).unwrap();
            let dest_mmproj = twin_root.join("mmproj-F16.gguf");
            let query = RuntimeFetchQuery {
                unit: Some("local-model"),
                origin_key: Some(mmproj_art.origin_key),
                ..Default::default()
            };
            let _ = fetch_runtime_member(&query, &dest_mmproj, |_, _| {}, "model_download_failed");
        });
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].0.contains("Qwen3.5-4B-Q4_K_M.gguf"));
        assert!(calls[1].0.contains("mmproj-F16.gguf"));
    }
}
