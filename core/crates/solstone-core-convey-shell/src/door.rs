// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-side paired-device door transport.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::{Extension, Router};
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
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::time::{Sleep, sleep};

use crate::session::{SessionState, classify_session};
use crate::{DoorOutcome, DoorWithheldReason};

const MAX_CONCURRENT_STREAMS: usize = 8;
const AUTHORIZATION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const PAIRING_REAPER_INTERVAL: Duration = Duration::from_millis(250);
const MAX_PAIRING_CARRIERS: usize = 4;
const MAX_PAIRING_FAILURES: usize = 3;

/// A stream accepted during an open pairing window. The outer confinement
/// layer preserves that queued admission while retaining its authoritative
/// closed-window check for streams accepted later.
#[derive(Clone, Copy)]
pub(crate) struct PairingWindowAdmission;

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
    pub pairing_reaper_task: Option<tokio::task::JoinHandle<()>>,
    pub pairing_cap_refusals: Option<Arc<AtomicU64>>,
}

trait PairingDelay: Send + Sync {
    fn delay(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

struct TokioPairingDelay;

impl PairingDelay for TokioPairingDelay {
    fn delay(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(sleep(duration))
    }
}

struct PairingCarrierState {
    pair_lock: AsyncMutex<()>,
    in_flight: AtomicUsize,
    failures: AtomicUsize,
    close_after_response: AtomicBool,
    close_sender: watch::Sender<bool>,
    delay: Arc<dyn PairingDelay>,
}

impl PairingCarrierState {
    fn new(delay: Arc<dyn PairingDelay>) -> (Arc<Self>, watch::Receiver<bool>) {
        let (close_sender, close_receiver) = watch::channel(false);
        (
            Arc::new(Self {
                pair_lock: AsyncMutex::new(()),
                in_flight: AtomicUsize::new(0),
                failures: AtomicUsize::new(0),
                close_after_response: AtomicBool::new(false),
                close_sender,
                delay,
            }),
            close_receiver,
        )
    }

    fn close(&self) {
        let _ = self.close_sender.send(true);
    }

    async fn dispatch_pair<F>(&self, dispatch: F) -> Response
    where
        F: Future<Output = Response>,
    {
        // Count from queue admission, not lock acquisition: the reaper must
        // preserve a request waiting behind another ceremony.
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        let _in_flight = InFlightPair(&self.in_flight);
        let _lock = self.pair_lock.lock().await;
        let response = dispatch.await;
        record_pair_dispatch(self, response.status()).await;
        response
    }
}

struct PairingCarrierRegistry {
    active: AtomicUsize,
    refusals: Arc<AtomicU64>,
    next_id: AtomicU64,
    carriers: Mutex<std::collections::HashMap<u64, std::sync::Weak<PairingCarrierState>>>,
    delay: Arc<dyn PairingDelay>,
}

impl PairingCarrierRegistry {
    fn new(delay: Arc<dyn PairingDelay>) -> Self {
        Self {
            active: AtomicUsize::new(0),
            refusals: Arc::new(AtomicU64::new(0)),
            next_id: AtomicU64::new(0),
            carriers: Mutex::new(std::collections::HashMap::new()),
            delay,
        }
    }

    fn admit(&self) -> Option<(u64, Arc<PairingCarrierState>, watch::Receiver<bool>)> {
        if self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_PAIRING_CARRIERS).then_some(active + 1)
            })
            .is_err()
        {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (state, receiver) = PairingCarrierState::new(self.delay.clone());
        self.carriers
            .lock()
            .expect("pairing carrier registry lock")
            .insert(id, Arc::downgrade(&state));
        Some((id, state, receiver))
    }

    fn release(&self, id: u64) {
        self.carriers
            .lock()
            .expect("pairing carrier registry lock")
            .remove(&id);
        self.active.fetch_sub(1, Ordering::AcqRel);
    }

    fn cap_refusals(&self) -> Arc<AtomicU64> {
        self.refusals.clone()
    }

