// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct synchronous channel. This intentionally uses no loopback forwarder:
//! no local listener, ephemeral port, or connection pool. Each confidential
//! generate call establishes and uses its own attested channel through
//! `solstone-core-generate-wire::confidential::AttestedEndpointTransport` and
//! `send_json_request`. The per-caller channel model rules out a shared,
//! long-lived daemon, and the Python forwarder's listener-thread/pool/epoch
//! lifecycle is unnecessary when one in-process caller owns each request.

use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key};
use rustls::pki_types::{CertificateDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as RustlsError, SignatureScheme,
    StreamOwned,
};
use solstone_core_spp_attest::{Policy, QuoteVerifier};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::{
    error::RatlsChannelError,
    ratls::{
        contract::{
            EXPORTER_BYTES, EXPORTER_LABEL, EXPORTER_PROOF_MEDIA_TYPE, EXPORTER_PROOF_PATH,
            PREFACE_MAGIC, exporter_context,
        },
        verify::{
            CompositeVerifier, VerifiedCertificateEvidence, verify_certificate_evidence,
            verify_exporter_proof,
        },
    },
};

const MAX_PROOF_RESPONSE_HEADERS: usize = 16 * 1024;
const MAX_PROOF_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
type PeerCertificate = Arc<Mutex<Option<Vec<u8>>>>;

/// Read/write transport carrying application requests after attestation.
pub trait AttestedIo: Read + Write {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()>;
}

impl AttestedIo for TcpStream {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(timeout)?;
        self.set_write_timeout(timeout)
    }
}

/// HTTP response received over an attested transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestedHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Bounded HTTP transport or protocol failure.
#[derive(Debug, thiserror::Error)]
pub enum AttestedHttpError {
    #[error("attested HTTP transport failed")]
    Transport(#[source] std::io::Error),
    #[error("attested HTTP protocol failed ({0})")]
    Protocol(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatlsEndpoint {
    pub host: String,
    pub port: u16,
    pub server_name: String,
}

impl RatlsEndpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            server_name: "spp-engine".into(),
        }
    }
}

struct PermissiveServerVerifier {
    supported_algorithms: WebPkiSupportedAlgorithms,
    peer_certificate: PeerCertificate,
}
impl fmt::Debug for PermissiveServerVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PermissiveServerVerifier")
            .finish_non_exhaustive()
    }
}
impl ServerCertVerifier for PermissiveServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        // VERIFY_NONE is not a weakness: attestation validation happens out-of-band via verify_certificate_evidence/verify_exporter_proof after the handshake.
        *self
            .peer_certificate
            .lock()
            .expect("peer certificate lock poisoned") = Some(end_entity.as_ref().to_vec());
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        // The client configuration permits TLS 1.3 only, so rustls cannot call this.
        Err(RustlsError::General(
            "TLS 1.2 is not supported by this client (pinned to TLS 1.3 only)".into(),
        ))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        let (remaining, parsed) = X509Certificate::from_der(cert.as_ref()).map_err(|_| {
            RustlsError::General("server certificate SPKI could not be parsed".into())
        })?;
        if !remaining.is_empty() {
            return Err(RustlsError::General(
                "server certificate has trailing bytes".into(),
            ));
        }
        let spki = SubjectPublicKeyInfoDer::from(parsed.public_key().raw.to_vec());
        verify_tls13_signature_with_raw_key(message, &spki, dss, &self.supported_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_algorithms.supported_schemes()
    }
}

fn tls_config() -> (Arc<ClientConfig>, PeerCertificate) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let peer_certificate = Arc::new(Mutex::new(None));
    let config = Arc::new(
        ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3 is supported")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PermissiveServerVerifier {
                supported_algorithms: provider.signature_verification_algorithms,
                peer_certificate: peer_certificate.clone(),
            }))
            .with_no_client_auth(),
    );
    (config, peer_certificate)
}

pub struct AttestedChannel {
    stream: StreamOwned<ClientConnection, TcpStream>,
    pub verified: VerifiedCertificateEvidence,
    pub last_used_monotonic: Instant,
    pub epoch: u64,
}

impl AttestedIo for AttestedChannel {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.sock.set_read_timeout(timeout)?;
        self.stream.sock.set_write_timeout(timeout)
    }
}

impl Read for AttestedChannel {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(buffer)
    }
}
impl Write for AttestedChannel {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buffer)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

