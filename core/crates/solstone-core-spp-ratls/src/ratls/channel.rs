// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Direct synchronous RA-TLS channel for confidential inference.
//!
//! Generate reuses a verified channel inside one process across requests.
//! There is no cross-process pool, no listener, and no daemon.
//! Transcription opens a fresh channel per request.

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls13_signature_with_raw_key};
use rustls::pki_types::{CertificateDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as RustlsError, SignatureScheme,
};
use solstone_core_spp_attest::{Policy, QuoteVerifier, nvgpu::GpuStatusAuthorization};
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::{
    error::RatlsChannelError,
    ratls::{
        contract::{
            EXPORTER_BYTES, EXPORTER_LABEL, EXPORTER_PROOF_MEDIA_TYPE, EXPORTER_PROOF_PATH,
            PREFACE_MAGIC, exporter_context,
        },
        http::{
            BoundedHttpError, MAX_PROOF_RESPONSE_BYTES, MAX_PROOF_RESPONSE_HEADERS,
            recv_bounded_http_response, response_status, retry_interrupted,
            write_all_retry_interrupted,
        },
        verify::{
            CompositeVerifier, VerifiedCertificateEvidence, verify_certificate_evidence,
            verify_exporter_proof,
        },
    },
};

type PeerCertificate = Arc<Mutex<Option<Vec<u8>>>>;

/// How long after admission an offline-status channel may start new requests.
/// The Generate pool's reuse age must never exceed it.
pub const OFFLINE_STATUS_REQUEST_WINDOW: Duration = Duration::from_secs(120);
/// Headroom kept between the last allowed request start and the deadline.
pub const OFFLINE_STATUS_ADMISSION_MARGIN: Duration = Duration::from_secs(10);
/// Remaining signed-status lifetime required when a channel is admitted.
pub const OFFLINE_STATUS_MIN_REMAINING: Duration = Duration::from_secs(
    OFFLINE_STATUS_REQUEST_WINDOW.as_secs() + OFFLINE_STATUS_ADMISSION_MARGIN.as_secs(),
);

/// This device's clocks, read at admission. Injectable for tests.
pub trait AdmissionClock {
    fn now_system(&self) -> SystemTime;
    fn now_monotonic(&self) -> Instant;
}

/// The system clocks.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemAdmissionClock;

impl AdmissionClock for SystemAdmissionClock {
    fn now_system(&self) -> SystemTime {
        SystemTime::now()
    }
    fn now_monotonic(&self) -> Instant {
        Instant::now()
    }
}

/// What a channel's GPU status authorizes after admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    /// Statuses were fetched with a fresh request nonce; the existing channel
    /// and request limits apply.
    OnlineNonce,
    /// New requests may start only inside the admission window, which ends at
    /// least the margin before this signed-status deadline.
    OfflineUntil(SystemTime),
}

/// The instant a channel was admitted, on both clocks, and its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelAdmission {
    pub at_system: SystemTime,
    pub at_monotonic: Instant,
    pub status: ChannelStatus,
}

impl ChannelAdmission {
    /// Admits a verified channel at this instant, or refuses an offline status
    /// that will not outlast the request window plus the margin.
    pub fn admit(
        gpu_status: GpuStatusAuthorization,
        at_system: SystemTime,
        at_monotonic: Instant,
    ) -> Result<Self, RatlsChannelError> {
        let status = match gpu_status {
            GpuStatusAuthorization::OnlineNonce => ChannelStatus::OnlineNonce,
            GpuStatusAuthorization::OfflineSignedAge { deadline, .. } => {
                match deadline.duration_since(at_system) {
                    Ok(remaining) if remaining >= OFFLINE_STATUS_MIN_REMAINING => {
                        ChannelStatus::OfflineUntil(deadline)
                    }
                    _ => {
                        return Err(RatlsChannelError {
                            reason_code: "status_deadline_insufficient",
                        });
                    }
                }
            }
        };
        Ok(Self {
            at_system,
            at_monotonic,
            status,
        })
    }

    /// Whether a new request may start now. Work already started is not
    /// re-checked; it finishes under the existing request and channel limits.
    pub fn permits_new_request(&self, now_system: SystemTime, now_monotonic: Instant) -> bool {
        match self.status {
            ChannelStatus::OnlineNonce => true,
            ChannelStatus::OfflineUntil(deadline) => {
                let wall = now_system
                    .duration_since(self.at_system)
                    .is_ok_and(|age| age <= OFFLINE_STATUS_REQUEST_WINDOW);
                let monotonic = now_monotonic.saturating_duration_since(self.at_monotonic)
                    <= OFFLINE_STATUS_REQUEST_WINDOW;
                wall && monotonic && now_system < deadline
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trailing {
    None,
    Eof,
    Surplus,
}

/// Read/write transport carrying application requests after attestation.
pub trait AttestedIo: Read + Write {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()>;
    fn trailing_after_body(&mut self) -> std::io::Result<Trailing>;
}

impl AttestedIo for TcpStream {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.set_read_timeout(timeout)?;
        self.set_write_timeout(timeout)
    }

    fn trailing_after_body(&mut self) -> std::io::Result<Trailing> {
        self.set_nonblocking(true)?;
        let mut buf = [0u8; 1];
        let res = match self.read(&mut buf) {
            Ok(0) => Ok(Trailing::Eof),
            Ok(_) => Ok(Trailing::Surplus),
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(Trailing::None),
            Err(e) => Err(e),
        };
        if self.set_nonblocking(false).is_err() {
            return Ok(Trailing::Surplus);
        }
        res
    }
}

impl<T: AttestedIo + ?Sized> AttestedIo for Box<T> {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        (**self).set_io_timeout(timeout)
    }

    fn trailing_after_body(&mut self) -> std::io::Result<Trailing> {
        (**self).trailing_after_body()
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
    #[error("confidential channel closed before a response")]
    ClosedBeforeResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    let mut config = ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 is supported")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PermissiveServerVerifier {
            supported_algorithms: provider.signature_verification_algorithms,
            peer_certificate: peer_certificate.clone(),
        }))
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    (Arc::new(config), peer_certificate)
}

