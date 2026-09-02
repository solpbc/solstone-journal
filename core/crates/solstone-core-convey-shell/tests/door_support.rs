// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Shared by door_e2e and authorization_gate; each binary uses a subset.
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use solstone_core_sol_link::ca::{generate_ca, jid_from_spki, sign_csr};
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsMutationError, ClientEntry, ClientRole, RemoveOutcome,
};
use spl_core::frame::{FLAG_CLOSE, FLAG_DATA, FLAG_OPEN, Frame, FrameDecoder, FrameDialer};
use spl_core::mux::{ResponseAssembler, WindowedUpload};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Deserialize)]
struct EchoQuery {
    response_bytes: Option<usize>,
}

/// What the test-only body drain observed before forming its HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EchoObservation {
    pub body_bytes: usize,
    pub complete: bool,
    pub body_error: Option<String>,
    pub status: u16,
}

#[derive(Clone)]
struct EchoState {
    observations: Arc<Mutex<Vec<EchoObservation>>>,
}

/// Test-crate-only body drain/echo surface. It deliberately reaches no journal code.
pub fn body_echo_router() -> Router {
    Router::new().route("/__door_test/echo", post(body_echo))
}

/// The AC18 variant records a handler's body-stream result after its carrier is gone.
pub fn body_echo_router_with_observations(
    observations: Arc<Mutex<Vec<EchoObservation>>>,
) -> Router {
    Router::new()
        .route("/__door_test/echo", post(body_echo_recorded))
        .with_state(EchoState { observations })
}

async fn body_echo(Query(query): Query<EchoQuery>, body: Body) -> Response {
    body_echo_inner(query, body, None).await
}

async fn body_echo_recorded(
    State(state): State<EchoState>,
    Query(query): Query<EchoQuery>,
    body: Body,
) -> Response {
    body_echo_inner(query, body, Some(&state.observations)).await
}

