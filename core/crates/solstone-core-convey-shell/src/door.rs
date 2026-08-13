// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-side paired-device door transport.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error as RustlsError,
    RootCertStore, SignatureScheme,
};
use serde_json::{Map, json};
use socket2::{SockRef, TcpKeepalive};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};
use solstone_core_convey_http::serve::{mux_builder, serve_connection};
use solstone_core_sol_link::ca::issue_server_certificate;
use solstone_core_sol_link::committed::load_committed_identity;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, read_authorized_clients,
};
use solstone_core_sol_link::pairing::nonces::{NonceStore, pairing_window_open};
use solstone_core_sol_link::{
    DeviceDoorAuthorization, DeviceDoorVerifier, spawn_authorization_refresh,
};
use spl_home::{DEFAULT_DECODER_BUFFER_BYTES, HomeConfig, HomeConnection, MuxLimits};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::{Sleep, sleep};

use crate::session::{SessionState, classify_session};
use crate::{DoorOutcome, DoorWithheldReason};

const MAX_CONCURRENT_STREAMS: usize = 8;
const AUTHORIZATION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

pub(super) struct DoorStartOptions {
    pub journal_root: std::path::PathBuf,
    pub port: u16,
    pub handshake_timeout: Duration,
    pub stream_stall_timeout: Duration,
    pub router: Router,
    pub carrier_loop_iterations: Arc<AtomicU64>,
    pub handshake_authorization_read_ticks: Arc<AtomicU64>,
    pub authorization_sender: watch::Sender<DeviceDoorAuthorization>,
}

pub(super) struct DoorStart {
    pub outcome: DoorOutcome,
    pub refresh_task: Option<tokio::task::JoinHandle<()>>,
    pub accept_task: Option<tokio::task::JoinHandle<()>>,
}

/// Identity observed from the accepted leaf. Keeping this as a struct leaves one
/// stable cell for W1b refusal classification without parallel state.
#[derive(Clone, Debug)]
struct AcceptedIdentity {
    did: LinkedDeviceDid,
}

type IdentityCell = Arc<Mutex<Option<AcceptedIdentity>>>;

/// `DeviceDoorVerifier` plus accept-local successful client identity capture.
struct DoorIdentityVerifier {
    inner: Arc<DeviceDoorVerifier>,
    identity: IdentityCell,
    pairing_window_open: bool,
}

impl std::fmt::Debug for DoorIdentityVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DoorIdentityVerifier")
            .finish_non_exhaustive()
    }
}

impl DoorIdentityVerifier {
    fn new(
        inner: Arc<DeviceDoorVerifier>,
        identity: IdentityCell,
        pairing_window_open: bool,
    ) -> Self {
        Self {
            inner,
            identity,
            pairing_window_open,
        }
    }
}