#[derive(Debug, Clone, Default)]
pub struct RecordFramingTracker {
    header_buf: [u8; 5],
    header_pos: usize,
    payload_remaining: usize,
}

impl RecordFramingTracker {
    pub fn new() -> Self {
        Self {
            header_buf: [0u8; 5],
            header_pos: 0,
            payload_remaining: 0,
        }
    }

    pub fn is_aligned(&self) -> bool {
        self.header_pos == 0 && self.payload_remaining == 0
    }

    pub fn observe(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if self.payload_remaining > 0 {
                let take = bytes.len().min(self.payload_remaining);
                self.payload_remaining -= take;
                bytes = &bytes[take..];
            } else if self.header_pos < 5 {
                let take = (5 - self.header_pos).min(bytes.len());
                self.header_buf[self.header_pos..self.header_pos + take]
                    .copy_from_slice(&bytes[..take]);
                self.header_pos += take;
                bytes = &bytes[take..];
                if self.header_pos == 5 {
                    let len = u16::from_be_bytes([self.header_buf[3], self.header_buf[4]]) as usize;
                    self.payload_remaining = len;
                    self.header_pos = 0;
                }
            }
        }
    }
}

pub struct CountingReader<'a> {
    sock: &'a mut TcpStream,
    tracker: &'a mut RecordFramingTracker,
}

impl<'a> Read for CountingReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.sock.read(buf)?;
        if n > 0 {
            self.tracker.observe(&buf[..n]);
        }
        Ok(n)
    }
}

pub struct AttestedChannel {
    pub conn: ClientConnection,
    pub sock: TcpStream,
    pub tracker: RecordFramingTracker,
    pub verified: VerifiedCertificateEvidence,
    pub epoch: u64,
    /// Set once, when the exporter proof verified. Returning a channel to a
    /// pool never renews it.
    pub admission: ChannelAdmission,
    /// The peer closed the TCP stream, with or without close_notify.
    saw_eof: bool,
    /// The TLS layer rejected a record; nothing more is read or sent.
    tls_failed: bool,
}

impl AttestedChannel {
    /// Whether this channel's status still authorizes starting a new request.
    pub fn status_permits_new_request(
        &self,
        now_system: SystemTime,
        now_monotonic: Instant,
    ) -> bool {
        self.admission
            .permits_new_request(now_system, now_monotonic)
    }

    pub fn clean_to_reuse(&mut self) -> bool {
        let state = match self.conn.process_new_packets() {
            Ok(state) => state,
            Err(_) => return false,
        };
        !self.saw_eof
            && !self.tls_failed
            && state.plaintext_bytes_to_read() == 0
            && !state.peer_has_closed()
            && !self.conn.wants_write()
            && self.tracker.is_aligned()
    }

    pub fn alive(&mut self) -> bool {
        if !self.clean_to_reuse() {
            return false;
        }
        if self.sock.set_nonblocking(true).is_err() {
            return false;
        }
        let mut reader = CountingReader {
            sock: &mut self.sock,
            tracker: &mut self.tracker,
        };
        let res = self.conn.read_tls(&mut reader);
        let restore_ok = self.sock.set_nonblocking(false).is_ok();
        if matches!(res, Ok(0)) {
            self.saw_eof = true;
        }
        if !restore_ok {
            return false;
        }
        matches!(res, Err(e) if e.kind() == ErrorKind::WouldBlock)
    }

