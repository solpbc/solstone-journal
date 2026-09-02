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
use axum::response::{IntoResponse, Response};
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
use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};
use solstone_core_convey_http::serve::{mux_builder, serve_connection};
use solstone_core_sol_link::ca::issue_server_certificate;
use solstone_core_sol_link::committed::load_committed_identity;
use solstone_core_sol_link::ledger::{
    AuthorizationLedger, AuthorizedClientsRead, read_authorized_clients,
};
use solstone_core_sol_link::pairing::nonces::{
    NonceStore, direct_pairing_window_open, relay_pairing_nonce_open,
};
use solstone_core_sol_link::{
    DeviceDoorAuthorization, DeviceDoorVerifier, spawn_authorization_refresh,
};
use spl_home::{DEFAULT_DECODER_BUFFER_BYTES, HomeConfig, HomeConnection, MuxLimits};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio::time::{Sleep, sleep};

use crate::relay_admission::{RelayAdmissionClaim, RelayAdmissionRegistry, RelayNonceIdentity};
use crate::session::{SessionState, classify_session};
use crate::{DoorOutcome, DoorWithheldReason};

const MAX_CONCURRENT_STREAMS: usize = 8;
const AUTHORIZATION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const PAIRING_REAPER_INTERVAL: Duration = Duration::from_millis(250);
// `HomeStream::poll_shutdown` queues the SPL close frame but does not wait for
// the carrier driver to flush it. Give that local queue one reaper tick to
// drain before closing a one-shot pairing carrier.
const PAIRING_CARRIER_DRAIN_GRACE: Duration = Duration::from_millis(250);
// A peer may pre-open an unrelated logical stream and never send a request.
// Close independently of stream bookkeeping after success, the failure cap,
// or a stale admission. The same bound gives a newly stale carrier time to
// receive the route-level refusal before its transport is retired, while four
// peers still cannot retain every cert-less carrier slot indefinitely.
const PAIRING_CLOSE_DEADLINE: Duration = Duration::from_secs(5);
const MAX_PAIRING_CARRIERS: usize = 4;
const MAX_PAIRING_FAILURES: usize = 3;

/// A stream accepted under direct or exact relay pairing authority.
#[derive(Clone, Debug)]
pub(crate) enum PairingAdmission {
    Direct,
    Relay(RelayNonceIdentity),
}

impl PairingAdmission {
    fn is_live(&self, store: &NonceStore, now: i64) -> bool {
        match self {
            Self::Direct => direct_pairing_window_open(store, now),
            Self::Relay(identity) => relay_pairing_nonce_open(store, identity.nonce_value(), now),
        }
    }
}

pub(super) struct DoorStartOptions {
    pub journal_root: std::path::PathBuf,
    pub port: u16,
    pub handshake_timeout: Duration,
    pub stream_stall_timeout: Duration,
    pub router: Router,
    pub carrier_loop_iterations: Arc<AtomicU64>,
    pub handshake_authorization_read_ticks: Arc<AtomicU64>,
    pub authorization_sender: watch::Sender<DeviceDoorAuthorization>,
    pub relay_admissions: Arc<RelayAdmissionRegistry>,
}

pub(super) struct DoorStart {
    pub outcome: DoorOutcome,
    pub refresh_task: Option<tokio::task::JoinHandle<()>>,
    pub accept_task: Option<tokio::task::JoinHandle<()>>,
    pub pairing_reaper_task: Option<tokio::task::JoinHandle<()>>,
    pub pairing_cap_refusals: Option<Arc<AtomicU64>>,
}

impl DoorStart {
    fn abort(&self) {
        if let Some(task) = &self.refresh_task {
            task.abort();
        }
        if let Some(task) = &self.accept_task {
            task.abort();
        }
        if let Some(task) = &self.pairing_reaper_task {
            task.abort();
        }
    }
}

/// Starts the paired-device door at process boot and again after first-run
/// finalize. Python starts the listener from `/init/finalize`; native used to
/// try only at serve() and then leave a withheld door down for the life of
/// the process.
pub(super) struct DoorLifecycle {
    parts: DoorStartParts,
    running: Mutex<Option<DoorStart>>,
    stopped: AtomicBool,
}