#[allow(clippy::too_many_arguments)] // Mirrors the Python channel oracle's call boundary.
pub fn establish_attested_channel(
    endpoint: &RatlsEndpoint,
    owner_nonce: &[u8],
    nvattest_dir: &Path,
    now: SystemTime,
    roots_dir: Option<&Path>,
    policy: Option<&Policy>,
    quote_verifier: Option<&dyn QuoteVerifier>,
    composite_verifier: &dyn CompositeVerifier,
    socket_timeout: Duration,
    epoch: u64,
) -> Result<AttestedChannel, RatlsChannelError> {
    let address = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?
        .next()
        .ok_or(RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    let mut socket =
        TcpStream::connect_timeout(&address, socket_timeout).map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    socket
        .set_read_timeout(Some(socket_timeout))
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    socket
        .set_write_timeout(Some(socket_timeout))
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    socket
        .write_all(PREFACE_MAGIC)
        .and_then(|_| socket.write_all(owner_nonce))
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    let name =
        ServerName::try_from(endpoint.server_name.clone()).map_err(|_| RatlsChannelError {
            reason_code: "tls_handshake_failed",
        })?;
    let (config, peer_certificate) = tls_config();
    let connection = ClientConnection::new(config, name).map_err(|_| RatlsChannelError {
        reason_code: "tls_handshake_failed",
    })?;
    let mut stream = StreamOwned::new(connection, socket);
    while stream.conn.is_handshaking() {
        stream
            .conn
            .complete_io(&mut stream.sock)
            .map_err(|_| RatlsChannelError {
                reason_code: "gateway_unreachable",
            })?;
    }
    let certificate = peer_certificate
        .lock()
        .expect("peer certificate lock poisoned")
        .clone()
        .ok_or(RatlsChannelError {
            reason_code: "tls_handshake_failed",
        })?;
    let verified = verify_certificate_evidence(
        &certificate,
        owner_nonce,
        now,
        nvattest_dir,
        roots_dir,
        policy,
        quote_verifier,
        composite_verifier,
    )
    .map_err(|error| RatlsChannelError {
        reason_code: error.reason_code,
    })?;
    let mut exporter = [0u8; EXPORTER_BYTES];
    stream
        .conn
        .export_keying_material(
            &mut exporter,
            EXPORTER_LABEL,
            Some(&exporter_context(owner_nonce, &verified.tls_spki_der)),
        )
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    let request = format!(
        "GET {EXPORTER_PROOF_PATH} HTTP/1.1\r\nHost: spp-engine\r\nContent-Length: 0\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    let proof = recv_proof_response(&mut stream)?;
    verify_exporter_proof(&proof, &verified.evidence, &exporter, owner_nonce, policy).map_err(
        |error| RatlsChannelError {
            reason_code: error.reason_code,
        },
    )?;
    stream
        .sock
        .set_read_timeout(None)
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    stream
        .sock
        .set_write_timeout(None)
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    Ok(AttestedChannel {
        stream,
        verified,
        last_used_monotonic: Instant::now(),
        epoch,
    })
}

/// Sends one JSON POST over an already attested transport.
pub fn send_json_request(
    stream: &mut dyn AttestedIo,
    host: &str,
    path: &str,
    bearer: Option<&str>,
    body: &[u8],
) -> Result<AttestedHttpResponse, AttestedHttpError> {
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(bearer) = bearer {
        request.push_str("Authorization: Bearer ");
        request.push_str(bearer);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(body))
        .and_then(|_| stream.flush())
        .map_err(AttestedHttpError::Transport)?;
    let response = recv_bounded_http_response(stream).map_err(http_error)?;
    let status = response_status(&response.status_line).map_err(AttestedHttpError::Protocol)?;
    Ok(AttestedHttpResponse {
        status,
        body: response.body,
    })
}

fn recv_proof_response(
    stream: &mut StreamOwned<ClientConnection, TcpStream>,
) -> Result<Vec<u8>, RatlsChannelError> {
    let response = recv_bounded_http_response(stream).map_err(|_| RatlsChannelError {
        reason_code: "proof_http_failed",
    })?;
    if response.status_line.as_slice() != b"HTTP/1.1 200 OK" {
        return Err(RatlsChannelError {
            reason_code: "proof_http_failed",
        });
    }
    for (name, value) in &response.headers {
        if name.eq_ignore_ascii_case(b"content-type")
            && std::str::from_utf8(value).ok().map(str::trim) != Some(EXPORTER_PROOF_MEDIA_TYPE)
        {
            return Err(RatlsChannelError {
                reason_code: "proof_http_failed",
            });
        }
    }
    Ok(response.body)
}

struct BoundedHttpResponse {
    status_line: Vec<u8>,
    headers: Vec<(Vec<u8>, Vec<u8>)>,
    body: Vec<u8>,
}

enum BoundedHttpError {
    Transport(std::io::Error),
    Protocol,
}

fn recv_bounded_http_response<R: Read + ?Sized>(
    stream: &mut R,
) -> Result<BoundedHttpResponse, BoundedHttpError> {
    let mut data = Vec::new();
    let marker = b"\r\n\r\n";
    while !data.windows(marker.len()).any(|window| window == marker) {
        if data.len() >= MAX_PROOF_RESPONSE_HEADERS {
            return Err(BoundedHttpError::Protocol);
        }
        let mut buffer = [0u8; 4096];
        let read_len = buffer.len().min(MAX_PROOF_RESPONSE_HEADERS - data.len());
        let count = stream
            .read(&mut buffer[..read_len])
            .map_err(BoundedHttpError::Transport)?;
        if count == 0 {
            return Err(BoundedHttpError::Protocol);
        }
        data.extend_from_slice(&buffer[..count]);
    }
    let split = data
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("marker present");
    let (head, remainder) = data.split_at(split);
    let mut body = remainder[marker.len()..].to_vec();
    let lines = http_header_lines(head).ok_or(BoundedHttpError::Protocol)?;
    let status_line = lines.first().ok_or(BoundedHttpError::Protocol)?.to_vec();
    let mut headers = Vec::new();
    let mut content_length = None;
    for line in &lines[1..] {
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(BoundedHttpError::Protocol);
        };
        let (name, value_with_colon) = line.split_at(colon);
        let value = &value_with_colon[1..];
        if name.eq_ignore_ascii_case(b"content-length") {
            content_length = std::str::from_utf8(value)
                .ok()
                .and_then(|text| text.trim().parse::<usize>().ok());
        }
        headers.push((name.to_vec(), value.to_vec()));
    }
    let length = content_length
        .filter(|length| *length <= MAX_PROOF_RESPONSE_BYTES)
        .ok_or(BoundedHttpError::Protocol)?;
    while body.len() < length {
        let mut buffer = [0u8; 65536];
        let remaining = (length - body.len()).min(buffer.len());
        let count = stream
            .read(&mut buffer[..remaining])
            .map_err(BoundedHttpError::Transport)?;
        if count == 0 {
            return Err(BoundedHttpError::Protocol);
        }
        body.extend_from_slice(&buffer[..count]);
    }
    Ok(BoundedHttpResponse {
        status_line,
        headers,
        body: body[..length].to_vec(),
    })
}

fn http_error(error: BoundedHttpError) -> AttestedHttpError {
    match error {
        BoundedHttpError::Transport(error) => AttestedHttpError::Transport(error),
        BoundedHttpError::Protocol => AttestedHttpError::Protocol("response_invalid"),
    }
}

fn response_status(status_line: &[u8]) -> Result<u16, &'static str> {
    let mut parts = status_line.split(|byte| *byte == b' ');
    let Some(version) = parts.next() else {
        return Err("status_invalid");
    };
    let Some(status) = parts.next() else {
        return Err("status_invalid");
    };
    if !version.starts_with(b"HTTP/") || status.len() != 3 {
        return Err("status_invalid");
    }
    std::str::from_utf8(status)
        .ok()
        .and_then(|status| status.parse().ok())
        .ok_or("status_invalid")
}