impl ClientCertVerifier for DoorIdentityVerifier {
    fn offer_client_auth(&self) -> bool {
        self.pairing_window_open || self.inner.offer_client_auth()
    }
    fn client_auth_mandatory(&self) -> bool {
        !self.pairing_window_open && self.inner.client_auth_mandatory()
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, RustlsError> {
        self.inner
            .verify_client_cert(end_entity, intermediates, now)?;
        let did = LinkedDeviceDid::try_from(
            format!("sha256:{}", spl_core::ca::sha256_hex(end_entity.as_ref())).as_str(),
        )
        .map_err(|_| {
            RustlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
        })?;
        let mut identity = self.identity.lock().map_err(|_| {
            RustlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
        })?;
        match identity.as_ref() {
            Some(existing) if existing.did != did => Err(RustlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
            Some(_) => Ok(ClientCertVerified::assertion()),
            None => {
                *identity = Some(AcceptedIdentity { did });
                Ok(ClientCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, RustlsError> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, RustlsError> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
    fn requires_raw_public_keys(&self) -> bool {
        self.inner.requires_raw_public_keys()
    }
}

pub(super) async fn start(options: DoorStartOptions) -> DoorStart {
    let withheld = match classify_session(&options.journal_root) {
        SessionState::Established => None,
        SessionState::Unestablished => Some(DoorWithheldReason::Unestablished),
        SessionState::Corrupt { .. } => Some(DoorWithheldReason::Corrupt),
    };
    if let Some(reason) = withheld {
        return DoorStart {
            outcome: DoorOutcome::Withheld(reason),
            refresh_task: None,
            accept_task: None,
        };
    }
    let identity = match load_committed_identity(&options.journal_root) {
        Ok(identity) => identity,
        Err(error) => {
            log::warn!("paired-device door withheld: committed identity unavailable: {error}");
            return DoorStart {
                outcome: DoorOutcome::Withheld(DoorWithheldReason::CommittedIdentityUnavailable),
                refresh_task: None,
                accept_task: None,
            };
        }
    };
    // Deliberately no SO_REUSEPORT. suze is both a hopper pool host and the
    // founder's live journal host, where Python owns 7657 with SO_REUSEPORT;
    // sharing it in a test would split the owner's live device connections.
    let listener =
        match TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, options.port))).await {
            Ok(listener) => listener,
            Err(source) => {
                log::error!(
                    "paired-device door could not bind port {}: {source}",
                    options.port
                );
                return DoorStart {
                    outcome: DoorOutcome::BindFailed {
                        port: options.port,
                        source,
                    },
                    refresh_task: None,
                    accept_task: None,
                };
            }
        };
    let bound = match listener.local_addr() {
        Ok(address) => address,
        Err(source) => {
            log::error!(
                "paired-device door could not inspect bound port {}: {source}",
                options.port
            );
            return DoorStart {
                outcome: DoorOutcome::BindFailed {
                    port: options.port,
                    source,
                },
                refresh_task: None,
                accept_task: None,
            };
        }
    };
    let issued = match issue_server_certificate(identity.ca(), identity.home_label()) {
        Ok(issued) => issued,
        Err(error) => {
            log::error!("paired-device door could not mint server certificate: {error}");
            return DoorStart {
                outcome: DoorOutcome::Withheld(DoorWithheldReason::CommittedIdentityUnavailable),
                refresh_task: None,
                accept_task: None,
            };
        }
    };
    // `spl_transport::tls::mtls_config` pins the CA fingerprint but does not
    // check certificate validity. The fresh server leaf therefore retains a
    // 30-day validity window; successful mTLS tests do not exercise that window.
    let mut roots = RootCertStore::empty();
    if roots
        .add(CertificateDer::from(identity.certificate_der().to_vec()))
        .is_err()
    {
        return DoorStart {
            outcome: DoorOutcome::Withheld(DoorWithheldReason::CommittedIdentityUnavailable),
            refresh_task: None,
            accept_task: None,
        };
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = match rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        provider,
    )
    .build()
    {
        Ok(verifier) => verifier,
        Err(error) => {
            log::error!("paired-device door could not build client verifier: {error}");
            return DoorStart {
                outcome: DoorOutcome::Withheld(DoorWithheldReason::CommittedIdentityUnavailable),
                refresh_task: None,
                accept_task: None,
            };
        }
    };
    // [check] This refresh adopts the sender supplied to bind_with_authorization
    // for carrier-loop revocation observation. The gate resolves its ledger
    // directly; serve() callers use the plain router and a disposable channel.
    let authorized_clients_path = AuthorizationLedger::new(&options.journal_root)
        .authorized_clients_path()
        .to_path_buf();
    let authorization = options.authorization_sender.subscribe();
    let refresh_task = spawn_authorization_refresh(
        AuthorizationLedger::new(&options.journal_root),
        options.authorization_sender,
        AUTHORIZATION_REFRESH_INTERVAL,
    );
    let config = Arc::new(DoorConnectionConfig {
        certificate_chain: vec![
            issued.certificate_der(),
            CertificateDer::from(identity.certificate_der().to_vec()),
        ],
        private_key: issued.private_key(),
        verifier,
        authorization,
        handshake_timeout: options.handshake_timeout,
        journal_root: options.journal_root,
        authorized_clients_path,
        carrier_loop_iterations: options.carrier_loop_iterations,
        handshake_authorization_read_ticks: options.handshake_authorization_read_ticks,
    });
    let accept_task = tokio::spawn(accept_loop(
        listener,
        options.router,
        config,
        options.stream_stall_timeout,
    ));
    DoorStart {
        outcome: DoorOutcome::Bound(bound),
        refresh_task: Some(refresh_task),
        accept_task: Some(accept_task),
    }
}

struct DoorConnectionConfig {
    certificate_chain: Vec<CertificateDer<'static>>,
    private_key: rustls::pki_types::PrivateKeyDer<'static>,
    verifier: Arc<dyn ClientCertVerifier>,
    authorization: watch::Receiver<DeviceDoorAuthorization>,
    handshake_timeout: Duration,
    journal_root: PathBuf,
    authorized_clients_path: PathBuf,
    carrier_loop_iterations: Arc<AtomicU64>,
    handshake_authorization_read_ticks: Arc<AtomicU64>,
}

async fn accept_loop(
    listener: TcpListener,
    router: Router,
    config: Arc<DoorConnectionConfig>,
    stream_stall_timeout: Duration,
) {
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        let router = router.clone();
        let config = config.clone();
        tokio::spawn(async move {
            serve_carrier(stream, Some(peer), router, config, stream_stall_timeout).await
        });
    }
}

async fn serve_carrier(
    stream: TcpStream,
    peer: Option<SocketAddr>,
    router: Router,
    config: Arc<DoorConnectionConfig>,
    stream_stall_timeout: Duration,
) {
    let keepalive = TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10))
        .with_retries(3);
    if let Err(error) = SockRef::from(&stream).set_tcp_keepalive(&keepalive) {
        log::debug!("paired-device door could not configure TCP keepalive: {error}");
    }
    let identity: IdentityCell = Arc::new(Mutex::new(None));
    let path = config.authorized_clients_path.clone();
    #[cfg(debug_assertions)]
    config
        .handshake_authorization_read_ticks
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let authorization = match tokio::time::timeout(
        Duration::from_millis(1000),
        tokio::task::spawn_blocking(move || read_authorized_clients(&path)),
    )
    .await
    {
        Ok(Ok(posture)) => DeviceDoorAuthorization::from(posture),
        Err(_) => {
            log::warn!("paired-device handshake authorization read timed out after 1000 ms");
            DeviceDoorAuthorization::from(AuthorizedClientsRead::Unreadable)
        }
        Ok(Err(error)) => {
            log::warn!("paired-device handshake authorization read task failed: {error}");
            DeviceDoorAuthorization::from(AuthorizedClientsRead::Unreadable)
        }
    };
    let pairing_window_open =
        pairing_window_open(&NonceStore::new(&config.journal_root), unix_seconds());
    let device_verifier = Arc::new(DeviceDoorVerifier::new(
        config.verifier.clone(),
        authorization,
    ));
    let home_config = HomeConfig {
        certificate_chain: config.certificate_chain.clone(),
        private_key: config.private_key.clone_key(),
        client_cert_verifier: Arc::new(DoorIdentityVerifier::new(
            device_verifier,
            identity.clone(),
            pairing_window_open,
        )),
        // Peer stream 9 is refused with `refuse(StreamLimit)` rather than tearing
        // down the carrier, so 8 safely bounds normal parallel requests. Per
        // carrier: 8 x 1 MiB inbound + 8 x MAX_STAGED_WRITE_BYTES_PER_STREAM
        // + the 16,777,223-byte decoder ceiling on a growing Vec = 33,554,439
        // bytes, about 32 MiB. There is no carrier cap, so this does not bound
        // memory across carriers.
        mux_limits: MuxLimits {
            max_concurrent_streams: MAX_CONCURRENT_STREAMS,
            decoder_buffer_bytes: DEFAULT_DECODER_BUFFER_BYTES,
        },
    };
    let mut connection = match tokio::time::timeout(
        config.handshake_timeout,
        HomeConnection::accept(stream, home_config),
    )
    .await
    {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            log::debug!("paired-device carrier TLS/mux failed: {error}");
            return;
        }
        Err(_) => {
            log::debug!("paired-device carrier handshake timed out");
            return;
        }
    };
    let Some(basis) = capture_to_basis(&identity, peer, pairing_window_open) else {
        log::debug!("paired-device carrier completed without an accepted identity");
        return;
    };
    let did = linked_device_did(&basis);
    if let Some(did) = &did {
        record_completed_handshake(&config.journal_root, did);
    }
    // The publisher feeds the door's carrier loop. Neither the gate nor the verifier consumes it for decisions.
    let mut authorization = config.authorization.clone();
    // `subscribe()` precedes `refresh_once()`, so this clone can inherit a
    // successful change that predates this carrier's fresh ledger handshake.
    authorization.mark_unchanged();
    let mut authorization_watch_open = true;
    loop {
        #[cfg(debug_assertions)]
        config
            .carrier_loop_iterations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::select! {
            stream = connection.accept_stream() => {
                let Ok(stream) = stream else { break; };
                let router = router.clone();
                let basis = basis.clone();
                tokio::spawn(async move {
                    let builder = mux_builder();
                    // A 60 s production bound is injected through `serve` for tests.
                    // Every non-zero successful write (including one enabled by a returned
                    // window credit during a slow 2 MiB transfer) resets this deadline.
                    let stream = StallBoundStream::new(stream, stream_stall_timeout);
                    if let Err(error) = serve_connection(stream, router, basis, &builder).await {
                        log::debug!("paired-device door stream failed: {error}");
                    }
                });
            }
            changed = authorization.changed(), if authorization_watch_open => {
                match changed {
                    Ok(()) => {
                        if did.as_ref().is_some_and(|did| {
                            close_for_revocation(authorization.borrow().as_read(), did)
                        }) {
                            let _ = connection.close();
                            break;
                        }
                    }
                    Err(_) => {
                        // The authorization publisher is closed (the refresh task stopped
                        // or panicked). Disable this arm rather than closing or spinning:
                        // the handshake and request gate resolve the ledger directly.
                        authorization_watch_open = false;
                    }
                }
            }
        }
    }
}

