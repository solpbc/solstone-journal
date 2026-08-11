// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Focused host-door coverage. The body-echo route is deliberately in this
//! integration-test crate, stronger than a feature-gated library test surface.

mod door_support;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Extension;
use axum::Router;
use axum::routing::get;
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_shell::{
    ConveyServeOptions, DoorOutcome, DoorWithheldReason, router, serve,
};
use solstone_core_sol_link::ledger::{AuthorizationLedger, AuthorizedClientsRead};
use spl_core::frame::{
    FLAG_CLOSE, FLAG_DATA, FLAG_OPEN, FLAG_RESET, Frame, FrameDecoder, FrameDialer,
};
use spl_core::mux::{ResponseAssembler, WindowedUpload};
use spl_transport::connection::request_once;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::TlsConnector;
use x509_parser::extensions::ParsedExtension;
use x509_parser::oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY};

use door_support::{
    EchoObservation, Fixture, body_echo_router, body_echo_router_with_observations,
    multiplexed_requests, tree,
};

#[derive(Debug)]
struct AcceptAnyServerCertificate;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCertificate {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn test_client_config(
    versions: &[&'static rustls::SupportedProtocolVersion],
    client: Option<(
        rustls::pki_types::CertificateDer<'static>,
        rustls::pki_types::PrivateKeyDer<'static>,
    )>,
) -> rustls::ClientConfig {
    let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(versions)
    .expect("protocol versions")
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertificate));
    match client {
        Some((certificate, key)) => builder
            .with_client_auth_cert(vec![certificate], key)
            .expect("client identity"),
        None => builder.with_no_client_auth(),
    }
}

async fn tls_handshake(config: rustls::ClientConfig, port: u16) -> std::io::Result<()> {
    let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .expect("TCP connects");
    let mut stream = TlsConnector::from(Arc::new(config))
        .connect(
            rustls::pki_types::ServerName::try_from("spl.local").expect("name"),
            stream,
        )
        .await?;
    let mut byte = [0_u8; 1];
    match tokio::time::timeout(
        Duration::from_secs(1),
        tokio::io::AsyncReadExt::read(&mut stream, &mut byte),
    )
    .await
    {
        Ok(Err(error)) => Err(error),
        Ok(Ok(0)) => Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            "TLS peer closed after handshake",
        )),
        Ok(Ok(_)) => Ok(()),
        Err(_) => Ok(()),
    }
}

async fn exchange_over_carrier(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    decoder: &mut FrameDecoder,
    stream_id: u32,
) -> std::io::Result<()> {
    let request =
        b"GET /api/system/status HTTP/1.1\r\nhost: spl.local\r\ncontent-length: 0\r\n\r\n";
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_OPEN | FLAG_DATA, request.to_vec())
                .encode()
                .expect("request frame"),
        )
        .await?;
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_CLOSE, Vec::new())
                .encode()
                .expect("close frame"),
        )
        .await?;
    carrier.flush().await?;
    let mut buffer = [0_u8; 4096];
    loop {
        let read = carrier.read(&mut buffer).await?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "carrier closed",
            ));
        }
        decoder.feed(&buffer[..read]);
        for frame in decoder
            .drain()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
        {
            if frame.stream_id == stream_id && frame.flags & FLAG_CLOSE != 0 {
                return Ok(());
            }
        }
    }
}

/// Read a stalled stream's frames without returning WINDOW credit, until the
/// server resets it. Raw reads do not make the stream progress: only WINDOW
/// grants return its 1 MiB receive credit.
async fn await_stalled_stream_reset(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    decoder: &mut FrameDecoder,
    stream_id: u32,
) {
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = carrier
            .read(&mut buffer)
            .await
            .expect("stalled stream reads");
        assert!(
            read > 0,
            "carrier closed instead of resetting only stream A"
        );
        decoder.feed(&buffer[..read]);
        for frame in decoder.drain().expect("stalled stream frames") {
            if frame.stream_id == stream_id && frame.flags & FLAG_RESET != 0 {
                return;
            }
        }
    }
}

