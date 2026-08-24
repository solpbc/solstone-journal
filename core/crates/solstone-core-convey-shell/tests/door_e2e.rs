// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Focused host-door coverage. The body-echo route is deliberately in this
//! integration-test crate, stronger than a feature-gated library test surface.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime};

use axum::Extension;
use axum::Router;
use axum::http::StatusCode;
use axum::routing::{get, post};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use nix::sys::stat::Mode;
use nix::unistd::mkfifo;
use sha2::{Digest, Sha256};
use solstone_core_convey_http::identity::AccessBasis;
use solstone_core_convey_shell::authorization_gate::authorized_router;
use solstone_core_convey_shell::{
    ConveyServeOptions, DoorOutcome, DoorWithheldReason, bind_with_authorization, router, serve,
};
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, read_authorized_clients,
};
use solstone_core_sol_link::pairing::addresses::{EndpointScope, LocalEndpoint, PairingSnapshot};
use solstone_core_sol_link::{DeviceDoorAuthorization, authorization_publication_ticks};
use spl_core::frame::{
    FLAG_CLOSE, FLAG_DATA, FLAG_OPEN, FLAG_RESET, Frame, FrameDecoder, FrameDialer,
};
use spl_core::mux::{ResponseAssembler, WindowedUpload};
use spl_transport::client::TransportClient;
use spl_transport::connection::request_once;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, accept_hdr_async, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        handshake::server::Request as ServerRequest,
        http::{HeaderValue, StatusCode as WsStatusCode, header::AUTHORIZATION},
    },
};
use x509_parser::extensions::ParsedExtension;
use x509_parser::oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY};
use x509_parser::pem::parse_x509_pem;

use crate::door_support::{
    EchoObservation, Fixture, body_echo_router, body_echo_router_with_observations,
    get_over_carrier, multiplexed_requests, tree,
};
use crate::warn_capture;

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

fn assert_tls_alert(result: std::io::Result<()>, alert: rustls::AlertDescription) {
    let error = result.expect_err("TLS handshake must fail");
    let rustls_error = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<rustls::Error>());
    assert!(matches!(
        rustls_error,
        Some(rustls::Error::AlertReceived(received)) if *received == alert
    ));
}

struct BlockingAuthorizationFifo {
    path: std::path::PathBuf,
    drain: Option<File>,
}

impl BlockingAuthorizationFifo {
    fn replace(path: std::path::PathBuf) -> Self {
        let _ = fs::remove_file(&path);
        mkfifo(&path, Mode::S_IRUSR | Mode::S_IWUSR).expect("authorization FIFO creates");
        let drain = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("authorization FIFO drain opens");
        Self {
            path,
            drain: Some(drain),
        }
    }
}

impl Drop for BlockingAuthorizationFifo {
    fn drop(&mut self) {
        self.drain.take();
        let _ = fs::remove_file(&self.path);
    }
}

async fn exchange_over_carrier<S>(
    carrier: &mut tokio_rustls::client::TlsStream<S>,
    decoder: &mut FrameDecoder,
    stream_id: u32,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> std::io::Result<spl_core::http::HttpResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = spl_core::http::build_request(method, path, headers, body);
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
    let mut assembler = ResponseAssembler::new(stream_id);
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
            if frame.stream_id == stream_id {
                let output = assembler
                    .feed(&frame.encode().map_err(std::io::Error::other)?)
                    .map_err(std::io::Error::other)?;
                for control in output.pongs.into_iter().chain(output.emit_frames) {
                    carrier.write_all(&control).await?;
                }
                carrier.flush().await?;
                if assembler.is_closed() {
                    return assembler.into_response().map_err(std::io::Error::other);
                }
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

async fn live_certless_carrier(
    port: u16,
) -> std::io::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let tcp = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .await
        .expect("certless TCP carrier");
    certless_carrier(tcp).await
}

async fn certless_carrier<S>(stream: S) -> std::io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    TlsConnector::from(Arc::new(test_client_config(
        &[&rustls::version::TLS13],
        None,
    )))
    .connect(
        rustls::pki_types::ServerName::try_from("spl.local").expect("name"),
        stream,
    )
    .await
}

fn pairing_now() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64
}

fn open_pairing_window(fixture: &Fixture, nonce: &str) {
    let now = pairing_now();
    solstone_core_sol_link::pairing::nonces::NonceStore::new(&fixture.root)
        .add(nonce.into(), "phone".into(), "".into(), false, now)
        .expect("open pairing window");
}

fn foreign_client_config() -> rustls::ClientConfig {
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
    test_client_config(
        &[&rustls::version::TLS13],
        Some((
            rustls::pki_types::CertificateDer::from(foreign_client.der().to_vec()),
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                foreign_key.serialize_der(),
            )),
        )),
    )
}