/// Turns sustained SPL write-credit starvation into one stream-local timeout.
struct StallBoundStream<S> {
    inner: S,
    timeout: Duration,
    stalled: Option<Pin<Box<Sleep>>>,
}

impl<S> StallBoundStream<S> {
    fn new(inner: S, timeout: Duration) -> Self {
        Self {
            inner,
            timeout,
            stalled: None,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for StallBoundStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for StallBoundStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(context, bytes) {
            Poll::Ready(Ok(written)) => {
                if written > 0 {
                    self.stalled = None;
                }
                Poll::Ready(Ok(written))
            }
            Poll::Ready(Err(error)) => {
                self.stalled = None;
                Poll::Ready(Err(error))
            }
            Poll::Pending => {
                if self.stalled.is_none() {
                    self.stalled = Some(Box::pin(sleep(self.timeout)));
                }
                if self
                    .stalled
                    .as_mut()
                    .expect("sleep created")
                    .as_mut()
                    .poll(context)
                    .is_ready()
                {
                    self.stalled = None;
                    Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "SPL stream write credit stalled",
                    )))
                } else {
                    Poll::Pending
                }
            }
        }
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

fn record_completed_handshake(journal_root: &std::path::Path, did: &LinkedDeviceDid) {
    match AuthorizationLedger::new(journal_root).touch_last_seen(did.as_str()) {
        Ok(true) => {}
        Ok(false) => log::debug!("paired-device DID was absent while recording last seen"),
        Err(error) => log::debug!("paired-device last-seen update failed: {error}"),
    }
    let mut extra = Map::new();
    extra.insert("fingerprint".to_owned(), json!(did.as_str()));
    let envelope = CallosumEnvelope {
        tract: "link".to_owned(),
        event: "last_seen".to_owned(),
        ts: None,
        extra,
    };
    let Ok(mut line) = serde_json::to_string(&envelope) else {
        return;
    };
    line.push('\n');
    // The native door has no honest tunnel identifier, so it is intentionally omitted.
    let sender = CallosumOneShotSender::new(
        journal_root.join("health/callosum.sock"),
        Duration::from_secs(1),
    );
    if sender.send_line(&line).is_err() {
        log::debug!("paired-device Callosum last-seen notification unavailable");
    }
}

