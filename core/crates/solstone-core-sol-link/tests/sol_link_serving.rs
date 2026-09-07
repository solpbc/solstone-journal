// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::json;
use solstone_core_ingest_contract::CONNECTION_BODY_LIMIT;
use solstone_core_sol_client::link_credentials::{
    LinkCredentialStore, PairingIdentity, StoreLoadOutcome, pem_cert_der,
};
use solstone_core_sol_client::resident::ShutdownSignal;
use solstone_core_sol_client::seam::{
    LinkServeBundle, LinkServeCarrierPolicy, LinkServeEndpoint, LinkServeErrorKind,
    LinkServeRelayControlEndpoint, LinkServeRequest, LinkServeRunner, LinkServeTransportErrorKind,
};
use solstone_core_sol_link::SplLinkServeRunner;
use solstone_core_sol_link::serve_test_support::{
    CurrentClientManager, JobSchedulerTestParams, OptionalJobScheduler, ReportedDescription,
    STATUS_PATH, StatusClock, StatusTracker, SystemStatusClock, bridge_names,
    bridge_policy_for_port, publish_device_description, publish_device_description_with,
};
use spl_core::bridge::{RequestHead, parse_request_head};
use spl_core::frame::{FLAG_CLOSE, FLAG_DATA, FLAG_WINDOW, Frame, FrameDecoder, RECOMMENDED_CHUNK};
use spl_core::mux::INITIAL_WINDOW;
use spl_transport::TransportError;
use spl_transport::client::{DialedCarrier, TransportClient};
use spl_transport::credential::{Credential, EndpointAddr};
use spl_transport::journal_bridge::{self, CarrierOpener, JournalBridgeConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;

#[derive(Debug, Default)]
struct CountingOpener {
    dials: AtomicUsize,
}

impl CountingOpener {
    fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }
}

impl CarrierOpener for CountingOpener {
    fn proxy_headers(
        &self,
        upstream_headers: &[(String, String)],
    ) -> Result<Vec<(String, String)>, TransportError> {
        Ok(upstream_headers.to_vec())
    }

    fn dial_carrier(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<DialedCarrier, TransportError>> + Send + '_>,
    > {
        Box::pin(async move {
            self.dials.fetch_add(1, Ordering::SeqCst);
            Err(TransportError::NoEndpoint)
        })
    }
}

struct TransportClientOpener {
    client: Arc<TransportClient>,
}

impl CarrierOpener for TransportClientOpener {
    fn proxy_headers(
        &self,
        upstream_headers: &[(String, String)],
    ) -> Result<Vec<(String, String)>, TransportError> {
        Ok(upstream_headers.to_vec())
    }

    fn dial_carrier(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<DialedCarrier, TransportError>> + Send + '_>,
    > {
        Box::pin(self.client.dial_carrier())
    }
}

#[derive(Debug)]
struct FixedStatusClock(Mutex<f64>);

impl FixedStatusClock {
    fn new(now: f64) -> Self {
        Self(Mutex::new(now))
    }
}

impl StatusClock for FixedStatusClock {
    fn now_unix_seconds(&self) -> f64 {
        *self.0.lock().expect("clock lock")
    }
}

fn ca_pem() -> String {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
    let params = CertificateParams::new(Vec::<String>::new()).expect("test params");
    params.self_signed(&key).expect("test ca").pem()
}

fn unused_loopback_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind probe")
        .local_addr()
        .expect("probe addr")
        .port()
}

fn self_signed_server() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("server key");
    let params = CertificateParams::new(vec!["spl.local".to_string()]).expect("server params");
    let cert = params.self_signed(&key).expect("server cert");
    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    (cert_der, key_der)
}

fn server_config(cert: CertificateDer<'static>, key: PrivateKeyDer<'static>) -> ServerConfig {
    ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("server protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config")
}

fn transport_credential(pin: Vec<u8>, port: u16) -> Credential {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client key");
    let params =
        CertificateParams::new(vec!["transport.test".to_string()]).expect("client cert params");
    let cert = params.self_signed(&key).expect("client cert");
    Credential {
        client_key_pem: key.serialize_pem(),
        client_cert_pem: cert.pem(),
        ca_chain_pem: vec![cert.pem()],
        ca_fp_prefix: pin,
        instance_id: "test-instance".to_string(),
        home_label: "Home".to_string(),
        endpoints: vec![EndpointAddr {
            host: "127.0.0.1".to_string(),
            port,
        }],
        home_attestation: None,
        local_endpoints: None,
        relay_origin: None,
        device_token: None,
        device_token_expires_at: None,
    }
}

async fn read_framed_request(
    tls: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> u32 {
    read_framed_request_bytes(tls).await.0
}

async fn read_framed_request_bytes(
    tls: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> (u32, Vec<u8>) {
    let mut decoder = FrameDecoder::new();
    let mut stream_id = 1u32;
    let mut closed = false;
    let mut raw = Vec::new();
    let mut recv_credit: i64 = INITIAL_WINDOW as i64;
    let mut unacked: i64 = 0;
    let mut buf = [0u8; 8192];
    while !closed {
        let n = tokio::time::timeout(Duration::from_secs(60), tls.read(&mut buf))
            .await
            .expect("framed request timeout")
            .expect("read framed request");
        if n == 0 {
            break;
        }
        decoder.feed(&buf[..n]);
        for frame in decoder.drain().expect("decode request frame") {
            if let Some(pong) = frame.control_pong() {
                tls.write_all(&pong.encode().expect("encode pong"))
                    .await
                    .expect("write pong");
                tls.flush().await.expect("flush pong");
                continue;
            }
            stream_id = frame.stream_id;
            if frame.flags & FLAG_DATA != 0 {
                let len = frame.payload.len() as i64;
                assert!(
                    len <= recv_credit,
                    "peer sent DATA past the un-granted mux window"
                );
                recv_credit -= len;
                unacked += len;
                raw.extend_from_slice(&frame.payload);
                if unacked >= (INITIAL_WINDOW as i64) / 2 {
                    let grant = unacked as u32;
                    recv_credit += unacked;
                    unacked = 0;
                    let window = Frame::new(stream_id, FLAG_WINDOW, grant.to_be_bytes().to_vec());
                    tls.write_all(&window.encode().expect("encode window"))
                        .await
                        .expect("write window");
                    tls.flush().await.expect("flush window");
                }
            }
            if frame.flags & FLAG_CLOSE != 0 {
                closed = true;
            }
        }
    }
    (stream_id, raw)
}

struct ScriptedHttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

struct CapturedUpstream {
    head: RequestHead,
    content_length: usize,
    body: Vec<u8>,
}

fn encode_scripted_http(response: &ScriptedHttpResponse) -> Vec<u8> {
    let reason = match response.status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {} {reason}\r\n", response.status);
    for (name, value) in &response.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", response.body.len()));
    let mut encoded = head.into_bytes();
    encoded.extend_from_slice(&response.body);
    encoded
}

async fn serve_and_capture_one_request(
    listener: TokioTcpListener,
    acceptor: TlsAcceptor,
    response: ScriptedHttpResponse,
) -> CapturedUpstream {
    let (tcp, _) = listener.accept().await.expect("accept transport peer");
    let mut tls = acceptor.accept(tcp).await.expect("accept tls");
    let (stream_id, raw) = read_framed_request_bytes(&mut tls).await;
    let validated = parse_request_head(&raw).expect("parse upstream request head");
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("header terminator")
        + 4;
    let body = raw[header_end..].to_vec();
    write_response_frame(
        &mut tls,
        stream_id,
        FLAG_DATA | FLAG_CLOSE,
        encode_scripted_http(&response),
    )
    .await;
    let _ = tls.shutdown().await;
    CapturedUpstream {
        head: validated.head,
        content_length: validated.content_length,
        body,
    }
}

async fn start_bridge_with_capture(
    response: ScriptedHttpResponse,
) -> (
    spl_transport::journal_bridge::JournalBridgeHandle,
    u16,
    tokio::task::JoinHandle<CapturedUpstream>,
) {
    let (server_cert, server_key) = self_signed_server();
    let pin = spl_core::ca::sha256(server_cert.as_ref())[..16].to_vec();
    let listener = TokioTcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind transport peer");
    let transport_port = listener.local_addr().expect("transport addr").port();
    let client = Arc::new(
        TransportClient::new(transport_credential(pin, transport_port), None)
            .expect("transport client"),
    );
    let acceptor = TlsAcceptor::from(Arc::new(server_config(server_cert, server_key)));
    let server = tokio::spawn(serve_and_capture_one_request(listener, acceptor, response));
    let tracker = Arc::new(StatusTracker::new(Arc::new(FixedStatusClock::new(0.0))));
    let handle = journal_bridge::start(JournalBridgeConfig {
        opener: Arc::new(TransportClientOpener { client }),
        bridge_names: bridge_names(),
        endpoint_hosts: vec!["127.0.0.1".to_string()],
        policy: bridge_policy_for_port(0, tracker),
    })
    .await
    .expect("bridge start");
    let bound = handle.port();
    (handle, bound, server)
}

async fn start_counting_bridge() -> (
    spl_transport::journal_bridge::JournalBridgeHandle,
    u16,
    Arc<CountingOpener>,
) {
    let opener = Arc::new(CountingOpener::default());
    let tracker = Arc::new(StatusTracker::new(Arc::new(FixedStatusClock::new(0.0))));
    let handle = journal_bridge::start(JournalBridgeConfig {
        opener: opener.clone(),
        bridge_names: bridge_names(),
        endpoint_hosts: vec!["192.168.1.10".to_string()],
        policy: bridge_policy_for_port(0, tracker),
    })
    .await
    .expect("bridge start");
    let bound = handle.port();
    (handle, bound, opener)
}

async fn http_exchange(port: u16, request: &[u8]) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect bridge");
    stream.write_all(request).await.expect("write request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), stream.read_to_end(&mut response))
        .await
        .expect("response timeout")
        .expect("read response");
    response
}

fn parse_client_http(raw: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let header_end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("client header terminator");
    let headers_text = std::str::from_utf8(&raw[..header_end]).expect("client headers utf8");
    let mut lines = headers_text.split("\r\n");
    let status_line = lines.next().expect("status line");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse::<u16>()
        .expect("status u16");
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    (status, headers, raw[header_end + 4..].to_vec())
}

fn header_values<'a>(head: &'a RequestHead, name: &str) -> Vec<&'a str> {
    head.headers
        .iter()
        .filter(|(existing, _)| existing == name)
        .map(|(_, value)| value.as_str())
        .collect()
}