async fn body_echo_inner(
    query: EchoQuery,
    body: Body,
    observations: Option<&Arc<Mutex<Vec<EchoObservation>>>>,
) -> Response {
    let result = to_bytes(body, 128 * 1024 * 1024).await;
    let (count, complete, body_error) = match result {
        Ok(bytes) => (bytes.len(), true, None),
        Err(error) => (0, false, Some(error.to_string())),
    };
    let status = if complete {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    if let Some(observations) = observations {
        observations
            .lock()
            .expect("echo observations lock")
            .push(EchoObservation {
                body_bytes: count,
                complete,
                body_error: body_error.clone(),
                status: status.as_u16(),
            });
    }
    let mut response = Response::builder()
        .status(status)
        .header(
            "x-body-bytes",
            HeaderValue::from_str(&count.to_string()).expect("count header"),
        )
        .header("x-body-complete", if complete { "true" } else { "false" });
    if let Some(error) = &body_error {
        response = response.header(
            "x-body-error",
            HeaderValue::from_str(error).unwrap_or(HeaderValue::from_static("body-error")),
        );
    }
    if let Some(size) = query.response_bytes {
        response
            .body(Body::from(vec![b'x'; size]))
            .expect("echo response")
    } else {
        response.header(header::CONTENT_TYPE, "application/json").body(Body::from(serde_json::json!({"body_bytes": count, "complete": complete, "body_error": body_error}).to_string())).expect("echo report")
    }
}

pub struct Fixture {
    pub root: PathBuf,
    ca_der: CertificateDer<'static>,
    clients: Vec<Client>,
    established_authorized_clients: Vec<u8>,
    instance_id: String,
}
pub struct Client {
    pub certificate: CertificateDer<'static>,
    pub private_key: PrivateKeyDer<'static>,
}

impl Fixture {
    pub fn established(client_count: usize) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "solstone-convey-door-{}-{}-{sequence}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::create_dir_all(root.join("link/ca")).expect("CA directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("established config");
        let ca = generate_ca().expect("CA");
        fs::write(root.join("link/ca/cert.pem"), ca.certificate_pem()).expect("CA cert");
        fs::write(root.join("link/ca/private.pem"), ca.private_key_pem()).expect("CA key");
        let instance_id = jid_from_spki(ca.spki_der()).expect("instance ID");
        fs::write(
            root.join("link/state.json"),
            format!(r#"{{"instance_id":"{instance_id}","home_label":"test home"}}"#),
        )
        .expect("state");
        let ca_der = CertificateDer::pem_slice_iter(ca.certificate_pem().as_bytes())
            .next()
            .expect("CA PEM")
            .expect("CA DER");
        let mut clients = Vec::new();
        let mut entries = Vec::new();
        for index in 0..client_count {
            let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client key");
            let request = CertificateParams::default()
                .serialize_request(&key)
                .expect("CSR")
                .pem()
                .expect("CSR PEM");
            let issued = sign_csr(&ca, &request, &format!("device {index}")).expect("client cert");
            let certificate = CertificateDer::pem_slice_iter(issued.pem().as_bytes())
                .next()
                .expect("client PEM")
                .expect("client DER");
            entries.push(serde_json::json!({"fingerprint": issued.cid(), "device_label": format!("device {index}"), "paired_at": "2026-01-01T00:00:00Z", "instance_id": instance_id, "role": "", "kind": "cert"}));
            clients.push(Client {
                certificate,
                private_key: PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                    key.serialize_der(),
                )),
            });
        }
        let established_authorized_clients = serde_json::to_vec(&entries).expect("entries JSON");
        fs::write(
            root.join("link/authorized_clients.json"),
            &established_authorized_clients,
        )
        .expect("entries");
        Self {
            root,
            ca_der,
            clients,
            established_authorized_clients,
            instance_id,
        }
    }
    pub fn client_config(&self, index: usize) -> rustls::ClientConfig {
        let client = &self.clients[index];
        spl_transport::tls::mtls_config(
            &spl_core::ca::sha256(self.ca_der.as_ref())[..16],
            vec![client.certificate.clone()],
            client.private_key.clone_key(),
        )
        .expect("mTLS config")
    }
    pub fn client_der(&self, index: usize) -> &[u8] {
        self.clients[index].certificate.as_ref()
    }

    pub fn client_identity(
        &self,
        index: usize,
    ) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let client = &self.clients[index];
        (client.certificate.clone(), client.private_key.clone_key())
    }

    pub fn ca_der(&self) -> &[u8] {
        self.ca_der.as_ref()
    }

    fn authorized_clients_path(&self) -> PathBuf {
        self.root.join("link/authorized_clients.json")
    }

    /// Restores `Present` posture with this fixture's originally-established clients.
    pub fn warm_authorization_to_present(&self) {
        let path = self.authorized_clients_path();
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                fs::remove_dir(&path).expect("unreadable directory removes")
            }
            Ok(_) => fs::remove_file(&path).expect("authorization file removes"),
            Err(_) => {}
        }
        fs::write(&path, &self.established_authorized_clients).expect("authorization warms");
    }

    /// Inode-changing induction: `EISDIR` on read, no privilege assumption needed.
    pub fn induce_unreadable_authorization(&self) {
        let path = self.authorized_clients_path();
        fs::remove_file(&path).expect("authorization file removes");
        fs::create_dir(&path).expect("authorization path becomes unreadable");
    }

    /// Inode-changing induction: a fresh, differently-inoded file with malformed JSON.
    pub fn induce_malformed_authorization(&self) {
        let path = self.authorized_clients_path();
        fs::remove_file(&path).expect("authorization file removes");
        fs::write(&path, b"{}").expect("malformed authorization writes");
    }

    /// Inode-changing induction: client 0's row is duplicated; other rows stay unique.
    pub fn induce_duplicate_cid_authorization(&self) {
        let path = self.authorized_clients_path();
        let mut entries: Vec<serde_json::Value> =
            serde_json::from_slice(&self.established_authorized_clients)
                .expect("established ledger parses");
        let first = entries
            .first()
            .expect("established ledger has a client")
            .clone();
        entries.push(first);
        fs::remove_file(&path).expect("authorization file removes");
        fs::write(
            &path,
            serde_json::to_vec(&entries).expect("duplicate ledger serializes"),
        )
        .expect("duplicate authorization writes");
    }

    pub fn remove_authorization(&self, index: usize) -> RemoveOutcome {
        let cid = format!(
            "sha256:{}",
            spl_core::ca::sha256_hex(self.client_der(index))
        );
        let mut attempts = 0;
        loop {
            let mut ledger = AuthorizationLedger::new(&self.root);
            match ledger.remove(&cid) {
                Ok(outcome) => return outcome,
                Err(AuthorizedClientsMutationError::Lock(_)) if attempts < 5 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(error) => panic!("authorization removal failed: {error}"),
            }
        }
    }

    /// Restores one fixture client to the authorization ledger.
    ///
    /// A prior unreadable-posture induction can leave a directory here; clear
    /// that non-file before `AuthorizationLedger::add` opens the ledger.
    pub fn restore_authorization(&self, index: usize) {
        let path = self.authorized_clients_path();
        if matches!(fs::metadata(&path), Ok(metadata) if metadata.is_dir()) {
            fs::remove_dir(&path).expect("unreadable authorization directory removes");
        }
        let entry = ClientEntry::new(
            format!(
                "sha256:{}",
                spl_core::ca::sha256_hex(self.client_der(index))
            ),
            format!("device {index}"),
            "2026-01-01T00:00:00Z",
            self.instance_id.clone(),
            ClientRole::Roleless,
        );
        let mut attempts = 0;
        loop {
            let mut ledger = AuthorizationLedger::new(&self.root);
            match ledger.add(entry.clone()) {
                Ok(_) => return,
                Err(AuthorizedClientsMutationError::Lock(_)) if attempts < 5 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(error) => panic!("authorization restore failed: {error}"),
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn tree(root: &std::path::Path) -> Vec<PathBuf> {
    fn visit(root: &std::path::Path, current: &std::path::Path, result: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(current) {
            for entry in entries.flatten() {
                let path = entry.path();
                result.push(path.strip_prefix(root).expect("relative").to_path_buf());
                if path.is_dir() {
                    visit(root, &path, result);
                }
            }
        }
    }
    let mut result = Vec::new();
    visit(root, root, &mut result);
    result.sort();
    result
}

pub async fn get_over_carrier(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    decoder: &mut FrameDecoder,
    dialer: &mut FrameDialer,
    path: &str,
) -> spl_core::http::HttpResponse {
    let stream_id = dialer.allocate();
    let request = format!("GET {path} HTTP/1.1\r\nhost: spl.local\r\ncontent-length: 0\r\n\r\n");
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_OPEN | FLAG_DATA, request.into_bytes())
                .encode()
                .expect("request frame"),
        )
        .await
        .expect("request writes");
    carrier
        .write_all(
            &Frame::new(stream_id, FLAG_CLOSE, Vec::new())
                .encode()
                .expect("request close frame"),
        )
        .await
        .expect("request closes");
    carrier.flush().await.expect("request flushes");

    let mut response = ResponseAssembler::new(stream_id);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = carrier.read(&mut buffer).await.expect("response reads");
        assert!(read > 0, "carrier closed before the response completed");
        decoder.feed(&buffer[..read]);
        for frame in decoder.drain().expect("response frames") {
            if frame.stream_id != stream_id {
                continue;
            }
            let output = response
                .feed(&frame.encode().expect("routed response frame"))
                .expect("response frame");
            for frame in output.pongs.into_iter().chain(output.emit_frames) {
                carrier
                    .write_all(&frame)
                    .await
                    .expect("control frame writes");
            }
        }
        carrier.flush().await.expect("control flushes");
        if response.is_closed() {
            return response.into_response().expect("complete response");
        }
    }
}