    fn reap_closed_windows(&self) {
        let mut carriers = self.carriers.lock().expect("pairing carrier registry lock");
        carriers.retain(|_, weak| {
            let Some(carrier) = weak.upgrade() else {
                return false;
            };
            if carrier.in_flight.load(Ordering::Acquire) == 0 {
                carrier.close();
            }
            true
        });
    }
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
            pairing_reaper_task: None,
            pairing_cap_refusals: None,
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
                pairing_reaper_task: None,
                pairing_cap_refusals: None,
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
                    pairing_reaper_task: None,
                    pairing_cap_refusals: None,
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
                pairing_reaper_task: None,
                pairing_cap_refusals: None,
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
                pairing_reaper_task: None,
                pairing_cap_refusals: None,
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
            pairing_reaper_task: None,
            pairing_cap_refusals: None,
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
                pairing_reaper_task: None,
                pairing_cap_refusals: None,
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
    let pairing_registry = Arc::new(PairingCarrierRegistry::new(Arc::new(TokioPairingDelay)));
    let pairing_reaper_task = tokio::spawn(pairing_reaper(
        options.journal_root.clone(),
        pairing_registry.clone(),
    ));
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
        pairing_registry: pairing_registry.clone(),
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
        pairing_reaper_task: Some(pairing_reaper_task),
        pairing_cap_refusals: Some(pairing_registry.cap_refusals()),
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
    pairing_registry: Arc<PairingCarrierRegistry>,
}

async fn pairing_reaper(journal_root: PathBuf, registry: Arc<PairingCarrierRegistry>) {
    loop {
        sleep(PAIRING_REAPER_INTERVAL).await;
        if !pairing_window_open(&NonceStore::new(&journal_root), unix_seconds()) {
            registry.reap_closed_windows();
        }
    }
}

struct InFlightPair<'a>(&'a AtomicUsize);

impl Drop for InFlightPair<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn constrain_pair_dispatch(
    State(state): State<Arc<PairingCarrierState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.uri().path() != spl_core::PAIR_PATH || request.method() != Method::POST {
        return next.run(request).await;
    }
    state.dispatch_pair(next.run(request)).await
}