fn json_ok() -> ScriptedHttpResponse {
    ScriptedHttpResponse {
        status: 200,
        headers: vec![
            ("etag".to_string(), "fixture-1".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ],
        body: b"{\"status\":\"ok\"}".to_vec(),
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

async fn write_response_frame(
    tls: &mut tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    stream_id: u32,
    flags: u8,
    payload: Vec<u8>,
) {
    let frame = Frame::new(stream_id, flags, payload);
    tls.write_all(&frame.encode().expect("encode response frame"))
        .await
        .expect("write response frame");
    tls.flush().await.expect("flush response frame");
}

async fn serve_latched_stream(
    listener: TokioTcpListener,
    acceptor: TlsAcceptor,
    release: oneshot::Receiver<()>,
    released: Arc<AtomicBool>,
) {
    let (tcp, _) = listener.accept().await.expect("accept transport peer");
    let mut tls = acceptor.accept(tcp).await.expect("accept tls");
    let stream_id = read_framed_request(&mut tls).await;
    let mut first_chunk = vec![b'x'; RECOMMENDED_CHUNK * 2 + 1];
    first_chunk[0] = b'A';
    let content_length = first_chunk.len() + 1;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {content_length}\r\n\r\n"
    );
    write_response_frame(&mut tls, stream_id, FLAG_DATA, head.into_bytes()).await;
    write_response_frame(&mut tls, stream_id, FLAG_DATA, first_chunk).await;
    release.await.expect("release latch");
    released.store(true, Ordering::SeqCst);
    write_response_frame(&mut tls, stream_id, FLAG_DATA | FLAG_CLOSE, vec![b'B']).await;
    let _ = tls.shutdown().await;
}

async fn http_get(port: u16, target: &str) -> (SocketAddr, String) {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect bridge");
    let peer = stream.peer_addr().expect("bridge peer addr");
    let request =
        format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
        .await
        .expect("response timeout")
        .expect("read response");
    (peer, String::from_utf8_lossy(&response).into_owned())
}

struct GateShutdown {
    released: Mutex<bool>,
    gate: Condvar,
}

impl GateShutdown {
    fn new() -> Self {
        Self {
            released: Mutex::new(false),
            gate: Condvar::new(),
        }
    }

    fn release(&self) {
        *self.released.lock().expect("shutdown lock") = true;
        self.gate.notify_all();
    }
}

impl ShutdownSignal for GateShutdown {
    fn wait(&self) {
        let mut released = self.released.lock().expect("shutdown lock");
        while !*released {
            released = self.gate.wait(released).expect("shutdown wait");
        }
    }
}

/// Issue one HTTP/1.1 GET over loopback and read the whole response.
///
/// Returns `None` when the peer accepts the connection but never answers —
/// the exact shape of the regression below, which must not be reported as a
/// hang or a panic.
fn loopback_get(port: u16, target: &str) -> Option<String> {
    use std::io::{Read as _, Write as _};

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(15)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("read timeout");
    stream
        .write_all(
            format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .ok()?;

    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if body_is_complete(&raw) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    if raw.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&raw).into_owned())
}

fn content_length_from_headers(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())?
    })
}

/// True once `raw` holds a full header block plus its declared body.
fn body_is_complete(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let Some(header_end) = text.find("\r\n\r\n") else {
        return false;
    };
    content_length_from_headers(&text[..header_end])
        .is_some_and(|length| raw.len() >= header_end + 4 + length)
}

fn request_is_complete(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let Some(header_end) = text.find("\r\n\r\n") else {
        return false;
    };
    content_length_from_headers(&text[..header_end])
        .is_none_or(|length| raw.len() >= header_end + 4 + length)
}

fn mock_device_token(instance_id: &str) -> String {
    use base64::Engine as _;
    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(
        r#"{{"sub":"device:laptop","device_fp":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","instance_id":"{instance_id}","aud":"spl-relay","scope":"session.dial","iss":"mock-relay","jti":"mock-id","iat":1700000000,"exp":2500000000}}"#
    ));
    format!("{header}.{payload}.sig")
}

fn spawn_mock_relay(enroll_status: Option<u16>, dial_status: u16) -> (String, Arc<AtomicUsize>) {
    use std::io::{Read as _, Write as _};

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind mock relay");
    let port = listener.local_addr().expect("mock relay address").port();
    let enroll_hits = Arc::new(AtomicUsize::new(0));
    let relay_enroll_hits = enroll_hits.clone();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else {
                continue;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("mock relay read timeout");
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !request_is_complete(&raw) {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => raw.extend_from_slice(&chunk[..read]),
                    Err(_) => break,
                }
            }
            let request_line = String::from_utf8_lossy(&raw)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            let response = if request_line.starts_with("POST /enroll/device") {
                relay_enroll_hits.fetch_add(1, Ordering::SeqCst);
                if let Some(status) = enroll_status {
                    let body = r#"{"error":"rejected"}"#;
                    format!(
                        "HTTP/1.1 {status} Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    let token = mock_device_token("home-instance");
                    let body = format!(r#"{{"device_token":"{token}"}}"#);
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                }
            } else if request_line.starts_with("GET /session/dial") {
                format!(
                    "HTTP/1.1 {dial_status} Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string()
            };
            stream
                .write_all(response.as_bytes())
                .expect("write mock relay response");
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });
    (format!("http://127.0.0.1:{port}"), enroll_hits)
}

fn spawn_lan_decoy_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind LAN decoy");
    let port = listener.local_addr().expect("LAN decoy address").port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let decoy_accepted = accepted.clone();
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            if connection.is_ok() {
                decoy_accepted.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    (port, accepted)
}

fn resident_serve_request(
    port: u16,
    policy: LinkServeCarrierPolicy,
    relay_origin: Option<&str>,
    endpoint_port: u16,
) -> LinkServeRequest {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let bundle_dir = PathBuf::from("/var/tmp").join(format!(
        "solstone-resident-serve-{}-{}-{}",
        std::process::id(),
        port,
        count
    ));
    let _ = std::fs::create_dir_all(&bundle_dir);
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client key");
    let params = CertificateParams::new(vec!["client.test".to_string()]).expect("client params");
    let cert = params.self_signed(&key).expect("client cert");
    let request = LinkServeRequest {
        label: "laptop".to_string(),
        port,
        policy,
        relay_origin: relay_origin.map(str::to_string),
        bundle: LinkServeBundle {
            private_key_pem: key.serialize_pem(),
            client_cert_pem: cert.pem(),
            ca_chain_pem: vec![ca_pem()],
            home_attestation: "attestation.jwt".to_string(),
            instance_id: "home-instance".to_string(),
            home_label: "Home".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            endpoints: vec![LinkServeEndpoint {
                host: "127.0.0.1".to_string(),
                port: endpoint_port,
            }],
            local_endpoints: json!([{"ip": "127.0.0.1", "port": 7657}]),
            relay_access: None,
        },
        bundle_dir,
    };
    std::fs::write(
        request.bundle_dir.join("cert.pem"),
        &request.bundle.client_cert_pem,
    )
    .unwrap();
    std::fs::write(
        request.bundle_dir.join("chain.pem"),
        request.bundle.ca_chain_pem.join("\n"),
    )
    .unwrap();
    std::fs::write(
        request.bundle_dir.join("peer.json"),
        json!({"instance_id": request.bundle.instance_id, "paired_at": request.bundle.paired_at})
            .to_string(),
    )
    .unwrap();
    request
}

fn declared_body_length(response: &str) -> usize {
    let header_end = response.find("\r\n\r\n").expect("header terminator");
    content_length_from_headers(&response[..header_end]).expect("Content-Length")
}

#[test]
fn status_request_does_not_open_carrier_but_ordinary_request_does() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let opener = Arc::new(CountingOpener::default());
        let tracker = Arc::new(StatusTracker::new(Arc::new(FixedStatusClock::new(0.0))));
        let handle = journal_bridge::start(JournalBridgeConfig {
            opener: opener.clone(),
            bridge_names: bridge_names(),
            endpoint_hosts: vec!["192.168.1.10".to_string()],
            policy: bridge_policy_for_port(0, tracker),
        })
        .await
        .expect("bridge start");
        let bound = handle.port();

        let (peer, status_response) = http_get(bound, STATUS_PATH).await;
        assert_eq!(
            peer,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bound)
        );
        assert!(status_response.starts_with("HTTP/1.1 200"));
        assert!(status_response.contains("Content-Type: application/json\r\n"));
        assert!(status_response.contains("Content-Length: "));
        let header_end = status_response.find("\r\n\r\n").expect("header terminator");
        assert_eq!(
            declared_body_length(&status_response),
            status_response.len() - header_end - 4
        );
        assert_eq!(opener.dials(), 0);

        let (_peer, ordinary_response) = http_get(bound, "/ordinary").await;
        assert!(ordinary_response.starts_with("HTTP/1.1 502"));
        assert!(opener.dials() >= 1);

        handle.shutdown_and_wait().await;
    });
}