struct DoorStartParts {
    journal_root: PathBuf,
    port: u16,
    handshake_timeout: Duration,
    stream_stall_timeout: Duration,
    router: Router,
    carrier_loop_iterations: Arc<AtomicU64>,
    handshake_authorization_read_ticks: Arc<AtomicU64>,
    authorization_sender: watch::Sender<DeviceDoorAuthorization>,
    relay_admissions: Arc<RelayAdmissionRegistry>,
}

impl DoorStartParts {
    fn from_options(options: DoorStartOptions) -> Self {
        Self {
            journal_root: options.journal_root,
            port: options.port,
            handshake_timeout: options.handshake_timeout,
            stream_stall_timeout: options.stream_stall_timeout,
            router: options.router,
            carrier_loop_iterations: options.carrier_loop_iterations,
            handshake_authorization_read_ticks: options.handshake_authorization_read_ticks,
            authorization_sender: options.authorization_sender,
            relay_admissions: options.relay_admissions,
        }
    }

    fn to_options(&self) -> DoorStartOptions {
        DoorStartOptions {
            journal_root: self.journal_root.clone(),
            port: self.port,
            handshake_timeout: self.handshake_timeout,
            stream_stall_timeout: self.stream_stall_timeout,
            router: self.router.clone(),
            carrier_loop_iterations: self.carrier_loop_iterations.clone(),
            handshake_authorization_read_ticks: self.handshake_authorization_read_ticks.clone(),
            authorization_sender: self.authorization_sender.clone(),
            relay_admissions: self.relay_admissions.clone(),
        }
    }
}

impl DoorLifecycle {
    pub(super) fn new(options: DoorStartOptions) -> Self {
        Self {
            parts: DoorStartParts::from_options(options),
            running: Mutex::new(None),
            stopped: AtomicBool::new(false),
        }
    }

    pub(super) async fn ensure_started(&self) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        if self.is_bound() {
            return !self.stopped.load(Ordering::Acquire);
        }
        let started = start(self.parts.to_options()).await;
        self.install_started(started)
    }

    fn install_started(&self, started: DoorStart) -> bool {
        let mut running = self.running.lock().expect("door lifecycle lock");
        if self.stopped.load(Ordering::Acquire) {
            started.abort();
            self.parts.relay_admissions.clear_door_port();
            return false;
        }
        if running
            .as_ref()
            .is_some_and(|current| matches!(&current.outcome, DoorOutcome::Bound(_)))
        {
            started.abort();
            return true;
        }
        let ok = matches!(started.outcome, DoorOutcome::Bound(_));
        if ok || running.is_none() {
            if let Some(previous) = running.take() {
                previous.abort();
            }
            if let DoorOutcome::Bound(address) = &started.outcome {
                self.parts.relay_admissions.set_door_port(address.port());
            } else {
                self.parts.relay_admissions.clear_door_port();
            }
            *running = Some(started);
            ok
        } else {
            started.abort();
            false
        }
    }

    pub(super) fn is_bound(&self) -> bool {
        self.bound_addr().is_some()
    }

    pub(super) fn bound_addr(&self) -> Option<SocketAddr> {
        let running = self.running.lock().expect("door lifecycle lock");
        match running.as_ref().map(|current| &current.outcome) {
            Some(DoorOutcome::Bound(address)) => Some(*address),
            _ => None,
        }
    }

    pub(super) fn clone_outcome(&self) -> Option<DoorOutcome> {
        let running = self.running.lock().expect("door lifecycle lock");
        running
            .as_ref()
            .map(|current| clone_outcome(&current.outcome))
    }

    pub(super) fn pairing_cap_refusals(&self) -> u64 {
        let running = self.running.lock().expect("door lifecycle lock");
        running
            .as_ref()
            .and_then(|current| current.pairing_cap_refusals.as_ref())
            .map_or(0, |counter| counter.load(Ordering::Acquire))
    }

    pub(super) async fn stop_authorization_refresh(&self) {
        let task = {
            let mut running = self.running.lock().expect("door lifecycle lock");
            running
                .as_mut()
                .and_then(|current| current.refresh_task.take())
        };
        let Some(task) = task else {
            return;
        };
        task.abort();
        match task.await {
            Ok(()) | Err(_) => {}
        }
    }

    pub(super) async fn stop_pairing_reaper(&self) {
        let task = {
            let mut running = self.running.lock().expect("door lifecycle lock");
            running
                .as_mut()
                .and_then(|current| current.pairing_reaper_task.take())
        };
        let Some(task) = task else {
            return;
        };
        task.abort();
        match task.await {
            Ok(()) | Err(_) => {}
        }
    }

    pub(super) fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        let running = self.running.lock().expect("door lifecycle lock");
        if let Some(current) = running.as_ref() {
            current.abort();
        }
        self.parts.relay_admissions.clear_door_port();
    }
}