    /// Writes all pending TLS records, retrying interrupted socket writes.
    ///
    /// Plaintext is accepted into the TLS layer exactly once by the caller; this
    /// only drains ciphertext, so an interrupt can never duplicate request bytes.
    /// Any other socket error is returned with its original kind.
    fn drain_tls(&mut self) -> std::io::Result<()> {
        while self.conn.wants_write() {
            match self.conn.write_tls(&mut self.sock) {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl AttestedIo for AttestedChannel {
    fn set_io_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(timeout)?;
        self.sock.set_write_timeout(timeout)
    }

    fn trailing_after_body(&mut self) -> std::io::Result<Trailing> {
        let state = match self.conn.process_new_packets() {
            Ok(state) => state,
            Err(error) => {
                self.tls_failed = true;
                return Err(std::io::Error::new(ErrorKind::InvalidData, error));
            }
        };
        if state.plaintext_bytes_to_read() > 0 {
            return Ok(Trailing::Surplus);
        }
        if state.peer_has_closed() {
            return Ok(Trailing::Eof);
        }
        if self.sock.set_nonblocking(true).is_err() {
            return Ok(Trailing::Surplus);
        }
        let mut reader = CountingReader {
            sock: &mut self.sock,
            tracker: &mut self.tracker,
        };
        let res = self.conn.read_tls(&mut reader);
        let restore_ok = self.sock.set_nonblocking(false).is_ok();
        if !restore_ok {
            return Ok(Trailing::Surplus);
        }
        match res {
            Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(Trailing::None),
            Ok(0) => {
                self.saw_eof = true;
                Ok(Trailing::Eof)
            }
            Ok(_) => Ok(Trailing::Surplus),
            Err(_) => Ok(Trailing::Surplus),
        }
    }
}

impl Read for AttestedChannel {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.conn.reader().read(buffer) {
                // Ok(0) here means close_notify: end of stream now, without waiting
                // on a peer that may hold the socket half-open.
                Ok(n) => return Ok(n),
                // A TCP close without close_notify, once seen, reads as end of stream.
                Err(e) if e.kind() == ErrorKind::UnexpectedEof && self.saw_eof => return Ok(0),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if self.tls_failed {
                        return Err(std::io::Error::new(
                            ErrorKind::InvalidData,
                            "the TLS layer rejected a record",
                        ));
                    }
                    if self.saw_eof {
                        return Ok(0);
                    }
                    let mut reader = CountingReader {
                        sock: &mut self.sock,
                        tracker: &mut self.tracker,
                    };
                    if self.conn.read_tls(&mut reader)? == 0 {
                        self.saw_eof = true;
                    }
                    if let Err(error) = self.conn.process_new_packets() {
                        // Hand back plaintext that arrived before the TLS error, so a
                        // complete response is not lost to an alert behind it. The
                        // failure is latched so the next read fails at once.
                        self.tls_failed = true;
                        return match self.conn.reader().read(buffer) {
                            Ok(n) if n > 0 => Ok(n),
                            _ => Err(std::io::Error::new(ErrorKind::InvalidData, error)),
                        };
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Write for AttestedChannel {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        // Drain first, so the TLS layer has room and a socket error from earlier
        // bytes surfaces before more plaintext is accepted.
        self.drain_tls()?;
        let n = self.conn.writer().write(buffer)?;
        self.drain_tls()?;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.conn.writer().flush()?;
        self.drain_tls()?;
        retry_interrupted(|| self.sock.flush())
    }
}

#[allow(clippy::too_many_arguments)]
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
    establish_attested_channel_with_clock(
        endpoint,
        owner_nonce,
        nvattest_dir,
        now,
        roots_dir,
        policy,
        quote_verifier,
        composite_verifier,
        socket_timeout,
        epoch,
        &SystemAdmissionClock,
    )
}

/// [`establish_attested_channel`] with an injected admission clock.
#[allow(clippy::too_many_arguments)]
pub fn establish_attested_channel_with_clock(
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
    clock: &dyn AdmissionClock,
) -> Result<AttestedChannel, RatlsChannelError> {
    refuse_held_platform(std::env::consts::OS)?;
    establish_unheld_channel_with_clock(
        endpoint,
        owner_nonce,
        nvattest_dir,
        now,
        roots_dir,
        policy,
        quote_verifier,
        composite_verifier,
        socket_timeout,
        epoch,
        clock,
    )
}

/// Establishes only a literal loopback endpoint for native fixture qualification.
/// This entry is absent from builds without the test-hooks feature.
#[cfg(feature = "test-hooks")]
#[allow(clippy::too_many_arguments)]
pub fn establish_local_test_channel_with_clock(
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
    clock: &dyn AdmissionClock,
) -> Result<AttestedChannel, RatlsChannelError> {
    if !endpoint
        .host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
    {
        return Err(RatlsChannelError {
            reason_code: "test_endpoint_not_loopback",
        });
    }
    establish_unheld_channel_with_clock(
        endpoint,
        owner_nonce,
        nvattest_dir,
        now,
        roots_dir,
        policy,
        quote_verifier,
        composite_verifier,
        socket_timeout,
        epoch,
        clock,
    )
}

#[allow(clippy::too_many_arguments)]
fn establish_unheld_channel_with_clock(
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
    clock: &dyn AdmissionClock,
) -> Result<AttestedChannel, RatlsChannelError> {
    let addresses = resolve_engine_addresses(endpoint)?;
    let connect_timeout = if addresses.len() > 1 {
        socket_timeout.min(MULTI_ADDRESS_CONNECT_TIMEOUT)
    } else {
        socket_timeout
    };
    let started = Instant::now();
    establish_over_addresses(
        &addresses,
        owner_nonce,
        fresh_owner_nonce,
        || started.elapsed() < socket_timeout,
        |address, nonce| {
            // A later address is appraised at the time it is reached.
            establish_at_address(
                address,
                endpoint,
                nonce,
                nvattest_dir,
                now + started.elapsed(),
                roots_dir,
                policy,
                quote_verifier,
                composite_verifier,
                connect_timeout,
                socket_timeout,
                epoch,
                clock,
            )
        },
    )
}

/// Refuses before anything is resolved, connected or sent when owners on `os`
/// are held back from confidential processing. Every channel to the service,
/// production or qualification, starts here.
pub(crate) fn refuse_held_platform(os: &str) -> Result<(), RatlsChannelError> {
    if crate::nvattest_authority::owner_use_held(os) {
        return Err(RatlsChannelError {
            reason_code: "nvattest_platform_unsupported",
        });
    }
    Ok(())
}

/// Most engine addresses one establishment walks. The engine name normally
/// carries one to a few engines; the cap bounds what a bad answer can cost.
pub const MAX_ENGINE_ADDRESSES: usize = 8;
/// Connect budget per address when the name resolves to more than one engine,
/// so an unreachable engine hands over to the next instead of holding the
/// whole socket timeout.
pub const MULTI_ADDRESS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Every address the engine name resolves to, deduplicated, in random order,
/// capped at [`MAX_ENGINE_ADDRESSES`].
///
/// Identity is the attestation, never the address: each address is appraised
/// on its own before anything is sent, so a wrong or stale answer fails closed
/// exactly as a single wrong address always has. The random order spreads new
/// channels across engines; nothing about the choice is kept or sent.
fn resolve_engine_addresses(
    endpoint: &RatlsEndpoint,
) -> Result<Vec<SocketAddr>, RatlsChannelError> {
    let resolved = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    let mut addresses: Vec<SocketAddr> = Vec::new();
    for address in resolved {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    if addresses.is_empty() {
        return Err(RatlsChannelError {
            reason_code: "gateway_unreachable",
        });
    }
    shuffle_addresses(&mut addresses, &ring::rand::SystemRandom::new());
    addresses.truncate(MAX_ENGINE_ADDRESSES);
    Ok(addresses)
}

/// Fisher-Yates over the resolved addresses. Order is load spreading, not
/// security, so an entropy failure keeps the resolver's order.
fn shuffle_addresses(addresses: &mut [SocketAddr], rng: &dyn ring::rand::SecureRandom) {
    for index in (1..addresses.len()).rev() {
        let mut bytes = [0u8; 4];
        if rng.fill(&mut bytes).is_err() {
            return;
        }
        let pick = (u32::from_le_bytes(bytes) as usize) % (index + 1);
        addresses.swap(index, pick);
    }
}

fn fresh_owner_nonce(length: usize) -> Result<Vec<u8>, RatlsChannelError> {
    use ring::rand::SecureRandom;

    let mut nonce = vec![0u8; length];
    ring::rand::SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| RatlsChannelError {
            reason_code: "nonce_generation_failed",
        })?;
    Ok(nonce)
}

/// A failure on this device rather than at the engine: another address
/// cannot fix it, so the walk stops.
fn is_local_failure(reason_code: &str) -> bool {
    reason_code.starts_with("nvattest_") || reason_code == "nonce_generation_failed"
}

/// Which refusal to report when every address fails: an appraisal verdict from
/// an engine that answered outranks a transport or protocol failure, which
/// outranks an engine nobody reached.
fn refusal_rank(reason_code: &str) -> u8 {
    match reason_code {
        "gateway_unreachable" => 0,
        "tls_handshake_failed" | "proof_http_failed" => 1,
        _ => 2,
    }
}

/// Tries each address in turn until one admits a fully appraised channel.
///
/// The first attempt uses the caller's nonce; each later attempt draws a fresh
/// one. No new attempt starts once the walk's budget is spent. When every
/// address fails, the highest-ranked refusal is reported, so a rejected
/// appraisal is never reported as a mere outage.
fn establish_over_addresses<T>(
    addresses: &[SocketAddr],
    owner_nonce: &[u8],
    mut next_nonce: impl FnMut(usize) -> Result<Vec<u8>, RatlsChannelError>,
    within_budget: impl Fn() -> bool,
    mut attempt: impl FnMut(SocketAddr, &[u8]) -> Result<T, RatlsChannelError>,
) -> Result<T, RatlsChannelError> {
    let mut reported: Option<RatlsChannelError> = None;
    for (index, address) in addresses.iter().enumerate() {
        // The whole walk shares one socket-timeout budget: a stalled engine
        // costs one timeout, never one per address.
        if index > 0 && !within_budget() {
            break;
        }
        let drawn;
        let nonce = if index == 0 {
            owner_nonce
        } else {
            drawn = next_nonce(owner_nonce.len())?;
            drawn.as_slice()
        };
        match attempt(*address, nonce) {
            Ok(channel) => return Ok(channel),
            Err(error) if is_local_failure(error.reason_code) => return Err(error),
            Err(error) => {
                let replace = match &reported {
                    None => true,
                    Some(previous) => {
                        refusal_rank(error.reason_code) > refusal_rank(previous.reason_code)
                    }
                };
                if replace {
                    reported = Some(error);
                }
            }
        }
    }
    Err(reported.unwrap_or(RatlsChannelError {
        reason_code: "gateway_unreachable",
    }))
}

#[allow(clippy::too_many_arguments)]
fn establish_at_address(
    address: SocketAddr,
    endpoint: &RatlsEndpoint,
    owner_nonce: &[u8],
    nvattest_dir: &Path,
    now: SystemTime,
    roots_dir: Option<&Path>,
    policy: Option<&Policy>,
    quote_verifier: Option<&dyn QuoteVerifier>,
    composite_verifier: &dyn CompositeVerifier,
    connect_timeout: Duration,
    socket_timeout: Duration,
    epoch: u64,
    clock: &dyn AdmissionClock,
) -> Result<AttestedChannel, RatlsChannelError> {
    let mut socket =
        TcpStream::connect_timeout(&address, connect_timeout).map_err(|_| RatlsChannelError {
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
    let mut connection = ClientConnection::new(config, name).map_err(|_| RatlsChannelError {
        reason_code: "tls_handshake_failed",
    })?;
    let mut tracker = RecordFramingTracker::new();
    while connection.is_handshaking() {
        struct CountingIo<'a> {
            sock: &'a mut TcpStream,
            tracker: &'a mut RecordFramingTracker,
        }
        impl<'a> Read for CountingIo<'a> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.sock.read(buf)?;
                if n > 0 {
                    self.tracker.observe(&buf[..n]);
                }
                Ok(n)
            }
        }
        impl<'a> Write for CountingIo<'a> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.sock.write(buf)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.sock.flush()
            }
        }
        let mut io = CountingIo {
            sock: &mut socket,
            tracker: &mut tracker,
        };
        connection
            .complete_io(&mut io)
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
    connection
        .export_keying_material(
            &mut exporter,
            EXPORTER_LABEL,
            Some(&exporter_context(owner_nonce, &verified.tls_spki_der)),
        )
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;

    // Provisional until the exporter proof verifies; replaced below.
    let provisional = ChannelAdmission {
        at_system: now,
        at_monotonic: clock.now_monotonic(),
        status: ChannelStatus::OfflineUntil(SystemTime::UNIX_EPOCH),
    };
    let mut channel = AttestedChannel {
        conn: connection,
        sock: socket,
        tracker,
        verified,
        epoch,
        admission: provisional,
        saw_eof: false,
        tls_failed: false,
    };

    let request = format!(
        "GET {EXPORTER_PROOF_PATH} HTTP/1.1\r\nHost: spp-engine\r\nContent-Length: 0\r\n\r\n"
    );
    channel
        .write_all(request.as_bytes())
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    let proof = recv_proof_response(&mut channel)?;
    verify_exporter_proof(
        &proof,
        &channel.verified.evidence,
        &exporter,
        owner_nonce,
        policy,
    )
    .map_err(|error| RatlsChannelError {
        reason_code: error.reason_code,
    })?;
    // Admission: reread this device's clock now that the exporter proof has
    // verified, so time spent in the exchange counts against the status.
    channel.admission = ChannelAdmission::admit(
        channel.verified.verdict.gpu.status,
        clock.now_system(),
        clock.now_monotonic(),
    )?;
    channel
        .sock
        .set_read_timeout(None)
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    channel
        .sock
        .set_write_timeout(None)
        .map_err(|_| RatlsChannelError {
            reason_code: "gateway_unreachable",
        })?;
    Ok(channel)
}

fn recv_proof_response(channel: &mut AttestedChannel) -> Result<Vec<u8>, RatlsChannelError> {
    let response = recv_bounded_http_response(
        channel,
        MAX_PROOF_RESPONSE_HEADERS,
        MAX_PROOF_RESPONSE_BYTES,
    )
    .map_err(|_| RatlsChannelError {
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
    match channel.trailing_after_body() {
        Ok(Trailing::None) if channel.clean_to_reuse() => Ok(response.body),
        // A close right after the proof is not a bad proof. The ended channel
        // fails its first request and is never pooled.
        Ok(Trailing::Eof) => Ok(response.body),
        _ => Err(RatlsChannelError {
            reason_code: "proof_http_failed",
        }),
    }
}

fn http_error(error: BoundedHttpError) -> AttestedHttpError {
    match error {
        BoundedHttpError::Transport(error) => AttestedHttpError::Transport(error),
        BoundedHttpError::Protocol(msg) => AttestedHttpError::Protocol(msg),
    }
}

struct CountingStream<'a> {
    stream: &'a mut dyn AttestedIo,
    bytes_read: usize,
}

impl Read for CountingStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.stream.read(buf)?;
        self.bytes_read += n;
        Ok(n)
    }
}

fn peer_closed_kind(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::UnexpectedEof
            | ErrorKind::ConnectionReset
            | ErrorKind::BrokenPipe
            | ErrorKind::ConnectionAborted
    )
}