async fn complete_status_exchange(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    decoder: &mut FrameDecoder,
    stream_id: u32,
) -> spl_core::http::HttpResponse {
    let request =
        b"GET /api/system/status HTTP/1.1\r\nhost: spl.local\r\ncontent-length: 0\r\n\r\n";
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_OPEN | FLAG_DATA, request.to_vec())
                .encode()
                .expect("stream B request frame"),
        )
        .await
        .expect("stream B request writes");
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_CLOSE, Vec::new())
                .encode()
                .expect("stream B close frame"),
        )
        .await
        .expect("stream B close writes");
    carrier.flush().await.expect("stream B request flushes");

    let mut assembler = ResponseAssembler::new(stream_id);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = carrier.read(&mut buffer).await.expect("stream B reads");
        assert!(read > 0, "carrier closed before stream B completed");
        decoder.feed(&buffer[..read]);
        for frame in decoder.drain().expect("stream B frames") {
            if frame.stream_id != stream_id {
                continue;
            }
            let output = assembler
                .feed(&frame.encode().expect("routed stream B frame"))
                .expect("stream B response assembly");
            for frame in output.pongs.into_iter().chain(output.emit_frames) {
                carrier.write_all(&frame).await.expect("stream B controls");
            }
            carrier.flush().await.expect("stream B control flushes");
            if assembler.is_closed() {
                return assembler
                    .into_response()
                    .expect("complete stream B response");
            }
        }
    }
}

async fn live_carrier(
    fixture: &Fixture,
    port: u16,
) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
    let tcp = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .expect("one TCP carrier");
    TlsConnector::from(Arc::new(fixture.client_config(0)))
        .connect(
            rustls::pki_types::ServerName::try_from("spl.local").expect("name"),
            tcp,
        )
        .await
        .expect("mTLS carrier")
}

/// Send part of a declared upload, return all server window grants, then drop
/// the carrier without the request's explicit SPL close. `spl-home` maps that
/// carrier loss to `UnexpectedEof`; only an explicit `ReadEof` is clean EOF.
async fn partial_upload_then_drop(
    carrier: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    declared_body_len: usize,
    sent_body_len: usize,
) {
    let head = format!(
        "POST /__door_test/echo HTTP/1.1\r\nhost: spl.local\r\ncontent-length: {declared_body_len}\r\n\r\n"
    );
    partial_request_then_drop(carrier, head, declared_body_len, vec![b'x'; sent_body_len]).await;
}

/// Send valid chunked HTTP framing without its terminal chunk, then lose the
/// carrier. The intended decoded body is 2 MiB while the peer sends 1.5 MiB,
/// well past SPL's initial 1 MiB window.
async fn partial_chunked_upload_then_drop(
    carrier: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    intended_body_len: usize,
    sent_body_len: usize,
) {
    const CHUNK_LEN: usize = 256 * 1024;
    assert_eq!(sent_body_len % CHUNK_LEN, 0, "whole HTTP chunks");

    let mut body = Vec::with_capacity(sent_body_len + 128);
    for _ in 0..sent_body_len / CHUNK_LEN {
        body.extend_from_slice(format!("{CHUNK_LEN:x}\r\n").as_bytes());
        body.extend(std::iter::repeat_n(b'x', CHUNK_LEN));
        body.extend_from_slice(b"\r\n");
    }
    let head =
        "POST /__door_test/echo HTTP/1.1\r\nhost: spl.local\r\ntransfer-encoding: chunked\r\n\r\n"
            .to_owned();
    // This length belongs only to WindowedUpload's SPL staging; the HTTP
    // message deliberately has no Content-Length and no terminal chunk.
    let declared_wire_len = body.len() + (intended_body_len - sent_body_len) + 32;
    partial_request_then_drop(carrier, head, declared_wire_len, body).await;
}

async fn partial_request_then_drop(
    mut carrier: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    head: String,
    declared_wire_len: usize,
    body: Vec<u8>,
) {
    let stream_id = FrameDialer::default().allocate();
    let sent_body_len = body.len();
    let mut upload = WindowedUpload::new(stream_id, head.as_bytes(), declared_wire_len);
    let mut assembler = ResponseAssembler::new(stream_id);
    let mut decoder = FrameDecoder::new();
    let mut body_offset = 0;

    while upload.emitted_body_len() < sent_body_len {
        let mut wrote = false;
        loop {
            let capacity = upload.body_capacity();
            if capacity > 0 && body_offset < body.len() {
                let end = (body_offset + capacity).min(body.len());
                upload
                    .feed_body(&body[body_offset..end])
                    .expect("partial upload staging");
                body_offset = end;
            }
            let Some(frame) = upload.poll_send().expect("partial upload frame") else {
                break;
            };
            carrier
                .write_all(&frame)
                .await
                .expect("partial upload write");
            wrote = true;
        }
        if wrote {
            carrier.flush().await.expect("partial upload flush");
        }
        if upload.emitted_body_len() == sent_body_len {
            break;
        }

        let mut bytes = [0_u8; 64 * 1024];
        let read = carrier.read(&mut bytes).await.expect("window grant reads");
        assert!(
            read > 0,
            "carrier closed before partial upload reached target"
        );
        decoder.feed(&bytes[..read]);
        for frame in decoder.drain().expect("window grant frames") {
            let output = assembler
                .feed(&frame.encode().expect("routed window frame"))
                .expect("window grant assembly");
            for credit in output.window_grants {
                upload.grant(credit).expect("window credit");
            }
            for frame in output.pongs.into_iter().chain(output.emit_frames) {
                carrier
                    .write_all(&frame)
                    .await
                    .expect("window control write");
            }
        }
        carrier.flush().await.expect("window control flush");
    }
    assert_eq!(upload.emitted_body_len(), sent_body_len);
    drop(carrier);
}

