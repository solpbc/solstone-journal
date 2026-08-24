// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::gate::assert_head_targets_correspond;
use crate::guard::{
    GuardError, PinOwner, PruneAssessment, assess_prune, assess_prune_with_current_support,
    require_prunable,
};
use crate::mirror::{
    MULTIPART_THRESHOLD_BYTES, MirrorError, MirrorTarget, PublishMode, UpstreamHostPolicy,
    UpstreamMetadataKind, UpstreamVerification, consume_upstream_body_for_test,
    current_mirror_targets, github_release_digest_verification_for_test,
    huggingface_etag_verification_for_test, parse_absolute_url_for_test, provenance_row_for_test,
    resolve_location_for_test, select_publish_mode, validate_url_for_test,
};
use crate::pins::{
    PinsError, authority_origin_pins_from_test_path, historical_origin_pins, snapshot_versions,
    supported_release_versions, supported_release_versions_from_test_path,
};

fn temp(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "solstone-origin-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn upstream_policy() -> UpstreamHostPolicy<'static> {
    UpstreamHostPolicy {
        allowed_hosts: &["127.0.0.1"],
        allow_http: true,
    }
}

fn mirror_target(base: &str, bytes: &[u8], kind: UpstreamMetadataKind) -> MirrorTarget {
    MirrorTarget {
        origin_key: "assets/fixture/file.bin".to_owned(),
        sha256: sha256(bytes),
        size_bytes: bytes.len() as u64,
        upstream_url: format!("{base}/upstream"),
        unit: "fixture".to_owned(),
        version: "fixture".to_owned(),
        filename: "file.bin".to_owned(),
        metadata_kind: kind,
        metadata_url: format!("{base}/metadata"),
    }
}

#[test]
fn head_target_derivation_corresponds_without_a_growing_count() {
    assert_head_targets_correspond().unwrap();
}

#[test]
fn snapshot_files_and_transparency_log_are_bijective() {
    assert_eq!(
        snapshot_versions().unwrap(),
        supported_release_versions().unwrap()
    );
}

#[test]
fn mlx_shaped_unknown_key_refuses_by_default() {
    assert_eq!(
        assess_prune_with_current_support("assets/mlx-model/snapshot/weights.safetensors").unwrap(),
        PruneAssessment::Unknown
    );
}

#[test]
fn head_only_key_names_head_unreleased() {
    let assessment = assess_prune_with_current_support(
        "assets/ced-model/b5e9a4aad6438763c8da16079d77563fbed35c65/ced-tiny-q8_0.gguf",
    )
    .unwrap();
    assert_eq!(
        assessment,
        PruneAssessment::PinnedBy {
            owners: vec![PinOwner::HeadUnreleased],
        }
    );
}

#[test]
fn github_release_digest_matches_named_asset() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"mirror-body",
        UpstreamMetadataKind::GithubRelease,
    );
    let body = format!(
        r#"{{"assets":[{{"name":"file.bin","digest":"sha256:{}"}}]}}"#,
        target.sha256
    );
    assert_eq!(
        github_release_digest_verification_for_test(&target, &body).unwrap(),
        UpstreamVerification::UpstreamSha256
    );
}

#[test]
fn github_release_digest_rejects_wrong_digest() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"mirror-body",
        UpstreamMetadataKind::GithubRelease,
    );
    let body = r#"{"assets":[{"name":"file.bin","digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}]}"#;
    assert!(matches!(
        github_release_digest_verification_for_test(&target, body),
        Err(MirrorError::UnverifiedUpstream { .. })
    ));
}

#[test]
fn github_release_digest_rejects_wrong_filename() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"mirror-body",
        UpstreamMetadataKind::GithubRelease,
    );
    let body = format!(
        r#"{{"assets":[{{"name":"other.bin","digest":"sha256:{}"}}]}}"#,
        target.sha256
    );
    assert!(matches!(
        github_release_digest_verification_for_test(&target, &body),
        Err(MirrorError::UnverifiedUpstream { .. })
    ));
}

#[test]
fn github_release_digest_rejects_non_json_body() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"mirror-body",
        UpstreamMetadataKind::GithubRelease,
    );
    assert!(matches!(
        github_release_digest_verification_for_test(&target, "not json"),
        Err(MirrorError::UnverifiedUpstream { .. })
    ));
}

#[test]
fn huggingface_etag_sha256_matches_pin() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"lfs-hf",
        UpstreamMetadataKind::HuggingFace,
    );
    let (verification, expected_blob_oid) =
        huggingface_etag_verification_for_test(&target, &format!("sha256:{}", target.sha256))
            .unwrap();
    assert_eq!(verification, UpstreamVerification::UpstreamSha256);
    assert_eq!(expected_blob_oid, None);
}