/// Sends one JSON POST over an already attested transport.
pub fn send_json_request(
    stream: &mut dyn AttestedIo,
    host: &str,
    path: &str,
    bearer: Option<&str>,
    body: &[u8],
    checked_out: bool,
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
    if let Err(error) = write_all_retry_interrupted(stream, request.as_bytes())
        .and_then(|_| write_all_retry_interrupted(stream, body))
    {
        // On a pooled channel the service closed while we were writing: nothing it
        // could act on was received, so this is the retryable "closed" outcome.
        if checked_out && peer_closed_kind(error.kind()) {
            return Err(AttestedHttpError::ClosedBeforeResponse);
        }
        return Err(AttestedHttpError::Transport(error));
    }

    let mut counting = CountingStream {
        stream,
        bytes_read: 0,
    };
    let response = match recv_bounded_http_response(
        &mut counting,
        MAX_PROOF_RESPONSE_HEADERS,
        MAX_PROOF_RESPONSE_BYTES,
    ) {
        Ok(resp) => resp,
        Err(BoundedHttpError::Protocol("response_eof"))
            if checked_out && counting.bytes_read == 0 =>
        {
            return Err(AttestedHttpError::ClosedBeforeResponse);
        }
        Err(BoundedHttpError::Transport(err))
            if checked_out && counting.bytes_read == 0 && peer_closed_kind(err.kind()) =>
        {
            return Err(AttestedHttpError::ClosedBeforeResponse);
        }
        Err(err) => return Err(http_error(err)),
    };
    let status = response_status(&response.status_line).map_err(AttestedHttpError::Protocol)?;
    match stream.trailing_after_body() {
        Ok(Trailing::Surplus) => return Err(AttestedHttpError::Protocol("response_surplus")),
        // The response is complete; a close behind it only means this channel is
        // not reused, which clean_to_reuse already reflects.
        Ok(Trailing::None | Trailing::Eof) => {}
        // A TLS error behind a complete success is kept the same way. Behind any
        // other status the caller may send again (a refit), so fail now.
        Err(_) if (200..300).contains(&status) => {}
        Err(err) => return Err(AttestedHttpError::Transport(err)),
    }
    Ok(AttestedHttpResponse {
        status,
        body: response.body,
    })
}