async fn assert_certless_tls_refused(fixture: &Fixture) {
    let handle = serve(options(fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve closed window");
    let error = tls_handshake(
        test_client_config(&[&rustls::version::TLS13], None),
        door_port(handle.door_outcome()),
    )
    .await
    .expect_err("closed pairing window requires a client certificate");
    assert!(matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    ));
    handle.shutdown();
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
        carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
        handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
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

async fn loopback_post(address: SocketAddr, path: &str, body: &[u8]) -> u16 {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("loopback connects");
    let request = format!(
        "POST {path} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("request headers write");
    stream.write_all(body).await.expect("request body writes");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("response");
    std::str::from_utf8(&response)
        .expect("response is text")
        .split_whitespace()
        .nth(1)
        .expect("status token")
        .parse()
        .expect("status parses")
}

async fn loopback_status(address: SocketAddr) -> std::io::Result<u16> {
    let mut stream = tokio::net::TcpStream::connect(address).await?;
    stream
        .write_all(
            b"GET /api/system/status HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n",
        )
        .await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    std::str::from_utf8(&response)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "missing status"))?
        .parse()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

async fn collect_callosum_events(
    listener: tokio::net::UnixListener,
    expected_event: &str,
    expected_count: usize,
) -> Vec<serde_json::Value> {
    let mut events = Vec::with_capacity(expected_count);
    let mut reads = tokio::task::JoinSet::new();
    while events.len() < expected_count {
        tokio::select! {
            accepted = listener.accept() => {
                let (mut stream, _) = accepted.expect("Callosum accepts");
                reads.spawn(async move {
                    let mut line = String::new();
                    stream
                        .read_to_string(&mut line)
                        .await
                        .expect("Callosum line reads");
                    serde_json::from_str::<serde_json::Value>(&line)
                        .expect("Callosum JSON")
                });
            }
            message = reads.join_next(), if !reads.is_empty() => {
                let message = message
                    .expect("Callosum read task")
                    .expect("Callosum read task completes");
                if message["event"] == expected_event {
                    events.push(message);
                }
            }
        }
    }
    events
}

async fn loopback_json_request(
    address: SocketAddr,
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> (u16, Vec<u8>) {
    let body = serde_json::to_vec(body).expect("loopback request JSON");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("loopback connects");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("request head");
    stream.write_all(&body).await.expect("request body");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("response reads");
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response headers")
        + 4;
    let status = std::str::from_utf8(&response[..header_end])
        .expect("response headers UTF-8")
        .split_whitespace()
        .nth(1)
        .expect("response status")
        .parse()
        .expect("numeric response status");
    (status, response[header_end..].to_vec())
}

async fn mint_from_loopback(
    handle: &solstone_core_convey_shell::ConveyServeHandle,
    label: &str,
) -> serde_json::Value {
    let (status, body) = loopback_json_request(
        handle.loopback_ipv4_addr(),
        "POST",
        "/app/network/pair-start",
        &serde_json::json!({"device_label": label, "same_machine": true}),
    )
    .await;
    assert_eq!(
        status,
        200,
        "pair-start: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("pair-start JSON")
}

async fn mint_relay_from_loopback(
    handle: &solstone_core_convey_shell::ConveyServeHandle,
    label: &str,
) -> serde_json::Value {
    let (status, body) = loopback_json_request(
        handle.loopback_ipv4_addr(),
        "POST",
        "/app/network/pair-start",
        &serde_json::json!({"device_label": label, "same_machine": false}),
    )
    .await;
    assert_eq!(
        status,
        200,
        "relay pair-start: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).expect("relay pair-start JSON")
}

fn configure_relay_pairing(fixture: &Fixture, relay_origin: &str) {
    let config = serde_json::json!({
        "setup": {"completed_at": 1},
        "link": {"posture": "spl", "relay_url": relay_origin},
    });
    fs::write(
        fixture.root.join("config/journal.json"),
        serde_json::to_vec(&config).expect("relay config JSON"),
    )
    .expect("relay config writes");
    let token = fixture.root.join("link/tokens/account.json");
    fs::create_dir_all(token.parent().expect("token parent")).expect("token parent creates");
    fs::write(token, br#"{"service_token":"door-e2e-service-token"}"#)
        .expect("service token writes");
}

fn configure_direct_port(fixture: &Fixture, port: u16) {
    let path = fixture.root.join("config/journal.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("config reads")).expect("config parses");
    let pairing = config
        .as_object_mut()
        .expect("config object")
        .entry("pairing")
        .or_insert_with(|| serde_json::json!({}));
    pairing
        .as_object_mut()
        .expect("pairing object")
        .insert("direct_port".to_owned(), serde_json::json!(port));
    fs::write(
        &path,
        serde_json::to_vec(&config).expect("config serializes"),
    )
    .expect("config writes");
}

fn pairing_snapshot() -> PairingSnapshot {
    PairingSnapshot {
        endpoints: vec![LocalEndpoint {
            ip: Ipv4Addr::new(10, 0, 0, 8).into(),
            scope: EndpointScope::Lan,
        }],
        route_ipv4: Some(Ipv4Addr::new(10, 0, 0, 8)),
    }
}

fn pairing_router(fixture: &Fixture, snapshot: PairingSnapshot) -> Router {
    router(fixture.root.clone()).layer(Extension(snapshot))
}

fn tree_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = std::fs::read_dir(root)
        .expect("tree reads")
        .map(|entry| entry.expect("tree entry").path())
        .collect::<Vec<_>>();
    paths.sort();
    let mut output = Vec::new();
    for path in paths {
        if path.is_dir() {
            output.extend(tree_paths(&path));
        } else {
            output.push(path);
        }
    }
    output
}

fn csr_pem(_label: &str) -> String {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("device key");
    rcgen::CertificateParams::default()
        .serialize_request(&key)
        .expect("device CSR")
        .pem()
        .expect("device CSR PEM")
}

async fn post_pair_over_certless_carrier(
    port: u16,
    token: Option<&str>,
    body: serde_json::Value,
) -> spl_core::http::HttpResponse {
    let mut carrier = live_certless_carrier(port)
        .await
        .expect("certless pairing carrier");
    post_pair_over_certless_stream(&mut carrier, token, body).await
}

async fn post_pair_over_certless_stream<S>(
    carrier: &mut tokio_rustls::client::TlsStream<S>,
    token: Option<&str>,
    body: serde_json::Value,
) -> spl_core::http::HttpResponse
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut decoder = FrameDecoder::new();
    let path = token.map_or_else(
        || spl_core::PAIR_PATH.to_owned(),
        |token| format!("{}?token={token}", spl_core::PAIR_PATH),
    );
    exchange_over_carrier(
        carrier,
        &mut decoder,
        1,
        "POST",
        &path,
        &[("content-type".into(), "application/json".into())],
        &serde_json::to_vec(&body).expect("pair request JSON"),
    )
    .await
    .expect("pair response")
}

const RELAY_TUNNEL_WRITE_MAX: usize = 64 * 1024;

type PairRelaySocket = WebSocketStream<TcpStream>;

/// A test-only relay that brokers one mobile pair-dial into the matching home
/// attach tunnel. It rejects application credentials in URLs, and it binds
/// the tunnel attachment to the registration bearer before upgrading either
/// side into the byte bridge.
struct SinglePairRelay {
    origin: String,
    listener_task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct SinglePairRelayState {
    windows: HashMap<String, PairRelayWindow>,
    tunnels: HashMap<String, PairRelayTunnel>,
    next_tunnel: u64,
}

struct PairRelayWindow {
    token: String,
    offers: mpsc::UnboundedSender<String>,
}

struct PairRelayTunnel {
    token: String,
    relay_key: String,
    attach: Option<oneshot::Sender<PairRelaySocket>>,
}

enum PairRelayConnection {
    Registration {
        relay_key: String,
        token: String,
    },
    PairDial {
        relay_key: String,
    },
    TunnelAttach {
        attach: oneshot::Sender<PairRelaySocket>,
    },
}

impl SinglePairRelay {
    async fn bind() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("pair relay listener binds");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("pair relay listener address")
        );
        let state = Arc::new(Mutex::new(SinglePairRelayState::default()));
        let listener_state = Arc::clone(&state);
        let listener_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Arc::clone(&listener_state);
                std::mem::drop(tokio::spawn(async move {
                    serve_single_pair_relay_connection(stream, state).await;
                }));
            }
        });
        Self {
            origin,
            listener_task,
        }
    }

    fn origin(&self) -> &str {
        &self.origin
    }
}

impl Drop for SinglePairRelay {
    fn drop(&mut self) {
        self.listener_task.abort();
    }
}

#[allow(clippy::result_large_err)]
async fn serve_single_pair_relay_connection(
    mut stream: TcpStream,
    state: Arc<Mutex<SinglePairRelayState>>,
) {
    if is_pair_relay_enroll_request(&stream).await {
        serve_pair_relay_enroll(&mut stream).await;
        return;
    }
    let accepted = Arc::new(Mutex::new(None));
    let accepted_for_callback = Arc::clone(&accepted);
    let state_for_callback = Arc::clone(&state);
    let websocket = accept_hdr_async(stream, move |request: &ServerRequest, response| {
        match inspect_single_pair_relay_request(request, &state_for_callback) {
            Ok(connection) => {
                *lock_pair_relay(&accepted_for_callback) = Some(connection);
                Ok(response)
            }
            Err(status) => Err(pair_relay_rejection(status)),
        }
    })
    .await;
    let Ok(websocket) = websocket else {
        return;
    };
    let Some(accepted) = lock_pair_relay(&accepted).take() else {
        return;
    };

    match accepted {
        PairRelayConnection::Registration { relay_key, token } => {
            let (offers, receiver) = mpsc::unbounded_channel();
            lock_pair_relay(&state)
                .windows
                .insert(relay_key.clone(), PairRelayWindow { token, offers });
            serve_single_pair_relay_registration(websocket, receiver).await;
            lock_pair_relay(&state).windows.remove(&relay_key);
        }
        PairRelayConnection::PairDial { relay_key } => {
            serve_single_pair_dial(websocket, state, &relay_key).await;
        }
        PairRelayConnection::TunnelAttach { attach } => {
            let _ = attach.send(websocket);
        }
    }
}

async fn is_pair_relay_enroll_request(stream: &TcpStream) -> bool {
    tokio::time::timeout(Duration::from_secs(1), async {
        let mut preview = [0_u8; 256];
        loop {
            let count = stream.peek(&mut preview).await.ok()?;
            if count == 0 {
                return None;
            }
            if let Some(line_end) = preview[..count].windows(2).position(|item| item == b"\r\n") {
                return Some(&preview[..line_end] == b"POST /enroll/device HTTP/1.1");
            }
            if count == preview.len() {
                return Some(false);
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

async fn serve_pair_relay_enroll(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut header_end = None;
    let mut content_length = None;
    let mut buffer = [0_u8; 1024];
    while request.len() < 64 * 1024 {
        let Ok(count) = stream.read(&mut buffer).await else {
            return;
        };
        if count == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..count]);
        if header_end.is_none()
            && let Some(offset) = request.windows(4).position(|item| item == b"\r\n\r\n")
        {
            let end = offset + 4;
            header_end = Some(end);
            content_length = std::str::from_utf8(&request[..offset])
                .ok()
                .and_then(|headers| {
                    headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                });
        }
        if header_end.is_some_and(|end| request.len() >= end + content_length.unwrap_or(0)) {
            break;
        }
    }
    let body = br#"{"device_token":"test-device-token"}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(response.as_bytes()).await.is_ok() {
        let _ = stream.write_all(body).await;
        let _ = stream.shutdown().await;
    }
}

fn inspect_single_pair_relay_request(
    request: &ServerRequest,
    state: &Arc<Mutex<SinglePairRelayState>>,
) -> Result<PairRelayConnection, WsStatusCode> {
    match request.uri().path() {
        "/session/pair-window" => inspect_pair_window_registration(request),
        "/session/pair-dial" => inspect_pair_dial(request, state),
        path if path.starts_with("/tunnel/") => inspect_pair_tunnel_attach(request, state),
        _ => Err(WsStatusCode::NOT_FOUND),
    }
}

fn inspect_pair_window_registration(
    request: &ServerRequest,
) -> Result<PairRelayConnection, WsStatusCode> {
    let Some(token) = pair_relay_bearer(request) else {
        return Err(WsStatusCode::UNAUTHORIZED);
    };
    let Some(relay_key) = pair_relay_header(request, "sec-pair-key") else {
        return Err(WsStatusCode::BAD_REQUEST);
    };
    if request.uri().query().is_some() {
        return Err(WsStatusCode::BAD_REQUEST);
    }
    Ok(PairRelayConnection::Registration { relay_key, token })
}

fn inspect_pair_dial(
    request: &ServerRequest,
    state: &Arc<Mutex<SinglePairRelayState>>,
) -> Result<PairRelayConnection, WsStatusCode> {
    let Some(relay_key) = pair_relay_header(request, "sec-pair-key") else {
        return Err(WsStatusCode::BAD_REQUEST);
    };
    if request.uri().query().is_some() || request.headers().contains_key(AUTHORIZATION) {
        return Err(WsStatusCode::BAD_REQUEST);
    }
    if !lock_pair_relay(state).windows.contains_key(&relay_key) {
        return Err(WsStatusCode::UNAUTHORIZED);
    }
    Ok(PairRelayConnection::PairDial { relay_key })
}

fn inspect_pair_tunnel_attach(
    request: &ServerRequest,
    state: &Arc<Mutex<SinglePairRelayState>>,
) -> Result<PairRelayConnection, WsStatusCode> {
    let Some(token) = pair_relay_bearer(request) else {
        return Err(WsStatusCode::UNAUTHORIZED);
    };
    let Some(tunnel_id) = request.uri().path().strip_prefix("/tunnel/") else {
        return Err(WsStatusCode::NOT_FOUND);
    };
    let Some(relay_key) = pair_relay_header(request, "sec-pair-key") else {
        return Err(WsStatusCode::BAD_REQUEST);
    };
    if request.uri().query().is_some() || tunnel_id.is_empty() {
        return Err(WsStatusCode::BAD_REQUEST);
    }
    let mut state = lock_pair_relay(state);
    let Some(tunnel) = state.tunnels.get_mut(tunnel_id) else {
        return Err(WsStatusCode::NOT_FOUND);
    };
    if tunnel.token != token {
        return Err(WsStatusCode::FORBIDDEN);
    }
    if tunnel.relay_key != relay_key {
        return Err(WsStatusCode::FORBIDDEN);
    }
    let Some(attach) = tunnel.attach.take() else {
        return Err(WsStatusCode::UNAUTHORIZED);
    };
    Ok(PairRelayConnection::TunnelAttach { attach })
}

fn pair_relay_bearer(request: &ServerRequest) -> Option<String> {
    request
        .headers()
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

fn pair_relay_header(request: &ServerRequest, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn pair_relay_rejection(
    status: WsStatusCode,
) -> tokio_tungstenite::tungstenite::handshake::server::ErrorResponse {
    tokio_tungstenite::tungstenite::http::Response::builder()
        .status(status)
        .body(Some("pair relay rejected".to_owned()))
        .expect("pair relay rejection response")
}

async fn serve_single_pair_relay_registration(
    mut websocket: PairRelaySocket,
    mut offers: mpsc::UnboundedReceiver<String>,
) {
    loop {
        tokio::select! {
            Some(tunnel_id) = offers.recv() => {
                let control = format!(r#"{{"type":"incoming","tunnel_id":"{tunnel_id}"}}"#);
                if websocket.send(Message::Text(control.into())).await.is_err() {
                    return;
                }
            }
            message = websocket.next() => match message {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(Message::Ping(payload))) => {
                    if websocket.send(Message::Pong(payload)).await.is_err() {
                        return;
                    }
                }
                Some(Ok(Message::Binary(_) | Message::Text(_) | Message::Pong(_) | Message::Frame(_))) => {}
            },
        }
    }
}

async fn serve_single_pair_dial(
    websocket: PairRelaySocket,
    state: Arc<Mutex<SinglePairRelayState>>,
    relay_key: &str,
) {
    let receiver = {
        let mut state = lock_pair_relay(&state);
        let Some(window) = state.windows.get(relay_key) else {
            return;
        };
        let token = window.token.clone();
        let offers = window.offers.clone();
        let tunnel_id = format!("door-e2e-pair-window-{}", state.next_tunnel);
        state.next_tunnel += 1;
        let (attach, receiver) = oneshot::channel();
        state.tunnels.insert(
            tunnel_id.clone(),
            PairRelayTunnel {
                token,
                relay_key: relay_key.to_owned(),
                attach: Some(attach),
            },
        );
        if offers.send(tunnel_id.clone()).is_err() {
            state.tunnels.remove(&tunnel_id);
            return;
        }
        (tunnel_id, receiver)
    };
    let (tunnel_id, receiver) = receiver;
    let Ok(attach) = receiver.await else {
        return;
    };
    bridge_pair_relay_sockets(websocket, attach).await;
    lock_pair_relay(&state).tunnels.remove(&tunnel_id);
}

async fn bridge_pair_relay_sockets(mobile: PairRelaySocket, home: PairRelaySocket) {
    let (mobile_side, home_side) = tokio::io::duplex(2 * 1024 * 1024);
    tokio::select! {
        _ = pump_pair_relay_socket(mobile, mobile_side) => {}
        _ = pump_pair_relay_socket(home, home_side) => {}
    }
}

async fn pump_pair_relay_socket(
    websocket: PairRelaySocket,
    stream: tokio::io::DuplexStream,
) -> io::Result<()> {
    let (mut websocket_sink, mut websocket_stream) = websocket.split();
    let (mut stream_reader, mut stream_writer) = tokio::io::split(stream);
    let to_stream = async move {
        while let Some(message) = websocket_stream.next().await {
            match message.map_err(|_| io::Error::other("pair relay websocket receive failed"))? {
                Message::Binary(bytes) => {
                    stream_writer.write_all(&bytes).await?;
                    stream_writer.flush().await?;
                }
                Message::Close(_) => {
                    let _ = stream_writer.shutdown().await;
                    return Ok(());
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Text(_) | Message::Frame(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "pair relay received a non-binary tunnel message",
                    ));
                }
            }
        }
        Ok(())
    };
    let to_websocket = async move {
        let mut buffer = [0_u8; RELAY_TUNNEL_WRITE_MAX];
        loop {
            let count = stream_reader.read(&mut buffer).await?;
            if count == 0 {
                let _ = websocket_sink.close().await;
                return Ok(());
            }
            websocket_sink
                .send(Message::Binary(buffer[..count].to_vec().into()))
                .await
                .map_err(|_| io::Error::other("pair relay websocket send failed"))?;
        }
    };
    tokio::select! {
        result = to_stream => result,
        result = to_websocket => result,
    }
}

fn lock_pair_relay<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// WebSocket binary frames exposed as the raw stream the pairing TLS ceremony
/// already uses. This is test-only outer-transport plumbing; the ceremony
/// itself continues through `post_pair_over_certless_stream` above.
struct RelayByteStream<S> {
    socket: WebSocketStream<S>,
    read_tail: Vec<u8>,
    read_pos: usize,
}

impl<S> RelayByteStream<S> {
    fn new(socket: WebSocketStream<S>) -> Self {
        Self {
            socket,
            read_tail: Vec::new(),
            read_pos: 0,
        }
    }

    fn copy_tail(&mut self, buffer: &mut ReadBuf<'_>) -> bool {
        if self.read_pos >= self.read_tail.len() {
            self.read_tail.clear();
            self.read_pos = 0;
            return false;
        }
        let count = buffer
            .remaining()
            .min(self.read_tail.len().saturating_sub(self.read_pos));
        if count == 0 {
            return true;
        }
        buffer.put_slice(&self.read_tail[self.read_pos..self.read_pos + count]);
        self.read_pos += count;
        if self.read_pos == self.read_tail.len() {
            self.read_tail.clear();
            self.read_pos = 0;
        }
        true
    }
}

impl<S> AsyncRead for RelayByteStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buffer.remaining() == 0 || self.copy_tail(buffer) {
            return Poll::Ready(Ok(()));
        }
        loop {
            match Stream::poll_next(Pin::new(&mut self.socket), context) {
                Poll::Ready(Some(Ok(Message::Binary(bytes)))) => {
                    if bytes.is_empty() {
                        continue;
                    }
                    self.read_tail = bytes.to_vec();
                    self.read_pos = 0;
                    let _ = self.copy_tail(buffer);
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                Poll::Ready(Some(Ok(Message::Close(_))) | None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Ok(Message::Text(_) | Message::Frame(_)))) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unexpected pair relay websocket message",
                    )));
                }
                Poll::Ready(Some(Err(_))) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "pair relay websocket read failed",
                    )));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S> AsyncWrite for RelayByteStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut socket = Pin::new(&mut self.socket);
        match Sink::poll_ready(socket.as_mut(), context) {
            Poll::Ready(Ok(())) => {
                let count = buffer.len().min(RELAY_TUNNEL_WRITE_MAX);
                Sink::start_send(socket, Message::Binary(buffer[..count].to_vec().into()))
                    .map_or_else(
                        |_| {
                            Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::BrokenPipe,
                                "pair relay websocket write failed",
                            )))
                        },
                        |_| Poll::Ready(Ok(count)),
                    )
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pair relay websocket not ready",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Sink::poll_flush(Pin::new(&mut self.socket), context).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pair relay websocket flush failed",
            )
        })
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Sink::poll_close(Pin::new(&mut self.socket), context).map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "pair relay websocket close failed",
            )
        })
    }
}