async fn record_pair_dispatch(state: &PairingCarrierState, status: StatusCode) {
    if status.is_success() {
        state.failures.store(0, Ordering::Release);
        return;
    }
    let failures = state.failures.fetch_add(1, Ordering::AcqRel) + 1;
    if failures >= MAX_PAIRING_FAILURES {
        // The stream task signals closure after this response has been written.
        state.close_after_response.store(true, Ordering::Release);
        return;
    }
    if status == StatusCode::GONE {
        state.delay.delay(Duration::from_secs(1)).await;
    }
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
        // bytes, about 32 MiB. The cert-less pairing population is separately
        // capped at four carriers; linked-device carriers remain independent.
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
    let pairing_carrier = matches!(basis, AccessBasis::PairingPeer { .. });
    let pairing_control = if pairing_carrier {
        match config.pairing_registry.admit() {
            Some(control) => Some(control),
            None => {
                // TLS has completed at this point. Refuse without writing a
                // mux response so the fifth pairing tunnel cannot hang or
                // masquerade as a TLS admission failure.
                log::warn!("pairing carrier cap reached; closing cert-less carrier");
                config
                    .pairing_registry
                    .refusals
                    .fetch_add(1, Ordering::AcqRel);
                let _ = connection.close();
                return;
            }
        }
    } else {
        None
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
    let mut pairing_close = pairing_control
        .as_ref()
        .map(|(_, _, receiver)| receiver.clone());
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
                let pairing_state = pairing_control.as_ref().map(|(_, state, _)| state.clone());
                let stream_has_pairing_window_admission = matches!(
                    &basis,
                    AccessBasis::PairingPeer { .. }
                ) && solstone_core_sol_link::pairing::nonces::pairing_window_open(
                    &NonceStore::new(&config.journal_root),
                    unix_seconds(),
                );
                tokio::spawn(async move {
                    let builder = mux_builder();
                    // A 60 s production bound is injected through `serve` for tests.
                    // Every non-zero successful write (including one enabled by a returned
                    // window credit during a slow 2 MiB transfer) resets this deadline.
                    let stream = StallBoundStream::new(stream, stream_stall_timeout);
                    let router = pairing_state.as_ref().map_or(router.clone(), |state| {
                        router.layer(middleware::from_fn_with_state(
                            state.clone(),
                            constrain_pair_dispatch,
                        ))
                    });
                    let router = if stream_has_pairing_window_admission {
                        router.layer(Extension(PairingWindowAdmission))
                    } else {
                        router
                    };
                    if let Err(error) = serve_connection(stream, router, basis, &builder).await {
                        log::debug!("paired-device door stream failed: {error}");
                    }
                    if pairing_state
                        .as_ref()
                        .is_some_and(|state| state.close_after_response.load(Ordering::Acquire))
                    {
                        pairing_state.expect("pairing state checked").close();
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
            changed = async {
                match &mut pairing_close {
                    Some(receiver) => receiver.changed().await,
                    None => std::future::pending().await,
                }
            } => {
                if changed.is_ok() {
                    let _ = connection.close();
                }
                break;
            }
        }
    }
    if let Some((id, _, _)) = pairing_control {
        config.pairing_registry.release(id);
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

#[cfg(test)]
mod access_tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use axum::response::IntoResponse;
    use axum::routing::get;
    use tower::ServiceExt;

    use super::{
        PairingCarrierRegistry, PairingCarrierState, PairingDelay, constrain_pair_dispatch,
        linked_device_did, record_pair_dispatch,
    };
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

    #[derive(Default)]
    struct RecordingDelay(Mutex<Vec<Duration>>);

    impl PairingDelay for RecordingDelay {
        fn delay(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.0.lock().expect("delay recorder lock").push(duration);
            Box::pin(std::future::ready(()))
        }
    }

    #[tokio::test]
    async fn pair_dispatch_cap_counts_pair_failures_and_records_only_410_backoff() {
        let delay = Arc::new(RecordingDelay::default());
        let (state, _) = PairingCarrierState::new(delay.clone());
        // Non-pair failures never reach record_pair_dispatch; this direct
        // helper assertion models the guarded middleware's early return.
        assert_eq!(state.failures.load(std::sync::atomic::Ordering::Acquire), 0);
        record_pair_dispatch(&state, StatusCode::SERVICE_UNAVAILABLE).await;
        assert_eq!(state.failures.load(std::sync::atomic::Ordering::Acquire), 1);
        record_pair_dispatch(&state, StatusCode::GONE).await;
        assert_eq!(
            delay.0.lock().expect("delay recorder lock").as_slice(),
            &[Duration::from_secs(1)]
        );
        record_pair_dispatch(&state, StatusCode::OK).await;
        assert_eq!(state.failures.load(std::sync::atomic::Ordering::Acquire), 0);
        record_pair_dispatch(&state, StatusCode::GONE).await;
        record_pair_dispatch(&state, StatusCode::GONE).await;
        record_pair_dispatch(&state, StatusCode::GONE).await;
        assert!(
            state
                .close_after_response
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert_eq!(
            delay.0.lock().expect("delay recorder lock").as_slice(),
            &[
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1)
            ],
            "the closing third failure does not request a delay"
        );
    }

    #[tokio::test]
    async fn non_pair_endpoint_failure_does_not_increment_the_pair_dispatch_cap() {
        let (state, _) = PairingCarrierState::new(Arc::new(RecordingDelay::default()));
        let app = Router::new()
            .route(
                "/ordinary-failure",
                get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .layer(middleware::from_fn_with_state(
                state.clone(),
                constrain_pair_dispatch,
            ));
        let response = app
            .oneshot(
                Request::get("/ordinary-failure")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(state.failures.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[test]
    fn reaper_skips_an_in_flight_pair_then_closes_when_it_finishes() {
        let registry = PairingCarrierRegistry::new(Arc::new(RecordingDelay::default()));
        let (_, state, close) = registry.admit().expect("carrier admission");
        state
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        registry.reap_closed_windows();
        assert!(!*close.borrow(), "in-flight pairing survives the reaper");
        state
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        registry.reap_closed_windows();
        assert!(*close.borrow(), "idle pairing carrier is reaped");
    }

    #[tokio::test]
    async fn queued_pair_request_is_in_flight_before_it_acquires_the_carrier_lock() {
        let registry = PairingCarrierRegistry::new(Arc::new(RecordingDelay::default()));
        let (_, state, close) = registry.admit().expect("carrier admission");
        let held_lock = state.pair_lock.lock().await;
        let queued = state.clone();
        let dispatch = tokio::spawn(async move {
            queued
                .dispatch_pair(async { StatusCode::OK.into_response() })
                .await
        });

        tokio::task::yield_now().await;
        assert_eq!(
            state.in_flight.load(std::sync::atomic::Ordering::Acquire),
            1,
            "a request waiting for the pair lock is already in flight"
        );
        registry.reap_closed_windows();
        assert!(!*close.borrow(), "the reaper preserves the queued request");

        drop(held_lock);
        let _ = dispatch.await.expect("queued dispatch completes");
        registry.reap_closed_windows();
        assert!(
            *close.borrow(),
            "the carrier reaps after its queued request finishes"
        );
    }
}
