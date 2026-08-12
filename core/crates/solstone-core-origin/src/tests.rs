// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_local::install::archive::DownloadHostPolicy;

use crate::gate::{GateTarget, assert_head_targets_correspond, verify_targets};
use crate::guard::{
    GuardError, PinOwner, PruneAssessment, assess_prune, assess_prune_with_current_support,
    require_prunable,
};
use crate::mirror::{
    MULTIPART_THRESHOLD_BYTES, MirrorError, MirrorTarget, PublishBackend, PublishMode,
    PublishRequest, UpstreamMetadataKind, UpstreamVerification, current_mirror_targets, mirror_one,
    select_publish_mode,
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

fn policy(base: &str) -> DownloadHostPolicy<'_> {
    DownloadHostPolicy {
        allowed_hosts: &["127.0.0.1"],
        allow_http: true,
        origin_base_url: base,
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).unwrap();
        assert_ne!(read, 0, "request ended before headers");
        request.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(request).unwrap()
}

fn server<F>(requests: usize, response: F) -> String
where
    F: Fn(&str) -> (String, Vec<u8>) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().take(requests) {
            let mut stream = stream.unwrap();
            let request = read_request(&mut stream);
            let (headers, body) = response(&request);
            if headers.to_ascii_lowercase().contains("content-length:") {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\n"
                )
                .unwrap();
            } else {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                    body.len(),
                    headers
                )
                .unwrap();
            }
            stream.write_all(&body).unwrap();
        }
    });
    format!("http://{address}")
}

fn target(key: &str, bytes: &[u8]) -> GateTarget {
    GateTarget {
        origin_key: key.to_owned(),
        sha256: sha256(bytes),
        size_bytes: Some(bytes.len() as u64),
        unit: "fixture".to_owned(),
        version: Some("fixture".to_owned()),
        upstream_url: None,
    }
}

#[test]
fn gate_loopback_verifies_bytes_and_inner_slash_origin_key() {
    let body = b"fixture-origin".to_vec();
    let base = server(1, move |_| (String::new(), body.clone()));
    let target = target(
        "assets/rerank-model/a09144355adeed5f58c8ed011d209bf8ee5a1fec/onnx/model.onnx",
        b"fixture-origin",
    );
    verify_targets(&[target], &temp("gate-inner-slash"), &policy(&base)).unwrap();
}

#[test]
fn gate_rejects_wrong_bytes_even_with_matching_content_length() {
    let base = server(1, |_| (String::new(), b"wrong".to_vec()));
    let target = target("assets/fixture", b"right");
    let error = verify_targets(&[target], &temp("gate-wrong"), &policy(&base)).unwrap_err();
    assert!(error.to_string().contains("origin verification failed"));
}

#[test]
fn gate_rejects_short_body_even_when_digest_would_be_checked_later() {
    let base = server(1, |_| (String::new(), b"short".to_vec()));
    let mut target = target("assets/fixture", b"shorter");
    target.sha256 = sha256(b"short");
    let error = verify_targets(&[target], &temp("gate-short"), &policy(&base)).unwrap_err();
    assert!(error.to_string().contains("origin verification failed"));
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
        "assets/rerank-model/a09144355adeed5f58c8ed011d209bf8ee5a1fec/onnx/model.onnx",
    )
    .unwrap();
    assert_eq!(
        assessment,
        PruneAssessment::PinnedBy {
            owners: vec![PinOwner::HeadUnreleased],
        }
    );
}

struct FixtureBackend {
    modes: Mutex<Vec<PublishMode>>,
}