async fn fake_mobile_pair_dial(
    relay_origin: &str,
    relay_key: &str,
) -> RelayByteStream<MaybeTlsStream<TcpStream>> {
    let url = spl_core::relay::pair_dial_url(relay_origin).expect("pair dial URL");
    let mut request = url.into_client_request().expect("pair dial request");
    request.headers_mut().insert(
        "sec-pair-key",
        HeaderValue::from_str(relay_key).expect("relay key header"),
    );
    let (socket, _) = connect_async(request).await.expect("mobile pair dial");
    RelayByteStream::new(socket)
}

fn relay_key_hex(secret: &[u8; 8]) -> String {
    let bytes = spl_core::relay_window::derive_rk(secret);
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut value, "{byte:02x}").expect("hex string writes");
    }
    value
}

fn relay_key_from_pair_link(pair_link: &str, relay_origin: &str) -> String {
    match spl_core::pairlink::parse(pair_link).expect("relay pair link parses") {
        spl_core::pairlink::ParsedPairLink::Relay(link) => {
            assert_eq!(link.relay_origin, relay_origin);
            relay_key_hex(&link.s)
        }
        spl_core::pairlink::ParsedPairLink::Direct(_) => {
            panic!("SPL off-machine mint must emit a v06 relay link")
        }
    }
}

async fn two_pair_requests_on_one_carrier(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    nonce: &str,
) -> Vec<spl_core::http::HttpResponse> {
    let mut decoder = FrameDecoder::new();
    for stream_id in [1, 3] {
        let body = serde_json::to_vec(&serde_json::json!({
            "csr": csr_pem("concurrent"),
            "device_label": "concurrent",
        }))
        .expect("pair body");
        let request = spl_core::http::build_request(
            "POST",
            &format!("{}?token={nonce}", spl_core::PAIR_PATH),
            &[("content-type".to_owned(), "application/json".to_owned())],
            &body,
        );
        carrier
            .write_all(
                &Frame::new(stream_id, FLAG_OPEN | FLAG_DATA, request.to_vec())
                    .encode()
                    .expect("open frame"),
            )
            .await
            .expect("request writes");
        carrier
            .write_all(
                &Frame::new(stream_id, FLAG_CLOSE, Vec::new())
                    .encode()
                    .expect("close frame"),
            )
            .await
            .expect("request close writes");
    }
    carrier.flush().await.expect("concurrent request flush");
    let mut assemblers = std::collections::BTreeMap::from([
        (1_u32, ResponseAssembler::new(1)),
        (3_u32, ResponseAssembler::new(3)),
    ]);
    let mut responses = Vec::new();
    let mut bytes = [0_u8; 4096];
    while responses.len() < 2 {
        let read = carrier.read(&mut bytes).await.expect("response reads");
        assert!(
            read > 0,
            "carrier stays open until concurrent responses finish"
        );
        decoder.feed(&bytes[..read]);
        for frame in decoder.drain().expect("response frames") {
            let Some(assembler) = assemblers.get_mut(&frame.stream_id) else {
                continue;
            };
            let output = assembler
                .feed(&frame.encode().expect("response frame"))
                .expect("response assembly");
            for control in output.pongs.into_iter().chain(output.emit_frames) {
                carrier
                    .write_all(&control)
                    .await
                    .expect("response controls");
            }
            carrier.flush().await.expect("response control flush");
            if assembler.is_closed() {
                let assembler = assemblers
                    .remove(&frame.stream_id)
                    .expect("completed assembler");
                responses.push(assembler.into_response().expect("response"));
            }
        }
    }
    responses
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

#[derive(Debug, PartialEq, Eq)]
enum DoorConnectionOutcome {
    TcpRefused,
    AcceptedThenAlert,
    AcceptedThenTimeout,
}

async fn door_connection_outcome(fixture: &Fixture, port: u16) -> DoorConnectionOutcome {
    match tokio::time::timeout(Duration::from_secs(3), async {
        let stream = match tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
            Ok(stream) => stream,
            Err(_) => return DoorConnectionOutcome::TcpRefused,
        };
        let mut stream = match TlsConnector::from(Arc::new(fixture.client_config(0)))
            .connect(
                rustls::pki_types::ServerName::try_from("spl.local").expect("server name"),
                stream,
            )
            .await
        {
            Ok(stream) => stream,
            Err(_) => return DoorConnectionOutcome::AcceptedThenAlert,
        };
        let mut byte = [0_u8; 1];
        match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte)).await {
            Ok(Err(_)) | Ok(Ok(0)) => DoorConnectionOutcome::AcceptedThenAlert,
            Ok(Ok(_)) | Err(_) => DoorConnectionOutcome::AcceptedThenTimeout,
        }
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => DoorConnectionOutcome::AcceptedThenTimeout,
    }
}

async fn fresh_door_connection_is_refused(fixture: &Fixture, port: u16) -> bool {
    match tokio::time::timeout(Duration::from_secs(3), async {
        match tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
            Err(_) => true,
            Ok(stream) => match TlsConnector::from(Arc::new(fixture.client_config(0)))
                .connect(
                    rustls::pki_types::ServerName::try_from("spl.local").expect("server name"),
                    stream,
                )
                .await
            {
                Err(_) => true,
                Ok(mut stream) => {
                    let mut byte = [0_u8; 1];
                    matches!(
                        tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte)).await,
                        Ok(Err(_)) | Ok(Ok(0))
                    )
                }
            },
        }
    })
    .await
    {
        Ok(refused) => refused,
        Err(_) => panic!("fresh door connection timed out after shutdown"),
    }
}

#[test]
fn ac3_serve_accepts_only_the_prebuilt_router() {
    // `__door_test/basis` and the test-crate body-echo route are the only test
    // surfaces. The body-echo route is AC3's permitted exception, and both
    // live in this integration-test crate, stronger than a feature-gated route.
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
    assert_eq!(source.matches("pub async fn serve").count(), 1);
    assert!(serve.contains("ConveyServeHandle {"));
}

fn ledger_posture(fixture: &Fixture) -> AuthorizedClientsRead {
    AuthorizationLedger::new(&fixture.root).read_state()
}

#[tokio::test]
async fn ac1_carrier_loop_counter_advances() {
    let fixture = Fixture::established(1);
    let carrier_loop_iterations = Arc::new(AtomicU64::new(0));
    let handle = serve(ConveyServeOptions {
        journal_root: fixture.root.clone(),
        loopback_port: 0,
        door_port: 0,
        handshake_timeout: Duration::from_secs(2),
        stream_stall_timeout: Duration::from_secs(2),
        router: router(fixture.root.clone()),
        carrier_loop_iterations: carrier_loop_iterations.clone(),
        handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
    })
    .await
    .expect("serve");
    let mut carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;
    let mut decoder = FrameDecoder::new();
    let mut ids = FrameDialer::default();
    exchange_over_carrier(
        &mut carrier,
        &mut decoder,
        ids.allocate(),
        "GET",
        "/api/system/status",
        &[],
        &[],
    )
    .await
    .expect("carrier exchange");
    assert!(carrier_loop_iterations.load(Ordering::Relaxed) > 0);
    handle.shutdown();
}

#[tokio::test]
async fn ac1_publication_tick_counter_advances() {
    let fixture = Fixture::established(1);
    let before = authorization_publication_ticks();
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(authorization_publication_ticks() > before);
    handle.shutdown();
}

