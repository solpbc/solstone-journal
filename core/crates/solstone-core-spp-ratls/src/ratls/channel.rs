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
pub trait AttestedIo: Read + Write {}
impl<T: Read + Write + ?Sized> AttestedIo for T {}

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
    // Later confidential-lane wiring can adapt this direct Read + Write channel once the actual
    // caller is known.
    stream: StreamOwned<ClientConnection, TcpStream>,
    pub verified: VerifiedCertificateEvidence,
    pub last_used_monotonic: Instant,
    pub epoch: u64,
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
    use std::{
        io::{Read, Write},
        net::TcpListener,
        path::Path,
        sync::mpsc,
        thread,
    };

    use rcgen::{CertificateParams, CustomExtension, KeyPair, PKCS_ECDSA_P256_SHA256};
    use ring::{
        rand::SystemRandom,
        signature::{RSA_PKCS1_SHA256, RsaKeyPair},
    };
    use rustls::{
        ServerConfig, ServerConnection, StreamOwned,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
        server::{ClientHello, ResolvesServerCert},
        sign::CertifiedKey,
    };
    use solstone_core_spp_attest::{
        CpuBundle,
        nvgpu::claims::GpuAppraisal,
        snp::{CpuAppraisal, CpuTcb, TcbVersion},
    };
    use x509_parser::{pem::parse_x509_pem, prelude::FromDer};

    use super::*;
    use crate::{
        cadence::CompositeVerdict,
        error::CompositeVerificationError,
        ratls::{
            contract::{
                COMPOSITE_EVIDENCE_OID, CompositeEvidence, ExporterProof, exporter_binding,
                exporter_context,
            },
            verify::{CompositeVerificationInput, CompositeVerifier},
        },
        state::{AttestationFailureKind, AttestationStateStore},
    };

    struct AcceptingCompositeVerifier;
    struct RejectingCompositeVerifier;

    const TEST_AK_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDd4QO8Gw8LEy9I\nBWtcCN/nqi8GQAwYRMrPKcvR98vqWF6VImZoRQVLotl9ud+k/oSzDH/APeKzdhlH\nyMhHedGJxDProAmegyeIi7TtekICkM811sslns7kyCaCRRV90nQ0Gzmf7WWCsOJX\nRwAH3YQlAJAMfK/SLRnoVb3bG1qwfnCOrNuhsmcMZt7baMH8HpG1/rm39KrRFnow\n5lyIBuoGH9TXuellFb4MTMf8ntXMosqOHe4R3LpfaJ17KjBZkGPPwy+PRiVidfYt\nKkwrCWS30XPfsahxCIF8+aFFmb12fpDvR7g4K+7xrhmfL8SXx00n5C00ucmUSCRT\n0QekpVujAgMBAAECggEADkrHwE6n5ek68vMyaq/BqH0MYWUvwkJwI+8Hy4MgNfyy\nPv4DxbSodipLwy79anXgm13zPrFd0HyLfVXAHOaKakrio0tgQz8khUWmhmOJK/wi\n9M9cr5QutIr1/A8yJrQvOwoD6LrUfpohQkj3BgqtT+rc3IkNlEbGc/JN8/arnVGw\n73DA82n4ajY16mlGBTVVMVmBr77D1as0fAUF8mH5S1mCkmdHz1FJOyEDG3F4C/Gm\nR6ZAnEN/dd4eCTI4GMcfrUZV/LXZuwE2xkUoCMP/7h7S+PA08RIDLZu/b/2c4kZp\nN37Hr+BvmPRpui74NzQszMXvpuLVxTr0/CebRisr5QKBgQD95euOEySX0bsyyQsq\ngwPycxpTJl9h0xUBUuZ69zRXMbYY6tc7xAlQaZSYaNfsFGLvPYxelZGVgXbw+nEI\nKoFuz70AI8HrDX1fb6IOQ6NwGT2qPkqZpySmJC9CQ8WcRwX8sdKVrigHPvI0i4tt\nQh4DOYOgmo1BTnWYRk0EuCEX5wKBgQDftzyvg4WaVp5xyZVyc8bCooMwfbnRKvBk\nYwwpxAJZQssRtWgy6gZ7f1m4wIaxmdNay9oKzMoGEaq6hb23uTe90nNyNtvb69I/\nBdJT6/HA8fmRyyyLqfcBLhpiqVlWWEuk1NHvti96VkjgVz7scVIV34kvEqPV+z/X\nVT/ibvX25QKBgQCaQ3cydIkYQVL3EVXad44PYkYNXVQ4sLKjgkYNUmOX0tlsHEu3\nwW1TUUL6s0D17JEMAR5nXYL+DpJA6jmBF6patJeGHTO2aBTTxpT1C72i34MrC/vx\nja9jzrp0DY9kW3bUyQpE7XLerC0nJd4J/VEU7n3+N8k5c71ZTuV+x4074wKBgCuE\nLR3G65oV90QS/isBMkxx6CrqidaSD6i3S4pkQkCyqWWMb/RXaWNkZkN1z72EOoSS\n2pr3MuTzUs5tbXXrZVhbM3GoEiQ5PvBbZYpFfwUVDIK7jrKsIQvtt9wxLNuK2Uv6\nyctjGOEnH43j6q17bYgrrzek3JGnCcgNIRwekWGxAoGAe6Et5hGK+jLoRmUIvoZw\nBriUxphnVgDIvZpWb3ZYNdKlHHVQh4GwClJ3cfFoTYKjI1KRdUjEAZg8dAmAr+gN\niEljJylwJzlVN6TJ43FQ2UwqcglRHGyir9lyfCaNvDJqAuGSfGF6AhRZl7MSpcou\nE9nC0XWcWIVtpOJj3T+19Jk=\n-----END PRIVATE KEY-----\n";
    const TEST_AK_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA3eEDvBsPCxMvSAVrXAjf\n56ovBkAMGETKzynL0ffL6lhelSJmaEUFS6LZfbnfpP6Eswx/wD3is3YZR8jIR3nR\nicQz66AJnoMniIu07XpCApDPNdbLJZ7O5MgmgkUVfdJ0NBs5n+1lgrDiV0cAB92E\nJQCQDHyv0i0Z6FW92xtasH5wjqzbobJnDGbe22jB/B6Rtf65t/Sq0RZ6MOZciAbq\nBh/U17npZRW+DEzH/J7VzKLKjh3uEdy6X2ideyowWZBjz8Mvj0YlYnX2LSpMKwlk\nt9Fz37GocQiBfPmhRZm9dn6Q70e4OCvu8a4Zny/El8dNJ+QtNLnJlEgkU9EHpKVb\nowIDAQAB\n-----END PUBLIC KEY-----\n";

    impl CompositeVerifier for AcceptingCompositeVerifier {
        fn verify(
            &self,
            _: CpuBundle<'_>,
            _: CompositeVerificationInput<'_>,
        ) -> Result<CompositeVerdict, CompositeVerificationError> {
            Ok(test_verdict())
        }
    }

    impl CompositeVerifier for RejectingCompositeVerifier {
        fn verify(
            &self,
            _: CpuBundle<'_>,
            _: CompositeVerificationInput<'_>,
        ) -> Result<CompositeVerdict, CompositeVerificationError> {
            Err(CompositeVerificationError {
                reason_code: "composite_appraisal_failed",
            })
        }
    }

    #[derive(Debug)]
    struct StaticCertResolver(Arc<CertifiedKey>);

    impl ResolvesServerCert for StaticCertResolver {
        fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    fn server_config(
        versions: &[&'static rustls::SupportedProtocolVersion],
        certificate: CertificateDer<'static>,
        private_key: PrivateKeyDer<'static>,
    ) -> ServerConfig {
        let provider = rustls::crypto::ring::default_provider();
        let signing_key = provider
            .key_provider
            .load_private_key(private_key)
            .expect("test signing key");
        let certified_key = CertifiedKey::new(vec![certificate], signing_key);
        ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(versions)
            .expect("test protocol versions")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(StaticCertResolver(Arc::new(certified_key))))
    }

    fn test_verdict() -> CompositeVerdict {
        let tcb = TcbVersion {
            boot_loader: None,
            tee: None,
            snp: None,
            microcode: None,
            fmc: None,
        };
        CompositeVerdict {
            verified: true,
            legs: ["cpu", "gpu"],
            substrate: String::new(),
            checked_at: SystemTime::UNIX_EPOCH,
            cpu: CpuAppraisal {
                steps: Vec::new(),
                hcla_version: 0,
                report_version: 0,
                cpuid_family: None,
                cpuid_model: None,
                cpuid_step: None,
                tcb: CpuTcb {
                    current: tcb.clone(),
                    reported: tcb.clone(),
                    committed: tcb.clone(),
                    launch: tcb,
                },
                pcr_sha256: String::new(),
                host_data_hex: String::new(),
                measurement_hex: String::new(),
                chip_id_hex: String::new(),
            },
            gpu: GpuAppraisal {
                steps: Vec::new(),
                driver_version: String::new(),
                vbios_version: String::new(),
                hwmodel: String::new(),
                ueid: String::new(),
                oemid: String::new(),
                eat_nonce: String::new(),
                claims_version: String::new(),
                arch: String::new(),
                envelope_gpu_uuid: String::new(),
            },
        }
    }

    fn test_quote(binding: &[u8; EXPORTER_BYTES]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut pcrs = Vec::new();
        pcrs.extend_from_slice(&1u32.to_le_bytes());
        pcrs.extend_from_slice(&0x000bu16.to_le_bytes());
        pcrs.push(1);
        pcrs.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
        pcrs.extend_from_slice(&[0; 5]);
        for _ in 1..8 {
            pcrs.extend_from_slice(&[0; 16]);
        }
        pcrs.extend_from_slice(&1u32.to_le_bytes());
        pcrs.extend_from_slice(&1u32.to_le_bytes());
        pcrs.extend_from_slice(&32u16.to_le_bytes());
        pcrs.extend_from_slice(&[0x42; 32]);
        pcrs.extend_from_slice(&[0; 32]);
        for _ in 1..8 {
            pcrs.extend_from_slice(&[0; 66]);
        }

        let pcr_digest = ring::digest::digest(&ring::digest::SHA256, &[0x42; 32]);
        let mut quote_message = Vec::new();
        quote_message.extend_from_slice(&0xff54_4347u32.to_be_bytes());
        quote_message.extend_from_slice(&0x8018u16.to_be_bytes());
        quote_message.extend_from_slice(&0u16.to_be_bytes());
        quote_message.extend_from_slice(&32u16.to_be_bytes());
        quote_message.extend_from_slice(binding);
        quote_message.extend_from_slice(&0u64.to_be_bytes());
        quote_message.extend_from_slice(&0u32.to_be_bytes());
        quote_message.extend_from_slice(&0u32.to_be_bytes());
        quote_message.push(1);
        quote_message.extend_from_slice(&0u64.to_be_bytes());
        quote_message.extend_from_slice(&1u32.to_be_bytes());
        quote_message.extend_from_slice(&0x000bu16.to_be_bytes());
        quote_message.push(1);
        quote_message.push(1);
        quote_message.extend_from_slice(&32u16.to_be_bytes());
        quote_message.extend_from_slice(pcr_digest.as_ref());

        let (_, private_key) = parse_x509_pem(TEST_AK_PRIVATE_PEM.as_bytes()).expect("test AK PEM");
        let key = RsaKeyPair::from_pkcs8(&private_key.contents).expect("test AK PKCS#8");
        let mut signature = vec![0; key.public().modulus_len()];
        key.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            &quote_message,
            &mut signature,
        )
        .expect("test quote signature");
        let mut quote_signature = Vec::new();
        quote_signature.extend_from_slice(&0x0014u16.to_be_bytes());
        quote_signature.extend_from_slice(&0x000bu16.to_be_bytes());
        quote_signature.extend_from_slice(&(signature.len() as u16).to_be_bytes());
        quote_signature.extend_from_slice(&signature);
        (quote_message, quote_signature, pcrs)
    }

    fn proof_response(proof: ExporterProof) -> Vec<u8> {
        let proof = proof.to_der();
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {EXPORTER_PROOF_MEDIA_TYPE}\r\nContent-Length: {}\r\n\r\n",
            proof.len()
        )
        .into_bytes()
        .into_iter()
        .chain(proof)
        .collect()
    }

    fn certificate_with_evidence(owner_nonce: &[u8]) -> (ServerConfig, CompositeEvidence) {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
        let params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
        let base_certificate = params.self_signed(&key).expect("base certificate");
        let (_, parsed) =
            X509Certificate::from_der(base_certificate.der()).expect("parse base certificate");
        let evidence = CompositeEvidence {
            owner_nonce: owner_nonce.to_vec(),
            tls_spki_der: parsed.public_key().raw.to_vec(),
            amd_report: Vec::new(),
            hcl_report: Vec::new(),
            ak_public_key_pem: TEST_AK_PUBLIC_PEM.as_bytes().to_vec(),
            quote_message: Vec::new(),
            quote_signature: Vec::new(),
            quote_pcrs: Vec::new(),
            amd_ark_pem: Vec::new(),
            amd_ask_pem: Vec::new(),
            amd_vcek_pem: Vec::new(),
            gpu_envelope: b"test GPU envelope".to_vec(),
        };
        let mut params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
        let mut extension = CustomExtension::from_oid_content(
            &[
                2,
                25,
                3_708_997_813,
                3_535_365_757,
                2_172_800_616,
                1_077_671_698,
            ],
            evidence.to_der(),
        );
        extension.set_criticality(true);
        params.custom_extensions.push(extension);
        let certificate = params.self_signed(&key).expect("evidence certificate");
        let config = server_config(
            &[&rustls::version::TLS13],
            CertificateDer::from(certificate.der().to_vec()),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        );
        (config, evidence)
    }

    fn certificate_without_evidence() -> ServerConfig {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
        let params = CertificateParams::new(vec!["spp-engine".to_owned()]).expect("params");
        let certificate = params.self_signed(&key).expect("certificate");
        server_config(
            &[&rustls::version::TLS13],
            CertificateDer::from(certificate.der().to_vec()),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
    }

    enum GatewayResponse {
        None,
        Static(Vec<u8>),
        ValidProof(Box<CompositeEvidence>),
    }

    fn start_gateway(
        config: ServerConfig,
        response: GatewayResponse,
    ) -> (u16, mpsc::Receiver<Vec<u8>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let port = listener.local_addr().expect("listener address").port();
        let (observed_sender, observed_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("client connects");
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("timeout");
            let mut preface = vec![0; PREFACE_MAGIC.len() + 32];
            socket.read_exact(&mut preface).expect("preface and nonce");
            let mut stream = StreamOwned::new(
                ServerConnection::new(Arc::new(config)).expect("server connection"),
                socket,
            );
            while stream.conn.is_handshaking() {
                if stream.conn.complete_io(&mut stream.sock).is_err() {
                    let _ = observed_sender.send(Vec::new());
                    return;
                }
            }
            let mut received = vec![0; 1024];
            let count = stream.read(&mut received).unwrap_or(0);
            received.truncate(count);
            let response = match response {
                GatewayResponse::None => None,
                GatewayResponse::Static(response) => Some(response),
                GatewayResponse::ValidProof(evidence) => {
                    let mut exporter = [0; EXPORTER_BYTES];
                    stream
                        .conn
                        .export_keying_material(
                            &mut exporter,
                            EXPORTER_LABEL,
                            Some(&exporter_context(
                                &evidence.owner_nonce,
                                &evidence.tls_spki_der,
                            )),
                        )
                        .expect("server exporter");
                    let binding = exporter_binding(
                        &evidence.owner_nonce,
                        &evidence.tls_spki_der,
                        &exporter,
                        &evidence.gpu_envelope,
                    );
                    let (quote_message, quote_signature, quote_pcrs) = test_quote(&binding);
                    Some(proof_response(ExporterProof {
                        owner_nonce: evidence.owner_nonce,
                        tls_spki_der: evidence.tls_spki_der,
                        tls_exporter: exporter.to_vec(),
                        quote_message,
                        quote_signature,
                        quote_pcrs,
                    }))
                }
            };
            if let Some(response) = response {
                stream.write_all(&response).expect("proof response");
            }
            observed_sender.send(received).expect("observation");
        });
        (port, observed_receiver, handle)
    }

    fn endpoint(port: u16) -> RatlsEndpoint {
        RatlsEndpoint::new("127.0.0.1", port)
    }

    fn establish(
        port: u16,
        verifier: &dyn CompositeVerifier,
    ) -> Result<AttestedChannel, RatlsChannelError> {
        establish_attested_channel(
            &endpoint(port),
            &[7; 32],
            Path::new("."),
            SystemTime::now(),
            None,
            None,
            None,
            verifier,
            Duration::from_secs(2),
            4,
        )
    }

    fn rejected(result: Result<AttestedChannel, RatlsChannelError>) -> RatlsChannelError {
        match result {
            Err(error) => error,
            Ok(_) => panic!("test gateway unexpectedly established a channel"),
        }
    }

    #[test]
    fn tls_client_rejects_a_tls12_only_gateway() {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
        let certificate = CertificateParams::new(vec!["spp-engine".to_owned()])
            .expect("params")
            .self_signed(&key)
            .expect("cert");
        let config = ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(certificate.der().to_vec())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .expect("TLS 1.2 server config");
        let (port, observed, handle) = start_gateway(config, GatewayResponse::None);
        assert_eq!(
            rejected(establish(port, &AcceptingCompositeVerifier)).reason_code,
            "gateway_unreachable"
        );
        let _ = observed.recv().expect("gateway observation");
        handle.join().expect("gateway thread");
    }

    #[test]
    fn certificate_rejection_writes_no_exporter_http_payload_and_closes() {
        let (config, _) = certificate_with_evidence(&[7; 32]);
        let (port, observed, handle) = start_gateway(config, GatewayResponse::None);
        let error = rejected(establish(port, &RejectingCompositeVerifier));
        assert!(observed.recv().expect("gateway observation").is_empty());
        assert_eq!(error.reason_code, "composite_appraisal_failed");
        handle.join().expect("gateway thread");
    }

    #[test]
    fn exporter_mismatch_rejects_after_a_verified_certificate() {
        let (config, evidence) = certificate_with_evidence(&[7; 32]);
        assert_eq!(
            COMPOSITE_EVIDENCE_OID,
            "2.25.3708997813.3535365757.2172800616.1077671698"
        );
        let proof = ExporterProof {
            owner_nonce: vec![7; 32],
            tls_spki_der: evidence.tls_spki_der.clone(),
            tls_exporter: vec![0; EXPORTER_BYTES],
            quote_message: Vec::new(),
            quote_signature: Vec::new(),
            quote_pcrs: Vec::new(),
        }
        .to_der();
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {EXPORTER_PROOF_MEDIA_TYPE}\r\nContent-Length: {}\r\n\r\n", proof.len()).into_bytes().into_iter().chain(proof).collect();
        let (port, observed, handle) = start_gateway(config, GatewayResponse::Static(response));
        assert_eq!(
            rejected(establish(port, &AcceptingCompositeVerifier)).reason_code,
            "exporter_mismatch"
        );
        assert!(
            observed
                .recv()
                .expect("gateway observation")
                .starts_with(b"GET /._sol/spp/exporter-proof HTTP/1.1")
        );
        handle.join().expect("gateway thread");
    }

    #[test]
    fn proof_headers_cannot_overshoot_the_cap() {
        let (config, _) = certificate_with_evidence(&[7; 32]);
        let prefix = b"HTTP/1.1 200 OK\r\nX: ";
        let suffix = b"\r\nContent-Length: 0\r\n\r\n";
        let padding_len = MAX_PROOF_RESPONSE_HEADERS + 1 - prefix.len() - (suffix.len() - 4);
        let response = prefix
            .iter()
            .copied()
            .chain(std::iter::repeat_n(b'a', padding_len))
            .chain(suffix.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(
            response
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("response marker"),
            MAX_PROOF_RESPONSE_HEADERS + 1
        );
        let (port, observed, handle) = start_gateway(config, GatewayResponse::Static(response));
        assert_eq!(
            rejected(establish(port, &AcceptingCompositeVerifier)).reason_code,
            "proof_http_failed"
        );
        assert!(
            observed
                .recv()
                .expect("gateway observation")
                .starts_with(b"GET /._sol/spp/exporter-proof HTTP/1.1")
        );
        handle.join().expect("gateway thread");
    }

    #[test]
    fn proof_headers_require_crlf_line_endings() {
        let (config, _) = certificate_with_evidence(&[7; 32]);
        let response = b"HTTP/1.1 200 OK\nContent-Length: 0\r\n\r\n".to_vec();
        let (port, observed, handle) = start_gateway(config, GatewayResponse::Static(response));
        assert_eq!(
            rejected(establish(port, &AcceptingCompositeVerifier)).reason_code,
            "proof_http_failed"
        );
        assert!(
            observed
                .recv()
                .expect("gateway observation")
                .starts_with(b"GET /._sol/spp/exporter-proof HTTP/1.1")
        );
        handle.join().expect("gateway thread");
    }

    #[test]
    fn evidence_bearing_certificate_and_bound_exporter_proof_establish_channel() {
        let (config, evidence) = certificate_with_evidence(&[7; 32]);
        let (port, observed, handle) =
            start_gateway(config, GatewayResponse::ValidProof(Box::new(evidence)));
        let channel = establish(port, &AcceptingCompositeVerifier).expect("attested channel");
        assert_eq!(channel.verified.verdict, test_verdict());
        assert!(
            observed
                .recv()
                .expect("gateway observation")
                .starts_with(b"GET /._sol/spp/exporter-proof HTTP/1.1")
        );
        handle.join().expect("gateway thread");
    }

    #[test]
    fn closed_loopback_port_is_unreachable_and_evidence_rejection_is_failed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("temporary listener");
        let closed_port = listener.local_addr().expect("address").port();
        drop(listener);
        assert_eq!(
            rejected(establish(closed_port, &AcceptingCompositeVerifier)).reason_code,
            "gateway_unreachable"
        );
        let (port, observed, handle) =
            start_gateway(certificate_without_evidence(), GatewayResponse::None);
        let error = rejected(establish(port, &AcceptingCompositeVerifier));
        assert_ne!(error.reason_code, "gateway_unreachable");
        assert_eq!(
            crate::classify_channel_failure(error.reason_code),
            AttestationFailureKind::Failed
        );
        assert!(observed.recv().expect("gateway observation").is_empty());
        handle.join().expect("gateway thread");
    }

    #[test]
    fn teardown_precedes_failure_recording() {
        let (port, observed, handle) =
            start_gateway(certificate_without_evidence(), GatewayResponse::None);
        let error = rejected(establish(port, &AcceptingCompositeVerifier));
        assert!(observed.recv().expect("gateway observed EOF").is_empty());
        handle.join().expect("gateway thread");
        let store = AttestationStateStore::new();
        store.record_attestation_failed(
            crate::classify_channel_failure(error.reason_code),
            error.reason_code,
        );
        let state = store.get_attestation_state();
        assert!(state.session.is_none());
        assert_eq!(
            state.failure.expect("failure").reason_code,
            "certificate_extension_missing"
        );
    }

    #[test]
    fn json_request_uses_plain_read_write_transport_with_bounded_response_parsing() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("listener address");
        let handle = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("client");
            let mut request = vec![0; 1024];
            let count = socket.read(&mut request).expect("read request");
            request.truncate(count);
            socket
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\n{}")
                .expect("response");
            request
        });
        let mut stream = TcpStream::connect(address).expect("connect");

        let response = send_json_request(
            &mut stream,
            "example.test",
            "/v1/chat/completions",
            Some("secret"),
            br#"{"model":"test"}"#,
        )
        .expect("application response");
        let request = handle.join().expect("server thread");

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