fn clone_outcome(outcome: &DoorOutcome) -> DoorOutcome {
    match outcome {
        DoorOutcome::Bound(address) => DoorOutcome::Bound(*address),
        DoorOutcome::Withheld(reason) => DoorOutcome::Withheld(reason.clone()),
        DoorOutcome::BindFailed { port, source } => DoorOutcome::BindFailed {
            port: *port,
            source: std::io::Error::new(source.kind(), format!("{source}")),
        },
    }
}

/// After a successful `/init/finalize`, start the door that boot withheld.
pub(super) async fn open_after_finalize(
    axum::extract::State(door): axum::extract::State<Arc<DoorLifecycle>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let is_finalize = request.method() == Method::POST && request.uri().path() == "/init/finalize";
    let response = next.run(request).await;
    if !is_finalize || !response.status().is_success() {
        return response;
    }
    if door.ensure_started().await {
        if let Some(address) = door.bound_addr() {
            eprintln!("convey: paired-device door listening on {address}");
        }
        return response;
    }
    solstone_core_convey_http::envelope::error_envelope(
        "convey_operation_failed",
        "Setup was saved, but secure network access did not start.",
        "the paired-device door did not start",
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
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
    active_streams: AtomicUsize,
    failures: AtomicUsize,
    successful_response_pending: AtomicBool,
    dispatch_sealed: AtomicBool,
    close_deadline_armed: AtomicBool,
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
                active_streams: AtomicUsize::new(0),
                failures: AtomicUsize::new(0),
                successful_response_pending: AtomicBool::new(false),
                dispatch_sealed: AtomicBool::new(false),
                close_deadline_armed: AtomicBool::new(false),
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

    fn arm_close_deadline(self: &Arc<Self>) {
        if !self.close_deadline_armed.swap(true, Ordering::AcqRel) {
            let deadline_state = Arc::clone(self);
            tokio::spawn(async move {
                deadline_state.delay.delay(PAIRING_CLOSE_DEADLINE).await;
                deadline_state.close();
            });
        }
    }

    async fn dispatch_pair<F>(self: &Arc<Self>, dispatch: F) -> Response
    where
        F: Future<Output = Response>,
    {
        // Count from queue admission, not lock acquisition: the reaper must
        // preserve a request waiting behind another ceremony.
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        let _in_flight = InFlightPair(&self.in_flight);
        let _lock = self.pair_lock.lock().await;
        if self.dispatch_sealed.load(Ordering::Acquire) {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        let response = dispatch.await;
        record_pair_dispatch(self, response.status()).await;
        response
    }
}

struct PairingCarrierRegistry {
    active: AtomicUsize,
    refusals: Arc<AtomicU64>,
    next_id: AtomicU64,
    carriers: Mutex<std::collections::HashMap<u64, PairingCarrier>>,
    delay: Arc<dyn PairingDelay>,
}

struct PairingCarrier {
    state: std::sync::Weak<PairingCarrierState>,
    admission: PairingAdmission,
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

    fn admit(
        &self,
        admission: PairingAdmission,
    ) -> Option<(u64, Arc<PairingCarrierState>, watch::Receiver<bool>)> {
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
            .insert(
                id,
                PairingCarrier {
                    state: Arc::downgrade(&state),
                    admission,
                },
            );
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

    fn reap_closed_windows(&self, journal_root: &std::path::Path, now: i64) {
        let store = NonceStore::new(journal_root);
        let mut carriers = self.carriers.lock().expect("pairing carrier registry lock");
        carriers.retain(|_, entry| {
            let Some(carrier) = entry.state.upgrade() else {
                return false;
            };
            if !entry.admission.is_live(&store, now)
                && carrier.in_flight.load(Ordering::Acquire) == 0
                && !carrier.successful_response_pending.load(Ordering::Acquire)
            {
                carrier.arm_close_deadline();
            }
            true
        });
    }
}

/// Identity observed from the accepted leaf. Keeping this as a struct leaves one
/// per-connection refusal-classification state without parallel state.
#[derive(Clone, Debug)]
struct AcceptedIdentity {
    cid: LinkedDeviceCid,
}

type IdentityCell = Arc<Mutex<Option<AcceptedIdentity>>>;

/// `DeviceDoorVerifier` plus accept-local successful client identity capture.
struct DoorIdentityVerifier {
    inner: Arc<DeviceDoorVerifier>,
    identity: IdentityCell,
    certless_pairing_admitted: bool,
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
        certless_pairing_admitted: bool,
    ) -> Self {
        Self {
            inner,
            identity,
            certless_pairing_admitted,
        }
    }
}

impl ClientCertVerifier for DoorIdentityVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }
    fn client_auth_mandatory(&self) -> bool {
        !self.certless_pairing_admitted
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
        let cid = LinkedDeviceCid::try_from(
            format!("sha256:{}", spl_core::ca::sha256_hex(end_entity.as_ref())).as_str(),
        )
        .map_err(|_| {
            RustlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
        })?;
        let mut identity = self.identity.lock().map_err(|_| {
            RustlsError::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
        })?;
        match identity.as_ref() {
            Some(existing) if existing.cid != cid => Err(RustlsError::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
            Some(_) => Ok(ClientCertVerified::assertion()),
            None => {
                *identity = Some(AcceptedIdentity { cid });
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
    // Deliberately no SO_REUSEPORT. The existing Python service owns 7657 with
    // SO_REUSEPORT;
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
        relay_admissions: options.relay_admissions,
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
    relay_admissions: Arc<RelayAdmissionRegistry>,
}

async fn pairing_reaper(journal_root: PathBuf, registry: Arc<PairingCarrierRegistry>) {
    loop {
        sleep(PAIRING_REAPER_INTERVAL).await;
        registry.reap_closed_windows(&journal_root, unix_seconds());
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
    if !crate::authorization_gate::PAIR_PATHS.contains(&request.uri().path())
        || request.method() != Method::POST
    {
        return next.run(request).await;
    }
    state.dispatch_pair(next.run(request)).await
}

async fn record_pair_dispatch(state: &Arc<PairingCarrierState>, status: StatusCode) {
    if status.is_success() {
        state.failures.store(0, Ordering::Release);
        // Consuming the nonce makes this carrier look stale to the reaper before
        // the mux and TLS drivers have finished delivering the successful
        // response. Preserve it until the stream writer finishes, then close
        // this one-shot carrier so it cannot retain a pairing slot.
        state
            .successful_response_pending
            .store(true, Ordering::Release);
        state.close_after_response.store(true, Ordering::Release);
        state.arm_close_deadline();
        return;
    }
    let failures = state.failures.fetch_add(1, Ordering::AcqRel) + 1;
    if failures >= MAX_PAIRING_FAILURES {
        // Seal while holding the dispatch lock. A queued fourth request checks
        // this state after acquiring that same lock, so it cannot start a new
        // ceremony while the carrier drains or a sibling stream remains open.
        state.dispatch_sealed.store(true, Ordering::Release);
        state.close_after_response.store(true, Ordering::Release);
        state.arm_close_deadline();
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
    let store = NonceStore::new(&config.journal_root);
    let now = unix_seconds();
    let relay_admission = peer.and_then(|address| config.relay_admissions.take(address));
    let (pairing_admission, certless_pairing_admitted) = match relay_admission {
        Some(RelayAdmissionClaim::Current(identity))
            if relay_pairing_nonce_open(&store, identity.nonce_value(), now) =>
        {
            (Some(PairingAdmission::Relay(identity)), true)
        }
        // A trusted relay bridge remains relay-typed even when its exact
        // authority has gone stale. It must fail closed rather than inherit an
        // unrelated direct window that happens to be live at the same time.
        Some(RelayAdmissionClaim::Current(identity) | RelayAdmissionClaim::Stale(identity)) => {
            (Some(PairingAdmission::Relay(identity)), true)
        }
        None if direct_pairing_window_open(&store, now) => (Some(PairingAdmission::Direct), true),
        None => (None, false),
    };
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
            certless_pairing_admitted,
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
            // warn, not debug: this is the one line that explains a client-observed
            // connection drop from the server's side, and production runs at the
            // `warn` default (main.rs `install_logger`) with no per-module override.
            log::warn!("paired-device carrier TLS/mux failed: {error}");
            return;
        }
        Err(_) => {
            log::warn!("paired-device carrier handshake timed out");
            return;
        }
    };
    let Some(basis) = capture_to_basis(&identity, peer, certless_pairing_admitted) else {
        log::debug!("paired-device carrier completed without an accepted identity");
        return;
    };
    let pairing_carrier = matches!(basis, AccessBasis::PairingPeer { .. });
    let pairing_control = if pairing_carrier {
        let admission = pairing_admission
            .clone()
            .expect("pairing basis requires a pairing admission");
        match config.pairing_registry.admit(admission) {
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
    let cid = linked_device_cid(&basis);
    if let Some(cid) = &cid {
        record_completed_handshake(&config.journal_root, cid);
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
                let pairing_admission = pairing_admission.clone();
                let pairing_state = pairing_control.as_ref().map(|(_, state, _)| state.clone());
                if let Some(state) = &pairing_state {
                    state.active_streams.fetch_add(1, Ordering::AcqRel);
                }
                let stream_pairing_admission = matches!(&basis, AccessBasis::PairingPeer { .. })
                    .then(|| match &pairing_admission {
                        Some(PairingAdmission::Direct)
                            if direct_pairing_window_open(
                                &NonceStore::new(&config.journal_root),
                                unix_seconds(),
                            ) => Some(PairingAdmission::Direct),
                        Some(PairingAdmission::Relay(identity)) => {
                            Some(PairingAdmission::Relay(identity.clone()))
                        }
                        Some(PairingAdmission::Direct) | None => None,
                    })
                    .flatten();
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
                    let router = match stream_pairing_admission {
                        Some(admission) => router.layer(Extension(admission)),
                        None => router,
                    };
                    if let Err(error) = serve_connection(stream, router, basis, &builder).await {
                        // warn, not debug: see the TLS/mux-failed warn above — same reasoning.
                        log::warn!("paired-device door stream failed: {error}");
                    }
                    if let Some(state) = pairing_state {
                        let remaining = state.active_streams.fetch_sub(1, Ordering::AcqRel) - 1;
                        if remaining == 0 && state.close_after_response.load(Ordering::Acquire) {
                        state.delay.delay(PAIRING_CARRIER_DRAIN_GRACE).await;
                        state.close();
                        }
                    }
                });
            }
            changed = authorization.changed(), if authorization_watch_open => {
                match changed {
                    Ok(()) => {
                        if cid.as_ref().is_some_and(|cid| {
                            close_for_revocation(authorization.borrow().as_read(), cid)
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

fn record_completed_handshake(journal_root: &std::path::Path, cid: &LinkedDeviceCid) {
    match AuthorizationLedger::new(journal_root).touch_last_seen(cid.as_str()) {
        Ok(true) => {}
        Ok(false) => log::debug!("paired-device CID was absent while recording last seen"),
        Err(error) => log::debug!("paired-device last-seen update failed: {error}"),
    }
    let mut extra = Map::new();
    extra.insert("fingerprint".to_owned(), json!(cid.as_str()));
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
    certless_pairing_admitted: bool,
) -> Option<AccessBasis> {
    match identity.lock().ok()?.clone() {
        Some(accepted) => Some(AccessBasis::LinkedDevice {
            carrier: carrier_from_peer(peer),
            cid: accepted.cid,
        }),
        None if certless_pairing_admitted => Some(AccessBasis::PairingPeer {
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

fn linked_device_cid(basis: &AccessBasis) -> Option<LinkedDeviceCid> {
    match basis {
        AccessBasis::LinkedDevice { cid, .. } => Some(cid.clone()),
        // A cert-less pairing carrier has no device identity to refresh or revoke.
        AccessBasis::PairingPeer { .. } => None,
        AccessBasis::Localhost => None,
    }
}

fn close_for_revocation(posture: &AuthorizedClientsRead, cid: &LinkedDeviceCid) -> bool {
    // The handshake fails closed on every non-`Present` posture it reads from the ledger itself.
    // Once a device is authenticated, only a definite `Present` removal observed on this arm
    // ends its carrier: a transient malformed/unreadable read must not discard captured material
    // that exists nowhere else, and a dead publication now ends no carrier at all.
    matches!(posture, AuthorizedClientsRead::Present(entries) if !entries.iter().any(|entry| entry.fingerprint == cid.as_str()))
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
    fn completed_start_cannot_publish_after_shutdown() {
        let (authorization_sender, _) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let relay_admissions = Arc::new(RelayAdmissionRegistry::new());
        let lifecycle = DoorLifecycle::new(DoorStartOptions {
            journal_root: PathBuf::from("/var/tmp/solstone-door-stopped-unit"),
            port: 0,
            handshake_timeout: Duration::from_secs(1),
            stream_stall_timeout: Duration::from_secs(1),
            router: Router::new(),
            carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
            authorization_sender,
            relay_admissions: Arc::clone(&relay_admissions),
        });
        lifecycle.shutdown();

        assert!(!lifecycle.install_started(DoorStart {
            outcome: DoorOutcome::Bound("127.0.0.1:47657".parse().expect("Door address")),
            refresh_task: None,
            accept_task: None,
            pairing_reaper_task: None,
            pairing_cap_refusals: None,
        }));
        assert_eq!(lifecycle.bound_addr(), None);
        assert_eq!(relay_admissions.door_availability(), None);
    }

    #[test]
    fn losing_concurrent_start_does_not_advance_the_running_door_generation() {
        let (authorization_sender, _) = watch::channel(DeviceDoorAuthorization::from(
            AuthorizedClientsRead::Missing,
        ));
        let relay_admissions = Arc::new(RelayAdmissionRegistry::new());
        let lifecycle = DoorLifecycle::new(DoorStartOptions {
            journal_root: PathBuf::from("/var/tmp/solstone-door-concurrent-start-unit"),
            port: 0,
            handshake_timeout: Duration::from_secs(1),
            stream_stall_timeout: Duration::from_secs(1),
            router: Router::new(),
            carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
            handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
            authorization_sender,
            relay_admissions: Arc::clone(&relay_admissions),
        });
        let first_address = "127.0.0.1:47657".parse().expect("first Door address");
        assert!(lifecycle.install_started(DoorStart {
            outcome: DoorOutcome::Bound(first_address),
            refresh_task: None,
            accept_task: None,
            pairing_reaper_task: None,
            pairing_cap_refusals: None,
        }));
        let first_generation = relay_admissions
            .door_availability()
            .expect("first Door generation");

        assert!(lifecycle.install_started(DoorStart {
            outcome: DoorOutcome::Bound("127.0.0.1:47658".parse().expect("losing Door address")),
            refresh_task: None,
            accept_task: None,
            pairing_reaper_task: None,
            pairing_cap_refusals: None,
        }));

        assert_eq!(lifecycle.bound_addr(), Some(first_address));
        assert!(relay_admissions.is_current(first_generation));
    }

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
        let cid = LinkedDeviceCid::try_from(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        assert!(!close_for_revocation(&AuthorizedClientsRead::Missing, &cid));
        assert!(!close_for_revocation(
            &AuthorizedClientsRead::Unreadable,
            &cid
        ));
        assert!(!close_for_revocation(
            &AuthorizedClientsRead::Malformed,
            &cid
        ));
        assert!(close_for_revocation(
            &AuthorizedClientsRead::Present(Vec::new()),
            &cid
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
        PAIRING_CLOSE_DEADLINE, PairingAdmission, PairingCarrierRegistry, PairingCarrierState,
        PairingDelay, constrain_pair_dispatch, linked_device_cid, record_pair_dispatch,
    };
    use solstone_core_convey_http::identity::{AccessBasis, Carrier, LinkedDeviceCid};

    const VALID_CID: &str =
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn carrier_basis_handling_keeps_linked_identity_and_accepts_pairing_peers() {
        let linked = AccessBasis::LinkedDevice {
            carrier: Carrier::Direct,
            cid: LinkedDeviceCid::try_from(VALID_CID).unwrap(),
        };
        assert_eq!(linked_device_cid(&linked).unwrap().as_str(), VALID_CID);

        assert!(
            linked_device_cid(&AccessBasis::PairingPeer {
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

    struct PendingDelay;

    impl PairingDelay for PendingDelay {
        fn delay(&self, _duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn pair_dispatch_cap_counts_pair_failures_and_records_only_410_backoff() {
        let delay = Arc::new(RecordingDelay::default());
        let (state, close) = PairingCarrierState::new(delay.clone());
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
        state
            .active_streams
            .store(1, std::sync::atomic::Ordering::Release);
        record_pair_dispatch(&state, StatusCode::OK).await;
        tokio::task::yield_now().await;
        assert_eq!(state.failures.load(std::sync::atomic::Ordering::Acquire), 0);
        assert!(
            state
                .successful_response_pending
                .load(std::sync::atomic::Ordering::Acquire),
            "successful pairing remains live until its response reaches the client"
        );
        assert!(
            state
                .close_after_response
                .load(std::sync::atomic::Ordering::Acquire),
            "successful pairing closes its one-shot carrier after the response"
        );
        assert!(
            *close.borrow(),
            "the injected hard deadline closes despite unrelated stream state"
        );
        state
            .successful_response_pending
            .store(false, std::sync::atomic::Ordering::Release);
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
                Duration::from_secs(5),
                Duration::from_secs(1),
                Duration::from_secs(1)
            ],
            "the closing third failure skips backoff and reuses the armed hard deadline"
        );
    }

    #[tokio::test]
    async fn third_failure_seals_dispatch_and_closes_despite_a_stalled_sibling_stream() {
        let (state, close) = PairingCarrierState::new(Arc::new(RecordingDelay::default()));
        state
            .active_streams
            .store(1, std::sync::atomic::Ordering::Release);

        for _ in 0..3 {
            let response = state
                .dispatch_pair(async { StatusCode::BAD_REQUEST.into_response() })
                .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        let fourth_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran = fourth_ran.clone();
        let fourth = state
            .dispatch_pair(async move {
                ran.store(true, std::sync::atomic::Ordering::Release);
                StatusCode::OK.into_response()
            })
            .await;
        assert_eq!(fourth.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(
            !fourth_ran.load(std::sync::atomic::Ordering::Acquire),
            "a fourth pairing ceremony is never dispatched"
        );
        tokio::task::yield_now().await;
        assert!(
            *close.borrow(),
            "the hard deadline closes even while a sibling stream stays active"
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

    #[tokio::test]
    async fn reaper_skips_an_in_flight_pair_then_arms_a_bounded_close() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary journal");
        let delay = Arc::new(RecordingDelay::default());
        let registry = PairingCarrierRegistry::new(delay.clone());
        let (_, state, close) = registry
            .admit(PairingAdmission::Direct)
            .expect("carrier admission");
        state
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        registry.reap_closed_windows(temporary.path(), 0);
        tokio::task::yield_now().await;
        assert!(!*close.borrow(), "in-flight pairing survives the reaper");
        assert!(
            delay.0.lock().expect("delay recorder lock").is_empty(),
            "the reaper does not arm a deadline while a request is in flight"
        );
        state
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        registry.reap_closed_windows(temporary.path(), 0);
        tokio::task::yield_now().await;
        assert!(
            *close.borrow(),
            "idle pairing carrier closes after its bound"
        );
        assert_eq!(
            delay.0.lock().expect("delay recorder lock").as_slice(),
            &[PAIRING_CLOSE_DEADLINE],
            "the stale carrier gets one bounded route-refusal opportunity"
        );
    }

    #[tokio::test]
    async fn successful_pair_response_is_not_reaped_before_peer_closure() {
        let temporary = tempfile::TempDir::new_in("/var/tmp").expect("temporary journal");
        let registry = PairingCarrierRegistry::new(Arc::new(PendingDelay));
        let (_, state, close) = registry
            .admit(PairingAdmission::Direct)
            .expect("carrier admission");
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
        registry.reap_closed_windows(temporary.path(), 0);
        assert!(!*close.borrow(), "the reaper preserves the queued request");

        drop(held_lock);
        let _ = dispatch.await.expect("queued dispatch completes");
        registry.reap_closed_windows(temporary.path(), 0);
        assert!(
            !*close.borrow(),
            "the reaper preserves a successful response until its peer closes"
        );
    }
}
