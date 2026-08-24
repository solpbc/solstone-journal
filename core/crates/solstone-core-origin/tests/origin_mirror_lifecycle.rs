// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_local::install::archive::DownloadHostPolicy;
use solstone_core_origin::gate::{GateTarget, head_gate_targets, verify_targets};
use solstone_core_origin::mirror::{
    MULTIPART_THRESHOLD_BYTES, MirrorError, MirrorOutcome, MirrorTarget, PublishBackend,
    PublishMode, PublishRequest, UpstreamHostPolicy, UpstreamMetadataKind, UpstreamVerification,
    mirror_one, select_publish_mode,
};

struct LoopbackServer {
    base: String,
    accepted: Arc<AtomicU64>,
    request_lines: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    addr: SocketAddr,
    handle: Option<JoinHandle<()>>,
}

impl LoopbackServer {
    fn accepted(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }

    fn request_lines(&self) -> Vec<String> {
        self.request_lines.lock().unwrap().clone()
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            match handle.join() {
                Ok(()) => {}
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
    }
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

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

fn upstream_policy() -> UpstreamHostPolicy<'static> {
    UpstreamHostPolicy {
        allowed_hosts: &["127.0.0.1"],
        allow_http: true,
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
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

fn spawn_server<F>(mut write_response: F) -> LoopbackServer
where
    F: FnMut(&str, &mut TcpStream) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicU64::new(0));
    let request_lines = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let thread_accepted = Arc::clone(&accepted);
    let thread_request_lines = Arc::clone(&request_lines);
    let thread_stop = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        loop {
            let (mut stream, _) = listener.accept().unwrap();
            if thread_stop.load(Ordering::SeqCst) {
                break;
            }
            thread_accepted.fetch_add(1, Ordering::Relaxed);
            let request = read_request(&mut stream);
            thread_request_lines
                .lock()
                .unwrap()
                .push(request.lines().next().unwrap_or_default().to_owned());
            write_response(&request, &mut stream);
        }
    });
    LoopbackServer {
        base: format!("http://{addr}"),
        accepted,
        request_lines,
        stop,
        addr,
        handle: Some(handle),
    }
}

fn server<F>(response: F) -> LoopbackServer
where
    F: Fn(&str) -> (String, Vec<u8>) + Send + 'static,
{
    spawn_server(move |request, stream| {
        let (headers, body) = response(request);
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
    })
}