fn http_header_lines(head: &[u8]) -> Option<Vec<&[u8]>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < head.len() {
        match head[index] {
            b'\r' if head.get(index + 1) == Some(&b'\n') => {
                lines.push(&head[start..index]);
                index += 2;
                start = index;
            }
            b'\r' | b'\n' => return None,
            _ => index += 1,
        }
    }
    lines.push(&head[start..]);
    Some(lines)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use super::{AttestedIo, send_json_request};

    struct ScriptedIo {
        written: Vec<u8>,
        unread: Cursor<Vec<u8>>,
    }

    impl Read for ScriptedIo {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.unread.read(buffer)
        }
    }

    impl Write for ScriptedIo {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.written.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl AttestedIo for ScriptedIo {
        fn set_io_timeout(&mut self, _: Option<std::time::Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn json_request_uses_plain_read_write_transport_with_bounded_response_parsing() {
        let mut stream = ScriptedIo {
            written: Vec::new(),
            unread: Cursor::new(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\n{}".to_vec()),
        };

        let response = send_json_request(
            &mut stream,
            "example.test",
            "/v1/chat/completions",
            Some("secret"),
            br#"{"model":"test"}"#,
        )
        .expect("application response");
        let request = stream.written;

        assert_eq!(response.status, 201);
        assert_eq!(response.body, br"{}");
        assert!(request.starts_with(b"POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(
            request
                .windows(b"Authorization: Bearer secret".len())
                .any(|window| window == b"Authorization: Bearer secret")
        );
    }
}