fn capture_to_basis(
    identity: &IdentityCell,
    peer: Option<SocketAddr>,
    pairing_window_open: bool,
) -> Option<AccessBasis> {
    match identity.lock().ok()?.clone() {
        Some(accepted) => Some(AccessBasis::LinkedDevice {
            carrier: carrier_from_peer(peer),
            did: accepted.did,
        }),
        None if pairing_window_open => Some(AccessBasis::PairingPeer {
            carrier: carrier_from_peer(peer),
        }),
        None => None,
    }
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_secs()
        .try_into()
        .expect("Unix seconds fit i64")
}

fn linked_device_did(basis: &AccessBasis) -> Option<LinkedDeviceDid> {
    match basis {
        AccessBasis::LinkedDevice { did, .. } => Some(did.clone()),
        // A cert-less pairing carrier has no device identity to refresh or revoke.
        AccessBasis::PairingPeer { .. } => None,
        AccessBasis::Localhost => None,
    }
}

#[cfg(test)]
mod access_tests {
    use super::linked_device_did;
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceDid};

    const VALID_DID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn carrier_basis_handling_keeps_linked_identity_and_accepts_pairing_peers() {
        let linked = AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            did: LinkedDeviceDid::try_from(VALID_DID).unwrap(),
        };
        assert_eq!(linked_device_did(&linked).unwrap().as_str(), VALID_DID);

        assert!(
            linked_device_did(&AccessBasis::PairingPeer {
                carrier: Carrier::Direct,
            })
            .is_none()
        );
    }
}

