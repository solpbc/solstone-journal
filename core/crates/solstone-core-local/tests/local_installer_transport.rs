// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use socket2::{Domain, Protocol, Socket, Type};
use solstone_core_assets::{Artifact, Backend, Platform, catalog, resolve};
use solstone_core_journal_config::{
    JournalConfigRead,
    parakeet_coreml::{
        ParakeetCoremlSentinel, parakeet_coreml_cache_dir, parakeet_coreml_model_root,
        parakeet_coreml_sentinel_path, read_valid_parakeet_coreml_sentinel,
    },
};
use solstone_core_local::install::archive::{self, ArchiveError, DownloadHostPolicy};
use solstone_core_local::install::test_hooks::{
    install_coreml_with_rows, install_coreml_with_seams,
};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);
const LOOPBACK_DOWNLOAD_HOSTS: &[&str] = &["127.0.0.1"];
const COREML_UNIT: &str = "parakeet-coreml";

fn temp(name: &str) -> PathBuf {
    let suffix = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "solstone-local-installer-transport-{name}-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn loopback_download_policy(origin_base_url: &str) -> DownloadHostPolicy<'_> {
    DownloadHostPolicy {
        allowed_hosts: LOOPBACK_DOWNLOAD_HOSTS,
        allow_http: true,
        origin_base_url,
    }
}

fn loopback_host(address: SocketAddr) -> String {
    address.ip().to_string()
}

fn held_refusal_reservation() -> (Socket, SocketAddr) {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
    socket
        .bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())
        .unwrap();
    let address = socket.local_addr().unwrap().as_socket().unwrap();
    (socket, address)
}

fn foreign_loopback_witness() -> (TcpListener, String) {
    let listener = TcpListener::bind("[::1]:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    (listener, format!("http://[::1]:{}", address.port()))
}

fn assert_no_connection(listener: &TcpListener) {
    match listener.accept() {
        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
        Ok(_) => panic!("refused host accepted an unexpected connection"),
        Err(error) => panic!("check refused-host connection: {error}"),
    }
}

fn request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).unwrap();
        assert_ne!(read, 0, "request ended before its headers");
        request.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(request).unwrap()
}

fn request_host(stream: &mut TcpStream) -> String {
    let request = request(stream);
    let host = request
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("host").then_some(value.trim())
        })
        .expect("request has Host header");
    host.split(':').next().unwrap().to_owned()
}

fn request_path(stream: &mut TcpStream) -> String {
    request(stream)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request has a path")
        .to_owned()
}

fn server(bytes: Vec<u8>, requests: usize) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        }
    });
    (format!("http://{address}"), handle)
}

fn response_server(responses: Vec<String>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut paths = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            paths.push(request_path(&mut stream));
            stream.write_all(response.as_bytes()).unwrap();
        }
        paths
    });
    (format!("http://{address}"), handle)
}

fn record_http_hosts(
    listener: TcpListener,
    connections: usize,
    response: String,
) -> thread::JoinHandle<BTreeSet<String>> {
    thread::spawn(move || {
        let mut contacted = BTreeSet::new();
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            contacted.insert(request_host(&mut stream));
            stream.write_all(response.as_bytes()).unwrap();
        }
        contacted
    })
}

fn self_signed_leaf() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("server key");
    let params = CertificateParams::new(vec!["127.0.0.1".to_string()]).expect("server params");
    let cert = params.self_signed(&key).expect("server cert");
    (
        CertificateDer::from(cert.der().to_vec()),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
}

fn invalid_trust_config() -> Arc<ServerConfig> {
    let (cert, key) = self_signed_leaf();
    Arc::new(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("server protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .expect("server config"),
    )
}

fn record_tls_hosts(
    listener: TcpListener,
    connections: usize,
) -> thread::JoinHandle<BTreeSet<String>> {
    let host = loopback_host(listener.local_addr().unwrap());
    let config = invalid_trust_config();
    thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) = listener.accept().unwrap();
            let mut connection = ServerConnection::new(Arc::clone(&config)).unwrap();
            let _ = connection.complete_io(&mut stream);
        }
        BTreeSet::from([host])
    })
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

fn assert_only_origin_host(contacted: BTreeSet<String>, expected_host: &str) {
    assert_eq!(contacted, BTreeSet::from([expected_host.to_owned()]));
}

