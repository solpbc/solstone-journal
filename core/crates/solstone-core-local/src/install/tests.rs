// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use super::{
    InstallVerb, archive, cleanup_legacy_cuda_oci_dirs, dispatch, fingerprint, lease,
    local_backend_choice, manifest, pins, publish_staged_tree_with, readiness, status,
};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};

use crate::nvidia::NVIDIA_PROBE_SCHEMA;

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("solstone-local-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn status_corpus_has_exact_case_count() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../../fixtures/install_status.json")).unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 63);
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
fn mlx_target_fingerprint_matches_python_reference() {
    // Captured with:
    // python3 -c 'from solstone.think.models import QWEN_35_9B; from solstone.think.providers import mlx_install; print(mlx_install.target_fingerprint(QWEN_35_9B))'
    // on the current reference tree, for the Qwen MLX registry entry.
    let expected_json = r#"{"model_pin":{"model_id":"qwen3.5:9b","repo":"mlx-community/Qwen3.5-9B-MLX-8bit","revision":"84f7c2deea248d8df56240f88102def51c7ed5d6","soft_token_budget":null,"unit":"mlx-snapshot"},"provider":"local","runtime":"mlx"}"#;
    let mut input = serde_json::from_str::<serde_json::Map<String, Value>>(expected_json).unwrap();
    input.remove("provider");
    let actual = fingerprint::mlx_fingerprint(input).unwrap();
    assert_eq!(actual["target_fingerprint_json"], expected_json);
    assert_eq!(
        actual["target_fingerprint_sha256"],
        "04cb931cecdab39e1b7b92d675b36f06e06eee492ebbb606d41a4a8c1fec7ce5"
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

    let mlx = dispatch(
        InstallVerb::FingerprintMlx,
        json!({"journal":root,"model_id":"qwen3.5:9b"}),
    )
    .unwrap();
    let mlx = mlx.result.unwrap();
    assert_eq!(
        mlx["target_fingerprint_json"],
        r#"{"model_pin":{"model_id":"qwen3.5:9b","repo":"mlx-community/Qwen3.5-9B-MLX-8bit","revision":"84f7c2deea248d8df56240f88102def51c7ed5d6","soft_token_budget":null,"unit":"mlx-snapshot"},"provider":"local","runtime":"mlx"}"#
    );
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
fn failed_local_publish_restores_the_existing_tree() {
    assert_failed_publish_restores_the_existing_tree("local-publish-rollback");
}

#[test]
fn failed_mlx_publish_restores_the_existing_tree() {
    assert_failed_publish_restores_the_existing_tree("mlx-publish-rollback");
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
fn backend_choice_requires_a_trusted_cuda_artifact() {
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
    let absent = local_backend_choice(&root, Some(probe.clone()));
    assert_eq!(absent.backend, crate::Backend::Vulkan);
    let artifact = pins::cache_root(&root)
        .join("cuda")
        .join(&key)
        .join(digest)
        .join("llama-server");
    fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    fs::write(&artifact, b"sm_86 sm_89 sm_120a sm_121a").unwrap();
    let trusted = local_backend_choice(&root, Some(probe));
    assert_eq!(trusted.backend, crate::Backend::Cuda);
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
fn download_digest_mismatch_removes_destination_and_partial_file() {
    let root = temp("download-mismatch");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
            .unwrap();
    });
    let destination = root.join("artifact.tar.gz");
    assert!(matches!(
        archive::download(
            &format!("http://{address}"),
            &destination,
            "00",
            |_received, _total| {}
        ),
        Err(archive::ArchiveError::DigestMismatch)
    ));
    server.join().unwrap();
    assert!(!destination.exists());
    assert!(!root.join(".artifact.tar.gz.part").exists());
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
        status::bump_progress(state, Some(2), None, &mut clock)
            .unwrap()
            .is_none()
    );
    thread::sleep(Duration::from_millis(1050));
    let state = status::read_status(&root, "local").unwrap();
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
fn two_real_processes_cannot_hold_the_same_lease() {
    if let Ok(root) = std::env::var("SOLSTONE_LOCAL_LEASE_HELPER") {
        assert!(
            lease::acquire(&PathBuf::from(root), "local")
                .unwrap()
                .is_none()
        );
        return;
    }
    let root = temp("two-process-lease");
    let _held = lease::acquire(&root, "local").unwrap().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("install::tests::two_real_processes_cannot_hold_the_same_lease")
        .env("SOLSTONE_LOCAL_LEASE_HELPER", &root)
        .status()
        .unwrap();
    assert!(status.success());
    let mut state = status::idle_status("local");
    state.target_fingerprint_json = Some("{}".to_owned());
    state.target_fingerprint_sha256 = Some("x".to_owned());
    let state = status::transition(state, "downloading", None, None).unwrap();
    let written = status::write_status(&root, state).unwrap();
    assert_eq!(written.revision, 1);
    assert!(
        serde_json::from_slice::<Value>(&fs::read(status::status_path(&root, "local")).unwrap())
            .is_ok()
    );
    let _ = fs::remove_dir_all(root);
}