/// Posts one qualification multipart transcription request over an already attested transport.
///
/// This is the qualification probe's transcription post, not the production ASR path.
pub(crate) fn send_transcription_request(
    stream: &mut dyn AttestedIo,
    host: &str,
    credential: Option<&str>,
    wav: &[u8],
) -> Result<AttestedHttpResponse, AttestedHttpError> {
    use ring::rand::SecureRandom;

    let mut boundary_bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut boundary_bytes)
        .map_err(|_| AttestedHttpError::Protocol("transcription_entropy_failed"))?;
    let boundary = format!(
        "solstone-confidential-stt-{}",
        boundary_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(
        format!(
            "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"response_format\"\r\n\r\nverbose_json\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"timestamp_granularities[]=word\"\r\n\r\nword\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let mut request = format!(
        "POST /v1/audio/transcriptions HTTP/1.1\r\nHost: {host}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(bearer) = credential {
        request.push_str("Authorization: Bearer ");
        request.push_str(bearer);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");

    write_all_retry_interrupted(stream, request.as_bytes())
        .and_then(|_| write_all_retry_interrupted(stream, &body))
        .and_then(|_| stream.flush())
        .map_err(AttestedHttpError::Transport)?;
    let response =
        recv_bounded_http_response(stream, MAX_PROOF_RESPONSE_HEADERS, MAX_PROOF_RESPONSE_BYTES)
            .map_err(http_error)?;
    let status = response_status(&response.status_line).map_err(AttestedHttpError::Protocol)?;
    Ok(AttestedHttpResponse {
        status,
        body: response.body,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn engine(last: u8) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, last], 9443))
    }

    fn refused(reason_code: &'static str) -> RatlsChannelError {
        RatlsChannelError { reason_code }
    }

    fn fixed_nonce(length: usize) -> Result<Vec<u8>, RatlsChannelError> {
        Ok(vec![0xAB; length])
    }

    #[test]
    fn no_shipped_platform_is_refused_a_channel() {
        for os in ["linux", "macos", "windows"] {
            assert_eq!(refuse_held_platform(os), Ok(()), "{os}");
        }
    }

    #[test]
    fn first_admitting_address_ends_the_walk() {
        let mut tried = Vec::new();
        let result = establish_over_addresses(
            &[engine(1), engine(2)],
            &[1u8; 32],
            fixed_nonce,
            || true,
            |address, _nonce| {
                tried.push(address);
                Ok::<_, RatlsChannelError>(address)
            },
        );
        assert_eq!(result, Ok(engine(1)));
        assert_eq!(tried, vec![engine(1)]);
    }

    #[test]
    fn an_unreachable_engine_hands_over_to_the_next_with_a_fresh_nonce() {
        let mut nonces = Vec::new();
        let result = establish_over_addresses(
            &[engine(1), engine(2)],
            &[1u8; 32],
            fixed_nonce,
            || true,
            |address, nonce| {
                nonces.push(nonce.to_vec());
                if address == engine(1) {
                    Err(refused("gateway_unreachable"))
                } else {
                    Ok(address)
                }
            },
        );
        assert_eq!(result, Ok(engine(2)));
        assert_eq!(nonces, vec![vec![1u8; 32], vec![0xAB; 32]]);
    }

    #[test]
    fn a_rejected_appraisal_on_one_engine_does_not_block_an_admitted_one() {
        let result = establish_over_addresses(
            &[engine(1), engine(2)],
            &[1u8; 32],
            fixed_nonce,
            || true,
            |address, _nonce| {
                if address == engine(1) {
                    Err(refused("pcr_pin_mismatch"))
                } else {
                    Ok(address)
                }
            },
        );
        assert_eq!(result, Ok(engine(2)));
    }

    #[test]
    fn a_local_failure_stops_the_walk() {
        let mut tried = 0;
        let result: Result<SocketAddr, _> = establish_over_addresses(
            &[engine(1), engine(2)],
            &[1u8; 32],
            fixed_nonce,
            || true,
            |_address, _nonce| {
                tried += 1;
                Err(refused("nvattest_unavailable"))
            },
        );
        assert_eq!(result, Err(refused("nvattest_unavailable")));
        assert_eq!(tried, 1);
    }

    #[test]
    fn an_answering_engine_s_refusal_is_reported_over_an_outage_in_either_order() {
        for order in [
            ["gateway_unreachable", "pcr_pin_mismatch"],
            ["pcr_pin_mismatch", "gateway_unreachable"],
        ] {
            let mut reasons = order.into_iter();
            let result: Result<SocketAddr, _> = establish_over_addresses(
                &[engine(1), engine(2)],
                &[1u8; 32],
                fixed_nonce,
                || true,
                |_address, _nonce| Err(refused(reasons.next().unwrap())),
            );
            assert_eq!(result, Err(refused("pcr_pin_mismatch")));
        }
    }

    #[test]
    fn every_engine_unreachable_reads_as_unreachable() {
        let result: Result<SocketAddr, _> = establish_over_addresses(
            &[engine(1), engine(2), engine(3)],
            &[1u8; 32],
            fixed_nonce,
            || true,
            |_address, _nonce| Err(refused("gateway_unreachable")),
        );
        assert_eq!(result, Err(refused("gateway_unreachable")));
    }

    #[test]
    fn a_single_address_keeps_the_callers_nonce_and_its_refusal() {
        let mut nonces = Vec::new();
        let result: Result<SocketAddr, _> = establish_over_addresses(
            &[engine(1)],
            &[7u8; 32],
            |_length| panic!("a single address never draws a second nonce"),
            || true,
            |_address, nonce| {
                nonces.push(nonce.to_vec());
                Err(refused("pcr_pin_mismatch"))
            },
        );
        assert_eq!(result, Err(refused("pcr_pin_mismatch")));
        assert_eq!(nonces, vec![vec![7u8; 32]]);
    }

    #[test]
    fn a_failed_nonce_draw_stops_the_walk() {
        let result: Result<SocketAddr, _> = establish_over_addresses(
            &[engine(1), engine(2)],
            &[1u8; 32],
            |_length| Err(refused("nonce_generation_failed")),
            || true,
            |_address, _nonce| Err(refused("gateway_unreachable")),
        );
        assert_eq!(result, Err(refused("nonce_generation_failed")));
    }

    #[test]
    fn a_spent_budget_starts_no_further_attempt() {
        let mut tried = 0;
        let result: Result<SocketAddr, _> = establish_over_addresses(
            &[engine(1), engine(2), engine(3)],
            &[1u8; 32],
            fixed_nonce,
            || false,
            |_address, _nonce| {
                tried += 1;
                Err(refused("gateway_unreachable"))
            },
        );
        assert_eq!(result, Err(refused("gateway_unreachable")));
        assert_eq!(tried, 1);
    }

    #[test]
    fn an_appraisal_verdict_outranks_a_protocol_failure() {
        let mut reasons = [
            "proof_http_failed",
            "pcr_pin_mismatch",
            "tls_handshake_failed",
        ]
        .into_iter();
        let result: Result<SocketAddr, _> = establish_over_addresses(
            &[engine(1), engine(2), engine(3)],
            &[1u8; 32],
            fixed_nonce,
            || true,
            |_address, _nonce| Err(refused(reasons.next().unwrap())),
        );
        assert_eq!(result, Err(refused("pcr_pin_mismatch")));
    }

    #[test]
    fn shuffling_keeps_every_address_once() {
        let original: Vec<SocketAddr> = (1..=8).map(engine).collect();
        let mut shuffled = original.clone();
        shuffle_addresses(&mut shuffled, &ring::rand::SystemRandom::new());
        let mut sorted = shuffled.clone();
        sorted.sort();
        assert_eq!(sorted, original);
    }

    #[test]
    fn a_literal_address_resolves_to_itself_alone() {
        let endpoint = RatlsEndpoint::new("127.0.0.1", 9443);
        assert_eq!(
            resolve_engine_addresses(&endpoint),
            Ok(vec![SocketAddr::from(([127, 0, 0, 1], 9443))])
        );
    }

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
        fn set_io_timeout(&mut self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
        fn trailing_after_body(&mut self) -> std::io::Result<Trailing> {
            if self.unread.position() < self.unread.get_ref().len() as u64 {
                Ok(Trailing::Surplus)
            } else {
                Ok(Trailing::None)
            }
        }
    }

    fn offline(deadline: SystemTime) -> GpuStatusAuthorization {
        GpuStatusAuthorization::OfflineSignedAge {
            verified_at: SystemTime::UNIX_EPOCH,
            deadline,
        }
    }

    #[test]
    fn admission_needs_the_request_window_plus_margin_of_signed_status() {
        assert_eq!(OFFLINE_STATUS_MIN_REMAINING, Duration::from_secs(130));
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_996_648);
        let mono = Instant::now();
        let exact = ChannelAdmission::admit(offline(at + Duration::from_secs(130)), at, mono)
            .expect("130 s remaining admits");
        assert_eq!(
            exact.status,
            ChannelStatus::OfflineUntil(at + Duration::from_secs(130))
        );
        assert_eq!(exact.at_system, at);
        for short in [129, 0] {
            assert_eq!(
                ChannelAdmission::admit(offline(at + Duration::from_secs(short)), at, mono),
                Err(RatlsChannelError {
                    reason_code: "status_deadline_insufficient"
                }),
                "{short}"
            );
        }
        assert!(ChannelAdmission::admit(offline(at - Duration::from_secs(1)), at, mono).is_err());
        // The online status needs no signed-age headroom.
        assert_eq!(
            ChannelAdmission::admit(GpuStatusAuthorization::OnlineNonce, at, mono)
                .expect("online")
                .status,
            ChannelStatus::OnlineNonce
        );
    }

    #[test]
    fn new_requests_start_only_inside_the_admission_window_on_both_clocks() {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_996_648);
        let mono = Instant::now();
        let admission =
            ChannelAdmission::admit(offline(at + Duration::from_secs(130)), at, mono).unwrap();
        let window = OFFLINE_STATUS_REQUEST_WINDOW;
        assert!(admission.permits_new_request(at, mono));
        assert!(admission.permits_new_request(at + window, mono + window));
        let past = window + Duration::from_secs(1);
        assert!(!admission.permits_new_request(at + past, mono + window));
        assert!(!admission.permits_new_request(at + window, mono + past));
        // A wall clock set backwards is not a fresh window.
        assert!(!admission.permits_new_request(at - Duration::from_secs(1), mono));
        // The deadline itself still bounds a window that a later deadline
        // would otherwise allow.
        let tight = ChannelAdmission {
            at_system: at,
            at_monotonic: mono,
            status: ChannelStatus::OfflineUntil(at + Duration::from_secs(5)),
        };
        assert!(tight.permits_new_request(at + Duration::from_secs(4), mono));
        assert!(!tight.permits_new_request(at + Duration::from_secs(5), mono));
        // Online channels keep their existing limits only.
        let online =
            ChannelAdmission::admit(GpuStatusAuthorization::OnlineNonce, at, mono).unwrap();
        assert!(online.permits_new_request(at + 10 * window, mono + 10 * window));
    }

    #[test]
    fn json_request_has_exact_authorized_content_length_and_body_framing() {
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
            false,
        )
        .expect("application response");
        let request = stream.written;

        assert_eq!(response.status, 201);
        assert_eq!(response.body, br"{}");
        assert_eq!(
            request,
            b"POST /v1/chat/completions HTTP/1.1\r\nHost: example.test\r\nContent-Type: application/json\r\nContent-Length: 16\r\nAuthorization: Bearer secret\r\n\r\n{\"model\":\"test\"}"
        );
    }

    #[test]
    fn json_request_without_bearer_has_no_authorization_line() {
        let mut stream = ScriptedIo {
            written: Vec::new(),
            unread: Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec()),
        };
        send_json_request(
            &mut stream,
            "spp-engine",
            "/v1/chat/completions",
            None,
            br#"{}"#,
            false,
        )
        .expect("application response");
        assert_eq!(
            stream.written,
            b"POST /v1/chat/completions HTTP/1.1\r\nHost: spp-engine\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"
        );
    }

    #[test]
    fn json_response_parser_rejects_truncated_and_oversized_bodies() {
        for response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n{}".to_vec(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                MAX_PROOF_RESPONSE_BYTES + 1
            )
            .into_bytes(),
        ] {
            let mut stream = ScriptedIo {
                written: Vec::new(),
                unread: Cursor::new(response),
            };
            assert!(matches!(
                send_json_request(&mut stream, "host", "/path", None, br#"{}"#, false),
                Err(AttestedHttpError::Protocol(
                    "response_body_eof" | "response_content_length_invalid"
                ))
            ));
        }
    }

    struct ResetOnWrite {
        kind: ErrorKind,
    }

    impl Read for ResetOnWrite {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }
    impl Write for ResetOnWrite {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(self.kind))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl AttestedIo for ResetOnWrite {
        fn set_io_timeout(&mut self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
        fn trailing_after_body(&mut self) -> std::io::Result<Trailing> {
            Ok(Trailing::Eof)
        }
    }

    #[test]
    fn a_pooled_channel_closed_while_writing_is_the_closed_outcome() {
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionAborted,
            ErrorKind::UnexpectedEof,
        ] {
            let pooled =
                send_json_request(&mut ResetOnWrite { kind }, "h", "/p", None, b"{}", true);
            assert!(
                matches!(pooled, Err(AttestedHttpError::ClosedBeforeResponse)),
                "{kind:?}"
            );
            let fresh =
                send_json_request(&mut ResetOnWrite { kind }, "h", "/p", None, b"{}", false);
            assert!(
                matches!(fresh, Err(AttestedHttpError::Transport(ref error)) if error.kind() == kind),
                "{kind:?}"
            );
        }
        let other = send_json_request(
            &mut ResetOnWrite {
                kind: ErrorKind::PermissionDenied,
            },
            "h",
            "/p",
            None,
            b"{}",
            true,
        );
        assert!(matches!(other, Err(AttestedHttpError::Transport(_))));
    }
}