#[test]
fn status_path_in_header_or_query_does_not_skip_the_carrier() {
    // Production `bridge_policy` answers locally only when `head.path() == STATUS_PATH`.
    // `RequestHead::path` is the request target with the query string stripped, and it
    // never inspects headers. So STATUS_PATH text in a query or header cannot take the
    // zero-dial branch; only an exact request-line path match can.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let opener = Arc::new(CountingOpener::default());
        let tracker = Arc::new(StatusTracker::new(Arc::new(FixedStatusClock::new(0.0))));
        let handle = journal_bridge::start(JournalBridgeConfig {
            opener: opener.clone(),
            bridge_names: bridge_names(),
            endpoint_hosts: vec!["192.168.1.10".to_string()],
            policy: bridge_policy_for_port(0, tracker),
        })
        .await
        .expect("bridge start");
        let bound = handle.port();

        let (_peer, query_response) =
            http_get(bound, &format!("/ordinary?x={STATUS_PATH}")).await;
        assert!(query_response.starts_with("HTTP/1.1 502"));
        assert!(opener.dials() >= 1);

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", bound))
            .await
            .expect("connect bridge");
        let request = format!(
            "GET /ordinary HTTP/1.1\r\nHost: 127.0.0.1:{bound}\r\nX-Dummy: {STATUS_PATH}\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write request");
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut response))
            .await
            .expect("response timeout")
            .expect("read response");
        let header_response = String::from_utf8_lossy(&response);
        assert!(header_response.starts_with("HTTP/1.1 502"));
        assert!(opener.dials() >= 2);

        handle.shutdown_and_wait().await;
    });
}

#[test]
fn proxied_response_streams_before_upstream_completion() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let (server_cert, server_key) = self_signed_server();
        let pin = spl_core::ca::sha256(server_cert.as_ref())[..16].to_vec();
        let listener = TokioTcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind transport peer");
        let transport_port = listener.local_addr().expect("transport addr").port();
        let client = Arc::new(
            TransportClient::new(transport_credential(pin, transport_port), None)
                .expect("transport client"),
        );
        let acceptor = TlsAcceptor::from(Arc::new(server_config(server_cert, server_key)));
        let (release_tx, release_rx) = oneshot::channel();
        let released = Arc::new(AtomicBool::new(false));
        let server = tokio::spawn(serve_latched_stream(
            listener,
            acceptor,
            release_rx,
            released.clone(),
        ));
        let tracker = Arc::new(StatusTracker::new(Arc::new(FixedStatusClock::new(0.0))));
        let handle = journal_bridge::start(JournalBridgeConfig {
            opener: Arc::new(TransportClientOpener { client }),
            bridge_names: bridge_names(),
            endpoint_hosts: vec!["127.0.0.1".to_string()],
            policy: bridge_policy_for_port(0, tracker),
        })
        .await
        .expect("bridge start");
        let bound = handle.port();

        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", bound))
            .await
            .expect("connect bridge");
        let request = format!(
            "GET /ordinary HTTP/1.1\r\nHost: 127.0.0.1:{bound}\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write request");

        let mut response = Vec::new();
        let header_end = loop {
            let mut buf = [0u8; 1024];
            let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
                .await
                .expect("response head timeout")
                .expect("read response head");
            assert_ne!(n, 0, "bridge closed before response head");
            response.extend_from_slice(&buf[..n]);
            if let Some(index) = response.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        assert!(String::from_utf8_lossy(&response[..header_end]).starts_with("HTTP/1.1 200"));
        let mut body = response.split_off(header_end);

        // A buffering implementation cannot satisfy this read until the
        // upstream producer emits `B` or closes the response after the latch.
        if body.is_empty() {
            body.resize(1, 0);
            tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut body[..1]))
                .await
                .expect("first streamed body byte timeout")
                .expect("read first streamed body byte");
        }
        assert_eq!(body[0], b'A');
        assert!(!released.load(Ordering::SeqCst));

        release_tx.send(()).expect("release upstream stream");
        tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut body))
            .await
            .expect("response completion timeout")
            .expect("read response completion");
        assert_eq!(body.last().copied(), Some(b'B'));
        assert!(released.load(Ordering::SeqCst));

        server.await.expect("server task");
        handle.shutdown_and_wait().await;
    });
}

#[test]
fn resident_serve_answers_the_local_status_route_while_on_duty() {
    // Regression: `start` spawns the bridge accept loop onto its runtime and
    // `serve` then parks the calling thread in a blocking `ShutdownSignal::wait`
    // for the process's whole lifetime. On a current-thread runtime nothing
    // polls that accept loop until shutdown, so the listener binds — the kernel
    // completes handshakes from the backlog, so the port looks healthy — while
    // every request hangs and returns zero bytes.
    //
    // This must drive the real session lifecycle: `serve` on its own thread and
    // a genuine loopback request. Every other test here drives the bridge under
    // its own `block_on`, which keeps the runtime driven and hides this
    // entirely. No journal, relay, or peer is involved: the status route is
    // answered locally and never forwarded upstream.
    let session = SplLinkServeRunner
        .start(resident_serve_request(
            0,
            LinkServeCarrierPolicy::Direct,
            None,
            unused_loopback_port(),
        ))
        .expect("serve session starts");
    let port = session.bound_port();

    let shutdown = Arc::new(GateShutdown::new());
    let serve_shutdown = Arc::clone(&shutdown);
    let resident = std::thread::spawn(move || session.serve(serve_shutdown.as_ref()));

    let response = loopback_get(port, STATUS_PATH);
    shutdown.release();
    resident
        .join()
        .expect("resident thread")
        .expect("clean shutdown");

    let after = loopback_get(port, STATUS_PATH);
    assert!(
        after.is_none(),
        "status route still answered after cancellation: {after:?}"
    );

    let response = response.expect(
        "status route returned no bytes — the bridge accept loop is not being polled while \
         the resident command is on duty",
    );
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected status response: {response}"
    );
    // `manager_alive` is how `status_body` surfaces the bridge's
    // `listener_active`; true here proves the listener is genuinely on duty
    // and not merely bound.
    assert!(
        response.contains("\"manager_alive\":true"),
        "status payload should report an active listener: {response}"
    );
}

#[test]
fn serve_relay_only_enrollment_failure_makes_no_lan_connection_attempt() {
    let (relay_origin, _enroll_hits) = spawn_mock_relay(Some(401), 503);
    let (decoy_port, decoy_accepted) = spawn_lan_decoy_listener();

    let error = match SplLinkServeRunner.start(resident_serve_request(
        0,
        LinkServeCarrierPolicy::RelayOnly,
        Some(&relay_origin),
        decoy_port,
    )) {
        Ok(_) => panic!("relay-only enrollment rejection must fail before bridge startup"),
        Err(error) => error,
    };

    assert!(matches!(
        error.kind,
        LinkServeErrorKind::Transport(LinkServeTransportErrorKind::RelayControlRejected {
            endpoint: LinkServeRelayControlEndpoint::EnrollDevice,
            status: 401,
        })
    ));
    assert_eq!(decoy_accepted.load(Ordering::SeqCst), 0);
}