#[test]
fn huggingface_git_blob_metadata_routes_without_upstream_bytes() {
    const BLOB_OID: &str = "688882a700000000000000000000000000000000";
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"synthetic",
        UpstreamMetadataKind::HuggingFace,
    );
    let (verification, expected_blob_oid) =
        huggingface_etag_verification_for_test(&target, BLOB_OID).unwrap();
    assert_eq!(verification, UpstreamVerification::UpstreamGitBlobSha1);
    assert_eq!(expected_blob_oid.as_deref(), Some(BLOB_OID));
}

#[test]
fn huggingface_etag_rejects_neither_form() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"lfs-hf",
        UpstreamMetadataKind::HuggingFace,
    );
    assert!(matches!(
        huggingface_etag_verification_for_test(&target, "not-a-digest"),
        Err(MirrorError::UnverifiedUpstream { .. })
    ));
}

#[test]
fn validate_url_accepts_loopback_when_allowed() {
    assert_eq!(
        validate_url_for_test("http://127.0.0.1:9/upstream", &upstream_policy()).unwrap(),
        "http://127.0.0.1:9/upstream"
    );
}

#[test]
fn validate_url_refuses_disallowed_host() {
    assert!(matches!(
        validate_url_for_test("http://127.0.0.2:9/object", &upstream_policy()),
        Err(MirrorError::UpstreamHostRefused { .. })
    ));
}

#[test]
fn resolve_location_accepts_root_relative_path() {
    assert_eq!(
        parse_absolute_url_for_test("http://127.0.0.1:9/upstream").unwrap(),
        "http://127.0.0.1:9/upstream"
    );
    assert_eq!(
        resolve_location_for_test("http://127.0.0.1:9/upstream", "/object").unwrap(),
        "http://127.0.0.1:9/object"
    );
}

#[test]
fn provenance_row_fields_are_stable_for_a_fixed_timestamp() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"shared-read-back",
        UpstreamMetadataKind::GithubRelease,
    );
    let first = provenance_row_for_test(
        &target,
        UpstreamVerification::UpstreamSha256,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let second = provenance_row_for_test(
        &target,
        UpstreamVerification::UpstreamSha256,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first["origin_key"], target.origin_key);
    assert_eq!(first["pin_sha256"], target.sha256);
    assert_eq!(first["read_back"], "sha256");
    assert_eq!(first["size_bytes"], target.size_bytes);
    assert_eq!(first["timestamp"], "2026-01-01T00:00:00Z");
    assert_eq!(first["unit"], target.unit);
    assert_eq!(first["upstream_url"], target.upstream_url);
    assert_eq!(first["verified"], "upstream-sha256");
    assert_eq!(first["version"], target.version);
}