/// Drive several HTTP uploads over one already-handshaken SPL carrier.
pub async fn multiplexed_requests(
    carrier: &mut tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    requests: Vec<(&str, Vec<u8>)>,
) -> Vec<spl_core::http::HttpResponse> {
    let mut ids = spl_core::frame::FrameDialer::default();
    let stream_ids = (0..requests.len())
        .map(|_| ids.allocate())
        .collect::<Vec<_>>();
    let mut uploads = requests
        .iter()
        .zip(&stream_ids)
        .map(|((path, body), stream_id)| {
            WindowedUpload::new(
                *stream_id,
                format!(
                    "POST {path} HTTP/1.1\r\nhost: spl.local\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
                body.len(),
            )
        })
        .collect::<Vec<_>>();
    let mut assemblers = stream_ids
        .iter()
        .map(|stream_id| ResponseAssembler::new(*stream_id))
        .collect::<Vec<_>>();
    let mut carrier_decoder = spl_core::frame::FrameDecoder::new();
    let mut offsets = vec![0; requests.len()];
    while !assemblers.iter().all(ResponseAssembler::is_closed) {
        let mut wrote = false;
        for index in 0..uploads.len() {
            let body = &requests[index].1;
            loop {
                let capacity = uploads[index].body_capacity();
                if capacity > 0 && offsets[index] < body.len() {
                    let end = (offsets[index] + capacity).min(body.len());
                    uploads[index]
                        .feed_body(&body[offsets[index]..end])
                        .expect("upload staging");
                    offsets[index] = end;
                }
                let mut sent = false;
                while let Some(frame) = uploads[index].poll_send().expect("upload frame") {
                    carrier.write_all(&frame).await.expect("carrier writes");
                    wrote = true;
                    sent = true;
                }
                // A drained 256 KiB staging batch can be immediately refilled
                // while initial-window credit remains. Stop only once the mux
                // declines another frame (or the complete body has closed).
                if !sent || (offsets[index] == body.len() && uploads[index].is_done()) {
                    break;
                }
            }
        }
        if wrote {
            carrier.flush().await.expect("carrier flushes");
        }
        let mut bytes = [0_u8; 64 * 1024];
        let read = carrier.read(&mut bytes).await.expect("carrier reads");
        assert!(
            read > 0,
            "carrier closed before multiplexed responses completed"
        );
        carrier_decoder.feed(&bytes[..read]);
        for frame in carrier_decoder.drain().expect("carrier frame") {
            let Some(index) = stream_ids
                .iter()
                .position(|stream_id| *stream_id == frame.stream_id)
            else {
                continue;
            };
            let output = assemblers[index]
                .feed(&frame.encode().expect("routed frame"))
                .expect("response frame");
            for credit in output.window_grants {
                uploads[index].grant(credit).expect("window credit");
            }
            for frame in output.pongs.into_iter().chain(output.emit_frames) {
                carrier.write_all(&frame).await.expect("control frame");
            }
        }
        carrier.flush().await.expect("control flushes");
    }
    assemblers
        .into_iter()
        .map(|assembler| assembler.into_response().expect("complete response"))
        .collect()
}