#[tokio::test]
async fn ac2_stop_authorization_refresh_stops_only_the_publisher() {
    let fixture = Fixture::established(1);
    assert!(fixture.remove_authorization(0).authorized_removed);
    let (authorization_sender, mut authorization_receiver) = watch::channel(
        DeviceDoorAuthorization::from(AuthorizedClientsRead::Missing),
    );
    let mut handle = solstone_core_convey_shell::bind_with_authorization(
        ConveyServeOptions {
            journal_root: fixture.root.clone(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: Duration::from_secs(2),
            stream_stall_timeout: Duration::from_secs(2),
            router: router(fixture.root.clone()),
            carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
        },
        solstone_core_convey_shell::authorization_gate::DoorRouter::unconfined(router(
            fixture.root.clone(),
        )),
        authorization_sender,
    )
    .await
    .expect("serve");
    let port = door_port(handle.door_outcome());

    handle.stop_authorization_refresh().await;
    authorization_receiver.borrow_and_update();
    assert!(
        tokio::time::timeout(Duration::from_millis(600), authorization_receiver.changed())
            .await
            .is_err(),
        "the stopped door publisher must not update its own authorization watch"
    );
    let outcome = door_connection_outcome(&fixture, port).await;
    assert_eq!(
        outcome,
        DoorConnectionOutcome::AcceptedThenAlert,
        "door connection after refresh stop had unexpected outcome: {outcome:?}"
    );
    assert!(matches!(
        loopback_status(handle.loopback_ipv4_addr()).await,
        Ok(200)
    ));
    handle.shutdown();
}

#[tokio::test]
async fn ac3_stop_authorization_refresh_is_noop_when_door_is_withheld() {
    let root = std::env::temp_dir().join(format!(
        "solstone-door-stop-refresh-withheld-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("config")).expect("config");
    let mut handle = serve(ConveyServeOptions {
        journal_root: root.clone(),
        loopback_port: 0,
        door_port: 0,
        handshake_timeout: Duration::from_secs(1),
        stream_stall_timeout: Duration::from_secs(1),
        router: Router::new(),
        carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
        handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
    })
    .await
    .expect("loopback");
    assert!(matches!(handle.door_outcome(), DoorOutcome::Withheld(_)));
    handle.stop_authorization_refresh().await;
    handle.shutdown();
    std::fs::remove_dir_all(root).expect("temporary root removes");
}

#[tokio::test]
async fn finalize_starts_the_door_boot_withheld_for_an_unestablished_journal() {
    let root = std::env::temp_dir().join(format!(
        "solstone-door-finalize-opens-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("config")).expect("config");
    let handle = serve(ConveyServeOptions {
        journal_root: root.clone(),
        loopback_port: 0,
        door_port: 0,
        handshake_timeout: Duration::from_secs(2),
        stream_stall_timeout: Duration::from_secs(2),
        router: solstone_core_sol_link::http::init_router(root.clone()),
        carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
        handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
    })
    .await
    .expect("loopback");
    assert!(matches!(
        handle.door_outcome(),
        DoorOutcome::Withheld(DoorWithheldReason::Unestablished)
    ));
    assert!(handle.live_door_addr().is_none());

    let loopback = handle.loopback_ipv4_addr();
    assert_eq!(
        loopback_post(loopback, "/init/mark/regenerate", b"{}").await,
        200,
        "wizard mark regenerate"
    );
    assert_eq!(
        loopback_post(loopback, "/init/mark/lock", b"{}").await,
        200,
        "wizard mark lock"
    );
    assert!(
        handle.live_door_addr().is_none(),
        "locking the journal id does not start the door"
    );
    assert_eq!(
        loopback_post(loopback, "/init/finalize", b"{}").await,
        200,
        "wizard finalize"
    );
    let address = handle
        .live_door_addr()
        .expect("finalize starts the withheld door");
    let peer_root = std::env::temp_dir().join(format!(
        "solstone-door-finalize-peer-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&peer_root);
    fs::create_dir_all(peer_root.join("config")).expect("peer config");
    let peer = serve(ConveyServeOptions {
        journal_root: peer_root.clone(),
        loopback_port: 0,
        door_port: 0,
        handshake_timeout: Duration::from_secs(2),
        stream_stall_timeout: Duration::from_secs(2),
        router: Router::new(),
        carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
        handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
    })
    .await
    .expect("peer loopback");
    assert!(
        tokio::net::TcpStream::connect(address).await.is_ok(),
        "the second journal can reach the home door"
    );
    peer.shutdown();
    handle.shutdown();
    fs::remove_dir_all(peer_root).expect("peer root removes");
    fs::remove_dir_all(root).expect("temporary root removes");
}

#[tokio::test]
async fn ac4_shutdown_aborts_every_listener_task() {
    let fixture = Fixture::established(1);
    let (authorization_sender, mut authorization_receiver) = watch::channel(
        DeviceDoorAuthorization::from(AuthorizedClientsRead::Missing),
    );
    let handle = bind_with_authorization(
        options(&fixture, router(fixture.root.clone()), 0),
        solstone_core_convey_shell::authorization_gate::DoorRouter::unconfined(router(
            fixture.root.clone(),
        )),
        authorization_sender,
    )
    .await
    .expect("serve");
    let loopback = handle.loopback_ipv4_addr();
    let port = door_port(handle.door_outcome());
    assert!(matches!(loopback_status(loopback).await, Ok(200)));
    let mut carrier = live_carrier(&fixture, port).await;
    let mut decoder = FrameDecoder::new();
    let mut ids = FrameDialer::default();
    exchange_over_carrier(
        &mut carrier,
        &mut decoder,
        ids.allocate(),
        "GET",
        "/api/system/status",
        &[],
        &[],
    )
    .await
    .expect("carrier exchange");

    handle.shutdown();
    authorization_receiver.borrow_and_update();
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(1), loopback_status(loopback)).await,
        Ok(Ok(200))
    ));
    assert!(fresh_door_connection_is_refused(&fixture, port).await);
    assert!(
        tokio::time::timeout(Duration::from_millis(600), authorization_receiver.changed())
            .await
            .is_err(),
        "shutdown must stop this door's authorization publisher"
    );
}

#[test]
fn ac5_posture_induction_helpers_change_inodes_and_postures() {
    let fixture = Fixture::established(1);
    let path = fixture.root.join("link/authorized_clients.json");
    let mut previous = File::open(&path).expect("authorization file opens");

    fixture.warm_authorization_to_present();
    assert!(matches!(
        read_authorized_clients(&path),
        AuthorizedClientsRead::Present(_)
    ));
    let warmed_inode = std::fs::metadata(&path).expect("warmed metadata").ino();
    assert_ne!(
        warmed_inode,
        previous.metadata().expect("original metadata").ino()
    );
    previous = File::open(&path).expect("warmed authorization opens");

    fixture.induce_unreadable_authorization();
    assert_eq!(
        read_authorized_clients(&path),
        AuthorizedClientsRead::Unreadable
    );
    let unreadable_inode = std::fs::metadata(&path).expect("unreadable metadata").ino();
    assert_ne!(
        unreadable_inode,
        previous.metadata().expect("warmed metadata").ino()
    );
    previous = File::open(&path).expect("unreadable authorization directory opens");

    fixture.warm_authorization_to_present();
    assert!(matches!(
        read_authorized_clients(&path),
        AuthorizedClientsRead::Present(_)
    ));
    let rewarmed_inode = std::fs::metadata(&path).expect("rewarmed metadata").ino();
    assert_ne!(
        rewarmed_inode,
        previous.metadata().expect("unreadable metadata").ino()
    );
    previous = File::open(&path).expect("rewarmed authorization opens");

    fixture.induce_malformed_authorization();
    assert_eq!(
        read_authorized_clients(&path),
        AuthorizedClientsRead::Malformed
    );
    let malformed_inode = std::fs::metadata(&path).expect("malformed metadata").ino();
    assert_ne!(
        malformed_inode,
        previous.metadata().expect("rewarmed metadata").ino()
    );
    previous = File::open(&path).expect("malformed authorization opens");

    fixture.warm_authorization_to_present();
    assert!(matches!(
        read_authorized_clients(&path),
        AuthorizedClientsRead::Present(_)
    ));
    assert_ne!(
        std::fs::metadata(&path)
            .expect("final warmed metadata")
            .ino(),
        previous.metadata().expect("malformed metadata").ino()
    );
}

#[test]
fn ac6_remove_authorization_uses_the_ledger_writer() {
    let fixture = Fixture::established(1);
    let path = fixture.root.join("link/authorized_clients.json");
    let before_inode = std::fs::metadata(&path)
        .expect("authorization metadata")
        .ino();
    let did = format!("sha256:{}", spl_core::ca::sha256_hex(fixture.client_der(0)));

    let outcome = fixture.remove_authorization(0);

    assert!(outcome.authorized_removed);
    assert!(matches!(
        read_authorized_clients(&path),
        AuthorizedClientsRead::Present(entries) if !entries.iter().any(|entry| entry.fingerprint == did)
    ));
    assert_ne!(
        std::fs::metadata(&path)
            .expect("updated authorization metadata")
            .ino(),
        before_inode
    );
    assert!(
        fixture
            .root
            .join("link/authorized_clients.json.lock")
            .exists()
    );
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
    assert!(
        matches!(loopback_status(handle.loopback_ipv4_addr()).await, Ok(200)),
        "loopback IPv4 accepts a live HTTP request"
    );
    assert!(
        matches!(loopback_status(handle.loopback_ipv6_addr()).await, Ok(200)),
        "loopback IPv6 accepts a live HTTP request"
    );
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
    // An isolated checkout cannot originate an off-host connection; pure vectors cover Direct.
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
    let (_, leaf) = x509_parser::parse_x509_certificate(chain[0].as_ref()).expect("leaf parses");
    let (_, ca) = x509_parser::parse_x509_certificate(fixture.ca_der()).expect("CA parses");
    leaf.verify_signature(Some(ca.public_key()))
        .expect("served leaf verifies under the committed CA key");
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
async fn ac6_authorized_client_is_admitted() {
    let fixture = Fixture::established(1);
    let did = format!("sha256:{}", spl_core::ca::sha256_hex(fixture.client_der(0)));
    assert!(matches!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(entries) if entries.iter().any(|entry| entry.fingerprint == did)
    ));
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    assert!(
        request_result(&fixture, 0, door_port(handle.door_outcome()))
            .await
            .is_ok(),
        "Present authorization admits the listed client"
    );
    handle.shutdown();
}

#[tokio::test]
async fn two_journal_custom_ports_complete_own_pairings_and_refuse_cross_root_certificates() {
    let fixture_a = Fixture::established(1);
    let fixture_b = Fixture::established(1);
    let handle_a = serve(options(
        &fixture_a,
        pairing_router(&fixture_a, pairing_snapshot()),
        0,
    ))
    .await
    .expect("serve A");
    let handle_b = serve(options(
        &fixture_b,
        pairing_router(&fixture_b, pairing_snapshot()),
        0,
    ))
    .await
    .expect("serve B");
    let port_a = door_port(handle_a.door_outcome());
    let port_b = door_port(handle_b.door_outcome());
    assert_ne!(port_a, 0);
    assert_ne!(port_b, 0);
    assert_ne!(port_a, port_b);
    configure_direct_port(&fixture_a, port_a);
    configure_direct_port(&fixture_b, port_b);

    let mint_a = mint_from_loopback(&handle_a, "A phone").await;
    let mint_b = mint_from_loopback(&handle_b, "B phone").await;
    let link_a = mint_a["pair_link"].as_str().expect("A pair link");
    let link_b = mint_b["pair_link"].as_str().expect("B pair link");
    for (link, port) in [(link_a, port_a), (link_b, port_b)] {
        let spl_core::pairlink::ParsedPairLink::Direct(parsed) =
            spl_core::pairlink::parse(link).expect("pair link parses")
        else {
            panic!("local pairing must emit a direct link");
        };
        assert_eq!(parsed.candidates[0].port, port);
    }
    let credential_a =
        spl_transport::pairing::pair_from_link(link_a, "A phone", &serde_json::Map::new())
            .await
            .expect("A completes its own pairing ceremony");
    let credential_b =
        spl_transport::pairing::pair_from_link(link_b, "B phone", &serde_json::Map::new())
            .await
            .expect("B completes its own pairing ceremony");
    TransportClient::new(credential_a, None)
        .expect("A credential has strict mTLS configuration")
        .dial_carrier()
        .await
        .expect("A credential authenticates to A");
    TransportClient::new(credential_b, None)
        .expect("B credential has strict mTLS configuration")
        .dial_carrier()
        .await
        .expect("B credential authenticates to B");

    let error_a_on_b = tls_handshake(
        test_client_config(
            &[&rustls::version::TLS13],
            Some(fixture_a.client_identity(0)),
        ),
        port_b,
    )
    .await
    .expect_err("A's cert must fail at B's TLS authorization layer");
    assert!(
        matches!(
            error_a_on_b.kind(),
            std::io::ErrorKind::InvalidData
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset
        ),
        "A-against-B must fail at TLS, not connect-refused: {error_a_on_b:?}"
    );
    let error_b_on_a = tls_handshake(
        test_client_config(
            &[&rustls::version::TLS13],
            Some(fixture_b.client_identity(0)),
        ),
        port_a,
    )
    .await
    .expect_err("B's cert must fail at A's TLS authorization layer");
    assert!(
        matches!(
            error_b_on_a.kind(),
            std::io::ErrorKind::InvalidData
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset
        ),
        "B-against-A must fail at TLS, not connect-refused: {error_b_on_a:?}"
    );

    handle_a.shutdown();
    handle_b.shutdown();
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
    assert!(matches!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(_)
    ));
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
async fn pairing_window_admits_a_certless_carrier_only_for_the_pair_route() {
    let fixture = Fixture::established(0);
    let (authorization_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let door_base = pairing_router(&fixture, pairing_snapshot());
    let handle = bind_with_authorization(
        options(&fixture, pairing_router(&fixture, pairing_snapshot()), 0),
        solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
            door_base,
            fixture.root.clone(),
            authorization,
        ),
        authorization_sender,
    )
    .await
    .expect("serve");
    // The door was already serving with no nonce file. This proves optional
    // client auth is decided per carrier, not frozen at bind time.
    open_pairing_window(&fixture, "certless");
    let port = door_port(handle.door_outcome());
    let mut carrier = live_certless_carrier(port)
        .await
        .expect("certless TLS admission");
    let mut decoder = FrameDecoder::new();
    let mut ids = FrameDialer::default();
    for (method, path, body) in [
        ("POST", "/app/network/pair-start", br#"{}"#.as_slice()),
        (
            "GET",
            "/app/network/api/pair/nonce-status?nonce=certless",
            b"".as_slice(),
        ),
    ] {
        let response = exchange_over_carrier(
            &mut carrier,
            &mut decoder,
            ids.allocate(),
            method,
            path,
            &[],
            body,
        )
        .await
        .expect("carrier response");
        assert_eq!(response.status, 403, "pairing peer cannot use owner route");
    }
    let response = exchange_over_carrier(
        &mut carrier,
        &mut decoder,
        ids.allocate(),
        "POST",
        "/app/network/pair?token=certless",
        &[("content-type".into(), "application/json".into())],
        br#"{"csr":"not a CSR","device_label":"phone"}"#,
    )
    .await
    .expect("pair route reaches handler");
    assert_eq!(
        response.status,
        400,
        "pair route reached the ceremony rather than confinement: {}",
        String::from_utf8_lossy(&response.body)
    );
    assert!(
        tls_handshake(foreign_client_config(), port).await.is_err(),
        "an open pairing window still refuses a foreign-CA client certificate"
    );
    handle.shutdown();

    let no_file = Fixture::established(0);
    let no_file_store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&no_file.root);
    assert!(!no_file_store.path().exists(), "no nonce file fixture");
    assert!(
        no_file_store.snapshot().is_empty(),
        "no nonce file reads empty"
    );
    assert!(
        !solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(
            &no_file_store,
            pairing_now()
        )
    );
    assert_certless_tls_refused(&no_file).await;

    let used = Fixture::established(0);
    let used_store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&used.root);
    let used_now = pairing_now();
    used_store
        .add("used".into(), "phone".into(), "".into(), false, used_now)
        .expect("used nonce writes");
    assert!(
        used_store
            .consume("used", used_now)
            .expect("used nonce consumes")
            .expect("entry")
            .used
    );
    assert!(
        used_store.snapshot().iter().all(|entry| entry.used),
        "all entries are used"
    );
    assert!(
        !solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(&used_store, used_now)
    );
    assert_certless_tls_refused(&used).await;

    let expired = Fixture::established(0);
    let expired_store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&expired.root);
    let expired_now = pairing_now();
    expired_store
        .add(
            "expired".into(),
            "phone".into(),
            "".into(),
            false,
            expired_now - 301,
        )
        .expect("expired nonce writes");
    assert!(
        expired_store
            .snapshot()
            .iter()
            .all(|entry| entry.expires_at <= expired_now),
        "all entries are expired"
    );
    assert!(
        !solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(
            &expired_store,
            expired_now
        )
    );
    assert_certless_tls_refused(&expired).await;

    let consumed_last = Fixture::established(0);
    let consumed_store =
        solstone_core_sol_link::pairing::nonces::NonceStore::new(&consumed_last.root);
    let consumed_now = pairing_now();
    consumed_store
        .add(
            "last".into(),
            "phone".into(),
            "".into(),
            false,
            consumed_now,
        )
        .expect("last nonce writes");
    consumed_store
        .consume("last", consumed_now)
        .expect("last nonce consumes")
        .expect("last entry");
    assert!(
        consumed_store
            .peek("last")
            .expect("last nonce remains observable")
            .used,
        "last nonce was just consumed"
    );
    assert!(
        !solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(
            &consumed_store,
            consumed_now
        )
    );
    assert_certless_tls_refused(&consumed_last).await;
}

#[tokio::test]
async fn pairing_peer_unpair_on_either_prefix_is_confined_and_does_not_mutate_the_ledger() {
    let fixture = Fixture::established(1);
    let ledger_path = fixture.root.join("link/authorized_clients.json");
    let before = fs::read(&ledger_path).expect("ledger before");
    let fingerprint = serde_json::from_slice::<serde_json::Value>(&before).expect("ledger JSON")[0]
        ["fingerprint"]
        .as_str()
        .expect("fingerprint")
        .to_owned();
    let (authorization_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let handle = bind_with_authorization(
        options(&fixture, pairing_router(&fixture, pairing_snapshot()), 0),
        solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
            pairing_router(&fixture, pairing_snapshot()),
            fixture.root.clone(),
            authorization,
        ),
        authorization_sender,
    )
    .await
    .expect("serve");
    open_pairing_window(&fixture, "unpair-confined");
    let mut carrier = live_certless_carrier(door_port(handle.door_outcome()))
        .await
        .expect("certless TLS admission");
    let mut decoder = FrameDecoder::new();
    let mut ids = FrameDialer::default();
    let tunnel_body = b"pairing tunnel may only use /app/network/pair".to_vec();
    let body = format!(r#"{{"fingerprint":"{fingerprint}"}}"#);
    for path in ["/app/network/unpair", "/app/link/unpair"] {
        let response = exchange_over_carrier(
            &mut carrier,
            &mut decoder,
            ids.allocate(),
            "POST",
            path,
            &[("content-type".into(), "application/json".into())],
            body.as_bytes(),
        )
        .await
        .expect("carrier response");
        assert_eq!(response.status, 403, "{path}");
        assert_eq!(response.body, tunnel_body, "{path}");
    }
    assert_eq!(fs::read(&ledger_path).expect("ledger after"), before);
    handle.shutdown();
}

#[tokio::test]
async fn corrupt_nonce_store_closes_certless_admission_but_a_real_open_window_admits() {
    let fixture = Fixture::established(0);
    std::fs::create_dir_all(fixture.root.join("link")).expect("link directory");
    std::fs::write(fixture.root.join("link/nonces.json"), b"not JSON")
        .expect("corrupt nonce store");
    let closed = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve closed window");
    assert!(
        tls_handshake(
            test_client_config(&[&rustls::version::TLS13], None),
            door_port(closed.door_outcome()),
        )
        .await
        .is_err(),
        "corrupt store is closed"
    );
    closed.shutdown();

    open_pairing_window(&fixture, "opened-after-corruption");
    let open = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("serve open window");
    let mut carrier = live_certless_carrier(door_port(open.door_outcome()))
        .await
        .expect("open window admits certless TLS");
    let mut decoder = FrameDecoder::new();
    let response = exchange_over_carrier(
        &mut carrier,
        &mut decoder,
        1,
        "POST",
        "/app/network/pair?token=opened-after-corruption",
        &[("content-type".into(), "application/json".into())],
        br#"{"csr":"not a CSR","device_label":"phone"}"#,
    )
    .await
    .expect("admitted carrier reaches pair route");
    assert_eq!(response.status, 400);
    open.shutdown();
}

#[tokio::test]
async fn pairing_confinement_applies_to_real_certless_carriers_and_unmatched_paths() {
    let fixture = Fixture::established(0);
    let (authorization_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    // This probe is intentionally integration-test-only. It is a named route
    // outside the pairing allow-list; the second probe below matches no route.
    let door_base = router(fixture.root.clone()).merge(Router::new().route(
        "/__door_test/pairing-probe",
        post(|| async { StatusCode::OK }),
    ));
    let door_router = solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
        door_base,
        fixture.root.clone(),
        authorization,
    );
    let handle = bind_with_authorization(
        options(&fixture, router(fixture.root.clone()), 0),
        door_router,
        authorization_sender,
    )
    .await
    .expect("serve");
    let store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&fixture.root);
    let now = pairing_now();
    store
        .add("first".into(), "phone".into(), "".into(), false, now)
        .expect("open window");
    assert!(
        solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(&store, now),
        "open fixture"
    );
    let mut carrier = live_certless_carrier(door_port(handle.door_outcome()))
        .await
        .expect("open window admits certless carrier");
    let mut decoder = FrameDecoder::new();
    let mut ids = FrameDialer::default();

    store
        .consume("first", now)
        .expect("consume")
        .expect("first nonce");
    assert!(
        store.peek("first").expect("used nonce remains").used,
        "closed fixture is consumed"
    );
    assert!(
        !solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(&store, now),
        "window closes before route"
    );
    let closed = exchange_over_carrier(
        &mut carrier,
        &mut decoder,
        ids.allocate(),
        "POST",
        "/app/network/pair?token=first",
        &[],
        &[],
    )
    .await
    .expect("closed response");
    assert_eq!(closed.status, 403);
    assert_eq!(closed.body, b"pairing window closed");

    store
        .add("second".into(), "phone".into(), "".into(), false, now)
        .expect("reopen window");
    assert!(
        solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(&store, now),
        "reopened fixture"
    );
    let tunnel_body = b"pairing tunnel may only use /app/network/pair".to_vec();
    for path in [
        "/app/network/%70air?token=second",
        "/__door_test/pairing-probe",
        "/__door_test/no-such-route",
    ] {
        let response = exchange_over_carrier(
            &mut carrier,
            &mut decoder,
            ids.allocate(),
            "POST",
            path,
            &[],
            &[],
        )
        .await
        .expect("confinement response");
        assert_eq!(response.status, 403, "{path}");
        assert_eq!(response.body, tunnel_body, "{path}");
    }
    assert_ne!(
        closed.body, tunnel_body,
        "window body remains distinct from tunnel body"
    );
    handle.shutdown();
}

#[tokio::test]
async fn stopped_reaper_cannot_replace_request_level_closed_window_refusal() {
    let fixture = Fixture::established(0);
    let (authorization_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let door_base = pairing_router(&fixture, pairing_snapshot());
    let mut handle = bind_with_authorization(
        options(&fixture, pairing_router(&fixture, pairing_snapshot()), 0),
        solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
            door_base,
            fixture.root.clone(),
            authorization,
        ),
        authorization_sender,
    )
    .await
    .expect("serve");
    open_pairing_window(&fixture, "reaper-stopped");
    let port = door_port(handle.door_outcome());
    let mut carrier = live_certless_carrier(port)
        .await
        .expect("open window admits carrier");
    handle.stop_pairing_reaper().await;
    let store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&fixture.root);
    store
        .consume("reaper-stopped", pairing_now())
        .expect("consume")
        .expect("entry");
    assert!(store.peek("reaper-stopped").expect("used entry").used);
    let mut decoder = FrameDecoder::new();
    let response = exchange_over_carrier(
        &mut carrier,
        &mut decoder,
        1,
        "POST",
        "/app/network/pair?token=reaper-stopped",
        &[("content-type".into(), "application/json".into())],
        br#"{"csr":"not a CSR","device_label":"phone"}"#,
    )
    .await
    .expect("confinement response with reaper stopped");
    assert_eq!(response.status, 403);
    assert_eq!(response.body, b"pairing window closed");
    handle.shutdown();
}

#[tokio::test]
async fn fifth_certless_pairing_carrier_closes_after_tls_without_a_response() {
    let fixture = Fixture::established(0);
    let (authorization_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let door_base = pairing_router(&fixture, pairing_snapshot());
    let handle = bind_with_authorization(
        options(&fixture, pairing_router(&fixture, pairing_snapshot()), 0),
        solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
            door_base,
            fixture.root.clone(),
            authorization,
        ),
        authorization_sender,
    )
    .await
    .expect("serve");
    open_pairing_window(&fixture, "cap");
    let port = door_port(handle.door_outcome());
    let mut admitted = Vec::new();
    for _ in 0..4 {
        admitted.push(
            live_certless_carrier(port)
                .await
                .expect("first four TLS handshakes complete"),
        );
    }
    let mut fifth = live_certless_carrier(port)
        .await
        .expect("fifth TLS handshake completes before cap refusal");
    let mut byte = [0_u8; 1];
    let closed_without_response = match fifth.read(&mut byte).await {
        Ok(0) => true,
        Err(error) => error.kind() == std::io::ErrorKind::UnexpectedEof,
        Ok(_) => false,
    };
    assert!(
        closed_without_response,
        "cap writes no HTTP/mux response before closing the carrier"
    );
    assert_eq!(
        handle.pairing_cap_refusals(),
        1,
        "cap refusal is logged/observable"
    );
    drop(admitted);
    handle.shutdown();
}

#[tokio::test]
async fn concurrent_pair_requests_on_one_carrier_leave_one_ledger_entry_and_one_burned_nonce() {
    let fixture = Fixture::established(0);
    let (authorization_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let door_base = pairing_router(&fixture, pairing_snapshot());
    let handle = bind_with_authorization(
        options(&fixture, pairing_router(&fixture, pairing_snapshot()), 0),
        solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
            door_base,
            fixture.root.clone(),
            authorization,
        ),
        authorization_sender,
    )
    .await
    .expect("serve");
    let mint = mint_from_loopback(&handle, "concurrent").await;
    let nonce = mint["nonce"].as_str().expect("mint nonce");
    let mut carrier = live_certless_carrier(door_port(handle.door_outcome()))
        .await
        .expect("certless carrier");
    let responses = two_pair_requests_on_one_carrier(&mut carrier, nonce).await;
    let statuses = responses
        .iter()
        .map(|response| response.status)
        .collect::<Vec<_>>();
    assert!(statuses.contains(&200), "statuses: {statuses:?}");
    assert!(statuses.contains(&410), "statuses: {statuses:?}");
    let store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&fixture.root);
    assert!(
        store.peek(nonce).is_none(),
        "the losing consume collects the burned nonce"
    );
    let mut ledger = AuthorizationLedger::new(&fixture.root);
    assert_eq!(
        ledger.snapshot().len(),
        1,
        "one ceremony reached the ledger"
    );
    handle.shutdown();
}