fn close_for_revocation(posture: &AuthorizedClientsRead, did: &LinkedDeviceDid) -> bool {
    // The handshake fails closed on every non-`Present` posture it reads from the ledger itself.
    // Once a device is authenticated, only a definite `Present` removal observed on this arm
    // ends its carrier: a transient malformed/unreadable read must not discard captured material
    // that exists nowhere else, and a dead publication now ends no carrier at all.
    matches!(posture, AuthorizedClientsRead::Present(entries) if !entries.iter().any(|entry| entry.fingerprint == did.as_str()))
}

fn carrier_from_peer(peer: Option<SocketAddr>) -> Carrier {
    match peer.map(|address| address.ip()) {
        Some(IpAddr::V4(address)) if address.is_loopback() => Carrier::ViaSpl,
        Some(IpAddr::V6(address))
            if address.is_loopback()
                || address.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()) =>
        {
            Carrier::ViaSpl
        }
        Some(_) | None => Carrier::Direct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac5_peer_mode_vectors() {
        assert_eq!(
            carrier_from_peer(Some("127.0.0.1:1".parse().unwrap())),
            Carrier::ViaSpl
        );
        assert_eq!(
            carrier_from_peer(Some("[::ffff:127.0.0.1]:1".parse().unwrap())),
            Carrier::ViaSpl
        );
        assert_eq!(
            carrier_from_peer(Some("192.0.2.1:1".parse().unwrap())),
            Carrier::Direct
        );
        assert_eq!(carrier_from_peer(None), Carrier::Direct);
    }

    #[test]
    fn certless_window_stamps_a_pairing_basis_but_closed_window_does_not() {
        let identity = Arc::new(Mutex::new(None));
        assert!(matches!(
            capture_to_basis(&identity, Some("127.0.0.1:1".parse().unwrap()), true),
            Some(AccessBasis::PairingPeer { .. })
        ));
        assert!(capture_to_basis(&identity, Some("127.0.0.1:1".parse().unwrap()), false).is_none());
    }

    #[test]
    fn revocation_only_closes_definite_present_removal() {
        let did = LinkedDeviceDid::try_from(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        assert!(!close_for_revocation(&AuthorizedClientsRead::Missing, &did));
        assert!(!close_for_revocation(
            &AuthorizedClientsRead::Unreadable,
            &did
        ));
        assert!(!close_for_revocation(
            &AuthorizedClientsRead::Malformed,
            &did
        ));
        assert!(close_for_revocation(
            &AuthorizedClientsRead::Present(Vec::new()),
            &did
        ));
    }
}