fn status_server<F>(response: F) -> LoopbackServer
where
    F: Fn(&str) -> (u16, String, Vec<u8>) + Send + 'static,
{
    spawn_server(move |request, stream| {
        let (status, headers, body) = response(request);
        write!(
            stream,
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(&body).unwrap();
    })
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
fn gate_loopback_verifies_bytes_and_inner_slash_origin_key() {
    let body = b"fixture-origin".to_vec();
    let mut origin = server(move |_| (String::new(), body.clone()));
    let target = target(
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Decoder.mlmodelc/analytics/coremldata.bin",
        b"fixture-origin",
    );
    verify_targets(&[target], &temp("gate-inner-slash"), &policy(&origin.base)).unwrap();
    origin.shutdown();
    assert_eq!(
        origin.request_lines(),
        vec![
            "GET /assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/Decoder.mlmodelc/analytics/coremldata.bin HTTP/1.1"
                .to_owned()
        ]
    );
}

#[test]
fn gate_rejects_wrong_bytes_even_with_matching_content_length() {
    let origin = server(|_| (String::new(), b"wrong".to_vec()));
    let target = target("assets/fixture", b"right");
    let error = verify_targets(&[target], &temp("gate-wrong"), &policy(&origin.base)).unwrap_err();
    assert!(error.to_string().contains("origin verification failed"));
}

#[test]
fn gate_rejects_short_body_even_when_digest_would_be_checked_later() {
    let origin = server(|_| (String::new(), b"short".to_vec()));
    let mut target = target("assets/fixture", b"shorter");
    target.sha256 = sha256(b"short");
    let error = verify_targets(&[target], &temp("gate-short"), &policy(&origin.base)).unwrap_err();
    assert!(error.to_string().contains("origin verification failed"));
}

#[test]
fn full_head_target_set_can_be_verified_when_authority_sizes_are_absent() {
    let body = b"head-target-fixture";
    let mut targets = head_gate_targets().unwrap();
    for target in &mut targets {
        target.sha256 = sha256(body);
        if target.size_bytes.is_some() {
            target.size_bytes = Some(body.len() as u64);
        }
    }
    let origin = server(move |_| (String::new(), body.to_vec()));
    verify_targets(
        &targets,
        &temp("full-head-target-set"),
        &policy(&origin.base),
    )
    .unwrap();
}

#[test]
fn mirror_uses_loopback_github_digest_metadata_then_reads_back() {
    let body = b"mirror-body".to_vec();
    let digest = sha256(&body);
    let origin = server(move |request| {
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
        &origin.base,
        b"mirror-body",
        UpstreamMetadataKind::GithubRelease,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-github");
    let outcome = mirror_one(
        &target,
        &backend,
        &root,
        &root.join("log.jsonl"),
        &policy(&origin.base),
        &upstream_policy(),
    )
    .unwrap();
    assert_eq!(
        outcome,
        MirrorOutcome::Mirrored {
            origin_key: target.origin_key,
            verification: UpstreamVerification::UpstreamSha256,
        }
    );
}

#[test]
fn mirror_follows_validated_loopback_redirects() {
    let body = b"redirected-object".to_vec();
    let digest = sha256(&body);
    let mut origin = status_server(move |request| {
        if request.starts_with("GET /metadata ") {
            (
                200,
                String::new(),
                format!(r#"{{"assets":[{{"name":"file.bin","digest":"sha256:{digest}"}}]}}"#)
                    .into_bytes(),
            )
        } else if request.starts_with("GET /upstream ") {
            (302, "Location: /object\r\n".to_owned(), Vec::new())
        } else {
            (200, String::new(), body.clone())
        }
    });
    let target = mirror_target(
        &origin.base,
        b"redirected-object",
        UpstreamMetadataKind::GithubRelease,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-redirect");
    assert!(matches!(
        mirror_one(
            &target,
            &backend,
            &root,
            &root.join("log.jsonl"),
            &policy(&origin.base),
            &upstream_policy(),
        ),
        Ok(MirrorOutcome::Mirrored { .. })
    ));
    origin.shutdown();
    assert_eq!(origin.accepted(), 4);
    assert_eq!(
        origin.request_lines(),
        vec![
            "GET /metadata HTTP/1.1".to_owned(),
            "GET /upstream HTTP/1.1".to_owned(),
            "GET /object HTTP/1.1".to_owned(),
            "GET /assets/fixture/file.bin HTTP/1.1".to_owned(),
        ]
    );
}

#[test]
fn mirror_refuses_redirect_to_a_disallowed_host() {
    let body = b"redirect-refusal".to_vec();
    let digest = sha256(&body);
    let mut origin = status_server(move |request| {
        if request.starts_with("GET /metadata ") {
            (
                200,
                String::new(),
                format!(r#"{{"assets":[{{"name":"file.bin","digest":"sha256:{digest}"}}]}}"#)
                    .into_bytes(),
            )
        } else {
            (
                302,
                "Location: http://127.0.0.2:9/object\r\n".to_owned(),
                Vec::new(),
            )
        }
    });
    let target = mirror_target(
        &origin.base,
        b"redirect-refusal",
        UpstreamMetadataKind::GithubRelease,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-redirect-refusal");
    assert!(matches!(
        mirror_one(
            &target,
            &backend,
            &root,
            &root.join("log.jsonl"),
            &policy(&origin.base),
            &upstream_policy(),
        ),
        Err(MirrorError::UpstreamHostRefused { .. })
    ));
    origin.shutdown();
    assert_eq!(origin.accepted(), 2);
    assert_eq!(
        origin.request_lines(),
        vec![
            "GET /metadata HTTP/1.1".to_owned(),
            "GET /upstream HTTP/1.1".to_owned(),
        ]
    );
}

#[test]
fn mirror_uses_loopback_huggingface_git_blob_metadata() {
    const CONFIG_JSON_BLOB_OID: &str = "9e26dfeeb6e641a33dae4961196235bdb965b21b";
    const CONFIG_JSON_SHA256: &str =
        "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

    let body = b"{}".to_vec();
    let origin = server(move |request| {
        if request.starts_with("GET /metadata ") {
            (
                format!("x-linked-etag: {CONFIG_JSON_BLOB_OID}\r\n"),
                Vec::new(),
            )
        } else {
            (String::new(), body.clone())
        }
    });
    let mut target = mirror_target(&origin.base, b"{}", UpstreamMetadataKind::HuggingFace);
    target.origin_key =
        "assets/parakeet-coreml/aed02740059203c4a87495924f685de3722ae9ce/config.json".to_owned();
    assert_eq!(target.sha256, CONFIG_JSON_SHA256);
    assert_eq!(target.size_bytes, 2);
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-hf");
    let log = root.join("log.jsonl");
    let outcome = mirror_one(
        &target,
        &backend,
        &root,
        &log,
        &policy(&origin.base),
        &upstream_policy(),
    )
    .unwrap();
    assert!(matches!(
        outcome,
        MirrorOutcome::Mirrored {
            verification: UpstreamVerification::UpstreamGitBlobSha1,
            ..
        }
    ));
    let provenance: Value = serde_json::from_str(fs::read_to_string(log).unwrap().trim()).unwrap();
    assert_eq!(provenance["verified"], "upstream-git-blob-sha1");
}

#[test]
fn mirror_refuses_huggingface_git_blob_etag_that_does_not_match_the_body() {
    // This did not fail before wave 1: the old allowlist rejected the metadata
    // before it fetched the body. It now proves the body identity is checked.
    let body = b"unlisted-hf".to_vec();
    let origin = server(move |request| {
        if request.starts_with("GET /metadata ") {
            (
                "x-linked-etag: 688882a700000000000000000000000000000000\r\n".to_owned(),
                Vec::new(),
            )
        } else {
            (String::new(), body.clone())
        }
    });
    let target = mirror_target(
        &origin.base,
        b"unlisted-hf",
        UpstreamMetadataKind::HuggingFace,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-unlisted-size-only");
    assert!(matches!(
        mirror_one(
            &target,
            &backend,
            &root,
            &root.join("log.jsonl"),
            &policy(&origin.base),
            &upstream_policy(),
        ),
        Err(MirrorError::UpstreamGitBlobDigestMismatch { .. })
    ));
    assert!(backend.modes.lock().unwrap().is_empty());
}

#[test]
fn mirror_uses_loopback_huggingface_sha256_metadata() {
    let body = b"lfs-hf".to_vec();
    let digest = sha256(&body);
    let origin = server(move |request| {
        if request.starts_with("GET /metadata ") {
            (format!("x-linked-etag: sha256:{digest}\r\n"), Vec::new())
        } else {
            (String::new(), body.clone())
        }
    });
    let target = mirror_target(&origin.base, b"lfs-hf", UpstreamMetadataKind::HuggingFace);
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-hf-sha256");
    let outcome = mirror_one(
        &target,
        &backend,
        &root,
        &root.join("log.jsonl"),
        &policy(&origin.base),
        &upstream_policy(),
    )
    .unwrap();
    assert!(matches!(
        outcome,
        MirrorOutcome::Mirrored {
            verification: UpstreamVerification::UpstreamSha256,
            ..
        }
    ));
}

#[test]
fn mirror_read_back_failure_leaves_no_provenance_row() {
    let body = b"mirror-failure".to_vec();
    let digest = sha256(&body);
    let origin = server(move |request| {
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
        &origin.base,
        b"mirror-failure",
        UpstreamMetadataKind::GithubRelease,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-readback-failure");
    let log = root.join("log.jsonl");
    assert!(
        mirror_one(
            &target,
            &backend,
            &root,
            &log,
            &policy(&origin.base),
            &upstream_policy()
        )
        .is_err()
    );
    assert!(!log.exists());
}

#[test]
fn mirror_read_back_is_shared_after_single_shot_publish() {
    let body = b"shared-read-back".to_vec();
    let digest = sha256(&body);
    let origin = server(move |request| {
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
        &origin.base,
        b"shared-read-back",
        UpstreamMetadataKind::GithubRelease,
    );
    let backend = FixtureBackend {
        modes: Mutex::new(Vec::new()),
    };
    let root = temp("mirror-shared-read-back");
    let first_log = root.join("single.jsonl");
    let second_log = root.join("second-single.jsonl");
    // `PublishMode` is selected solely at the backend call. Both calls below
    // therefore take the unconditional shared read-back below that call.
    mirror_one(
        &target,
        &backend,
        &root.join("single"),
        &first_log,
        &policy(&origin.base),
        &upstream_policy(),
    )
    .unwrap();
    mirror_one(
        &target,
        &backend,
        &root.join("multipart"),
        &second_log,
        &policy(&origin.base),
        &upstream_policy(),
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
    let origin = server(move |_| (String::new(), origin_body.clone()));
    // 127.0.0.1 with its own ephemeral port, not 127.0.0.2: macOS configures
    // only 127.0.0.1 on lo0, so the second loopback IP is unbindable there and
    // this test panicked before asserting anything. What it proves is unchanged
    // -- the gate contacts the origin and no other listening endpoint.
    let witness = TcpListener::bind("127.0.0.1:0").unwrap();
    witness.set_nonblocking(true).unwrap();
    let witness_url = format!("http://{}", witness.local_addr().unwrap());
    let mut target = target("assets/origin-only", b"origin-only");
    target.upstream_url = Some(witness_url);
    verify_targets(
        &[target],
        &temp("origin-no-fallback"),
        &policy(&origin.base),
    )
    .unwrap();
    match witness.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("gate contacted the unused witness listener"),
        Err(error) => panic!("witness listener failed: {error}"),
    }
}