#[test]
fn serve_relay_only_dial_failure_after_bridge_startup_leaves_lan_decoy_untouched() {
    let (relay_origin, enroll_hits) = spawn_mock_relay(None, 503);
    let (decoy_port, decoy_accepted) = spawn_lan_decoy_listener();
    let session = SplLinkServeRunner
        .start(resident_serve_request(
            0,
            LinkServeCarrierPolicy::RelayOnly,
            Some(&relay_origin),
            decoy_port,
        ))
        .expect("relay-only bridge starts after enrollment");
    let port = session.bound_port();

    let shutdown = Arc::new(GateShutdown::new());
    let serve_shutdown = Arc::clone(&shutdown);
    let resident = std::thread::spawn(move || session.serve(serve_shutdown.as_ref()));

    let response = loopback_get(port, "/ordinary").expect("ordinary request response");
    assert!(
        response.starts_with("HTTP/1.1 502"),
        "unexpected ordinary response: {response}"
    );

    shutdown.release();
    resident
        .join()
        .expect("resident thread")
        .expect("clean shutdown");

    assert_eq!(decoy_accepted.load(Ordering::SeqCst), 0);
    assert!(enroll_hits.load(Ordering::SeqCst) >= 1);
}

fn multipart_ingest_body() -> (String, Vec<u8>) {
    let boundary = "ingest-test-boundary";
    let envelope = r#"{"day":"20260815","segment":"143000_1","files":[{"submitted":"a.bin"},{"submitted":"b.bin"}]}"#;
    let file_a = vec![0x11u8; 5 * 1024 * 1024];
    let file_b = vec![0x22u8; 5 * 1024 * 1024];
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"envelope\"\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(envelope.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"a.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&file_a);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"b.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&file_b);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary.to_string(), body)
}

#[test]
fn v3_multipart_ingest_request_reaches_carrier_unchanged() {
    block_on(async {
        let (boundary, body) = multipart_ingest_body();
        assert!(
            body.len() > 9 * 1024 * 1024,
            "multipart fixture must exceed the retired eight-mebibyte ceiling"
        );
        let (handle, port, server) = start_bridge_with_capture(json_ok()).await;
        let request = format!(
            "POST /app/devices/ingest?source=test HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nX-Solstone-Protocol-Version: 3\r\nX-Test-Marker: abc123\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let mut raw = request.into_bytes();
        raw.extend_from_slice(&body);
        let client_raw = http_exchange(port, &raw).await;
        let captured = server.await.expect("capture task");
        handle.shutdown_and_wait().await;

        assert_eq!(captured.head.method, "POST");
        assert_eq!(captured.head.path(), "/app/devices/ingest");
        assert_eq!(captured.head.query(), Some("source=test"));
        assert_eq!(
            header_values(&captured.head, "x-solstone-protocol-version"),
            vec!["3"]
        );
        assert!(header_values(&captured.head, "x-solstone-observer").is_empty());
        assert_eq!(
            header_values(&captured.head, "x-test-marker"),
            vec!["abc123"]
        );
        assert_eq!(captured.body, body);

        let (status, headers, response_body) = parse_client_http(&client_raw);
        assert_eq!(status, 200);
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "etag" && value == "fixture-1")
        );
        assert_eq!(response_body, b"{\"status\":\"ok\"}");
    });
}

#[test]
fn body_size_at_boundary_reaches_carrier_unchanged() {
    block_on(async {
        let (handle, port, server) = start_bridge_with_capture(json_ok()).await;
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect bridge");
        let head = format!(
            "POST /app/devices/ingest HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/octet-stream\r\nContent-Length: {CONNECTION_BODY_LIMIT}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).await.expect("write head");
        let chunk = [0x5Au8; 65536];
        let mut remaining = CONNECTION_BODY_LIMIT;
        while remaining > 0 {
            let n = remaining.min(chunk.len());
            stream
                .write_all(&chunk[..n])
                .await
                .expect("write body chunk");
            remaining -= n;
        }
        let mut client_raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(60), stream.read_to_end(&mut client_raw))
            .await
            .expect("response timeout")
            .expect("read response");
        let captured = server.await.expect("capture task");
        handle.shutdown_and_wait().await;

        assert_eq!(captured.content_length, CONNECTION_BODY_LIMIT);
        assert_eq!(captured.body.len(), CONNECTION_BODY_LIMIT);
        assert_eq!(captured.body.first().copied(), Some(0x5A));
        assert_eq!(captured.body.last().copied(), Some(0x5A));
        let checksum = captured
            .body
            .iter()
            .fold(0u64, |acc, byte| acc.wrapping_add(u64::from(*byte)));
        assert_eq!(checksum, 0x5A * CONNECTION_BODY_LIMIT as u64);
        assert!(parse_client_http(&client_raw).0 == 200);
    });
}

#[test]
fn body_over_limit_by_one_byte_rejected_pre_dial() {
    block_on(async {
        let (handle, port, opener) = start_counting_bridge().await;
        let request = format!(
            "POST /ordinary HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            CONNECTION_BODY_LIMIT + 1
        );
        let client_raw = http_exchange(port, request.as_bytes()).await;
        assert!(String::from_utf8_lossy(&client_raw).starts_with("HTTP/1.1 413"));
        assert_eq!(opener.dials(), 0);
        handle.shutdown_and_wait().await;
    });
}

#[test]
fn body_with_absent_content_length_treated_as_empty() {
    block_on(async {
        let (handle, port, server) = start_bridge_with_capture(json_ok()).await;
        let request = format!(
            "POST /ordinary HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Test-Marker: nosmuggle\r\nConnection: close\r\n\r\nSMUGGLE"
        );
        let client_raw = http_exchange(port, request.as_bytes()).await;
        let captured = server.await.expect("capture task");
        handle.shutdown_and_wait().await;

        let (status, _, _) = parse_client_http(&client_raw);
        assert_ne!(status, 413);
        assert_eq!(status, 200);
        assert_eq!(captured.content_length, 0);
        assert!(captured.body.is_empty());
        assert_eq!(
            header_values(&captured.head, "x-test-marker"),
            vec!["nosmuggle"]
        );
    });
}

#[test]
fn duplicate_content_length_rejected_pre_dial() {
    block_on(async {
        let (handle, port, opener) = start_counting_bridge().await;
        let request = format!(
            "POST /ordinary HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n"
        );
        let client_raw = http_exchange(port, request.as_bytes()).await;
        assert!(String::from_utf8_lossy(&client_raw).starts_with("HTTP/1.1 400"));
        assert_eq!(opener.dials(), 0);
        handle.shutdown_and_wait().await;
    });
}

async fn assert_get_round_trip(target: &str, status: u16, body: &[u8]) {
    let scripted = ScriptedHttpResponse {
        status,
        headers: vec![
            ("etag".to_string(), "fixture-1".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ],
        body: body.to_vec(),
    };
    let (handle, port, server) = start_bridge_with_capture(scripted).await;
    let request = format!(
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Solstone-Protocol-Version: 3\r\nConnection: close\r\n\r\n"
    );
    let client_raw = http_exchange(port, request.as_bytes()).await;
    let captured = server.await.expect("capture task");
    handle.shutdown_and_wait().await;

    assert_eq!(captured.head.method, "GET");
    let (path, query) = target
        .split_once('?')
        .map(|(path, query)| (path, Some(query)))
        .unwrap_or((target, None));
    assert_eq!(captured.head.path(), path);
    assert_eq!(captured.head.query(), query);
    assert_eq!(
        header_values(&captured.head, "x-solstone-protocol-version"),
        vec!["3"]
    );

    let (got_status, headers, got_body) = parse_client_http(&client_raw);
    assert_eq!(got_status, status);
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "etag" && value == "fixture-1")
    );
    assert_eq!(got_body, body);
}

const FILE_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn get_ingest_manifest_success_and_rejection_round_trip() {
    block_on(async {
        let ok = br#"{"days":{"20260815":{"segments":3}}}"#;
        let denied = br#"{"error":"linked device not authorized"}"#;
        assert_get_round_trip("/app/devices/ingest/manifest", 200, ok).await;
        assert_get_round_trip("/app/devices/ingest/manifest", 403, denied).await;
    });
}

#[test]
fn get_ingest_manifest_day_success_and_rejection_round_trip() {
    block_on(async {
        let ok = format!(
            r#"{{"version":1,"day":"20260815","segments":{{"143000":{{"files":[{{"name":"audio.m4a","size":4096,"sha256":"{FILE_SHA256}","status":"present"}}]}}}}}}"#
        );
        let missing = br#"{"error":"day not found"}"#;
        assert_get_round_trip("/app/devices/ingest/manifest/20260815", 200, ok.as_bytes()).await;
        assert_get_round_trip("/app/devices/ingest/manifest/20260815", 404, missing).await;
    });
}

