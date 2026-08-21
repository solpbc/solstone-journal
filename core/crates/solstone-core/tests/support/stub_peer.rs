// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{RootCertStore, ServerConfig};
use serde_json::json;
use spl_core::frame::{FLAG_CLOSE, FLAG_DATA, Frame, FrameDecoder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::oneshot;
use tokio_rustls::{TlsAcceptor, server::TlsStream};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum ResponseAction {
    Status { status: u16, body: Vec<u8> },
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequestRoute {
    pub method: String,
    pub path: String,
}

impl RequestRoute {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: "GET".to_string(),
            path: path.into(),
        }
    }

    pub fn post(path: impl Into<String>) -> Self {
        Self {
            method: "POST".to_string(),
            path: path.into(),
        }
    }
}

impl ResponseAction {
    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::Status {
            status,
            body: body.into(),
        }
    }

    pub fn manifest_empty() -> Self {
        Self::status(200, b"{}".to_vec())
    }
}

#[derive(Debug, Clone)]
pub struct PeerPlan {
    routes: BTreeMap<RequestRoute, VecDeque<ResponseAction>>,
}

impl PeerPlan {
    pub fn new(routes: impl IntoIterator<Item = (RequestRoute, Vec<ResponseAction>)>) -> Self {
        Self {
            routes: routes
                .into_iter()
                .map(|(route, actions)| (route, actions.into()))
                .collect(),
        }
    }

    fn next(&mut self, method: &str, path: &str) -> ResponseAction {
        let route = RequestRoute {
            method: method.to_string(),
            path: path.to_string(),
        };
        self.routes
            .get_mut(&route)
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| {
                ResponseAction::status(500, format!("unexpected peer request: {method} {path}"))
            })
    }
}

pub struct StubPeer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handshake_errors: Arc<Mutex<Vec<String>>>,
    bundle: Bundle,
    shutdown: Option<oneshot::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl StubPeer {
    pub fn new(plan: PeerPlan) -> Self {
        let bundle = Bundle::new();
        let config = server_config(&bundle);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub peer");
        listener.set_nonblocking(true).expect("set nonblocking");
        let address = listener.local_addr().expect("stub peer address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handshake_errors = Arc::new(Mutex::new(Vec::new()));
        let plan = Arc::new(Mutex::new(plan));
        let (shutdown, shutdown_rx) = oneshot::channel();
        let thread_requests = requests.clone();
        let thread_handshake_errors = handshake_errors.clone();
        let join = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("stub runtime");
            runtime.block_on(run_server(
                listener,
                TlsAcceptor::from(config),
                thread_requests,
                thread_handshake_errors,
                plan,
                shutdown_rx,
            ));
        });
        Self {
            address,
            requests,
            handshake_errors,
            bundle,
            shutdown: Some(shutdown),
            join: Some(join),
        }
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().expect("request lock").clone()
    }

    pub fn ingest_requests(&self) -> Vec<CapturedRequest> {
        self.requests()
            .into_iter()
            .filter(|request| request.method == "POST")
            .collect()
    }

    pub fn handshake_errors(&self) -> Vec<String> {
        self.handshake_errors
            .lock()
            .expect("handshake lock")
            .clone()
    }

    pub fn fixture(&self) -> Fixture {
        Fixture::new(self)
    }
}

impl Drop for StubPeer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(join) = self.join.take() {
            join.join().expect("join stub peer");
        }
    }
}

pub struct Fixture {
    journal: tempfile::TempDir,
}

impl Fixture {
    fn new(peer: &StubPeer) -> Self {
        let journal = tempfile::tempdir().expect("temporary journal");
        let peer_dir = journal.path().join("peers/remote-instance");
        fs::create_dir_all(&peer_dir).expect("peer directory");
        fs::write(peer_dir.join("private.pem"), &peer.bundle.client_key_pem).expect("client key");
        fs::write(peer_dir.join("cert.pem"), &peer.bundle.client_cert_pem)
            .expect("client certificate");
        fs::write(peer_dir.join("chain.pem"), &peer.bundle.ca_pem).expect("CA chain");
        fs::write(peer_dir.join("home_attestation.jwt"), "test-attestation").expect("attestation");
        fs::write(
            peer_dir.join("peer.json"),
            json!({
                "instance_id": "remote-instance",
                "label": "office",
                "local_endpoints": [{"ip": "127.0.0.1", "port": peer.port()}],
            })
            .to_string(),
        )
        .expect("peer metadata");
        Self { journal }
    }

    pub fn path(&self) -> &Path {
        self.journal.path()
    }

    pub fn add_segment(&self, stream: &str, key: &str, files: &[(&str, &[u8])]) {
        self.add_segment_for_day("20260203", stream, key, files);
    }

    pub fn add_segment_for_day(&self, day: &str, stream: &str, key: &str, files: &[(&str, &[u8])]) {
        let segment = self
            .path()
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(key);
        fs::create_dir_all(&segment).expect("segment directory");
        for (name, bytes) in files {
            fs::write(segment.join(name), bytes).expect("segment file");
        }
    }

    pub fn add_entity(&self, id: &str, entity: serde_json::Value) {
        let directory = self.path().join("entities").join(id);
        fs::create_dir_all(&directory).expect("entity directory");
        fs::write(directory.join("entity.json"), entity.to_string()).expect("entity identity");
    }

    pub fn add_facet(&self, name: &str, files: &[(&str, &[u8])]) {
        let directory = self.path().join("facets").join(name);
        fs::create_dir_all(&directory).expect("facet directory");
        for (relative, bytes) in files {
            let path = directory.join(relative);
            fs::create_dir_all(path.parent().expect("facet file parent"))
                .expect("facet file parent");
            fs::write(path, bytes).expect("facet file");
        }
    }

