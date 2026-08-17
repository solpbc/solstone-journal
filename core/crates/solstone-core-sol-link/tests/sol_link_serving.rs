// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::json;
use solstone_core_sol_client::resident::ShutdownSignal;
use solstone_core_sol_client::seam::{
    LinkServeBundle, LinkServeEndpoint, LinkServeRequest, LinkServeRunner,
};
use solstone_core_sol_link::SplLinkServeRunner;
use solstone_core_sol_link::serve_test_support::{
    STATUS_PATH, StatusClock, StatusTracker, bridge_names, bridge_policy_for_port,
};
use spl_core::frame::{FLAG_CLOSE, FLAG_DATA, Frame, FrameDecoder, RECOMMENDED_CHUNK};
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
    let mut decoder = FrameDecoder::new();
    let mut stream_id = 1u32;
    let mut closed = false;
    let mut buf = [0u8; 4096];
    while !closed {
        let n = tls.read(&mut buf).await.expect("read framed request");
        if n == 0 {
            break;
        }
        decoder.feed(&buf[..n]);
        for frame in decoder.drain().expect("decode request frame") {
            stream_id = frame.stream_id;
            if frame.flags & FLAG_CLOSE != 0 {
                closed = true;
            }
        }
    }
    stream_id
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
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
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

fn resident_serve_request(port: u16) -> LinkServeRequest {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client key");
    let params = CertificateParams::new(vec!["client.test".to_string()]).expect("client params");
    let cert = params.self_signed(&key).expect("client cert");
    LinkServeRequest {
        label: "laptop".to_string(),
        port,
        direct: true,
        relay_origin: None,
        bundle: LinkServeBundle {
            private_key_pem: key.serialize_pem(),
            client_cert_pem: cert.pem(),
            ca_chain_pem: vec![ca_pem()],
            home_attestation: "attestation.jwt".to_string(),
            instance_id: "home-instance".to_string(),
            home_label: "Home".to_string(),
            endpoints: vec![LinkServeEndpoint {
                host: "127.0.0.1".to_string(),
                port: unused_loopback_port(),
            }],
            local_endpoints: json!([{"ip": "127.0.0.1", "port": 7657}]),
        },
    }
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
        .start(resident_serve_request(0))
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