#[tokio::test]
async fn pair_start_pins_the_committed_ca_across_a_real_door_restart() {
    let fixture = Fixture::established(1);
    let first = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("first serve");
    let first_mint = mint_from_loopback(&first, "first device").await;
    let first_link =
        spl_core::pairlink::parse(first_mint["pair_link"].as_str().expect("pair link"))
            .expect("first pair link");
    let spl_core::pairlink::ParsedPairLink::Direct(first_link) = first_link else {
        panic!("same-machine mint emits a direct pair link");
    };
    first.shutdown();

    let second = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("second serve");
    let second_mint = mint_from_loopback(&second, "second device").await;
    let second_link =
        spl_core::pairlink::parse(second_mint["pair_link"].as_str().expect("pair link"))
            .expect("second pair link");
    let spl_core::pairlink::ParsedPairLink::Direct(second_link) = second_link else {
        panic!("same-machine mint emits a direct pair link");
    };
    assert_eq!(first_link.ca_fp_prefix, second_link.ca_fp_prefix);
    let committed_digest = spl_core::ca::sha256_hex(fixture.ca_der());
    assert_eq!(first_mint["ca_fingerprint"], committed_digest);
    assert_eq!(
        first_link.ca_fp_prefix,
        spl_core::ca::sha256(fixture.ca_der())[..16]
    );
    let identity = solstone_core_sol_link::committed::load_committed_identity(&fixture.root)
        .expect("committed identity");
    assert_ne!(
        first_mint["ca_fingerprint"],
        spl_core::ca::sha256_hex(identity.ca().spki_der()),
        "the advertised pin is certificate DER, never CA SPKI DER"
    );
    second.shutdown();
}

#[tokio::test]
async fn pair_link_prefix_matches_the_live_door_chain_but_not_its_leaf() {
    let fixture = Fixture::established(1);
    let handle = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("serve");
    let mint = mint_from_loopback(&handle, "phone").await;
    let parsed = spl_core::pairlink::parse(mint["pair_link"].as_str().expect("pair link"))
        .expect("pair link parses");
    let spl_core::pairlink::ParsedPairLink::Direct(parsed) = parsed else {
        panic!("mint emits direct pair link");
    };
    let chain = presented_chain(&fixture, door_port(handle.door_outcome())).await;
    let chain = chain
        .iter()
        .map(|certificate| certificate.as_ref().to_vec())
        .collect::<Vec<_>>();
    assert!(spl_core::ca::chain_matches_prefix(
        &chain,
        &parsed.ca_fp_prefix
    ));
    assert!(
        !spl_core::ca::cert_matches_prefix(&chain[0], &parsed.ca_fp_prefix),
        "the pin matches the presented CA chain member, not the fresh leaf"
    );
    handle.shutdown();
}