async fn wait_for_echo_observation(
    observations: Arc<Mutex<Vec<EchoObservation>>>,
) -> EchoObservation {
    tokio::time::timeout(Duration::from_secs(10), async move {
        loop {
            if let Some(observation) = observations
                .lock()
                .expect("echo observations lock")
                .first()
                .cloned()
            {
                return observation;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("body handler observed the carrier drop")
}

fn options(fixture: &Fixture, app: Router, door_port: u16) -> ConveyServeOptions {
    ConveyServeOptions {
        journal_root: fixture.root.clone(),
        loopback_port: 0,
        door_port,
        handshake_timeout: Duration::from_secs(2),
        stream_stall_timeout: Duration::from_secs(2),
        router: app,
    }
}

fn door_port(outcome: &DoorOutcome) -> u16 {
    match outcome {
        DoorOutcome::Bound(address) => address.port(),
        other => panic!("door did not bind: {other:?}"),
    }
}

async fn request(
    fixture: &Fixture,
    client: usize,
    port: u16,
    path: &str,
) -> spl_core::http::HttpResponse {
    request_once(
        Arc::new(fixture.client_config(client)),
        "127.0.0.1",
        port,
        "GET",
        path,
        &[],
        &[],
    )
    .await
    .expect("door request")
}

async fn request_result(
    fixture: &Fixture,
    client: usize,
    port: u16,
) -> Result<spl_core::http::HttpResponse, spl_transport::TransportError> {
    request_once(
        Arc::new(fixture.client_config(client)),
        "127.0.0.1",
        port,
        "GET",
        "/api/system/status",
        &[],
        &[],
    )
    .await
}

#[test]
fn ac3_serve_accepts_only_the_prebuilt_router() {
    // `__door_test/basis` above is the sole test-only route and lives in this
    // integration-test crate, which is stronger than a feature-gated library route.
    let source = include_str!("../src/lib.rs");
    let serve = source
        .split("pub async fn serve")
        .nth(1)
        .expect("serve entry exists")
        .split("async fn serve_loopback")
        .next()
        .expect("serve body ends");
    assert!(!serve.contains("router("));
    assert!(!serve.contains("solstone_core_sol_link::http::router"));
}

fn ledger_posture(fixture: &Fixture) -> AuthorizedClientsRead {
    AuthorizationLedger::new(&fixture.root).read_state()
}

async fn presented_chain(
    fixture: &Fixture,
    port: u16,
) -> Vec<rustls::pki_types::CertificateDer<'static>> {
    let stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .expect("door TCP connect");
    let stream = TlsConnector::from(Arc::new(fixture.client_config(0)))
        .connect(
            rustls::pki_types::ServerName::try_from("spl.local").expect("server name"),
            stream,
        )
        .await
        .expect("mTLS handshake");
    stream
        .get_ref()
        .1
        .peer_certificates()
        .expect("server chain")
        .to_vec()
}

#[tokio::test]
async fn ac1_binds_door_beside_loopback() {
    let fixture = Fixture::established(1);
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("loopback bind");
    assert!(handle.loopback_ipv4_addr().ip().is_loopback());
    let port = door_port(handle.door_outcome());
    assert!(
        tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac2_process_router_status_answers_over_door() {
    let fixture = Fixture::established(1);
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let response = request(
        &fixture,
        0,
        door_port(handle.door_outcome()),
        "/api/system/status",
    )
    .await;
    assert_eq!(response.status, 200);
    handle.shutdown();
}

async fn basis(Extension(basis): Extension<AccessBasis>) -> String {
    format!("{basis:?}")
}

#[tokio::test]
async fn ac4_accepted_leaf_digest_is_the_did() {
    let fixture = Fixture::established(2);
    let app =
        router(fixture.root.clone()).merge(Router::new().route("/__door_test/basis", get(basis)));
    let handle = serve(options(&fixture, app, 0)).await.expect("serve");
    let port = door_port(handle.door_outcome());
    let first = String::from_utf8(request(&fixture, 0, port, "/__door_test/basis").await.body)
        .expect("basis");
    let second = String::from_utf8(request(&fixture, 1, port, "/__door_test/basis").await.body)
        .expect("basis");
    let digest = |der: &[u8]| format!("sha256:{:x}", Sha256::digest(der));
    assert!(first.contains(&digest(fixture.client_der(0))));
    assert!(second.contains(&digest(fixture.client_der(1))));
    assert_ne!(first, second);
    handle.shutdown();
}

#[tokio::test]
async fn ac5_loopback_connection_is_via_spl() {
    // A lode cannot originate an off-host connection; pure vectors cover Direct.
    let fixture = Fixture::established(1);
    let app =
        router(fixture.root.clone()).merge(Router::new().route("/__door_test/basis", get(basis)));
    let handle = serve(options(&fixture, app, 0)).await.expect("serve");
    let response = request(
        &fixture,
        0,
        door_port(handle.door_outcome()),
        "/__door_test/basis",
    )
    .await;
    assert!(
        String::from_utf8(response.body)
            .expect("basis")
            .contains("ViaSpl")
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac12_ca_chain_is_the_committed_on_disk_der() {
    let fixture = Fixture::established(1);
    let before = std::fs::read(fixture.root.join("link/ca/cert.pem")).expect("CA bytes");
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let chain = presented_chain(&fixture, door_port(handle.door_outcome())).await;
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[1].as_ref(), fixture.ca_der());
    assert_eq!(
        std::fs::read(fixture.root.join("link/ca/cert.pem")).expect("CA bytes"),
        before
    );
    // This same mTLS config made the handshake above, proving its CA-fingerprint
    // pin interoperates with the committed CA rather than only structural parsing.
    handle.shutdown();
}

#[tokio::test]
async fn ac16_server_leaf_matches_reference_contract_and_is_fresh() {
    let fixture = Fixture::established(1);
    let first = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("first serve");
    let first_chain = presented_chain(&fixture, door_port(first.door_outcome())).await;
    first.shutdown();
    let second = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("second serve");
    let second_chain = presented_chain(&fixture, door_port(second.door_outcome())).await;
    let (_, leaf) =
        x509_parser::parse_x509_certificate(first_chain[0].as_ref()).expect("leaf parses");
    let (_, ca) = x509_parser::parse_x509_certificate(fixture.ca_der()).expect("CA parses");
    assert_eq!(
        leaf.subject()
            .iter_common_name()
            .next()
            .and_then(|value| value.as_str().ok()),
        Some("solstone link (test home)")
    );
    assert_eq!(leaf.issuer().as_raw(), ca.subject().as_raw());
    assert_eq!(
        leaf.tbs_certificate.subject_pki.algorithm.algorithm,
        OID_KEY_TYPE_EC_PUBLIC_KEY
    );
    assert_eq!(
        leaf.tbs_certificate
            .subject_pki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|value| value.as_oid().ok()),
        Some(OID_EC_P256)
    );
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!((leaf.validity().not_before.timestamp() - (now - 300)).abs() <= 5);
    assert!((leaf.validity().not_after.timestamp() - (now + 30 * 86_400)).abs() <= 5);
    let constraints = leaf
        .basic_constraints()
        .expect("basic constraints")
        .expect("basic constraints present");
    assert!(constraints.critical && !constraints.value.ca);
    let usage = leaf
        .extended_key_usage()
        .expect("EKU")
        .expect("EKU present");
    assert!(!usage.critical && usage.value.server_auth);
    assert!(!leaf.extensions().iter().any(|extension| matches!(
        extension.parsed_extension(),
        ParsedExtension::SubjectAlternativeName(_)
    )));
    let (_, second_leaf) =
        x509_parser::parse_x509_certificate(second_chain[0].as_ref()).expect("second leaf parses");
    assert_ne!(leaf.raw_serial(), second_leaf.raw_serial());
    assert_ne!(
        leaf.tbs_certificate.subject_pki.raw,
        second_leaf.tbs_certificate.subject_pki.raw
    );
    second.shutdown();
}

#[tokio::test]
async fn ac6_ca_signed_but_unlisted_client_is_refused() {
    let fixture = Fixture::established(1);
    fixture.remove_authorization(0);
    assert_eq!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(Vec::new())
    );
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    assert!(
        request_result(&fixture, 0, door_port(handle.door_outcome()))
            .await
            .is_err()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac6_no_client_certificate_fails_at_tls() {
    let fixture = Fixture::established(1);
    assert!(matches!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(_)
    ));
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let error = tls_handshake(
        test_client_config(&[&rustls::version::TLS13], None),
        door_port(handle.door_outcome()),
    )
    .await
    .expect_err("server must reject an absent client certificate during TLS");
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    ));
    handle.shutdown();
}

#[tokio::test]
async fn ac6_client_not_signed_by_journal_ca_is_refused() {
    let fixture = Fixture::established(1);
    let foreign_key =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("foreign key");
    let foreign_ca_key =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("foreign CA key");
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let foreign_ca = ca_params.self_signed(&foreign_ca_key).expect("foreign CA");
    let mut client_params = rcgen::CertificateParams::default();
    client_params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let foreign_client = client_params
        .signed_by(&foreign_key, &foreign_ca, &foreign_ca_key)
        .expect("foreign client");
    let config = test_client_config(
        &[&rustls::version::TLS13],
        Some((
            rustls::pki_types::CertificateDer::from(foreign_client.der().to_vec()),
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                foreign_key.serialize_der(),
            )),
        )),
    );
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    assert!(
        tls_handshake(config, door_port(handle.door_outcome()))
            .await
            .is_err()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac11_tls12_only_client_is_refused() {
    let fixture = Fixture::established(1);
    let config = test_client_config(&[&rustls::version::TLS12], Some(fixture.client_identity(0)));
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    assert!(
        tls_handshake(config, door_port(handle.door_outcome()))
            .await
            .is_err()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac6_unreadable_ledger_is_refused() {
    let fixture = Fixture::established(1);
    let path = fixture.root.join("link/authorized_clients.json");
    std::fs::remove_file(&path).expect("authorization file removes");
    std::fs::create_dir(&path).expect("unreadable authorization directory");
    assert_eq!(ledger_posture(&fixture), AuthorizedClientsRead::Unreadable);
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    assert!(
        request_result(&fixture, 0, door_port(handle.door_outcome()))
            .await
            .is_err()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac6_malformed_ledger_is_refused() {
    let fixture = Fixture::established(1);
    std::fs::write(fixture.root.join("link/authorized_clients.json"), b"{}")
        .expect("malformed authorization");
    assert_eq!(ledger_posture(&fixture), AuthorizedClientsRead::Malformed);
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    assert!(
        request_result(&fixture, 0, door_port(handle.door_outcome()))
            .await
            .is_err()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac7_missing_ledger_is_refused() {
    let fixture = Fixture::established(1);
    std::fs::remove_file(fixture.root.join("link/authorized_clients.json"))
        .expect("authorization removes");
    assert_eq!(ledger_posture(&fixture), AuthorizedClientsRead::Missing);
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    assert!(
        request_result(&fixture, 0, door_port(handle.door_outcome()))
            .await
            .is_err()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac9_revocation_refuses_a_new_connection_after_the_present_transition() {
    let fixture = Fixture::established(1);
    assert!(matches!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(_)
    ));
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let port = door_port(handle.door_outcome());
    assert!(request_result(&fixture, 0, port).await.is_ok());
    fixture.remove_authorization(0);
    assert_eq!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(Vec::new())
    );
    let refused = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if request_result(&fixture, 0, port).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(refused.is_ok(), "new dial did not observe revocation");
    handle.shutdown();
}

#[tokio::test]
async fn ac13_python_shaped_state_with_and_without_locked_at_opens_the_door() {
    for locked_at in [None, Some("2026-01-01T00:00:00Z")] {
        let fixture = Fixture::established(1);
        let state: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.root.join("link/state.json")).expect("state reads"),
        )
        .expect("state parses");
        let instance_id = state["instance_id"].as_str().expect("instance");
        // These are deliberately hand-written, verbatim Python-shaped literals.
        let python_state = match locked_at {
            None => r#"{"instance_id":"__INSTANCE_ID__","home_label":"test home"}"#,
            Some(_) => r#"{"instance_id":"__INSTANCE_ID__","home_label":"test home","locked_at":"2026-01-01T00:00:00Z"}"#,
        }
        .replace("__INSTANCE_ID__", instance_id);
        std::fs::write(fixture.root.join("link/state.json"), python_state)
            .expect("Python-shaped state writes");
        let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
            .await
            .expect("serve");
        let chain = presented_chain(&fixture, door_port(handle.door_outcome())).await;
        assert_eq!(chain[1].as_ref(), fixture.ca_der());
        handle.shutdown();
    }
}

#[tokio::test]
async fn ac15_absent_ca_withholds_door_without_minting_link_material() {
    let fixture = Fixture::established(0);
    std::fs::remove_dir_all(fixture.root.join("link/ca")).expect("CA removes");
    let before = tree(&fixture.root.join("link"));
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("loopback serves");
    assert!(matches!(
        handle.door_outcome(),
        DoorOutcome::Withheld(DoorWithheldReason::CommittedIdentityUnavailable)
    ));
    assert_eq!(tree(&fixture.root.join("link")), before);
    handle.shutdown();
}

#[tokio::test]
async fn ac10_revocation_closes_the_already_open_carrier_without_redial() {
    let fixture = Fixture::established(1);
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let mut carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;
    let mut decoder = FrameDecoder::new();
    let mut ids = FrameDialer::default();
    assert!(
        exchange_over_carrier(&mut carrier, &mut decoder, ids.allocate())
            .await
            .is_ok()
    );
    fixture.remove_authorization(0);
    assert_eq!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(Vec::new())
    );
    let transitioned = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if exchange_over_carrier(&mut carrier, &mut decoder, ids.allocate())
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        transitioned.is_ok(),
        "the same TLS carrier did not close after revocation"
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac10_transient_unreadable_posture_does_not_close_admitted_carrier() {
    let fixture = Fixture::established(1);
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let mut carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;
    let mut decoder = FrameDecoder::new();
    let mut ids = FrameDialer::default();
    assert!(
        exchange_over_carrier(&mut carrier, &mut decoder, ids.allocate())
            .await
            .is_ok()
    );
    let authorization = fixture.root.join("link/authorized_clients.json");
    std::fs::remove_file(&authorization).expect("authorization removes");
    std::fs::create_dir(&authorization).expect("unreadable authorization directory");
    assert_eq!(ledger_posture(&fixture), AuthorizedClientsRead::Unreadable);
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    assert!(
        exchange_over_carrier(&mut carrier, &mut decoder, ids.allocate())
            .await
            .is_ok(),
        "unreadable posture closed an admitted carrier"
    );
    std::fs::remove_dir(&authorization).expect("unreadable directory removes");
    std::fs::write(&authorization, b"[]").expect("definite revocation writes");
    assert_eq!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(Vec::new())
    );
    let transitioned = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if exchange_over_carrier(&mut carrier, &mut decoder, ids.allocate())
                .await
                .is_err()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        transitioned.is_ok(),
        "definite Present removal did not close carrier"
    );
    handle.shutdown();
}

#[cfg(unix)]
#[tokio::test]
async fn ac21_handshake_touches_native_devices_ledger_and_emits_callosum() {
    let fixture = Fixture::established(1);
    let health = fixture.root.join("health");
    std::fs::create_dir(&health).expect("health directory");
    let socket = health.join("callosum.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("Callosum listener");
    let received = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("Callosum accepts");
        let mut line = String::new();
        stream
            .read_to_string(&mut line)
            .await
            .expect("Callosum line reads");
        line
    });
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let port = door_port(handle.door_outcome());
    assert!(request_result(&fixture, 0, port).await.is_ok());
    let did = format!("sha256:{}", spl_core::ca::sha256_hex(fixture.client_der(0)));
    let message: serde_json::Value =
        serde_json::from_str(&received.await.expect("Callosum task")).expect("Callosum JSON");
    assert_eq!(message["tract"], "link");
    assert_eq!(message["event"], "last_seen");
    assert_eq!(message["fingerprint"], did);
    assert!(
        std::fs::read_to_string(fixture.root.join("link/devices.json"))
            .expect("devices ledger")
            .contains(&did)
    );
    handle.shutdown();

    let without_socket = Fixture::established(1);
    let handle = serve(options(
        &without_socket,
        router(without_socket.root.clone()),
        0,
    ))
    .await
    .expect("serve without Callosum");
    assert!(
        request_result(&without_socket, 0, door_port(handle.door_outcome()))
            .await
            .is_ok()
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac17_two_streams_and_bidirectional_two_mib_complete() {
    const TWO_MIB: usize = 2 * 1024 * 1024;
    let fixture = Fixture::established(1);
    // This test-crate route is the D-9 exception: `serve` still receives the
    // prebuilt merged router and the library constructs no test surface.
    let app = router(fixture.root.clone()).merge(body_echo_router());
    let handle = serve(options(&fixture, app, 0)).await.expect("serve");
    let port = door_port(handle.door_outcome());
    let mut carrier = live_carrier(&fixture, port).await;
    let responses = tokio::time::timeout(
        Duration::from_secs(60),
        multiplexed_requests(
            &mut carrier,
            vec![
                (
                    "/__door_test/echo?response_bytes=2097152",
                    vec![b'a'; TWO_MIB],
                ),
                ("/__door_test/echo", vec![b'b'; TWO_MIB]),
            ],
        ),
    )
    .await
    .expect("one-carrier multiplexed exchange must complete");
    assert_eq!(responses.len(), 2);
    let first = &responses[0];
    let second = &responses[1];
    assert_eq!(first.status, 200);
    assert_eq!(first.body.len(), TWO_MIB);
    assert_eq!(second.status, 200);
    assert_eq!(second.header("x-body-bytes"), Some("2097152"));
    assert_eq!(second.header("x-body-complete"), Some("true"));
    handle.shutdown();
}

#[tokio::test]
async fn ac18_mid_body_carrier_death_is_not_a_complete_echo() {
    const TWO_MIB: usize = 2 * 1024 * 1024;
    const SENT_BODY: usize = 3 * TWO_MIB / 4;
    let fixture = Fixture::established(1);
    let observations = Arc::new(Mutex::new(Vec::new()));
    let app = router(fixture.root.clone())
        .merge(body_echo_router_with_observations(observations.clone()));
    let handle = serve(options(&fixture, app, 0)).await.expect("serve");
    let carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;

    // `spl-home` maps STREAM_GONE to UnexpectedEof and STREAM_RESET to
    // ConnectionReset; only explicit ReadEof is clean. Nothing upstream pins
    // that mapping, so this is regression coverage for the pinned dependency.
    partial_upload_then_drop(carrier, TWO_MIB, SENT_BODY).await;
    let observation = wait_for_echo_observation(observations).await;
    assert!(
        !(200..300).contains(&observation.status),
        "a mid-body carrier death must not produce a 2xx: {observation:?}"
    );
    assert_ne!(
        observation.body_bytes, TWO_MIB,
        "a mid-body carrier death must not report the declared body as complete"
    );
    assert!(
        !observation.complete || observation.body_error.is_some(),
        "a short body must be distinguished from a clean complete body: {observation:?}"
    );
    handle.shutdown();

    let fixture = Fixture::established(1);
    let observations = Arc::new(Mutex::new(Vec::new()));
    let app = router(fixture.root.clone())
        .merge(body_echo_router_with_observations(observations.clone()));
    let handle = serve(options(&fixture, app, 0)).await.expect("serve");
    let carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;

    // Unlike the length-delimited case above, this request has no length oracle:
    // chunked HTTP framing is valid until the deliberately omitted terminal chunk.
    // STREAM_GONE must therefore remain an I/O error rather than clean EOF. Hyper
    // independently rejects a clean EOF before the terminal chunk, so this is
    // regression coverage over hyper plus pinned spl-home, not proof that this
    // door wrapper alone preserves the mapping.
    partial_chunked_upload_then_drop(carrier, TWO_MIB, SENT_BODY).await;
    let observation = wait_for_echo_observation(observations).await;
    assert!(
        !(200..300).contains(&observation.status),
        "a chunked mid-body carrier death must not produce a 2xx: {observation:?}"
    );
    assert!(
        !observation.complete,
        "a chunked mid-body carrier death must not be a clean end: {observation:?}"
    );
    assert!(
        observation.body_error.is_some(),
        "a chunked mid-body carrier death must reach the handler as an error: {observation:?}"
    );
    assert_ne!(
        observation.body_bytes, TWO_MIB,
        "a chunked mid-body carrier death must not report the intended body as complete"
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac19_silent_tcp_handshake_times_out_without_blocking_next_client() {
    let fixture = Fixture::established(1);
    let mut serve_options = options(&fixture, router(fixture.root.clone()), 0);
    serve_options.handshake_timeout = Duration::from_millis(100);
    let handle = serve(serve_options).await.expect("serve");
    let port = door_port(handle.door_outcome());
    let mut silent = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .expect("silent TCP peer connects");

    let admitted = request_result(&fixture, 0, port);
    let silent_dropped = async {
        let mut byte = [0_u8; 1];
        match silent.read(&mut byte).await {
            Ok(0) | Err(_) => (),
            Ok(_) => panic!("silent peer received unexpected handshake data"),
        }
    };
    let (admitted, silent_dropped) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(10), admitted),
        tokio::time::timeout(Duration::from_secs(10), silent_dropped),
    );
    assert!(
        admitted
            .expect("authorized device admission completed")
            .is_ok(),
        "a silent handshake must not block the next authorized client"
    );
    silent_dropped.expect("silent TCP handshake was dropped");
    handle.shutdown();
}

#[tokio::test]
async fn ac20_stalled_reader_resets_only_its_stream() {
    const TWO_MIB: usize = 2 * 1024 * 1024;
    let fixture = Fixture::established(1);
    let mut serve_options = options(
        &fixture,
        router(fixture.root.clone()).merge(body_echo_router()),
        0,
    );
    serve_options.stream_stall_timeout = Duration::from_millis(100);
    let handle = serve(serve_options).await.expect("serve");
    let mut carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;

    tokio::time::timeout(Duration::from_secs(30), async {
        let mut ids = FrameDialer::default();
        let stalled_stream = ids.allocate();
        let request = format!(
            "POST /__door_test/echo?response_bytes={TWO_MIB} HTTP/1.1\r\nhost: spl.local\r\ncontent-length: 0\r\n\r\n"
        );
        carrier
            .write_all(
                &Frame::new(stalled_stream, FLAG_OPEN | FLAG_DATA, request.into_bytes())
                    .encode()
                    .expect("stalled request frame"),
            )
            .await
            .expect("stalled request writes");
        carrier
            .write_all(
                &Frame::new(stalled_stream, FLAG_CLOSE, Vec::new())
                    .encode()
                    .expect("stalled request close"),
            )
            .await
            .expect("stalled request close writes");
        carrier.flush().await.expect("stalled request flushes");

        let mut decoder = FrameDecoder::new();
        await_stalled_stream_reset(&mut carrier, &mut decoder, stalled_stream).await;

        let response = complete_status_exchange(&mut carrier, &mut decoder, ids.allocate()).await;
        assert_eq!(response.status, 200, "stream B remains live after A resets");
    })
    .await
    .expect("stalled stream reset and independent stream B exchange must complete");
    handle.shutdown();
}

#[tokio::test]
async fn ac22_door_bind_failure_preserves_live_loopback_and_is_structured() {
    let fixture = Fixture::established(0);
    let held = std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
        .expect("hold door port");
    let port = held.local_addr().expect("held address").port();
    let handle = serve(options(&fixture, router(fixture.root.clone()), port))
        .await
        .expect("loopback survives");
    assert!(
        matches!(handle.door_outcome(), DoorOutcome::BindFailed { port: actual, .. } if *actual == port)
    );
    assert!(
        tokio::net::TcpStream::connect(handle.loopback_ipv4_addr())
            .await
            .is_ok()
    );
    assert_eq!(held.local_addr().expect("holder remains").port(), port);
    handle.shutdown();
}

#[tokio::test]
async fn ac23_unestablished_or_corrupt_session_withholds_door_without_ca_write() {
    for corrupt in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "solstone-door-withheld-{}-{corrupt}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).expect("config");
        if corrupt {
            std::fs::write(root.join("config/journal.json"), b"{").expect("corrupt config");
        }
        let before = tree(&root.join("link"));
        let handle = serve(ConveyServeOptions {
            journal_root: root.clone(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: Duration::from_secs(1),
            stream_stall_timeout: Duration::from_secs(1),
            router: Router::new(),
        })
        .await
        .expect("loopback");
        assert!(matches!(
            handle.door_outcome(),
            DoorOutcome::Withheld(DoorWithheldReason::Unestablished | DoorWithheldReason::Corrupt)
        ));
        assert_eq!(tree(&root.join("link")), before);
        handle.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }
}