fn assert_origin_unavailable(
    artifact: &Artifact,
    destination: &Path,
    policy: &DownloadHostPolicy<'_>,
    expected_host: &str,
) {
    match archive::download_verified(artifact, destination, policy, |_, _| {}).unwrap_err() {
        ArchiveError::OriginUnavailable { host, .. } => {
            assert_eq!(host, expected_host);
        }
        error => panic!("expected OriginUnavailable, got {error:?}"),
    }
}

fn coreml_config(cache_dir: &Path) -> JournalConfigRead {
    JournalConfigRead {
        present: true,
        sha256: None,
        config: Some(
            json!({"transcribe": {"parakeet": {"cache_dir": cache_dir}}})
                .as_object()
                .unwrap()
                .clone(),
        ),
    }
}

fn coreml_row(filename: &'static str, bytes: &[u8]) -> Artifact {
    Artifact {
        unit: COREML_UNIT,
        version: "test",
        filename,
        sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
        size_bytes: bytes.len() as u64,
        upstream_url: "https://upstream.invalid/test",
        origin_key: Box::leak(format!("test/{filename}").into_boxed_str()),
        artifact_key: None,
        platform: Some(Platform::MacosArm64),
        backend: None,
        extracted_binary_sha256: None,
    }
}

fn publish_tree(staging: &Path, target: &Path) -> std::io::Result<()> {
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    fs::rename(staging, target)
}

fn write_sentinel(path: &Path, sentinel: &ParakeetCoremlSentinel) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(sentinel).unwrap()).unwrap();
}