    pub fn add_import(
        &self,
        id: &str,
        import_json: serde_json::Value,
        imported_json: serde_json::Value,
        content_manifest: Option<&[serde_json::Value]>,
    ) {
        let directory = self.path().join("imports").join(id);
        fs::create_dir_all(&directory).expect("import directory");
        fs::write(directory.join("import.json"), import_json.to_string()).expect("import metadata");
        fs::write(directory.join("imported.json"), imported_json.to_string())
            .expect("import result");
        if let Some(content_manifest) = content_manifest {
            let text = content_manifest
                .iter()
                .map(serde_json::Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(directory.join("content_manifest.jsonl"), text).expect("import manifest");
        }
    }

    pub fn set_config(&self, config: serde_json::Value) {
        let directory = self.path().join("config");
        fs::create_dir_all(&directory).expect("config directory");
        fs::write(directory.join("journal.json"), config.to_string()).expect("journal config");
    }
}

struct Bundle {
    ca_pem: String,
    client_key_pem: String,
    client_cert_pem: String,
    server_cert: CertificateDer<'static>,
    server_key: PrivateKeyDer<'static>,
    client_ca: CertificateDer<'static>,
}

impl Bundle {
    fn new() -> Self {
        let ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("CA key");
        let mut ca_params = CertificateParams::default();
        let mut distinguished_name = DistinguishedName::new();
        distinguished_name.push(DnType::CommonName, "stub peer CA");
        ca_params.distinguished_name = distinguished_name;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        ca_params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
        ];
        let ca = ca_params.self_signed(&ca_key).expect("CA certificate");

        let server_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("server key");
        let mut server_params =
            CertificateParams::new(vec!["spl.local".to_string()]).expect("server params");
        server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server = server_params
            .signed_by(&server_key, &ca, &ca_key)
            .expect("server certificate");

        let client_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client key");
        let mut client_params = CertificateParams::default();
        client_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let client = client_params
            .signed_by(&client_key, &ca, &ca_key)
            .expect("client certificate");

        Self {
            ca_pem: ca.pem(),
            client_key_pem: client_key.serialize_pem(),
            client_cert_pem: client.pem(),
            server_cert: CertificateDer::from(server.der().to_vec()),
            server_key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
            client_ca: CertificateDer::from(ca.der().to_vec()),
        }
    }
}

fn server_config(bundle: &Bundle) -> Arc<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.add(bundle.client_ca.clone()).expect("client CA root");
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .expect("client verifier");
    Arc::new(
        ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![bundle.server_cert.clone(), bundle.client_ca.clone()],
                bundle.server_key.clone_key(),
            )
            .expect("server config"),
    )
}

async fn run_server(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handshake_errors: Arc<Mutex<Vec<String>>>,
    plan: Arc<Mutex<PeerPlan>>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let listener = TokioTcpListener::from_std(listener).expect("tokio listener");
    loop {
        let accepted = tokio::select! {
            _ = &mut shutdown => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else {
            return;
        };
        let acceptor = acceptor.clone();
        let requests = requests.clone();
        let handshake_errors = handshake_errors.clone();
        let plan = plan.clone();
        tokio::spawn(async move {
            match acceptor.accept(stream).await {
                Ok(tls) => {
                    let _ = handle_connection(tls, &requests, &plan).await;
                }
                Err(error) => handshake_errors
                    .lock()
                    .expect("handshake lock")
                    .push(error.to_string()),
            }
        });
    }
}

async fn handle_connection(
    mut tls: TlsStream<tokio::net::TcpStream>,
    requests: &Arc<Mutex<Vec<CapturedRequest>>>,
    plan: &Arc<Mutex<PeerPlan>>,
) -> io::Result<()> {
    let mut decoder = FrameDecoder::new();
    let mut streams = BTreeMap::<u32, Vec<u8>>::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = tls.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        decoder.feed(&buffer[..read]);
        for frame in decoder.drain().map_err(io::Error::other)? {
            if let Some(pong) = frame.control_pong() {
                tls.write_all(&pong.encode().map_err(io::Error::other)?)
                    .await?;
                continue;
            }
            if frame.flags & FLAG_DATA != 0 {
                streams
                    .entry(frame.stream_id)
                    .or_default()
                    .extend_from_slice(&frame.payload);
            }
            if frame.flags & FLAG_CLOSE == 0 {
                continue;
            }
            let raw = streams.remove(&frame.stream_id).unwrap_or_default();
            let request = parse_request(raw)?;
            let action = plan
                .lock()
                .expect("plan lock")
                .next(&request.method, &request.path);
            requests.lock().expect("request lock").push(request);
            match action {
                ResponseAction::Drop => return Ok(()),
                ResponseAction::Status { status, body } => {
                    let response = http_response(status, &body);
                    let frame = Frame::new(frame.stream_id, FLAG_DATA | FLAG_CLOSE, response);
                    tls.write_all(&frame.encode().map_err(io::Error::other)?)
                        .await?;
                    tls.flush().await?;
                }
            }
        }
    }
}

fn parse_request(raw: Vec<u8>) -> io::Result<CapturedRequest> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request head missing"))?;
    let head = std::str::from_utf8(&raw[..split])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut lines = head.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request line missing"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Ok(CapturedRequest {
        method,
        path,
        headers,
        body: raw[split + 4..].to_vec(),
    })
}

fn http_response(status: u16, body: &[u8]) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        418 => "I'm a teapot",
        500 => "Internal Server Error",
        _ => "Stub Response",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}