#[test]
fn get_ingest_segments_success_and_rejection_round_trip() {
    block_on(async {
        let ok = format!(
            r#"{{"protocol_version":3,"total":1,"items":[{{"key":"20260815/143000","observed":true,"files":[{{"name":"audio.m4a","size":4096,"sha256":"{FILE_SHA256}","status":"present"}}]}}]}}"#
        );
        let denied = br#"{"error":"linked device not authorized"}"#;
        assert_get_round_trip(
            "/app/devices/ingest/segments/20260815?source=test",
            200,
            ok.as_bytes(),
        )
        .await;
        assert_get_round_trip("/app/devices/ingest/segments/20260815", 403, denied).await;
    });
}

fn metadata_reply(revision: u64) -> serde_json::Value {
    json!({"protocol_version":1,"revision":revision,"reported":null,"owner_label":"Desk",
        "display_label":"Desk","updated_at":null,"journal":{"name":"Home","version":"2.0.0"}})
}

fn metadata_http_exchange(
    listener: &std::net::TcpListener,
    status: u16,
    reply: serde_json::Value,
    before_reply: impl FnOnce(),
) -> (String, serde_json::Value) {
    use std::io::{Read, Write};
    let (mut stream, _) = listener.accept().expect("accept metadata request");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("read bound");
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("write bound");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("request header");
        head.push(byte[0]);
        assert!(head.len() < 8192);
    }
    let head = String::from_utf8(head).expect("header");
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("length"))
        })
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).expect("body");
    before_reply();
    let reply = reply.to_string();
    write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).expect("response");
    (
        head.lines().next().expect("request line").to_owned(),
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

#[test]
fn metadata_conflict_resamples_latest_description_through_actual_http() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().unwrap().port();
    let newest = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let server_newest = newest.clone();
    let server = std::thread::spawn(move || {
        assert!(
            metadata_http_exchange(&listener, 200, metadata_reply(1), || {})
                .0
                .starts_with("GET ")
        );
        let first = metadata_http_exchange(&listener, 409, json!({}), || {
            server_newest.store(1, std::sync::atomic::Ordering::SeqCst);
        });
        assert!(first.0.starts_with("PUT "));
        assert_eq!(first.1["reported"]["name"], "Old");
        assert_eq!(first.1["expected_revision"], 1);
        metadata_http_exchange(&listener, 200, metadata_reply(2), || {});
        let second = metadata_http_exchange(&listener, 200, metadata_reply(3), || {});
        assert_eq!(second.1["reported"]["name"], "New");
        assert_eq!(second.1["expected_revision"], 2);
        assert!(second.1.get("owner_label").is_none());
    });
    let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    publish_device_description_with(tracker, port, 0, || ReportedDescription {
        name: Some(
            if newest.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                "Old"
            } else {
                "New"
            }
            .into(),
        ),
        ..Default::default()
    });
    server.join().expect("server");
}

#[test]
fn metadata_get_cannot_publish_after_generation_change_or_bad_version() {
    for invalidate_generation in [true, false] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let port = listener.local_addr().unwrap().port();
        let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
        let server_tracker = tracker.clone();
        let mut reply = metadata_reply(0);
        if !invalidate_generation {
            reply["protocol_version"] = json!(2);
        }
        let server = std::thread::spawn(move || {
            metadata_http_exchange(&listener, 200, reply, || {
                if invalidate_generation {
                    server_tracker.bump_generation_for_test();
                }
            });
            listener
        });
        publish_device_description(tracker.clone(), port, 0);
        let listener = server.join().expect("server");
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "no stale PUT"
        );
    }
}

fn test_bundle_and_store(
    label: &str,
) -> (
    PathBuf,
    LinkCredentialStore,
    PairingIdentity,
    LinkServeBundle,
    Vec<u8>,
) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = PathBuf::from("/var/tmp").join(format!(
        "solstone-link-serving-{}-{}-{}",
        label,
        std::process::id(),
        count
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create bundle dir");

    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client key");
    let params = CertificateParams::new(vec!["laptop.test".to_string()]).expect("client params");
    let cert = params.self_signed(&key).expect("client cert");
    let ca_cert_pem = ca_pem();

    std::fs::write(path.join("cert.pem"), cert.pem()).expect("cert");
    std::fs::write(path.join("chain.pem"), &ca_cert_pem).expect("chain");
    std::fs::write(
        path.join("peer.json"),
        serde_json::json!({
            "instance_id": "test-home-instance",
            "home_label": "Home",
            "paired_at": "2026-07-26T00:00:00Z"
        })
        .to_string(),
    )
    .expect("peer");

    let store = LinkCredentialStore::new(path.clone(), label);
    let der = pem_cert_der(&ca_cert_pem).unwrap();
    let ca_fp = der.clone();
    let ca_fingerprint = format!("sha256:{}", spl_core::ca::sha256_hex(&der));
    let cert_sha256 = format!("sha256:{}", spl_core::ca::sha256_hex(cert.pem().as_bytes()));
    let identity = PairingIdentity {
        cert_sha256,
        instance_id: "test-home-instance".to_string(),
        ca_fingerprint,
    };

    let bundle = LinkServeBundle {
        private_key_pem: key.serialize_pem(),
        client_cert_pem: cert.pem(),
        ca_chain_pem: vec![ca_cert_pem],
        home_attestation: "test_jwt".to_string(),
        instance_id: "test-home-instance".to_string(),
        home_label: "Home".to_string(),
        paired_at: "2026-07-26T00:00:00Z".to_string(),
        endpoints: Vec::new(),
        local_endpoints: serde_json::json!([]),
        relay_access: None,
    };

    (path, store, identity, bundle, ca_fp)
}

fn access_http_exchange(
    listener: &std::net::TcpListener,
    status: u16,
    headers: &[(&str, &str)],
    body: &[u8],
) -> String {
    use std::io::{Read, Write};
    let (mut stream, _) = listener.accept().expect("accept access request");
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("read bound");
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("write bound");
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("request header");
        head.push(byte[0]);
        assert!(head.len() < 8192);
    }
    let head = String::from_utf8(head).expect("header");
    let mut extra = String::new();
    for (k, v) in headers {
        extra.push_str(&format!("{k}: {v}\r\n"));
    }
    write!(
        stream,
        "HTTP/1.1 {status} Status\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write headers");
    stream.write_all(body).expect("write body");
    head.lines().next().expect("request line").to_owned()
}

#[allow(clippy::too_many_arguments)]
fn test_scheduler(
    tracker: Arc<StatusTracker>,
    client_manager: Arc<CurrentClientManager>,
    store: LinkCredentialStore,
    identity: PairingIdentity,
    policy: LinkServeCarrierPolicy,
    configured_relay_origin: Option<String>,
    bundle: LinkServeBundle,
    ca_fp_prefix: Vec<u8>,
) -> Arc<OptionalJobScheduler> {
    Arc::new(OptionalJobScheduler::new_for_test(JobSchedulerTestParams {
        tracker,
        client_manager,
        store,
        identity,
        policy,
        configured_relay_origin,
        bundle,
        ca_fp_prefix,
    }))
}

#[test]
fn access_redirect_is_refused_preserving_current_access() {
    let (_bundle_dir, store, identity, bundle, ca_fp) = test_bundle_and_store("laptop-redirect");
    store
        .publish_ready(
            "https://old.relay.app",
            "initial_token",
            2000000000,
            &identity,
        )
        .unwrap();
    let initial_outcome = store.load_access();
    assert!(matches!(initial_outcome, StoreLoadOutcome::Ready(_)));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().unwrap().port();
    let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    let client_mgr = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        tracker,
        client_mgr,
        store.clone(),
        identity,
        LinkServeCarrierPolicy::RelayPermitted,
        None,
        bundle,
        ca_fp,
    );

    let server = std::thread::spawn(move || {
        access_http_exchange(
            &listener,
            302,
            &[("Location", "http://127.0.0.1:9999/other")],
            b"",
        );
    });

    scheduler.run_access_job(port, 0);
    server.join().expect("server");

    assert_eq!(store.load_access(), initial_outcome);
}

#[test]
fn access_body_overflow_preserves_current_access() {
    let (_bundle_dir, store, identity, bundle, ca_fp) = test_bundle_and_store("laptop-overflow");
    store
        .publish_ready(
            "https://old.relay.app",
            "initial_token",
            2000000000,
            &identity,
        )
        .unwrap();
    let initial_outcome = store.load_access();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().unwrap().port();
    let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    let client_mgr = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        tracker,
        client_mgr,
        store.clone(),
        identity,
        LinkServeCarrierPolicy::RelayPermitted,
        None,
        bundle,
        ca_fp,
    );

    let oversize = vec![b'A'; 65537];
    let server = std::thread::spawn(move || {
        access_http_exchange(
            &listener,
            200,
            &[("Content-Type", "application/json")],
            &oversize,
        );
    });

    scheduler.run_access_job(port, 0);
    server.join().expect("server");

    assert_eq!(store.load_access(), initial_outcome);
}