#[test]
fn download_artifact_refuses_disallowed_redirect_target_in_envelope() {
    let root = temp("download-refused-redirect");
    let (foreign_listener, foreign_base) = foreign_loopback_witness();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let served = listener.try_clone().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = served.accept().unwrap();
        stream
            .write_all(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: {foreign_base}/file\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
    });
    let destination = root.join("artifact");
    let artifact = fixture_artifact(format!("http://{address}/start"), "artifact", b"");
    let error = archive::download_verified(
        &artifact,
        &destination,
        &loopback_download_policy(&format!("http://{address}")),
        |_received, _total| {},
    )
    .unwrap_err();
    server.join().unwrap();
    assert!(matches!(error, ArchiveError::HostRefused { host } if host == "::1"));
    assert_no_connection(&foreign_listener);
    assert!(!destination.exists());
    assert!(!root.join(".artifact.part").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn download_verified_follows_allowlisted_redirect_chain_and_commits_verified_bytes() {
    let root = temp("download-redirect-chain");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut paths = Vec::new();
        for response in [
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{address}/middle\r\nConnection: close\r\n\r\n"
            ),
            format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{address}/final\r\nConnection: close\r\n\r\n"
            ),
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_owned(),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            paths.push(request_path(&mut stream));
            stream.write_all(response.as_bytes()).unwrap();
        }
        paths
    });
    let destination = root.join("artifact");
    let artifact = fixture_artifact(format!("http://{address}/start"), "artifact", b"hello");
    let mut progress = Vec::new();
    archive::download_verified(
        &artifact,
        &destination,
        &loopback_download_policy(&format!("http://{address}")),
        |received, total| progress.push((received, total)),
    )
    .unwrap();
    assert_eq!(
        server.join().unwrap(),
        ["/test-origin", "/middle", "/final"]
    );
    assert_eq!(fs::read(&destination).unwrap(), b"hello");
    assert!(!root.join(".artifact.part").exists());
    assert_eq!(progress.last(), Some(&(5, Some(5))));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn download_verified_requests_the_origin_key_not_the_catalog_upstream_url() {
    let root = temp("download-origin-key");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let received = stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
            .unwrap();
        String::from_utf8_lossy(&request[..received]).into_owned()
    });
    let destination = root.join("artifact");
    let artifact = fixture_artifact(
        "https://github.com/upstream/that-must-not-be-contacted".to_owned(),
        "artifact",
        b"hello",
    );
    archive::download_verified(
        &artifact,
        &destination,
        &loopback_download_policy(&format!("http://{address}")),
        |_received, _total| {},
    )
    .unwrap();
    assert!(
        server
            .join()
            .unwrap()
            .starts_with("GET /test-origin HTTP/1.1\r\n")
    );
    assert_eq!(fs::read(&destination).unwrap(), b"hello");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn download_verified_resolves_relative_locations_for_redirect_statuses() {
    for status in [301, 303, 307, 308] {
        let root = temp(&format!("download-relative-{status}"));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Redirect\r\nLocation: ../final\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .unwrap();
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
        });
        let destination = root.join("artifact");
        let artifact =
            fixture_artifact(format!("http://{address}/nested/start"), "artifact", b"ok");
        archive::download_verified(
            &artifact,
            &destination,
            &loopback_download_policy(&format!("http://{address}")),
            |_received, _total| {},
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"ok", "status={status}");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn download_artifact_reports_redirect_hop_limit() {
    let root = temp("download-hop-limit");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for index in 0..6 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{address}/hop-{index}\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .unwrap();
        }
        6
    });
    let destination = root.join("artifact");
    let artifact = fixture_artifact(format!("http://{address}/start"), "artifact", b"");
    let error = archive::download_verified(
        &artifact,
        &destination,
        &loopback_download_policy(&format!("http://{address}")),
        |_received, _total| {},
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ArchiveError::RedirectHopLimitExceeded { limit: 5 }
    ));
    assert_eq!(server.join().unwrap(), 6);
    assert!(!destination.exists());
    assert!(!root.join(".artifact.part").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn download_verified_reports_size_mismatch_before_digest_mismatch() {
    let root = temp("download-size-mismatch");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello")
            .unwrap();
    });
    let destination = root.join("artifact");
    let mut artifact = fixture_artifact(format!("http://{address}"), "artifact", b"hello");
    artifact.size_bytes = 4;
    artifact.sha256 = "00";
    assert!(matches!(
        archive::download_verified(
            &artifact,
            &destination,
            &loopback_download_policy(&format!("http://{address}")),
            |_received, _total| {},
        ),
        Err(ArchiveError::SizeMismatch {
            expected: 4,
            actual: 5
        })
    ));
    server.join().unwrap();
    assert!(!destination.exists());
    assert!(!root.join(".artifact.part").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn download_verified_accepts_the_real_loopback_origin() {
    let root = temp("download-upper-host");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .unwrap();
    });
    let destination = root.join("artifact");
    let artifact = fixture_artifact(format!("http://{address}/x"), "artifact", b"ok");
    archive::download_verified(
        &artifact,
        &destination,
        &loopback_download_policy(&format!("http://{address}")),
        |_received, _total| {},
    )
    .unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"ok");
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
    let mut artifact = fixture_artifact(format!("http://{address}"), "artifact.tar.gz", b"hello");
    artifact.sha256 = "00";
    assert!(matches!(
        archive::download_verified(
            &artifact,
            &destination,
            &loopback_download_policy(&format!("http://{address}")),
            |_received, _total| {},
        ),
        Err(ArchiveError::DigestMismatch)
    ));
    server.join().unwrap();
    assert!(!destination.exists());
    assert!(!root.join(".artifact.tar.gz.part").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn userinfo_origin_is_refused_before_any_accept() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let artifact = fixture_artifact("https://github.com/upstream".to_owned(), "artifact", b"");
    let origin_base_url = format!("http://{address}@blocked.test");
    let policy = DownloadHostPolicy {
        allowed_hosts: &["blocked.test"],
        allow_http: true,
        origin_base_url: &origin_base_url,
    };
    let root = temp("download-userinfo-live");
    let destination = root.join("artifact");
    let error =
        archive::download_verified(&artifact, &destination, &policy, |_, _| {}).unwrap_err();
    assert!(matches!(error, ArchiveError::UrlUserinfoRefused { .. }));
    assert!(listener.accept().is_err());
    assert!(!destination.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn origin_failures_have_a_distinct_reason_code_and_retain_the_host() {
    let root = temp("download-origin-failures");
    let (_reservation, refused_address) = held_refusal_reservation();
    let refused_artifact =
        fixture_artifact("https://github.com/upstream".to_owned(), "artifact", b"");
    let refused_base = format!("http://{refused_address}");
    assert_origin_unavailable(
        &refused_artifact,
        &root.join("refused"),
        &loopback_download_policy(&refused_base),
        &loopback_host(refused_address),
    );

    let tls_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tls_address = tls_listener.local_addr().unwrap();
    let tls_host = loopback_host(tls_address);
    let tls_server = record_tls_hosts(tls_listener, 1);
    let tls_artifact = fixture_artifact("https://github.com/upstream".to_owned(), "artifact", b"");
    let tls_base = format!("https://{tls_address}");
    let tls_policy = DownloadHostPolicy {
        allowed_hosts: LOOPBACK_DOWNLOAD_HOSTS,
        allow_http: false,
        origin_base_url: &tls_base,
    };
    assert_origin_unavailable(&tls_artifact, &root.join("tls"), &tls_policy, &tls_host);
    tls_server.join().unwrap();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn origin_http_statuses_map_to_origin_unreachable() {
    for status in [403, 404, 500, 503] {
        let root = temp(&format!("download-origin-status-{status}"));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let host = loopback_host(address);
        let server = record_http_hosts(
            listener,
            1,
            format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        );
        let artifact = fixture_artifact("https://github.com/upstream".to_owned(), "artifact", b"");
        assert_origin_unavailable(
            &artifact,
            &root.join("artifact"),
            &loopback_download_policy(&format!("http://{address}")),
            &host,
        );
        server.join().unwrap();
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn flipped_catalog_units_contact_only_origin_for_each_live_failure_class() {
    let root = temp("origin-failure-contact-matrix");
    let artifacts = flipped_origin_artifacts();
    let connections = artifacts.len();

    let (_reservation, refused_address) = held_refusal_reservation();
    let refused_base = format!("http://{refused_address}");
    let refused_policy = loopback_download_policy(&refused_base);
    let refused_host = loopback_host(refused_address);
    let mut refused_contacts = BTreeSet::new();
    for artifact in &artifacts {
        assert_origin_unavailable(
            artifact,
            &root.join(format!("refused-{}", artifact.filename)),
            &refused_policy,
            &refused_host,
        );
        refused_contacts.insert(refused_host.clone());
    }
    assert_only_origin_host(refused_contacts, &refused_host);

    let tls_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let tls_address = tls_listener.local_addr().unwrap();
    let tls_host = loopback_host(tls_address);
    let tls_server = record_tls_hosts(tls_listener, connections);
    let tls_base = format!("https://{tls_address}");
    let tls_policy = DownloadHostPolicy {
        allowed_hosts: LOOPBACK_DOWNLOAD_HOSTS,
        allow_http: false,
        origin_base_url: &tls_base,
    };
    for artifact in &artifacts {
        assert_origin_unavailable(
            artifact,
            &root.join(format!("tls-{}", artifact.filename)),
            &tls_policy,
            &tls_host,
        );
    }
    assert_only_origin_host(tls_server.join().unwrap(), &tls_host);

    for status in [403, 404, 500, 503] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let host = loopback_host(address);
        let server = record_http_hosts(
            listener,
            connections,
            format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        );
        let base = format!("http://{address}");
        let policy = loopback_download_policy(&base);
        for artifact in &artifacts {
            assert_origin_unavailable(
                artifact,
                &root.join(format!("status-{status}-{}", artifact.filename)),
                &policy,
                &host,
            );
        }
        assert_only_origin_host(server.join().unwrap(), &host);
    }

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let host = loopback_host(address);
    let server = record_http_hosts(
        listener,
        connections,
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx".to_owned(),
    );
    let base = format!("http://{address}");
    let policy = loopback_download_policy(&base);
    for artifact in &artifacts {
        match archive::download_verified(
            artifact,
            &root.join(format!("wrong-bytes-{}", artifact.filename)),
            &policy,
            |_, _| {},
        )
        .unwrap_err()
        {
            ArchiveError::SizeMismatch { .. } => {}
            error => panic!("expected SizeMismatch, got {error:?}"),
        }
    }
    assert_only_origin_host(server.join().unwrap(), &host);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn coreml_install_stages_all_catalog_paths_from_the_origin() {
    let catalog_rows = catalog()
        .iter()
        .filter(|artifact| {
            artifact.unit == COREML_UNIT && artifact.platform == Some(Platform::MacosArm64)
        })
        .collect::<Vec<_>>();
    assert!(!catalog_rows.is_empty());

    let bytes = catalog_rows
        .iter()
        .enumerate()
        .map(|(index, _)| format!("coreml fixture {index}").into_bytes())
        .collect::<Vec<_>>();
    let rows = catalog_rows
        .iter()
        .zip(&bytes)
        .map(|(catalog_row, bytes)| Artifact {
            unit: COREML_UNIT,
            version: catalog_row.version,
            filename: catalog_row.filename,
            sha256: Box::leak(format!("{:x}", Sha256::digest(bytes)).into_boxed_str()),
            size_bytes: bytes.len() as u64,
            upstream_url: "https://upstream.invalid/provenance-only",
            origin_key: Box::leak(format!("test/{}", catalog_row.filename).into_boxed_str()),
            artifact_key: None,
            platform: Some(Platform::MacosArm64),
            backend: None,
            extracted_binary_sha256: None,
        })
        .collect::<Vec<_>>();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_bytes = bytes.clone();
    let server = thread::spawn(move || {
        for bytes in server_bytes {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        }
    });
    let root = temp("coreml-all-paths");
    let home = root.join("home");
    let configured = root.join("configured/cache");
    let config = coreml_config(&configured);
    let origin_base_url = format!("http://{address}");
    let policy = loopback_download_policy(&origin_base_url);
    let references = rows.iter().collect::<Vec<_>>();
    let target = install_coreml_with_rows(&home, &config, false, &policy, &references).unwrap();
    server.join().unwrap();

    for (row, bytes) in rows.iter().zip(&bytes) {
        assert_eq!(
            fs::read(target.join(row.filename)).unwrap(),
            bytes.as_slice()
        );
    }
    let sentinel: Value =
        serde_json::from_slice(&fs::read(parakeet_coreml_sentinel_path(&home)).unwrap()).unwrap();
    assert_eq!(sentinel["cache_dir"], configured.display().to_string());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn install_uses_configured_tree_but_default_sentinel_and_writes_atomically() {
    let temporary = temp("coreml-configured-tree");
    let home = temporary.join("home");
    let configured = temporary.join("override/cache");
    let config = coreml_config(&configured);
    let bytes = b"model";
    let artifact = coreml_row("Encoder.mlmodelc/weights/weight.bin", bytes);
    let rows = [&artifact];
    let (base, server) = server(bytes.to_vec(), 1);
    let policy = loopback_download_policy(&base);
    let target = install_coreml_with_rows(&home, &config, false, &policy, &rows).unwrap();
    server.join().unwrap();
    assert_eq!(
        target,
        configured.parent().unwrap().join("parakeet-tdt-0.6b-v3")
    );
    assert!(target.join(artifact.filename).is_file());
    let sentinel = parakeet_coreml_sentinel_path(&home);
    assert!(sentinel.is_file());
    assert!(
        !configured
            .parent()
            .unwrap()
            .join(".install-complete")
            .exists()
    );
    let record: Value = serde_json::from_slice(&fs::read(sentinel).unwrap()).unwrap();
    assert_eq!(record["cache_dir"], configured.display().to_string());
    assert!(read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").is_some());
    let _ = fs::remove_dir_all(temporary);
}

#[test]
fn sentinel_write_failure_after_publish_leaves_no_sentinel() {
    let temporary = temp("coreml-sentinel-write-failure");
    let home = temporary.join("home");
    let configured = temporary.join("cache");
    let config = coreml_config(&configured);
    let bytes = b"model";
    let artifact = coreml_row("Encoder.mlmodelc/weights/weight.bin", bytes);
    let rows = [&artifact];
    let (base, server) = server(bytes.to_vec(), 1);
    let policy = loopback_download_policy(&base);
    let mut publish = |staging: &Path, target: &Path| publish_tree(staging, target);
    let error = install_coreml_with_seams(
        &home,
        &config,
        false,
        &policy,
        ("darwin", "arm64"),
        &rows,
        &mut publish,
        &mut |_, _| Err(std::io::Error::other("injected sentinel failure")),
    )
    .unwrap_err();
    server.join().unwrap();
    assert_eq!(error.reason_code, "sentinel_write_failed");
    assert!(
        parakeet_coreml_model_root(&configured)
            .join(artifact.filename)
            .is_file()
    );
    assert!(!parakeet_coreml_sentinel_path(&home).exists());
    let _ = fs::remove_dir_all(temporary);
}

#[test]
fn check_complete_install_succeeds_without_requests() {
    let temporary = temp("coreml-check-complete");
    let home = temporary.join("home");
    let configured = temporary.join("cache");
    let config = coreml_config(&configured);
    let bytes = b"model";
    let artifact = coreml_row("Encoder.mlmodelc/weights/weight.bin", bytes);
    let rows = [&artifact];
    let (base, server) = server(bytes.to_vec(), 1);
    let download_policy = loopback_download_policy(&base);
    install_coreml_with_rows(&home, &config, false, &download_policy, &rows).unwrap();
    server.join().unwrap();
    assert!(read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").is_some());
    assert!(
        parakeet_coreml_model_root(&configured)
            .join(artifact.filename)
            .is_file()
    );
    let _ = fs::remove_dir_all(temporary);
}

#[test]
fn install_refuses_a_foreign_redirect_hop_without_writing() {
    let temporary = temp("coreml-foreign-redirect");
    let home = temporary.join("home");
    let config = coreml_config(&temporary.join("cache"));
    let artifact = coreml_row("model.mil", b"model");
    let rows = [&artifact];
    let (foreign_listener, foreign_base) = foreign_loopback_witness();
    let (base, server) = response_server(vec![format!(
        "HTTP/1.1 302 Found\r\nLocation: {foreign_base}/foreign\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )]);
    let download_policy = loopback_download_policy(&base);
    let error =
        install_coreml_with_rows(&home, &config, false, &download_policy, &rows).unwrap_err();
    assert_eq!(server.join().unwrap(), ["/test/model.mil"]);
    assert_eq!(error.reason_code, "download_host_refused");
    assert_no_connection(&foreign_listener);
    assert!(!parakeet_coreml_model_root(&parakeet_coreml_cache_dir(&config, &home)).exists());
    assert!(!parakeet_coreml_sentinel_path(&home).exists());
    let _ = fs::remove_dir_all(temporary);
}

#[test]
fn failed_download_preserves_a_preexisting_complete_tree_and_sentinel() {
    let temporary = temp("coreml-failed-download-preserves");
    let home = temporary.join("home");
    let configured = temporary.join("cache");
    let config = coreml_config(&configured);
    let first = coreml_row("one", b"old one");
    let second = coreml_row("two", b"old two");
    let rows = [&first, &second];
    let target = parakeet_coreml_model_root(&configured);
    for row in rows {
        let path = target.join(row.filename);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            if row.filename == "one" {
                b"old one"
            } else {
                b"old two"
            },
        )
        .unwrap();
    }
    fs::create_dir_all(&configured).unwrap();
    let record = ParakeetCoremlSentinel::new(configured.clone(), "darwin", "arm64", "0.14.0");
    write_sentinel(&parakeet_coreml_sentinel_path(&home), &record);
    let (base, server) = response_server(vec![
        "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nnew one".to_owned(),
    ]);
    let download_policy = loopback_download_policy(&base);
    let error =
        install_coreml_with_rows(&home, &config, true, &download_policy, &rows).unwrap_err();
    server.join().unwrap();
    assert_eq!(error.reason_code, "download_digest_mismatch");
    assert_eq!(fs::read(target.join("one")).unwrap(), b"old one");
    assert!(parakeet_coreml_sentinel_path(&home).is_file());
    assert!(read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").is_some());
    let _ = fs::remove_dir_all(temporary);
}

#[test]
fn interrupted_publish_leaves_no_partial_tree() {
    let temporary = temp("coreml-interrupted-publish");
    let home = temporary.join("home");
    let configured = temporary.join("cache");
    let config = coreml_config(&configured);
    let artifact = coreml_row("model.mil", b"model");
    let rows = [&artifact];
    let (base, server) = server(b"model".to_vec(), 1);
    let download_policy = loopback_download_policy(&base);
    let mut publish =
        |_staging: &Path, _target: &Path| Err(std::io::Error::other("interrupted publish"));
    let mut write = |path: &Path, sentinel: &ParakeetCoremlSentinel| {
        write_sentinel(path, sentinel);
        Ok(())
    };
    let error = install_coreml_with_seams(
        &home,
        &config,
        false,
        &download_policy,
        ("darwin", "arm64"),
        &rows,
        &mut publish,
        &mut write,
    )
    .unwrap_err();
    server.join().unwrap();
    assert_eq!(error.reason_code, "publish_failed");
    assert!(!parakeet_coreml_model_root(&configured).exists());
    assert!(!parakeet_coreml_sentinel_path(&home).exists());
    let _ = fs::remove_dir_all(temporary);
}

#[test]
fn force_reinstalls_an_incomplete_tree_and_verifies_it() {
    let temporary = temp("coreml-force-reinstall");
    let home = temporary.join("home");
    let configured = temporary.join("cache");
    let config = coreml_config(&configured);
    let artifact = coreml_row("model.mil", b"model");
    let rows = [&artifact];
    fs::create_dir_all(parakeet_coreml_model_root(&configured)).unwrap();
    let (base, server) = server(b"model".to_vec(), 1);
    let download_policy = loopback_download_policy(&base);
    install_coreml_with_rows(&home, &config, true, &download_policy, &rows).unwrap();
    server.join().unwrap();
    assert!(read_valid_parakeet_coreml_sentinel(&home, "darwin", "arm64").is_some());
    assert!(
        parakeet_coreml_model_root(&configured)
            .join(artifact.filename)
            .is_file()
    );
    let _ = fs::remove_dir_all(temporary);
}