#[test]
fn provenance_row_verified_serializes_git_blob_kebab_case() {
    let target = mirror_target(
        "http://127.0.0.1:1",
        b"{}",
        UpstreamMetadataKind::HuggingFace,
    );
    let sha256_row = provenance_row_for_test(
        &target,
        UpstreamVerification::UpstreamSha256,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let git_blob_row = provenance_row_for_test(
        &target,
        UpstreamVerification::UpstreamGitBlobSha1,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    assert_eq!(sha256_row["verified"], "upstream-sha256");
    assert_eq!(git_blob_row["verified"], "upstream-git-blob-sha1");
}

#[test]
fn consume_upstream_body_accepts_matching_git_blob() {
    const CONFIG_JSON_BLOB_OID: &str = "9e26dfeeb6e641a33dae4961196235bdb965b21b";
    let body = b"{}";
    let mut target = mirror_target(
        "http://127.0.0.1:1",
        body,
        UpstreamMetadataKind::HuggingFace,
    );
    target.origin_key =
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/config.json".to_owned();
    consume_upstream_body_for_test(
        &target,
        &temp("consume-match").join("body"),
        body,
        Some(CONFIG_JSON_BLOB_OID),
    )
    .unwrap();
}

#[test]
fn consume_upstream_body_rejects_git_blob_oid_that_does_not_match_bytes() {
    // consume_upstream_body is called from download_upstream, which mirror_one
    // reaches before backend.publish. An Err here is therefore the same
    // reject-before-publish point as the real-boundary counterpart.
    let body = b"unlisted-hf";
    let target = mirror_target(
        "http://127.0.0.1:1",
        body,
        UpstreamMetadataKind::HuggingFace,
    );
    assert!(matches!(
        consume_upstream_body_for_test(
            &target,
            &temp("consume-mismatch").join("body"),
            body,
            Some("688882a700000000000000000000000000000000"),
        ),
        Err(MirrorError::UpstreamGitBlobDigestMismatch { .. })
    ));
}

#[test]
fn current_mirror_targets_exclude_the_cuda_origin_rows_by_name() {
    let mirrored = current_mirror_targets()
        .unwrap()
        .into_iter()
        .map(|target| target.origin_key)
        .collect::<std::collections::BTreeSet<_>>();
    for artifact in solstone_core_assets::catalog()
        .iter()
        .filter(|artifact| artifact.unit == "llama-server-cuda")
    {
        assert!(!mirrored.contains(artifact.origin_key));
    }
    for artifact in solstone_core_assets::catalog()
        .iter()
        .filter(|artifact| artifact.unit != "llama-server-cuda")
    {
        assert!(mirrored.contains(artifact.origin_key));
    }
}

#[test]
fn guard_refuses_sol1_nvattest_naming_every_pinning_release() {
    let key = "providers/nvattest/libnvat-linux-x86_64-1.2.2-sol.1-archive.tar.xz";
    let expected = historical_origin_pins()
        .unwrap()
        .into_iter()
        .filter(|(_, pins)| pins.iter().any(|pin| pin.origin_key == key))
        .map(|(version, _)| PinOwner::Release(version))
        .collect::<Vec<_>>();
    let error = require_prunable(key).unwrap_err();
    match error {
        GuardError::Refused {
            origin_key,
            assessment: PruneAssessment::PinnedBy { owners },
        } => {
            assert_eq!(origin_key, key);
            assert_eq!(owners, expected);
        }
        error => panic!("expected pinned refusal, got {error:?}"),
    }
}

#[test]
fn guard_sol1_becomes_prunable_when_support_excludes_those_releases() {
    let key = "providers/nvattest/libnvat-linux-x86_64-1.2.2-sol.1-archive.tar.xz";
    let historical = historical_origin_pins().unwrap();
    let mut support = supported_release_versions().unwrap();
    for version in historical
        .iter()
        .filter(|(_, pins)| pins.iter().any(|pin| pin.origin_key == key))
        .map(|(version, _)| version)
    {
        support.remove(version);
    }
    assert!(matches!(
        require_prunable(key),
        Err(GuardError::Refused { .. })
    ));
    assert_eq!(
        assess_prune(key, &support).unwrap(),
        PruneAssessment::Prunable
    );
}

#[test]
fn guard_names_all_pinning_releases_and_head_for_a_cuda_key() {
    let key = solstone_core_assets::catalog()
        .iter()
        .find(|artifact| artifact.unit == "llama-server-cuda")
        .unwrap()
        .origin_key;
    let mut expected = historical_origin_pins()
        .unwrap()
        .into_iter()
        .filter(|(_, pins)| pins.iter().any(|pin| pin.origin_key == key))
        .map(|(version, _)| PinOwner::Release(version))
        .collect::<Vec<_>>();
    expected.push(PinOwner::HeadUnreleased);
    match require_prunable(key).unwrap_err() {
        GuardError::Refused {
            origin_key,
            assessment: PruneAssessment::PinnedBy { owners },
        } => {
            assert_eq!(origin_key, key);
            assert_eq!(owners, expected);
        }
        error => panic!("expected pinned refusal, got {error:?}"),
    }
}

#[test]
fn guard_refusal_display_names_releases_and_head_for_operators() {
    let key = solstone_core_assets::catalog()
        .iter()
        .find(|artifact| artifact.unit == "llama-server-cuda")
        .unwrap()
        .origin_key;
    let owners = historical_origin_pins()
        .unwrap()
        .into_keys()
        .map(PinOwner::Release)
        .chain(std::iter::once(PinOwner::HeadUnreleased))
        .map(|owner| match owner {
            PinOwner::Release(version) => version,
            PinOwner::HeadUnreleased => "HEAD (unreleased)".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        require_prunable(key).unwrap_err().to_string(),
        format!("refusing to prune {key}: pinned by {owners}")
    );
}

#[test]
fn guard_unknown_is_not_convertible_to_permission() {
    let unknown: Result<(), GuardError> = require_prunable("assets/mlx-model/unknown");
    assert!(matches!(unknown, Err(GuardError::Refused { .. })));
    let pinned: Result<(), GuardError> =
        require_prunable("providers/nvattest/libnvat-linux-x86_64-1.2.2-sol.1-archive.tar.xz");
    assert!(matches!(pinned, Err(GuardError::Refused { .. })));
}

#[test]
fn mirror_selects_single_shot_at_threshold_and_multipart_one_byte_above() {
    assert_eq!(
        select_publish_mode(MULTIPART_THRESHOLD_BYTES),
        PublishMode::SingleShot
    );
    assert_eq!(
        select_publish_mode(MULTIPART_THRESHOLD_BYTES + 1),
        PublishMode::Multipart
    );
}

#[test]
fn pins_fail_loudly_when_the_authority_is_missing_or_malformed() {
    let root = temp("authority-errors");
    let missing = root.join("missing.json");
    let missing_result = authority_origin_pins_from_test_path(&missing);
    assert!(matches!(
        missing_result,
        Err(PinsError::AuthorityRead { .. })
    ));
    let malformed = root.join("malformed.json");
    fs::write(&malformed, "not json").unwrap();
    let malformed_result = authority_origin_pins_from_test_path(&malformed);
    assert!(matches!(
        malformed_result,
        Err(PinsError::AuthorityParse { .. })
    ));
}

#[test]
fn pins_fail_loudly_when_the_authority_has_no_targets() {
    let path = temp("authority-empty").join("authority.json");
    fs::write(&path, r#"{"targets":{}}"#).unwrap();
    assert!(matches!(
        authority_origin_pins_from_test_path(&path),
        Err(PinsError::AuthorityTargetsEmpty { .. })
    ));
}

#[test]
fn pins_fail_loudly_when_the_transparency_log_is_missing_or_malformed() {
    let root = temp("transparency-errors");
    let missing = root.join("missing.jsonl");
    let missing_result = supported_release_versions_from_test_path(&missing);
    assert!(matches!(
        missing_result,
        Err(PinsError::TransparencyLogRead { .. })
    ));
    let malformed = root.join("malformed.jsonl");
    fs::write(&malformed, "not json\n").unwrap();
    let malformed_result = supported_release_versions_from_test_path(&malformed);
    assert!(matches!(
        malformed_result,
        Err(PinsError::TransparencyLogParse { .. })
    ));
}

#[test]
fn pins_fail_loudly_when_the_transparency_log_has_no_release_rows() {
    let path = temp("transparency-empty").join("log.jsonl");
    fs::write(&path, "\n").unwrap();
    assert!(matches!(
        supported_release_versions_from_test_path(&path),
        Err(PinsError::TransparencyLogEmpty { .. })
    ));
}

#[test]
fn pin_snapshots_old_nvattest_is_x86_64_only() {
    let old_key = "providers/nvattest/libnvat-linux-x86_64-1.2.2-sol.1-archive.tar.xz";
    for pins in historical_origin_pins()
        .unwrap()
        .into_values()
        .filter(|pins| pins.iter().any(|pin| pin.origin_key == old_key))
    {
        assert!(
            pins.iter()
                .filter(|pin| pin.unit == "nvattest")
                .all(|pin| pin.origin_key == old_key)
        );
    }
}

#[test]
fn pin_snapshots_record_sol1_nvattest_for_exactly_its_measured_releases() {
    let key = "providers/nvattest/libnvat-linux-x86_64-1.2.2-sol.1-archive.tar.xz";
    let recorded = historical_origin_pins()
        .unwrap()
        .into_iter()
        .filter(|(_, pins)| pins.iter().any(|pin| pin.origin_key == key))
        .map(|(version, _)| version)
        .collect::<std::collections::BTreeSet<_>>();
    // These immutable release tags were measured from
    // `git show <tag>:solstone/think/providers/nvattest_install.py`; this is
    // historical transcription fidelity, not a growing-value snapshot.
    let measured = ["1.0.12", "1.0.13", "1.0.15", "1.0.16", "1.0.17"]
        .into_iter()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(recorded, measured);
}

#[test]
fn pin_snapshots_declare_non_origin_upstream_hosts() {
    let pins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("pins");
    let filenames = fs::read_dir(&pins_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<std::collections::BTreeSet<_>>();
    let expected = snapshot_versions()
        .unwrap()
        .into_iter()
        .map(|version| pins_dir.join(format!("v{version}.json")))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(filenames, expected);
    for path in expected {
        let snapshot: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert!(
            snapshot
                .get("non_origin_upstream_hosts")
                .and_then(Value::as_array)
                .is_some_and(|hosts| !hosts.is_empty())
        );
    }
}