#[test]
fn access_stall_respects_deadline_and_preserves_access() {
    let (_bundle_dir, store, identity, bundle, ca_fp) = test_bundle_and_store("laptop-deadline");
    store
        .publish_ready(
            "https://old.relay.app",
            "initial_token",
            2000000000,
            &identity,
        )
        .unwrap();
    let initial_outcome = store.load_access();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().unwrap().port();
    let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    let client_mgr = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        tracker,
        client_mgr,
        store.clone(),
        identity,
        LinkServeCarrierPolicy::RelayPermitted,
        None,
        bundle,
        ca_fp,
    );

    let server = std::thread::spawn(move || {
        use std::io::Read;
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            std::thread::sleep(Duration::from_millis(500));
        }
    });

    scheduler.run_access_job(port, 0);
    assert_eq!(store.load_access(), initial_outcome);
    server.join().expect("server");
}

#[test]
fn access_wrong_instance_protocol_or_non_200_preserves_current() {
    let (_bundle_dir, store, identity, bundle, ca_fp) = test_bundle_and_store("laptop-wrong-inst");
    store
        .publish_ready(
            "https://old.relay.app",
            "initial_token",
            2000000000,
            &identity,
        )
        .unwrap();
    let initial_outcome = store.load_access();

    // 1. Wrong instance ID
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let port = listener.local_addr().unwrap().port();
        let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
        let client_mgr = Arc::new(CurrentClientManager::new(None));
        let scheduler = test_scheduler(
            tracker,
            client_mgr,
            store.clone(),
            identity.clone(),
            LinkServeCarrierPolicy::RelayPermitted,
            None,
            bundle.clone(),
            ca_fp.clone(),
        );
        let token = mock_device_token("wrong-instance-xyz");
        let wrong_instance_body = serde_json::to_vec(&json!({
            "status": "ready",
            "protocol_version": 2,
            "relay_origin": "https://new.relay.app",
            "instance_id": "wrong-instance-xyz",
            "device_token": token,
            "expires_at": "2500000000",
        }))
        .unwrap();
        let server = std::thread::spawn(move || {
            access_http_exchange(
                &listener,
                200,
                &[("Content-Type", "application/json")],
                &wrong_instance_body,
            );
        });
        scheduler.run_access_job(port, 0);
        server.join().expect("server");
        assert_eq!(store.load_access(), initial_outcome);
    }

    // 2. Wrong protocol_version (e.g. 1)
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let port = listener.local_addr().unwrap().port();
        let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
        let client_mgr = Arc::new(CurrentClientManager::new(None));
        let scheduler = test_scheduler(
            tracker,
            client_mgr,
            store.clone(),
            identity.clone(),
            LinkServeCarrierPolicy::RelayPermitted,
            None,
            bundle.clone(),
            ca_fp.clone(),
        );
        let token = mock_device_token(&identity.instance_id);
        let wrong_pv_body = serde_json::to_vec(&json!({
            "status": "ready",
            "protocol_version": 1,
            "relay_origin": "https://new.relay.app",
            "instance_id": identity.instance_id,
            "device_token": token,
            "expires_at": "2500000000",
        }))
        .unwrap();
        let server = std::thread::spawn(move || {
            access_http_exchange(
                &listener,
                200,
                &[("Content-Type", "application/json")],
                &wrong_pv_body,
            );
        });
        scheduler.run_access_job(port, 0);
        server.join().expect("server");
        assert_eq!(store.load_access(), initial_outcome);
    }

    // 3. HTTP 503 with ready/not_configured body is a failure, must preserve
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let port = listener.local_addr().unwrap().port();
        let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
        let client_mgr = Arc::new(CurrentClientManager::new(None));
        let scheduler = test_scheduler(
            tracker,
            client_mgr,
            store.clone(),
            identity.clone(),
            LinkServeCarrierPolicy::RelayPermitted,
            None,
            bundle.clone(),
            ca_fp.clone(),
        );
        let body_503 = serde_json::to_vec(&json!({
            "status": "not_configured",
            "protocol_version": 2,
        }))
        .unwrap();
        let server = std::thread::spawn(move || {
            access_http_exchange(
                &listener,
                503,
                &[("Content-Type", "application/json")],
                &body_503,
            );
        });
        scheduler.run_access_job(port, 0);
        server.join().expect("server");
        assert_eq!(store.load_access(), initial_outcome);
    }
}

#[test]
fn access_old_home_404_preserves_access_and_lan_no_enroll() {
    let (_bundle_dir, store, identity, bundle, ca_fp) =
        test_bundle_and_store("laptop-old-home-404");
    store
        .publish_ready(
            "https://old.relay.app",
            "initial_token",
            2000000000,
            &identity,
        )
        .unwrap();
    let initial_outcome = store.load_access();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().unwrap().port();
    let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    let client_mgr = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        tracker,
        client_mgr,
        store.clone(),
        identity,
        LinkServeCarrierPolicy::RelayPermitted,
        None,
        bundle,
        ca_fp,
    );

    let server = std::thread::spawn(move || {
        access_http_exchange(
            &listener,
            404,
            &[("Content-Type", "application/json")],
            b"{\"error\":\"not found\"}",
        );
    });

    scheduler.run_access_job(port, 0);
    server.join().expect("server");

    assert_eq!(store.load_access(), initial_outcome);
}

#[test]
fn stall_access_while_metadata_and_ordinary_traffic_progress() {
    let (_bundle_dir, store, identity, bundle, ca_fp) = test_bundle_and_store("laptop-stall-lanes");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().unwrap().port();

    let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    let client_mgr = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        tracker.clone(),
        client_mgr,
        store.clone(),
        identity,
        LinkServeCarrierPolicy::RelayPermitted,
        None,
        bundle,
        ca_fp,
    );

    let access_unblock = Arc::new(AtomicBool::new(false));
    let access_unblock_server = access_unblock.clone();
    let metadata_completed = Arc::new(AtomicBool::new(false));
    let metadata_completed_flag = metadata_completed.clone();

    let server = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream1, _) = listener.accept().expect("accept 1");
        let mut head1 = Vec::new();
        while !head1.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            stream1.read_exact(&mut b).unwrap();
            head1.push(b[0]);
        }
        let req1 = String::from_utf8(head1).unwrap();

        let (mut stream2, _) = listener.accept().expect("accept 2");
        let mut head2 = Vec::new();
        while !head2.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            stream2.read_exact(&mut b).unwrap();
            head2.push(b[0]);
        }
        let _req2 = String::from_utf8(head2).unwrap();

        let (mut access_stream, mut meta_stream) = if req1.contains("/relay/access") {
            (stream1, stream2)
        } else {
            (stream2, stream1)
        };

        let reply = metadata_reply(1).to_string();
        write!(
            meta_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        )
        .unwrap();
        metadata_completed_flag.store(true, Ordering::SeqCst);

        while !access_unblock_server.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        write!(
            access_stream,
            "HTTP/1.1 503 Busy\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });

    let sched = scheduler.clone();
    let burst = std::thread::spawn(move || {
        std::thread::scope(|s| {
            s.spawn(|| sched.run_access_job(port, 0));
            s.spawn(|| sched.run_metadata_job(port, 0));
        });
    });

    let start = std::time::Instant::now();
    while !metadata_completed.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        metadata_completed.load(Ordering::SeqCst),
        "metadata progressed while access was stalled"
    );

    access_unblock.store(true, Ordering::SeqCst);
    burst.join().expect("burst join");
    server.join().expect("server join");
}