#[tokio::test]
async fn pair_response_is_canonical_and_omits_empty_local_endpoints_on_raw_json() {
    let fixture = Fixture::established(0);
    let handle = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("serve populated snapshot");
    let mint = mint_from_loopback(&handle, "response device").await;
    let nonce = mint["nonce"].as_str().expect("nonce");
    let response = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(nonce),
        serde_json::json!({
            "csr": csr_pem("response device"),
            "device_label": "response device",
            "ignored_by_canonical_request": true,
        }),
    )
    .await;
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let raw: serde_json::Value = serde_json::from_slice(&response.body).expect("raw response JSON");
    assert!(
        raw["local_endpoints"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    let pair: spl_core::PairResponse =
        serde_json::from_slice(&response.body).expect("canonical PairResponse");
    let (_, certificate) = parse_x509_pem(pair.client_cert.as_bytes()).expect("client PEM");
    assert_eq!(
        pair.fingerprint,
        format!("sha256:{}", spl_core::ca::sha256_hex(&certificate.contents))
    );
    let identity = solstone_core_sol_link::committed::load_committed_identity(&fixture.root)
        .expect("committed identity");
    assert_eq!(
        pair.instance_id,
        solstone_core_sol_link::ca::jid_from_spki(identity.ca().spki_der()).expect("CA JID")
    );
    assert_eq!(pair.home_label, "test home");
    assert_eq!(pair.ca_chain.len(), 1);
    let (_, returned_ca) = parse_x509_pem(pair.ca_chain[0].as_bytes()).expect("returned CA PEM");
    assert_eq!(returned_ca.contents, fixture.ca_der());
    let attestation = pair.home_attestation.as_deref().expect("home attestation");
    assert!(
        solstone_core_sol_link::pairing::attestation::home_attestation_verifies(
            fixture.ca_der(),
            attestation
        )
    );
    let (header, claims, signature_length) =
        solstone_core_sol_link::pairing::attestation::inspect_home_attestation(attestation)
            .expect("attestation parses");
    assert_eq!(
        header,
        serde_json::json!({"alg":"ES256","typ":"home-attest"})
    );
    assert_eq!(claims["iss"], format!("home:{}", pair.instance_id));
    assert_eq!(claims["aud"], "spl-relay");
    assert_eq!(claims["scope"], "device.enroll");
    assert_eq!(claims["instance_id"], pair.instance_id);
    assert_eq!(claims["device_fp"], pair.fingerprint);
    assert_eq!(
        claims["exp"].as_i64().expect("exp") - claims["iat"].as_i64().expect("iat"),
        240
    );
    assert!(claims["jti"].as_str().is_some_and(|jti| !jti.contains('=')));
    assert_eq!(signature_length, 64);
    handle.shutdown();

    let empty = Fixture::established(0);
    let empty_handle = serve(options(
        &empty,
        pairing_router(&empty, PairingSnapshot::default()),
        0,
    ))
    .await
    .expect("serve empty snapshot");
    let empty_mint = mint_from_loopback(&empty_handle, "empty endpoints").await;
    let empty_response = post_pair_over_certless_carrier(
        door_port(empty_handle.door_outcome()),
        Some(empty_mint["nonce"].as_str().expect("nonce")),
        serde_json::json!({"csr": csr_pem("empty endpoints"), "device_label": "empty endpoints"}),
    )
    .await;
    assert_eq!(empty_response.status, 200);
    let empty_raw: serde_json::Value =
        serde_json::from_slice(&empty_response.body).expect("empty raw response JSON");
    assert!(
        empty_raw.get("local_endpoints").is_none(),
        "the canonical type would serialize None as null, so raw JSON must omit the key"
    );
    empty_handle.shutdown();
}

#[tokio::test]
async fn relay_pair_window_round_trip_delivers_the_complete_pair_response() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::established(0);
        let relay = SinglePairRelay::bind().await;
        configure_relay_pairing(&fixture, relay.origin());
        let (authorization_sender, authorization) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let door_base = pairing_router(&fixture, pairing_snapshot());
        let handle = bind_with_authorization(
            options(&fixture, pairing_router(&fixture, pairing_snapshot()), 0),
            solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
                door_base,
                fixture.root.clone(),
                authorization,
            ),
            authorization_sender,
        )
        .await
        .expect("serve relay pairing Door");
        let port = door_port(handle.door_outcome());
        assert_ne!(port, 0, "relay pairing Door must bind an OS-selected port");

        let other_fixture = Fixture::established(0);
        let (other_authorization_sender, other_authorization) = watch::channel(
            DeviceDoorAuthorization::from(AuthorizedClientsRead::Missing),
        );
        let other_door_base = pairing_router(&other_fixture, pairing_snapshot());
        let other_handle = bind_with_authorization(
            options(
                &other_fixture,
                pairing_router(&other_fixture, pairing_snapshot()),
                0,
            ),
            solstone_core_convey_shell::authorization_gate::authorized_router_with_router(
                other_door_base,
                other_fixture.root.clone(),
                other_authorization,
            ),
            other_authorization_sender,
        )
        .await
        .expect("serve concurrent relay pairing Door");
        let other_port = door_port(other_handle.door_outcome());
        assert_ne!(
            other_port, 0,
            "concurrent relay pairing Door must bind a port"
        );
        assert_ne!(
            port, other_port,
            "concurrent Doors must not share a fixed port"
        );
        other_handle.shutdown();

        let mint_a = mint_relay_from_loopback(&handle, "relay phone A").await;
        let nonce_a = mint_a["nonce"].as_str().expect("relay nonce A").to_owned();
        let relay_key_a = relay_key_from_pair_link(
            mint_a["pair_link"].as_str().expect("relay pair link A"),
            relay.origin(),
        );
        assert!(
            !solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(
                &solstone_core_sol_link::pairing::nonces::NonceStore::new(&fixture.root),
                pairing_now(),
            ),
            "the relay-only fixture has no direct pairing nonce"
        );

        let error = tls_handshake(test_client_config(&[&rustls::version::TLS13], None), port)
            .await
            .expect_err("an unregistered loopback address cannot claim relay A admission");
        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::InvalidData
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset
        ));

        let direct = mint_from_loopback(&handle, "direct phone").await;
        let direct_nonce = direct["nonce"].as_str().expect("direct nonce");
        let mut direct_carrier = live_certless_carrier(port)
            .await
            .expect("direct pairing carrier");
        let direct_relay_attempt = post_pair_over_certless_stream(
            &mut direct_carrier,
            Some(&nonce_a),
            serde_json::json!({
                "csr": csr_pem("direct phone against relay A"),
                "device_label": "direct phone against relay A",
            }),
        )
        .await;
        assert_eq!(
            direct_relay_attempt.status, 403,
            "a direct carrier cannot consume relay A"
        );
        let store = solstone_core_sol_link::pairing::nonces::NonceStore::new(&fixture.root);
        assert!(
            solstone_core_sol_link::pairing::nonces::relay_pairing_nonce_open(
                &store,
                &nonce_a,
                pairing_now(),
            ),
            "the rejected direct request does not consume relay A"
        );
        assert_eq!(
            AuthorizationLedger::new(&fixture.root).snapshot().len(),
            0,
            "the rejected direct request does not issue a certificate or mutate the ledger"
        );
        drop(direct_carrier);

        let stale = mint_relay_from_loopback(&handle, "stale relay phone").await;
        let stale_nonce = stale["nonce"]
            .as_str()
            .expect("stale relay nonce")
            .to_owned();
        let stale_relay_key = relay_key_from_pair_link(
            stale["pair_link"].as_str().expect("stale relay pair link"),
            relay.origin(),
        );
        store
            .consume(&stale_nonce, pairing_now())
            .expect("consume stale relay nonce")
            .expect("stale relay nonce was live");
        assert!(
            !solstone_core_sol_link::pairing::nonces::relay_pairing_nonce_open(
                &store,
                &stale_nonce,
                pairing_now(),
            ),
            "the brokered relay authority is stale before Door admission"
        );
        let stale_mobile = fake_mobile_pair_dial(relay.origin(), &stale_relay_key).await;
        let mut stale_carrier = certless_carrier(stale_mobile)
            .await
            .expect("a stale relay is admitted only as a confined pairing peer");
        let mut decoder = FrameDecoder::new();
        let downgrade_path = format!("{}?token={direct_nonce}", spl_core::PAIR_PATH);
        let downgrade_body = serde_json::to_vec(&serde_json::json!({
            "csr": csr_pem("stale relay against direct D"),
            "device_label": "stale relay against direct D",
        }))
        .expect("stale relay downgrade body");
        let downgrade = exchange_over_carrier(
            &mut stale_carrier,
            &mut decoder,
            1,
            "POST",
            &downgrade_path,
            &[("content-type".into(), "application/json".into())],
            &downgrade_body,
        )
        .await
        .expect("an admitted stale relay carrier returns its authorization refusal");
        assert_eq!(
            downgrade.status, 403,
            "a stale brokered relay carrier cannot inherit direct D admission"
        );
        assert!(
            solstone_core_sol_link::pairing::nonces::direct_pairing_window_open(
                &store,
                pairing_now(),
            ),
            "the refused stale relay carrier leaves direct D live"
        );
        assert_eq!(
            AuthorizationLedger::new(&fixture.root).snapshot().len(),
            0,
            "the stale relay authority downgrade is refused before ledger mutation"
        );

        let direct_response = post_pair_over_certless_carrier(
            port,
            Some(direct_nonce),
            serde_json::json!({
                "csr": csr_pem("direct phone"),
                "device_label": "direct phone",
            }),
        )
        .await;
        assert_eq!(
            direct_response.status, 200,
            "direct pairing remains available"
        );
        assert!(
            solstone_core_sol_link::pairing::nonces::relay_pairing_nonce_open(
                &store,
                &nonce_a,
                pairing_now(),
            ),
            "direct pairing does not consume relay A"
        );

        let mint_b = mint_relay_from_loopback(&handle, "relay phone B").await;
        let nonce_b = mint_b["nonce"].as_str().expect("relay nonce B").to_owned();
        let pair_link_b = mint_b["pair_link"]
            .as_str()
            .expect("relay pair link B")
            .to_owned();
        let _ = relay_key_from_pair_link(&pair_link_b, relay.origin());
        let credential = spl_transport::pairing::pair_from_link(
            &pair_link_b,
            "relay phone B",
            &serde_json::Map::new(),
        )
        .await
        .expect("the shipped relay pair-dial client completes the bridge and ceremony");
        let (_, certificate) =
            parse_x509_pem(credential.client_cert_pem.as_bytes()).expect("issued client PEM");
        let fingerprint = format!("sha256:{}", spl_core::ca::sha256_hex(&certificate.contents));
        // The consumed nonce remains until the next ordinary GC pass, but is
        // no longer live authority and cannot be cancelled or reused.
        let consumed = store
            .peek(&nonce_b)
            .expect("consumed relay nonce remains observable before GC");
        assert!(
            consumed.used,
            "relay nonce was consumed by the pairing ceremony"
        );

        let mint_c = mint_relay_from_loopback(&handle, "relay phone C").await;
        let nonce_c = mint_c["nonce"].as_str().expect("relay nonce C").to_owned();
        let mint_d = mint_relay_from_loopback(&handle, "relay phone D").await;
        let nonce_d = mint_d["nonce"].as_str().expect("relay nonce D").to_owned();
        let relay_key_d = relay_key_from_pair_link(
            mint_d["pair_link"].as_str().expect("relay pair link D"),
            relay.origin(),
        );

        let mobile_a = fake_mobile_pair_dial(relay.origin(), &relay_key_a).await;
        let mut carrier_a = certless_carrier(mobile_a)
            .await
            .expect("relay bridge A reaches certless Door admission");
        let mut decoder = FrameDecoder::new();
        let mut ids = FrameDialer::default();
        let mismatched_path = format!("{}?token={nonce_c}", spl_core::PAIR_PATH);
        let mismatched_body = serde_json::to_vec(&serde_json::json!({
            "csr": csr_pem("relay phone C mismatch"),
            "device_label": "relay phone C mismatch",
        }))
        .expect("mismatched pair request JSON");
        let mismatched = exchange_over_carrier(
            &mut carrier_a,
            &mut decoder,
            ids.allocate(),
            "POST",
            &mismatched_path,
            &[("content-type".into(), "application/json".into())],
            &mismatched_body,
        )
        .await
        .expect("mismatched relay request returns its authorization refusal");
        assert_eq!(mismatched.status, 403, "relay A cannot submit relay C");
        assert!(
            solstone_core_sol_link::pairing::nonces::relay_pairing_nonce_open(
                &store,
                &nonce_c,
                pairing_now(),
            ),
            "relay C remains live after A's mismatched request"
        );
        let mut ledger_after_mismatch = AuthorizationLedger::new(&fixture.root);
        assert_eq!(
            ledger_after_mismatch.snapshot().len(),
            2,
            "the mismatched relay request cannot issue a certificate or mutate the ledger"
        );
        drop(carrier_a);

        let mobile_d = fake_mobile_pair_dial(relay.origin(), &relay_key_d).await;
        let mut carrier_d = certless_carrier(mobile_d)
            .await
            .expect("relay bridge D reaches certless Door admission");
        let confined = exchange_over_carrier(
            &mut carrier_d,
            &mut FrameDecoder::new(),
            1,
            "POST",
            "/app/network/pair-start",
            &[],
            b"{}",
        )
        .await
        .expect("path-confined relay request returns its authorization refusal");
        assert_eq!(
            confined.status, 403,
            "relay admission remains pair-path confined"
        );
        drop(carrier_d);
        let relay_d_revoked_by = Instant::now() + Duration::from_secs(2);
        while solstone_core_sol_link::pairing::nonces::relay_pairing_nonce_open(
            &store,
            &nonce_d,
            pairing_now(),
        ) && Instant::now() < relay_d_revoked_by
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !solstone_core_sol_link::pairing::nonces::relay_pairing_nonce_open(
                &store,
                &nonce_d,
                pairing_now(),
            ),
            "closing the path-confined bridge revokes relay D's exact authority"
        );
        assert_eq!(
            ledger_after_mismatch.snapshot().len(),
            2,
            "the path-confined relay request cannot mutate the ledger"
        );

        let mut ledger = AuthorizationLedger::new(&fixture.root);
        let entries = ledger.snapshot();
        assert_eq!(
            entries.len(),
            2,
            "direct and relay B pairing add one client each"
        );
        assert!(entries.iter().any(|entry| entry.fingerprint == fingerprint));

        handle.shutdown();
    })
    .await
    .expect("relay pairing round trip completes within its bounded timeout");
}