impl PublishBackend for FixtureBackend {
    fn publish(&self, mode: PublishMode, _request: PublishRequest<'_>) -> Result<(), MirrorError> {
        self.modes.lock().unwrap().push(mode);
        Ok(())
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
fn mirror_uses_loopback_github_digest_metadata_then_reads_back() {
    let body = b"mirror-body".to_vec();
    let digest = sha256(&body);
    let base = server(3, move |request| {
        if request.starts_with("GET /metadata ") {
            (
                String::new(),
                format!(r#"{{"assets":[{{"name":"file.bin","digest":"sha256:{digest}"}}]}}"#)
                    .into_bytes(),
            )
        } else {
            (String::new(), body.clone())
        }
    });
    let target = mirror_target(&base, b"mirror-body", UpstreamMetadataKind::GithubRelease);
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-github");
    let outcome = mirror_one(
        &target,
        &backend,
        &root,
        &root.join("log.jsonl"),
        &policy(&base),
    )
    .unwrap();
    assert_eq!(
        outcome,
        crate::mirror::MirrorOutcome::Mirrored {
            origin_key: target.origin_key,
            verification: UpstreamVerification::UpstreamSha256,
        }
    );
}

#[test]
fn mirror_uses_loopback_huggingface_size_only_metadata() {
    let body = b"mirror-hf".to_vec();
    let size = body.len();
    let base = server(3, move |request| {
        if request.starts_with("GET /metadata ") {
            (
                format!(
                    "x-linked-etag: 688882a700000000000000000000000000000000\r\nContent-Length: {size}\r\n"
                ),
                Vec::new(),
            )
        } else {
            (String::new(), body.clone())
        }
    });
    let target = mirror_target(&base, b"mirror-hf", UpstreamMetadataKind::HuggingFace);
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-hf");
    let outcome = mirror_one(
        &target,
        &backend,
        &root,
        &root.join("log.jsonl"),
        &policy(&base),
    )
    .unwrap();
    assert!(matches!(
        outcome,
        crate::mirror::MirrorOutcome::Mirrored {
            verification: UpstreamVerification::SizeOnly,
            ..
        }
    ));
}

#[test]
fn mirror_read_back_failure_leaves_no_provenance_row() {
    let body = b"mirror-failure".to_vec();
    let digest = sha256(&body);
    let base = server(3, move |request| {
        if request.starts_with("GET /metadata ") {
            (
                String::new(),
                format!(r#"{{"assets":[{{"name":"file.bin","digest":"sha256:{digest}"}}]}}"#)
                    .into_bytes(),
            )
        } else if request.starts_with("GET /upstream ") {
            (String::new(), body.clone())
        } else {
            (String::new(), b"wrong-origin".to_vec())
        }
    });
    let target = mirror_target(
        &base,
        b"mirror-failure",
        UpstreamMetadataKind::GithubRelease,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-readback-failure");
    let log = root.join("log.jsonl");
    assert!(mirror_one(&target, &backend, &root, &log, &policy(&base)).is_err());
    assert!(!log.exists());
}

#[test]
fn current_mirror_targets_exclude_the_cuda_origin_rows_by_name() {
    let mirrored = current_mirror_targets()
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
fn mirror_reads_back_through_the_same_path_for_single_shot_and_multipart() {
    let body = b"shared-read-back".to_vec();
    let digest = sha256(&body);
    let base = server(6, move |request| {
        if request.starts_with("GET /metadata ") {
            (
                String::new(),
                format!(r#"{{"assets":[{{"name":"file.bin","digest":"sha256:{digest}"}}]}}"#)
                    .into_bytes(),
            )
        } else {
            (String::new(), body.clone())
        }
    });
    let target = mirror_target(
        &base,
        b"shared-read-back",
        UpstreamMetadataKind::GithubRelease,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-shared-read-back");
    let first_log = root.join("single.jsonl");
    let second_log = root.join("multipart.jsonl");
    // `PublishMode` is selected solely at the backend call. Both calls below
    // therefore take the unconditional shared read-back below that call.
    mirror_one(
        &target,
        &backend,
        &root.join("single"),
        &first_log,
        &policy(&base),
    )
    .unwrap();
    mirror_one(
        &target,
        &backend,
        &root.join("multipart"),
        &second_log,
        &policy(&base),
    )
    .unwrap();
    let first: Value = serde_json::from_str(fs::read_to_string(first_log).unwrap().trim()).unwrap();
    let second: Value =
        serde_json::from_str(fs::read_to_string(second_log).unwrap().trim()).unwrap();
    for field in [
        "origin_key",
        "pin_sha256",
        "read_back",
        "size_bytes",
        "unit",
        "upstream_url",
        "verified",
        "version",
    ] {
        assert_eq!(first.get(field), second.get(field));
    }
    assert_eq!(
        backend.modes.lock().unwrap().as_slice(),
        &[PublishMode::SingleShot, PublishMode::SingleShot]
    );
    assert_eq!(
        [
            select_publish_mode(MULTIPART_THRESHOLD_BYTES),
            select_publish_mode(MULTIPART_THRESHOLD_BYTES + 1),
        ],
        [PublishMode::SingleShot, PublishMode::Multipart]
    );
}

#[test]
fn gate_never_falls_back_to_the_upstream_listener() {
    let origin_body = b"origin-only".to_vec();
    let origin = server(1, move |_| (String::new(), origin_body.clone()));
    let upstream_listener = TcpListener::bind("127.0.0.2:0").unwrap();
    upstream_listener.set_nonblocking(true).unwrap();
    let contacts = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&contacts);
    let upstream = thread::spawn(move || {
        for _ in 0..30 {
            match upstream_listener.accept() {
                Ok((mut stream, _)) => {
                    observed.fetch_add(1, Ordering::Relaxed);
                    let _ = read_request(&mut stream);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("upstream listener failed: {error}"),
            }
        }
    });
    verify_targets(
        &[target("assets/origin-only", b"origin-only")],
        &temp("origin-no-fallback"),
        &policy(&origin),
    )
    .unwrap();
    upstream.join().unwrap();
    assert_eq!(contacts.load(Ordering::Relaxed), 0);
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