#[test]
fn combined_lanes_quiesce_with_carrier_close_and_subsequent_trigger_processes_latest_pending() {
    let (_bundle_dir, store, identity, bundle, ca_fp) =
        test_bundle_and_store("laptop-quiesce-pending");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port = listener.local_addr().unwrap().port();

    let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    let client_mgr = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        tracker.clone(),
        client_mgr,
        store.clone(),
        identity.clone(),
        LinkServeCarrierPolicy::RelayPermitted,
        None,
        bundle,
        ca_fp,
    );

    scheduler.retire();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    scheduler.trigger(runtime.handle(), port, 1);

    let (_bundle_dir2, store2, identity2, bundle2, ca_fp2) =
        test_bundle_and_store("laptop-coalesce");
    let listener2 = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
    let port2 = listener2.local_addr().unwrap().port();
    let tracker2 = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
    let client_mgr2 = Arc::new(CurrentClientManager::new(None));
    let scheduler2 = test_scheduler(
        tracker2,
        client_mgr2,
        store2.clone(),
        identity2,
        LinkServeCarrierPolicy::RelayPermitted,
        None,
        bundle2,
        ca_fp2,
    );

    let first_access_received = Arc::new(AtomicBool::new(false));
    let first_access_received_flag = first_access_received.clone();
    let access_count = Arc::new(AtomicUsize::new(0));
    let access_count_flag = access_count.clone();

    let server2 = std::thread::spawn(move || {
        use std::io::{Read, Write};
        while let Ok((mut stream, _)) = listener2.accept() {
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                if stream.read_exact(&mut b).is_err() {
                    break;
                }
                head.push(b[0]);
            }
            if head.is_empty() {
                continue;
            }
            let req = String::from_utf8_lossy(&head);
            if req.contains("/relay/access") {
                let c = access_count_flag.fetch_add(1, Ordering::SeqCst);
                first_access_received_flag.store(true, Ordering::SeqCst);
                let body = serde_json::to_vec(&json!({
                    "status": "not_configured",
                    "protocol_version": 2,
                }))
                .unwrap();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
                if c >= 1 {
                    break;
                }
            } else {
                let body = metadata_reply(1).to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        }
    });

    let sched2 = scheduler2.clone();
    let burst_thread = std::thread::spawn(move || {
        sched2.run_burst(port2);
    });

    let start = std::time::Instant::now();
    while !first_access_received.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(3)
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    scheduler2.trigger(runtime.handle(), port2, 2);

    burst_thread.join().expect("burst join");
    server2.join().expect("server join");

    assert!(access_count.load(Ordering::SeqCst) >= 2);
}

fn access_v2_reply(identity: &PairingIdentity, marker: &str) -> serde_json::Value {
    use base64::Engine as _;
    let claims = json!({"iss":"independent-issuer", "sub":format!("instance:{}", identity.instance_id), "aud":"spl-relay", "scope":"session.dial", "ver":2,
        "instance_id":identity.instance_id, "iat":1700000000i64, "exp":2500000000i64, "jti":marker});
    let token = format!(
        "e30.{}.sig",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    json!({"protocol_version":2,"status":"ready","instance_id":identity.instance_id,"relay_origin":"https://relay.example", "device_token":token,"expires_at":"2049-03-22T04:26:40Z"})
}

fn access_exchange_with(
    scheduler: &OptionalJobScheduler,
    status: u16,
    reply: serde_json::Value,
    before_reply: impl FnOnce() + Send + 'static,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        metadata_http_exchange(&listener, status, reply, before_reply);
    });
    scheduler.run_access_job(port, 0);
    server.join().unwrap();
}