#[tokio::test]
async fn minted_pair_link_drives_the_shipped_client_and_reconnects_as_its_fingerprint() {
    let fixture = Fixture::established(0);
    let handle = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("serve");
    let port = door_port(handle.door_outcome());
    configure_direct_port(&fixture, port);
    let mint = mint_from_loopback(&handle, "paired phone").await;
    let link = mint["pair_link"].as_str().expect("minted pair link");
    let spl_core::pairlink::ParsedPairLink::Direct(parsed) =
        spl_core::pairlink::parse(link).expect("minted direct pair link parses")
    else {
        panic!("same-machine mint emits a direct link");
    };
    assert_eq!(parsed.candidates[0].port, port);
    let credential =
        spl_transport::pairing::pair_from_link(link, "paired phone", &serde_json::Map::new())
            .await
            .expect("shipped pairing client completes ceremony");
    let (_, certificate) =
        parse_x509_pem(credential.client_cert_pem.as_bytes()).expect("credential cert");
    let fingerprint = format!("sha256:{}", spl_core::ca::sha256_hex(&certificate.contents));
    let mut ledger = AuthorizationLedger::new(&fixture.root);
    assert_eq!(
        ledger
            .get(&fingerprint)
            .expect("paired fingerprint record")
            .fingerprint,
        fingerprint
    );
    TransportClient::new(credential, None)
        .expect("credential has strict mTLS configuration")
        .dial_carrier()
        .await
        .expect("credential reconnects as the linked device");
    handle.shutdown();
}