#[test]
fn access_missing_protocol_disable_and_invalid_origin_preserve_current_revision() {
    let (path, store, identity, bundle, ca_fp) = test_bundle_and_store("strict-current");
    store
        .publish_ready("https://relay.example", "old", 2500000000, &identity)
        .unwrap();
    let old = store.load_access();
    let manager = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        Arc::new(StatusTracker::new(Arc::new(SystemStatusClock))),
        manager.clone(),
        store.clone(),
        identity.clone(),
        LinkServeCarrierPolicy::RelayOnly,
        None,
        bundle,
        ca_fp,
    );
    let mut invalid_origin = access_v2_reply(&identity, "bad-origin");
    invalid_origin["relay_origin"] = json!("https://user:secret@relay.example/path");
    for response in [
        json!({"status":"not_configured"}),
        json!({"status":"not_configured","protocol_version":null}),
        json!({"status":"not_configured","protocol_version":1}),
        invalid_origin,
    ] {
        access_exchange_with(&scheduler, 200, response, || {});
        assert_eq!(store.load_access(), old);
        assert_eq!(manager.incarnation(), 0);
    }
    access_exchange_with(
        &scheduler,
        200,
        json!({"status":"not_configured","protocol_version":2}),
        || {},
    );
    assert!(matches!(store.load_access(), StoreLoadOutcome::Disabled(_)));
    assert_eq!(manager.incarnation(), 1);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn access_http_completion_cannot_overwrite_successor_or_publish_after_shutdown() {
    for (label, shutdown, disabled) in [
        ("ready-successor", false, false),
        ("disable-successor", false, true),
        ("shutdown", true, false),
    ] {
        let (path, store, identity, bundle, ca_fp) = test_bundle_and_store(label);
        store
            .publish_ready("https://relay.example", "old", 2500000000, &identity)
            .unwrap();
        let manager = Arc::new(CurrentClientManager::new(None));
        let scheduler = test_scheduler(
            Arc::new(StatusTracker::new(Arc::new(SystemStatusClock))),
            manager.clone(),
            store.clone(),
            identity.clone(),
            LinkServeCarrierPolicy::RelayOnly,
            None,
            bundle,
            ca_fp,
        );
        let response = if disabled {
            json!({"status":"not_configured","protocol_version":2})
        } else {
            access_v2_reply(&identity, "late")
        };
        let writer = store.clone();
        let writer_identity = identity.clone();
        let retiring = manager.clone();
        access_exchange_with(&scheduler, 200, response, move || {
            if shutdown {
                retiring.retire();
            } else {
                writer
                    .publish_ready(
                        "https://relay.example",
                        "successor",
                        2500000000,
                        &writer_identity,
                    )
                    .unwrap();
            }
        });
        let StoreLoadOutcome::Ready(record) = store.load_access() else {
            panic!("ready preserved");
        };
        assert_eq!(
            record.device_token.as_deref(),
            Some(if shutdown { "old" } else { "successor" })
        );
        assert_eq!(manager.incarnation(), u64::from(shutdown));
        std::fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(feature = "access-test-hooks")]
#[test]
fn ready_requires_durability_and_uncertain_commit_reconciles_without_rewrite() {
    use solstone_core_sol_client::link_credentials::{StoreWriteFault, get_file_dev_ino};
    for fault in [
        StoreWriteFault::BeforeRename,
        StoreWriteFault::BundleSync,
        StoreWriteFault::ParentSync,
    ] {
        let (path, store, identity, bundle, ca_fp) = test_bundle_and_store("ready-durability");
        store
            .publish_ready("https://relay.example", "old", 2500000000, &identity)
            .unwrap();
        let manager = Arc::new(CurrentClientManager::new(None));
        let scheduler = test_scheduler(
            Arc::new(StatusTracker::new(Arc::new(SystemStatusClock))),
            manager.clone(),
            store.clone(),
            identity.clone(),
            LinkServeCarrierPolicy::RelayOnly,
            None,
            bundle,
            ca_fp,
        );
        store.inject_write_fault(fault);
        access_exchange_with(&scheduler, 200, access_v2_reply(&identity, "new"), || {});
        assert_eq!(
            manager.incarnation(),
            0,
            "uncommitted access cannot become live"
        );
        let StoreLoadOutcome::Ready(record) = store.load_access() else {
            panic!("ready");
        };
        if matches!(fault, StoreWriteFault::BeforeRename) {
            assert_eq!(record.device_token.as_deref(), Some("old"));
            assert!(!manager.persistence_uncertain_for_test());
        } else {
            assert_ne!(record.device_token.as_deref(), Some("old"));
            assert!(manager.persistence_uncertain_for_test());
            let inode = get_file_dev_ino(&path.join("relay_access.json")).unwrap();
            access_exchange_with(&scheduler, 503, json!({}), || {});
            assert_eq!(manager.incarnation(), 1);
            assert!(manager.get().0.is_some());
            assert!(!manager.persistence_uncertain_for_test());
            assert_eq!(
                get_file_dev_ino(&path.join("relay_access.json")).unwrap(),
                inode
            );
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(feature = "access-test-hooks")]
#[test]
fn failed_clear_cannot_erase_later_ready_and_refresh_failure_retires_only_its_incarnation() {
    use solstone_core_sol_client::link_credentials::StoreWriteFault;
    let (path, store, identity, bundle, ca_fp) = test_bundle_and_store("clear-refresh-order");
    store
        .publish_ready("https://relay.example", "old", 2500000000, &identity)
        .unwrap();
    let manager = Arc::new(CurrentClientManager::new(None));
    let scheduler = test_scheduler(
        Arc::new(StatusTracker::new(Arc::new(SystemStatusClock))),
        manager.clone(),
        store.clone(),
        identity.clone(),
        LinkServeCarrierPolicy::RelayOnly,
        None,
        bundle,
        ca_fp,
    );
    store.inject_write_fault(StoreWriteFault::BeforeRename);
    access_exchange_with(
        &scheduler,
        200,
        json!({"status":"not_configured","protocol_version":2}),
        || {},
    );
    assert_eq!(manager.incarnation(), 1);
    assert!(manager.persistence_uncertain_for_test());
    store
        .publish_ready(
            "https://relay.example",
            "external-successor",
            2500000000,
            &identity,
        )
        .unwrap();
    access_exchange_with(&scheduler, 503, json!({}), || {});
    let StoreLoadOutcome::Ready(record) = store.load_access() else {
        panic!("successor");
    };
    assert_eq!(record.device_token.as_deref(), Some("external-successor"));
    access_exchange_with(&scheduler, 200, access_v2_reply(&identity, "live"), || {});
    let hook = manager.hook_for_test(&store, &identity, "https://relay.example");
    let token = access_v2_reply(&identity, "refresh")["device_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let incarnation = manager.incarnation();
    store.inject_write_fault(StoreWriteFault::BeforeRename);
    hook(&token, 2500000000);
    assert_eq!(manager.incarnation(), incarnation + 1);
    assert!(manager.get().0.is_none());
    access_exchange_with(
        &scheduler,
        200,
        access_v2_reply(&identity, "replacement"),
        || {},
    );
    let successor = store.load_access();
    let successor_incarnation = manager.incarnation();
    hook(&token, 2500000000);
    assert_eq!(store.load_access(), successor);
    assert_eq!(manager.incarnation(), successor_incarnation);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn metadata_write_failure_is_honest_and_full_null_clears_only_full_response_name() {
    use solstone_core_sol_client::seam::LinkJournalMetadata;
    for fail in [false, true] {
        let (path, _store, identity, bundle, _ca_fp) =
            test_bundle_and_store("metadata-real-writer");
        let ca_prefix = identity
            .ca_fingerprint
            .strip_prefix("sha256:")
            .unwrap()
            .to_owned();
        let tracker = Arc::new(StatusTracker::with_metadata(
            Arc::new(SystemStatusClock),
            path.clone(),
            identity.instance_id.clone(),
            ca_prefix.clone(),
            bundle.paired_at.clone(),
            None,
            false,
        ));
        let meta = LinkJournalMetadata {
            instance_id: identity.instance_id,
            ca_fp_prefix: ca_prefix,
            paired_at: bundle.paired_at,
            journal_version: "1.0.0".into(),
            journal_name: Some("Old Home".into()),
            observed_at: 0.0,
        };
        if fail {
            std::fs::create_dir(path.join("journal_metadata.json")).unwrap();
        } else {
            std::fs::write(
                path.join("journal_metadata.json"),
                serde_json::to_vec(&meta).unwrap(),
            )
            .unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut response = metadata_reply(1);
        response["journal"]["name"] = json!(null);
        if !fail {
            response["reported"] = serde_json::to_value(ReportedDescription::default()).unwrap();
        }
        let server = std::thread::spawn(move || {
            metadata_http_exchange(&listener, 200, response, || {});
            listener
        });
        publish_device_description_with(tracker.clone(), port, 0, ReportedDescription::default);
        let listener = server.join().unwrap();
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "no PUT after failed write or exact match"
        );
        if !fail {
            let saved: LinkJournalMetadata =
                serde_json::from_slice(&std::fs::read(path.join("journal_metadata.json")).unwrap())
                    .unwrap();
            assert_eq!(saved.journal_name, None);
            assert_eq!(saved.journal_version, "2.0.0");
            // A legacy version-only response preserves a name already cached.
            std::fs::write(
                path.join("journal_metadata.json"),
                serde_json::to_vec(&meta).unwrap(),
            )
            .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                metadata_http_exchange(&listener, 404, json!({}), || {});
                let request = metadata_http_exchange(
                    &listener,
                    200,
                    json!({"version":{"current":"3.0.0"}}),
                    || {},
                );
                assert!(request.0.contains("/api/system/status"));
            });
            publish_device_description(tracker, port, 0);
            server.join().unwrap();
            let saved: LinkJournalMetadata =
                serde_json::from_slice(&std::fs::read(path.join("journal_metadata.json")).unwrap())
                    .unwrap();
            assert_eq!(saved.journal_name.as_deref(), Some("Old Home"));
            assert_eq!(saved.journal_version, "3.0.0");
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(feature = "access-test-hooks")]
#[test]
fn actual_opener_drops_late_tls_success_and_failure_after_replacement() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for succeed in [true, false] {
            let (server_cert, server_key) = self_signed_server();
            let pin = spl_core::ca::sha256(server_cert.as_ref())[..16].to_vec();
            let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let manager = Arc::new(CurrentClientManager::new(Some(Arc::new(
                TransportClient::new(transport_credential(pin, port), None).unwrap(),
            ))));
            let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
            let opener = manager.opener_for_test(tracker.clone());
            let (entered_tx, entered_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let peer = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                entered_tx.send(()).unwrap();
                release_rx.await.unwrap();
                if succeed {
                    let tls = TlsAcceptor::from(Arc::new(server_config(server_cert, server_key)))
                        .accept(tcp)
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    drop(tls);
                }
            });
            let dial = tokio::spawn(async move { opener.dial_carrier().await });
            entered_rx.await.unwrap();
            manager.swap(None);
            let successor = manager.incarnation();
            release_tx.send(()).unwrap();
            assert!(matches!(
                dial.await.unwrap(),
                Err(TransportError::NotPaired)
            ));
            assert_eq!(manager.incarnation(), successor);
            assert_eq!(
                tracker.carrier_events_for_test(),
                (0, 0),
                "stale completion cannot publish status or trigger work"
            );
            peer.await.unwrap();
        }
    });
}

#[cfg(feature = "access-test-hooks")]
#[test]
fn actual_access_request_opening_its_carrier_can_publish_ready() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (path, store, identity, mut bundle, _) = test_bundle_and_store("self-open-progress");
        let (server_cert, server_key) = self_signed_server();
        let pin = spl_core::ca::sha256(server_cert.as_ref())[..16].to_vec();
        let listener = TokioTcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        bundle.endpoints = vec![solstone_core_sol_client::seam::LinkServeEndpoint {
            host: "127.0.0.1".into(),
            port,
        }];
        let manager = Arc::new(CurrentClientManager::new(Some(Arc::new(
            TransportClient::new(transport_credential(pin.clone(), port), None).unwrap(),
        ))));
        let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
        let scheduler = test_scheduler(
            tracker.clone(),
            manager.clone(),
            store.clone(),
            identity.clone(),
            LinkServeCarrierPolicy::RelayPermitted,
            None,
            bundle,
            pin,
        );
        let response = ScriptedHttpResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: serde_json::to_vec(&access_v2_reply(&identity, "own-carrier")).unwrap(),
        };
        let peer = tokio::spawn(serve_and_capture_one_request(
            listener,
            TlsAcceptor::from(Arc::new(server_config(server_cert, server_key))),
            response,
        ));
        let bridge = journal_bridge::start(JournalBridgeConfig {
            opener: manager.opener_for_test(tracker.clone()),
            bridge_names: bridge_names(),
            endpoint_hosts: vec!["127.0.0.1".into()],
            policy: bridge_policy_for_port(0, tracker.clone()),
        })
        .await
        .unwrap();
        let bridge_port = bridge.port();
        tokio::task::spawn_blocking(move || scheduler.run_access_job(bridge_port, 0))
            .await
            .unwrap();
        assert!(matches!(store.load_access(), StoreLoadOutcome::Ready(_)));
        assert_eq!(manager.incarnation(), 1);
        assert_eq!(tracker.carrier_events_for_test().0, 1);
        assert_eq!(
            peer.await.unwrap().head.target,
            "/app/network/api/relay/access"
        );
        bridge.shutdown_and_wait().await;
        std::fs::remove_dir_all(path).unwrap();
    });
}

#[test]
fn changed_origin_ready_is_allowed_only_without_an_explicit_origin_selection() {
    for explicit in [None, Some("https://relay.example".to_owned())] {
        let (path, store, identity, bundle, ca_fp) = test_bundle_and_store("origin-intent");
        let manager = Arc::new(CurrentClientManager::new(None));
        let scheduler = test_scheduler(
            Arc::new(StatusTracker::new(Arc::new(SystemStatusClock))),
            manager.clone(),
            store.clone(),
            identity.clone(),
            LinkServeCarrierPolicy::RelayOnly,
            explicit.clone(),
            bundle,
            ca_fp,
        );
        access_exchange_with(&scheduler, 200, access_v2_reply(&identity, "A"), || {});
        let original = store.load_access();
        assert_eq!(manager.incarnation(), 1);
        let mut changed = access_v2_reply(&identity, "B");
        changed["relay_origin"] = json!("https://successor-relay.example");
        access_exchange_with(&scheduler, 200, changed, || {});
        if explicit.is_some() {
            assert_eq!(store.load_access(), original);
            assert_eq!(manager.incarnation(), 1);
        } else {
            let StoreLoadOutcome::Ready(record) = store.load_access() else {
                panic!("ready")
            };
            assert_eq!(
                record.relay_origin.as_deref(),
                Some("https://successor-relay.example")
            );
            assert_eq!(manager.incarnation(), 2);
        }
        std::fs::remove_dir_all(path).unwrap();
    }
}