#[tokio::test]
async fn minted_pair_link_reconnects_after_a_door_restart_with_a_fresh_leaf() {
    let fixture = Fixture::established(1);
    let first = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("first serve");
    let first_port = door_port(first.door_outcome());
    configure_direct_port(&fixture, first_port);
    let mint = mint_from_loopback(&first, "restart phone").await;
    let link = mint["pair_link"].as_str().expect("minted pair link");
    let credential =
        spl_transport::pairing::pair_from_link(link, "restart phone", &serde_json::Map::new())
            .await
            .expect("shipped pairing client completes ceremony");
    let first_chain = presented_chain(&fixture, door_port(first.door_outcome())).await;
    let (_, first_leaf) =
        x509_parser::parse_x509_certificate(first_chain[0].as_ref()).expect("first leaf parses");
    let restart_port = door_port(first.door_outcome());
    first.shutdown();
    drop(first);

    let second = {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let candidate = serve(options(
                &fixture,
                pairing_router(&fixture, pairing_snapshot()),
                restart_port,
            ))
            .await
            .expect("second serve starts");
            if matches!(
                candidate.door_outcome(),
                DoorOutcome::Bound(address) if address.port() == restart_port
            ) {
                break candidate;
            }
            candidate.shutdown();
            drop(candidate);
            if Instant::now() >= deadline {
                panic!("door did not rebind stored port {restart_port} within 2s after shutdown");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let second_chain = presented_chain(&fixture, door_port(second.door_outcome())).await;
    let (_, second_leaf) =
        x509_parser::parse_x509_certificate(second_chain[0].as_ref()).expect("second leaf parses");
    assert_ne!(
        first_leaf.raw_serial(),
        second_leaf.raw_serial(),
        "a restart that reused the first leaf would make this reconnect test pass vacuously"
    );
    TransportClient::new(credential, None)
        .expect("credential has strict mTLS configuration")
        .dial_carrier()
        .await
        .expect("stored credential reconnects after the door reminted its leaf");
    second.shutdown();
}

#[cfg(unix)]
#[tokio::test]
async fn ceremony_preserves_ca_burns_nonces_and_emits_distinct_label_notices() {
    let fixture = Fixture::established(0);
    let health = fixture.root.join("health");
    std::fs::create_dir(&health).expect("health directory");
    let socket = health.join("callosum.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("Callosum listener");
    let notices = tokio::spawn(collect_callosum_events(listener, "pair_complete", 2));
    let handle = serve(options(
        &fixture,
        pairing_router(&fixture, pairing_snapshot()),
        0,
    ))
    .await
    .expect("serve");
    let ca_before = tree_paths(&fixture.root.join("link/ca"))
        .into_iter()
        .map(|path| {
            (
                path.strip_prefix(&fixture.root)
                    .expect("relative")
                    .to_path_buf(),
                std::fs::read(path).expect("CA bytes"),
            )
        })
        .collect::<Vec<_>>();
    let first_mint = mint_from_loopback(&handle, "collision").await;
    // Keep another minted nonce live while exercising the consumed-nonce
    // refusal through the door: confinement correctly closes a carrier only
    // when this is the final nonce.
    let second_mint = mint_from_loopback(&handle, "collision").await;
    let link_before = tree_paths(&fixture.root.join("link"))
        .into_iter()
        .map(|path| {
            path.strip_prefix(fixture.root.join("link"))
                .expect("link relative")
                .to_path_buf()
        })
        .collect::<Vec<_>>();
    let first_nonce = first_mint["nonce"].as_str().expect("first nonce");
    let first = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        None,
        serde_json::json!({
            "nonce": first_nonce,
            "csr": csr_pem("collision"),
            "device_label": "collision",
            "arbitrary_unknown_top_level_field": {"accepted": true},
        }),
    )
    .await;
    assert_eq!(first.status, 200, "unknown fields remain accepted");
    let first_pair: spl_core::PairResponse =
        serde_json::from_slice(&first.body).expect("first pair response");
    let retry = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(first_nonce),
        serde_json::json!({"csr": csr_pem("retry"), "device_label": "collision"}),
    )
    .await;
    assert_eq!(retry.status, 410, "successful ceremony burns its nonce");

    let failed_mint = mint_from_loopback(&handle, "failed phone").await;
    let failed_nonce = failed_mint["nonce"].as_str().expect("failed nonce");
    let ledger_before_failed_phone = AuthorizationLedger::new(&fixture.root).snapshot();
    let failed_phone = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(failed_nonce),
        serde_json::json!({"csr":"not a CSR","device_label":"failed phone"}),
    )
    .await;
    assert_eq!(failed_phone.status, 400, "invalid phone CSR refuses");
    assert_eq!(
        AuthorizationLedger::new(&fixture.root).snapshot(),
        ledger_before_failed_phone,
        "failed phone ceremony adds no ledger artifact"
    );
    let failed_retry = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(failed_nonce),
        serde_json::json!({"csr":csr_pem("failed retry"),"device_label":"failed phone"}),
    )
    .await;
    assert_eq!(
        failed_retry.status, 410,
        "failed phone ceremony also burns its nonce"
    );

    let second_nonce = second_mint["nonce"].as_str().expect("second nonce");
    let malformed = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(second_nonce),
        serde_json::json!({
            "csr": csr_pem("collision"),
            "device_label": "collision",
            "sender_instance_id": "invalid id",
        }),
    )
    .await;
    assert_eq!(malformed.status, 400);
    let malformed: serde_json::Value =
        serde_json::from_slice(&malformed.body).expect("malformed refusal");
    assert_eq!(malformed["reason_code"], "pairing_request_invalid");
    assert_eq!(malformed["detail"], "sender_instance_id is invalid");
    let peer_request = serde_json::json!({"device_label":"peer", "role":"peer"});
    let (peer_status, peer_mint_body) = loopback_json_request(
        handle.loopback_ipv4_addr(),
        "POST",
        "/app/network/pair-start",
        &peer_request,
    )
    .await;
    assert_eq!(peer_status, 200);
    let peer_mint: serde_json::Value =
        serde_json::from_slice(&peer_mint_body).expect("peer mint JSON");
    let peer_nonce = peer_mint["nonce"].as_str().expect("peer nonce");
    let second = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(second_nonce),
        serde_json::json!({
            "csr": csr_pem("collision"),
            "device_label": "collision",
            "sender_instance_id": "valid-sender_1",
        }),
    )
    .await;
    assert_eq!(second.status, 200, "malformed sender did not consume nonce");
    // Keep a final phone nonce live while the peer refusal is retried; this
    // reaches the ceremony's 410 instead of confinement's closed-window 403.
    let (reserve_status, reserve_body) = loopback_json_request(
        handle.loopback_ipv4_addr(),
        "POST",
        "/app/network/pair-start",
        &serde_json::json!({"device_label":"reserve"}),
    )
    .await;
    assert_eq!(reserve_status, 200);
    assert!(
        serde_json::from_slice::<serde_json::Value>(&reserve_body)
            .expect("reserve mint JSON")["nonce"]
            .is_string()
    );
    let peer = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(peer_nonce),
        serde_json::json!({"csr": csr_pem("peer"), "device_label": "peer"}),
    )
    .await;
    assert_eq!(peer.status, 400);
    let peer: serde_json::Value = serde_json::from_slice(&peer.body).expect("peer refusal");
    assert_eq!(peer["reason_code"], "pairing_request_invalid");
    assert_eq!(
        peer["detail"],
        "peer pairing is not available on this build"
    );
    assert_ne!(peer["detail"], malformed["detail"]);
    let peer_retry = post_pair_over_certless_carrier(
        door_port(handle.door_outcome()),
        Some(peer_nonce),
        serde_json::json!({"csr": csr_pem("peer retry"), "device_label": "peer"}),
    )
    .await;
    assert_eq!(peer_retry.status, 410, "peer refusal also burns its nonce");

    let ca_after = tree_paths(&fixture.root.join("link/ca"))
        .into_iter()
        .map(|path| {
            (
                path.strip_prefix(&fixture.root)
                    .expect("relative")
                    .to_path_buf(),
                std::fs::read(path).expect("CA bytes"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ca_after, ca_before,
        "ceremony never rewrites committed CA material"
    );
    let link_after = tree_paths(&fixture.root.join("link"))
        .into_iter()
        .map(|path| {
            path.strip_prefix(fixture.root.join("link"))
                .expect("link relative")
                .to_path_buf()
        })
        .collect::<Vec<_>>();
    let new_paths = link_after
        .into_iter()
        .filter(|path| !link_before.contains(path))
        .collect::<Vec<_>>();
    assert_eq!(
        new_paths,
        vec![PathBuf::from("authorized_clients.json.lock")]
    );
    let mut ledger = AuthorizationLedger::new(&fixture.root);
    let entries = ledger.snapshot();
    assert_eq!(entries.len(), 2, "peer refusal adds no ledger entry");
    assert_eq!(entries[0].display_label(), "collision");
    assert_eq!(entries[0].label_ordinal, 1);
    assert_eq!(entries[1].display_label(), "collision (2)");
    assert_eq!(entries[1].label_ordinal, 2);
    assert_eq!(entries[0].fingerprint, first_pair.fingerprint);
    let notices = notices.await.expect("notice task");
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[0]["tract"], "link");
    assert_eq!(notices[0]["event"], "pair_complete");
    assert_eq!(notices[0]["device_label"], "collision");
    assert_eq!(notices[1]["device_label"], "collision (2)");
    handle.shutdown();
}

#[test]
fn production_pairing_paths_do_not_create_ca_material() {
    // Scope this check to the handlers and their direct pairing-domain callee,
    // not a repo-wide grep: fixtures legitimately generate CAs elsewhere.
    let handler = include_str!("../src/network.rs")
        .split("#[cfg(test)]")
        .next()
        .expect("production handler source");
    let domain = include_str!("../../solstone-core-sol-link/src/pairing/mod.rs")
        .split("#[cfg(test)]")
        .next()
        .expect("production pairing source");
    let committed = include_str!("../../solstone-core-sol-link/src/committed.rs")
        .split("#[cfg(test)]")
        .next()
        .expect("production committed identity source");
    let attestation = include_str!("../../solstone-core-sol-link/src/pairing/attestation.rs")
        .split("#[cfg(test)]")
        .next()
        .expect("production attestation source");
    let signing = include_str!("../../solstone-core-sol-link/src/ca.rs")
        .split("pub fn sign_csr")
        .nth(1)
        .expect("signing direct callee")
        .split("#[cfg(test)]")
        .next()
        .expect("production signing source");
    assert!(!handler.contains("generate_ca"));
    assert!(!domain.contains("generate_ca"));
    assert!(!committed.contains("generate_ca"));
    assert!(!attestation.contains("generate_ca"));
    assert!(!signing.contains("generate_ca"));
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
    fixture.induce_unreadable_authorization();
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
    fixture.induce_malformed_authorization();
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
async fn ac3_handshake_uses_the_ledger_while_publication_is_stale() {
    let fixture = Fixture::established(1);
    let (sender, publication) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let mut handle = solstone_core_convey_shell::bind_with_authorization(
        options(&fixture, router(fixture.root.clone()), 0),
        solstone_core_convey_shell::authorization_gate::DoorRouter::unconfined(router(
            fixture.root.clone(),
        )),
        sender,
    )
    .await
    .expect("serve");
    let port = door_port(handle.door_outcome());
    let did = format!("sha256:{}", spl_core::ca::sha256_hex(fixture.client_der(0)));

    let established = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                publication.borrow().as_read(),
                AuthorizedClientsRead::Present(entries)
                    if entries.iter().any(|entry| entry.fingerprint == did)
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        established.is_ok(),
        "publisher never reported the listed client"
    );

    assert!(fixture.remove_authorization(0).authorized_removed);
    assert!(matches!(
        publication.borrow().as_read(),
        AuthorizedClientsRead::Present(entries)
            if entries.iter().any(|entry| entry.fingerprint == did)
    ));
    assert!(
        fresh_door_connection_is_refused(&fixture, port).await,
        "the TLS verifier must read the ledger instead of the stale publication"
    );
    handle.stop_authorization_refresh().await;
    handle.shutdown();
}

#[tokio::test]
async fn ac4_each_carrier_snapshots_a_fresh_ledger_read() {
    let fixture = Fixture::established(1);
    let handshake_authorization_read_ticks = Arc::new(AtomicU64::new(0));
    let (sender, _) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let mut handle = solstone_core_convey_shell::bind_with_authorization(
        ConveyServeOptions {
            journal_root: fixture.root.clone(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: Duration::from_secs(2),
            stream_stall_timeout: Duration::from_secs(2),
            router: router(fixture.root.clone()),
            carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: handshake_authorization_read_ticks.clone(),
        },
        solstone_core_convey_shell::authorization_gate::DoorRouter::unconfined(router(
            fixture.root.clone(),
        )),
        sender,
    )
    .await
    .expect("serve");
    let port = door_port(handle.door_outcome());
    let before = handshake_authorization_read_ticks.load(Ordering::Relaxed);

    drop(live_carrier(&fixture, port).await);
    handle.stop_authorization_refresh().await;
    assert!(fixture.remove_authorization(0).authorized_removed);
    assert_eq!(
        ledger_posture(&fixture),
        AuthorizedClientsRead::Present(Vec::new())
    );
    assert!(fresh_door_connection_is_refused(&fixture, port).await);
    fixture.restore_authorization(0);
    drop(live_carrier(&fixture, port).await);

    // These three dials are the only carriers against this door in this window.
    assert_eq!(
        handshake_authorization_read_ticks.load(Ordering::Relaxed) - before,
        3
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac5b_hung_handshake_read_fails_closed_and_recovers() {
    let fixture = Fixture::established(1);
    let (sender, _) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let mut handle = solstone_core_convey_shell::bind_with_authorization(
        options(&fixture, router(fixture.root.clone()), 0),
        solstone_core_convey_shell::authorization_gate::DoorRouter::unconfined(router(
            fixture.root.clone(),
        )),
        sender,
    )
    .await
    .expect("serve");
    let port = door_port(handle.door_outcome());
    handle.stop_authorization_refresh().await;
    let path = fixture.root.join("link/authorized_clients.json");

    warn_capture::install_and_clear();
    log::warn!("handshake warn capture control");
    assert!(warn_capture::contains("warn capture control"));
    warn_capture::clear();
    let started = Instant::now();
    let result = {
        let _fifo = BlockingAuthorizationFifo::replace(path);
        tls_handshake(fixture.client_config(0), port).await
    };
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "hung read is bounded"
    );
    assert_tls_alert(result, rustls::AlertDescription::CertificateUnknown);
    assert!(warn_capture::contains(
        "handshake authorization read timed out"
    ));

    warn_capture::clear();
    fixture.restore_authorization(0);
    drop(live_carrier(&fixture, port).await);
    assert!(
        !warn_capture::contains("handshake authorization read timed out"),
        "completed read must not emit the handshake timeout warning"
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac7b_stale_publication_does_not_close_a_freshly_admitted_carrier() {
    let fixture = Fixture::established(1);
    assert!(fixture.remove_authorization(0).authorized_removed);
    let (sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let publication = receiver.clone();
    let door_router = authorized_router(fixture.root.clone(), receiver);
    let mut handle = bind_with_authorization(
        options(&fixture, router(fixture.root.clone()), 0),
        door_router,
        sender,
    )
    .await
    .expect("serve");
    assert_eq!(
        publication.borrow().as_read(),
        &AuthorizedClientsRead::Present(Vec::new()),
        "the stopped publisher retains the pre-restore revocation"
    );
    handle.stop_authorization_refresh().await;
    fixture.restore_authorization(0);

    // On origin/main this stale `Present([])` publication rejects the TLS
    // handshake. Without D6, the fresh handshake succeeds but this first
    // ready watch notification can close the carrier before request two.
    let mut carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;
    let mut decoder = FrameDecoder::new();
    let mut dialer = FrameDialer::default();
    let first = get_over_carrier(
        &mut carrier,
        &mut decoder,
        &mut dialer,
        "/api/system/status",
    )
    .await;
    assert_eq!(first.status, 200);
    // The first response is fully assembled before this second request wakes
    // `accept_stream`, making the stale watch arm deterministically ready.
    let second = get_over_carrier(
        &mut carrier,
        &mut decoder,
        &mut dialer,
        "/api/system/status",
    )
    .await;
    assert_eq!(second.status, 200);
    handle.shutdown();
}

#[tokio::test]
async fn ac8_dead_publisher_keeps_existing_and_fresh_carriers_usable() {
    let fixture = Fixture::established(1);
    let (sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let mut publication = receiver.clone();
    let door_router = authorized_router(fixture.root.clone(), receiver);
    let mut handle = bind_with_authorization(
        options(&fixture, router(fixture.root.clone()), 0),
        door_router,
        sender,
    )
    .await
    .expect("serve");
    let port = door_port(handle.door_outcome());
    let mut existing = live_carrier(&fixture, port).await;
    handle.stop_authorization_refresh().await;
    publication.borrow_and_update();
    assert!(
        tokio::time::timeout(Duration::from_millis(600), publication.changed())
            .await
            .is_err(),
        "the stopped door publisher must not update its own authorization watch"
    );

    let mut existing_decoder = FrameDecoder::new();
    let mut existing_dialer = FrameDialer::default();
    assert_eq!(
        get_over_carrier(
            &mut existing,
            &mut existing_decoder,
            &mut existing_dialer,
            "/api/system/status",
        )
        .await
        .status,
        200
    );

    let mut fresh = live_carrier(&fixture, port).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut fresh_decoder = FrameDecoder::new();
    let mut fresh_dialer = FrameDialer::default();
    assert_eq!(
        get_over_carrier(
            &mut fresh,
            &mut fresh_decoder,
            &mut fresh_dialer,
            "/api/system/status",
        )
        .await
        .status,
        200
    );
    handle.shutdown();
}

#[tokio::test]
async fn ac9_dead_publisher_leaves_gate_and_boot_asset_behavior_intact() {
    let fixture = Fixture::established(1);
    let (sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let door_router = authorized_router(fixture.root.clone(), receiver);
    let mut handle = bind_with_authorization(
        options(&fixture, router(fixture.root.clone()), 0),
        door_router,
        sender,
    )
    .await
    .expect("serve");
    let port = door_port(handle.door_outcome());
    let mut carrier = live_carrier(&fixture, port).await;
    let mut decoder = FrameDecoder::new();
    let mut dialer = FrameDialer::default();
    assert_eq!(
        get_over_carrier(
            &mut carrier,
            &mut decoder,
            &mut dialer,
            "/api/system/status",
        )
        .await
        .status,
        200
    );

    handle.stop_authorization_refresh().await;
    assert!(fixture.remove_authorization(0).authorized_removed);
    assert_eq!(
        get_over_carrier(
            &mut carrier,
            &mut decoder,
            &mut dialer,
            "/api/system/status",
        )
        .await
        .status,
        403
    );
    assert!(fresh_door_connection_is_refused(&fixture, port).await);
    for path in ["/favicon.ico", "/static/shell.html"] {
        assert_eq!(
            get_over_carrier(&mut carrier, &mut decoder, &mut dialer, path)
                .await
                .status,
            200,
            "{path} remains a boot-asset exemption"
        );
    }
    handle.shutdown();
}

#[tokio::test]
async fn ac10_closed_publisher_does_not_spin_the_carrier_loop() {
    let fixture = Fixture::established(1);
    let carrier_loop_iterations = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let door_router = authorized_router(fixture.root.clone(), receiver);
    let mut handle = bind_with_authorization(
        ConveyServeOptions {
            journal_root: fixture.root.clone(),
            loopback_port: 0,
            door_port: 0,
            handshake_timeout: Duration::from_secs(2),
            stream_stall_timeout: Duration::from_secs(2),
            router: router(fixture.root.clone()),
            carrier_loop_iterations: carrier_loop_iterations.clone(),
            handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
        },
        door_router,
        sender,
    )
    .await
    .expect("serve");
    let mut carrier = live_carrier(&fixture, door_port(handle.door_outcome())).await;
    handle.stop_authorization_refresh().await;

    // Exactly one carrier is open for this entire measurement window; no
    // other dial may contribute to this per-door-start counter.
    let t0 = tokio::time::timeout(Duration::from_secs(2), async {
        for _ in 0..10 {
            let before = carrier_loop_iterations.load(Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(100)).await;
            if carrier_loop_iterations.load(Ordering::Relaxed) == before {
                return before;
            }
        }
        panic!("carrier loop did not become idle after publisher closure");
    })
    .await
    .expect("carrier loop stabilization is bounded");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(carrier_loop_iterations.load(Ordering::Relaxed), t0);

    let mut decoder = FrameDecoder::new();
    let mut dialer = FrameDialer::default();
    assert_eq!(
        get_over_carrier(
            &mut carrier,
            &mut decoder,
            &mut dialer,
            "/api/system/status",
        )
        .await
        .status,
        200
    );
    let before_requests = carrier_loop_iterations.load(Ordering::Relaxed);
    for _ in 0..5 {
        assert_eq!(
            get_over_carrier(
                &mut carrier,
                &mut decoder,
                &mut dialer,
                "/api/system/status",
            )
            .await
            .status,
            200
        );
    }
    assert!(
        carrier_loop_iterations.load(Ordering::Relaxed) - before_requests >= 5,
        "request work advances the same counter"
    );
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
    let after = tree(&fixture.root.join("link"))
        .into_iter()
        .filter(|path| path != Path::new("nonces.json.lock"))
        .collect::<Vec<_>>();
    assert_eq!(after, before);
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
        exchange_over_carrier(
            &mut carrier,
            &mut decoder,
            ids.allocate(),
            "GET",
            "/api/system/status",
            &[],
            &[]
        )
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
            if exchange_over_carrier(
                &mut carrier,
                &mut decoder,
                ids.allocate(),
                "GET",
                "/api/system/status",
                &[],
                &[],
            )
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
        exchange_over_carrier(
            &mut carrier,
            &mut decoder,
            ids.allocate(),
            "GET",
            "/api/system/status",
            &[],
            &[]
        )
        .await
        .is_ok()
    );
    let authorization = fixture.root.join("link/authorized_clients.json");
    fixture.induce_unreadable_authorization();
    assert_eq!(ledger_posture(&fixture), AuthorizedClientsRead::Unreadable);
    tokio::time::sleep(Duration::from_millis(1_600)).await;
    assert!(
        exchange_over_carrier(
            &mut carrier,
            &mut decoder,
            ids.allocate(),
            "GET",
            "/api/system/status",
            &[],
            &[]
        )
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
            if exchange_over_carrier(
                &mut carrier,
                &mut decoder,
                ids.allocate(),
                "GET",
                "/api/system/status",
                &[],
                &[],
            )
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
    let received = tokio::spawn(collect_callosum_events(listener, "last_seen", 1));
    let handle = serve(options(&fixture, router(fixture.root.clone()), 0))
        .await
        .expect("serve");
    let port = door_port(handle.door_outcome());
    assert!(request_result(&fixture, 0, port).await.is_ok());
    let did = format!("sha256:{}", spl_core::ca::sha256_hex(fixture.client_der(0)));
    let message = received
        .await
        .expect("Callosum task")
        .into_iter()
        .next()
        .expect("last_seen message");
    assert_eq!(message["tract"], "link");
    assert_eq!(message["event"], "last_seen");
    assert_eq!(message["fingerprint"], did);
    let devices: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.root.join("link/devices.json")).expect("devices ledger"),
    )
    .expect("devices ledger JSON");
    let last_seen_at = devices[&did]["last_seen_at"]
        .as_str()
        .expect("accepted device records last_seen_at");
    let parsed =
        time::OffsetDateTime::parse(last_seen_at, &time::format_description::well_known::Rfc3339)
            .expect("last_seen_at is RFC3339");
    assert_eq!(parsed.offset(), time::UtcOffset::UTC, "last_seen_at is UTC");
    let now = time::OffsetDateTime::now_utc();
    assert!(
        parsed >= now - time::Duration::minutes(5) && parsed <= now + time::Duration::minutes(1),
        "last_seen_at is plausibly current"
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
        matches!(loopback_status(handle.loopback_ipv4_addr()).await, Ok(200)),
        "loopback answers a live request after the door bind failure"
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
            carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
        })
        .await
        .expect("loopback");
        assert!(matches!(
            handle.door_outcome(),
            DoorOutcome::Withheld(DoorWithheldReason::Unestablished | DoorWithheldReason::Corrupt)
        ));
        let after = tree(&root.join("link"))
            .into_iter()
            .filter(|path| path != Path::new("nonces.json.lock"))
            .collect::<Vec<_>>();
        assert_eq!(after, before);
        handle.shutdown();
        let _ = std::fs::remove_dir_all(root);
    }
}
