// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use solstone_core_ingest_contract::CONNECTION_BODY_LIMIT;
use solstone_core_sol_client::resident::ShutdownSignal;
use solstone_core_sol_client::seam::{
    LinkServeBundle, LinkServeCarrierPolicy, LinkServeError, LinkServeErrorKind, LinkServeFailure,
    LinkServeRelayControlEndpoint, LinkServeRelayErrorKind, LinkServeRequest, LinkServeRunner,
    LinkServeSession, LinkServeStatusSnapshot, LinkServeTransportErrorKind,
};
use spl_core::bridge::{BridgeNames, RequestHeaderPolicy};
use spl_transport::client::{DialedCarrier, TransportClient};
use spl_transport::credential::{Credential, EndpointAddr};
use spl_transport::journal_bridge::{
    self, BridgePolicy, BridgeStartError, CapabilityGate, CarrierOpener, JournalBridgeConfig,
    JournalBridgeHandle, JournalBridgeStatus, LocalResponse,
};
use spl_transport::relay_pairing::enroll_device;
use spl_transport::{RelayControlEndpoint, RelayError, TransportError, tls};

pub const STATUS_PATH: &str = "/_solstone/link/status";

#[derive(Debug, Clone, Copy, Default)]
pub struct SplLinkServeRunner;

impl LinkServeRunner for SplLinkServeRunner {
    fn start(
        &self,
        request: LinkServeRequest,
    ) -> Result<Box<dyn LinkServeSession>, LinkServeError> {
        ServeStarter::default().start(request)
    }
}

struct ServeStarter {
    enrollment: Arc<dyn RelayEnrollment>,
    clock: Arc<dyn StatusClock>,
}

impl Default for ServeStarter {
    fn default() -> Self {
        Self {
            enrollment: Arc::new(SplRelayEnrollment),
            clock: Arc::new(SystemStatusClock),
        }
    }
}

impl ServeStarter {
    fn start(
        &self,
        request: LinkServeRequest,
    ) -> Result<Box<dyn LinkServeSession>, LinkServeError> {
        // Must be multi-threaded: `LinkServeSession::serve` parks the calling
        // thread in a blocking `ShutdownSignal::wait()` for the process's whole
        // lifetime, and never re-enters the runtime until shutdown. A
        // current-thread runtime only polls spawned tasks while some thread is
        // inside `block_on`, so the bridge's accept loop — spawned by
        // `journal_bridge::start` below — would never run. The listener would
        // still bind (the kernel completes handshakes from the backlog), so the
        // port looks healthy while every request hangs and returns zero bytes.
        //
        // The worker count is pinned rather than left to default: this proxy
        // carries one person's loopback traffic over a single carrier, and the
        // default spawns one worker per core (33 threads on a large host). The
        // work is entirely async I/O, so two workers is ample — one can block
        // briefly on a task without stalling the accept loop.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| LinkServeError::new(LinkServeErrorKind::RuntimeUnavailable))?;
        let enrollment = self.enrollment.clone();
        let credential = runtime.block_on(credential_from_request(&request, enrollment))?;
        let client = Arc::new(
            match request.policy {
                LinkServeCarrierPolicy::RelayOnly => {
                    TransportClient::new_relay_only(credential, None)
                }
                LinkServeCarrierPolicy::Direct | LinkServeCarrierPolicy::RelayPermitted => {
                    TransportClient::new(credential, None)
                }
            }
            .map_err(|error| {
                LinkServeError::new(LinkServeErrorKind::Transport(map_transport_error(error)))
            })?,
        );
        let tracker = Arc::new(StatusTracker::new(self.clock.clone()));
        let opener = Arc::new(SolstoneCarrierOpener {
            client,
            tracker: tracker.clone(),
        });
        let policy = bridge_policy_for_port(request.port, tracker);
        let endpoint_hosts = request
            .bundle
            .endpoints
            .iter()
            .map(|endpoint| endpoint.host.clone())
            .collect::<Vec<_>>();
        let config = JournalBridgeConfig {
            opener,
            bridge_names: bridge_names(),
            endpoint_hosts,
            policy,
        };
        let handle = runtime
            .block_on(journal_bridge::start(config))
            .map_err(|error| map_bridge_start_error(error, request.port))?;
        Ok(Box::new(SplLinkServeSession {
            port: handle.port(),
            runtime,
            handle: Some(handle),
        }))
    }
}

struct SplLinkServeSession {
    port: u16,
    runtime: tokio::runtime::Runtime,
    handle: Option<JournalBridgeHandle>,
}

impl LinkServeSession for SplLinkServeSession {
    fn bound_port(&self) -> u16 {
        self.port
    }

    fn serve(mut self: Box<Self>, shutdown: &dyn ShutdownSignal) -> Result<(), LinkServeError> {
        shutdown.wait();
        if let Some(handle) = self.handle.take() {
            self.runtime.block_on(handle.shutdown_and_wait());
        }
        Ok(())
    }
}

struct SolstoneCarrierOpener {
    client: Arc<TransportClient>,
    tracker: Arc<StatusTracker>,
}

impl CarrierOpener for SolstoneCarrierOpener {
    fn proxy_headers(
        &self,
        upstream_headers: &[(String, String)],
    ) -> Result<Vec<(String, String)>, TransportError> {
        Ok(upstream_headers.to_vec())
    }

    fn dial_carrier(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DialedCarrier, TransportError>> + Send + '_>> {
        Box::pin(async move {
            let result = self.client.dial_carrier().await;
            match &result {
                Ok(_) => self.tracker.carrier_open_succeeded(),
                Err(error) => self.tracker.carrier_open_failed(error),
            }
            result
        })
    }
}

async fn credential_from_request(
    request: &LinkServeRequest,
    enrollment: Arc<dyn RelayEnrollment>,
) -> Result<Credential, LinkServeError> {
    let token = match request.policy {
        LinkServeCarrierPolicy::Direct => None,
        LinkServeCarrierPolicy::RelayPermitted | LinkServeCarrierPolicy::RelayOnly => {
            let origin = request
                .relay_origin
                .as_deref()
                .expect("relay policy carries a relay origin");
            Some(
                enrollment
                    .enroll(
                        origin,
                        &request.bundle.instance_id,
                        &request.bundle.home_attestation,
                    )
                    .await
                    .map_err(|error| {
                        LinkServeError::new(LinkServeErrorKind::Transport(map_transport_error(
                            error,
                        )))
                    })?,
            )
        }
    };
    Ok(Credential {
        client_key_pem: request.bundle.private_key_pem.clone(),
        client_cert_pem: request.bundle.client_cert_pem.clone(),
        ca_chain_pem: request.bundle.ca_chain_pem.clone(),
        ca_fp_prefix: ca_fp_prefix(&request.bundle)?,
        instance_id: request.bundle.instance_id.clone(),
        home_label: request.bundle.home_label.clone(),
        endpoints: match request.policy {
            LinkServeCarrierPolicy::RelayOnly => Vec::new(),
            LinkServeCarrierPolicy::Direct | LinkServeCarrierPolicy::RelayPermitted => request
                .bundle
                .endpoints
                .iter()
                .map(|endpoint| EndpointAddr {
                    host: endpoint.host.clone(),
                    port: endpoint.port,
                })
                .collect(),
        },
        home_attestation: Some(request.bundle.home_attestation.clone()),
        local_endpoints: Some(request.bundle.local_endpoints.clone()),
        relay_origin: match request.policy {
            LinkServeCarrierPolicy::Direct => None,
            LinkServeCarrierPolicy::RelayPermitted | LinkServeCarrierPolicy::RelayOnly => {
                request.relay_origin.clone()
            }
        },
        device_token: token,
        device_token_expires_at: None,
    })
}

fn ca_fp_prefix(bundle: &LinkServeBundle) -> Result<Vec<u8>, LinkServeError> {
    let chain_pem = bundle
        .ca_chain_pem
        .iter()
        .map(|cert| {
            if cert.ends_with('\n') {
                cert.clone()
            } else {
                format!("{cert}\n")
            }
        })
        .collect::<String>();
    let certs = tls::parse_certs(&chain_pem).map_err(|error| {
        LinkServeError::new(LinkServeErrorKind::Transport(map_transport_error(error)))
    })?;
    let Some(first) = certs.first() else {
        return Err(LinkServeError::new(LinkServeErrorKind::InvalidBundle));
    };
    Ok(spl_core::ca::sha256(first.as_ref())[..16].to_vec())
}

pub fn bridge_names() -> BridgeNames {
    BridgeNames {
        capability_cookie_name: "__solstone_link_cap".to_string(),
        upstream_cookie_prefix: String::new(),
        // Inert sentinels: capability_gate is Disabled, so check_caller_auth is unused; these names only exist so is_reserved_request_header does not strip the caller's real protocol-version and observer headers.
        observer_header_name: "x-solstone-link-serve-unused-observer".to_string(),
        protocol_version_header_name: "x-solstone-link-serve-unused-protocol-version".to_string(),
    }
}

fn bridge_policy(tracker: Arc<StatusTracker>) -> BridgePolicy {
    BridgePolicy {
        port: 0,
        capability_gate: CapabilityGate::Disabled,
        stream_response: Arc::new(|_| true),
        local_response: Arc::new(move |head, status| {
            if head.path() != STATUS_PATH {
                return None;
            }
            let body = status_body(&tracker.snapshot(*status));
            Some(LocalResponse {
                status: 200,
                content_type: "application/json".to_string(),
                body,
            })
        }),
        attribution_headers: Arc::new(|_| Vec::new()),
        request_headers: RequestHeaderPolicy::ForwardAll,
        max_request_body_bytes: CONNECTION_BODY_LIMIT,
    }
}

pub fn bridge_policy_for_port(port: u16, tracker: Arc<StatusTracker>) -> BridgePolicy {
    BridgePolicy {
        port,
        ..bridge_policy(tracker)
    }
}

fn status_body(snapshot: &LinkServeStatusSnapshot) -> Vec<u8> {
    let mut root = Map::new();
    root.insert(
        "active_requests".to_string(),
        Value::Number(snapshot.active_requests.into()),
    );
    root.insert(
        "connected_age_seconds".to_string(),
        option_f64(snapshot.connected_age_seconds),
    );
    root.insert("health".to_string(), Value::String(snapshot.health.clone()));
    root.insert(
        "last_connected_at".to_string(),
        option_f64(snapshot.last_connected_at),
    );
    root.insert(
        "last_failure".to_string(),
        snapshot
            .last_failure
            .as_ref()
            .map_or(Value::Null, |failure| {
                let mut item = Map::new();
                item.insert("at".to_string(), number_or_null(failure.at));
                item.insert("detail".to_string(), Value::String(failure.detail.clone()));
                item.insert("reason".to_string(), Value::String(failure.reason.clone()));
                Value::Object(item)
            }),
    );
    root.insert(
        "manager_alive".to_string(),
        Value::Bool(snapshot.manager_alive),
    );
    root.insert("next_retry_at".to_string(), Value::Null);
    root.insert(
        "reconnect_count".to_string(),
        Value::Number(snapshot.reconnect_count.into()),
    );
    root.insert("state".to_string(), Value::String(snapshot.state.clone()));
    serde_json::to_vec(&Value::Object(root)).expect("status snapshot must serialize")
}

fn option_f64(value: Option<f64>) -> Value {
    value.map_or(Value::Null, number_or_null)
}

fn number_or_null(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

pub struct StatusTracker {
    inner: Mutex<StatusTrackerState>,
    clock: Arc<dyn StatusClock>,
}

#[derive(Debug, Default)]
struct StatusTrackerState {
    last_connected_at: Option<f64>,
    last_failure: Option<LinkServeFailure>,
    reconnect_count: u64,
}

impl StatusTracker {
    pub fn new(clock: Arc<dyn StatusClock>) -> Self {
        Self {
            inner: Mutex::new(StatusTrackerState::default()),
            clock,
        }
    }

    fn carrier_open_succeeded(&self) {
        let mut state = self.inner.lock().expect("status tracker lock");
        state.last_connected_at = Some(self.clock.now_unix_seconds());
    }

    fn carrier_open_failed(&self, error: &TransportError) {
        let mut state = self.inner.lock().expect("status tracker lock");
        state.reconnect_count = state.reconnect_count.saturating_add(1);
        state.last_failure = Some(failure_from_transport(error, self.clock.now_unix_seconds()));
    }

    fn snapshot(&self, bridge: JournalBridgeStatus) -> LinkServeStatusSnapshot {
        let state = self.inner.lock().expect("status tracker lock");
        let now = self.clock.now_unix_seconds();
        let connected_age_seconds = if bridge.carrier_live {
            state
                .last_connected_at
                .map(|connected| (now - connected).max(0.0))
        } else {
            None
        };
        LinkServeStatusSnapshot {
            health: if bridge.listener_active && bridge.carrier_live {
                "healthy".to_string()
            } else {
                "unhealthy".to_string()
            },
            state: if bridge.carrier_live {
                "connected".to_string()
            } else if bridge.listener_active {
                "disconnected".to_string()
            } else {
                "closed".to_string()
            },
            manager_alive: bridge.listener_active,
            connected_age_seconds,
            last_connected_at: state.last_connected_at,
            last_failure: state.last_failure.clone(),
            next_retry_at: None,
            reconnect_count: state.reconnect_count,
            active_requests: bridge.active_requests,
        }
    }
}

fn failure_from_transport(error: &TransportError, at: f64) -> LinkServeFailure {
    let kind = map_transport_error_ref(error);
    LinkServeFailure {
        reason: serve_reason_code(&kind).to_string(),
        detail: serve_failure_detail(&kind).to_string(),
        at,
    }
}

pub trait StatusClock: Send + Sync {
    fn now_unix_seconds(&self) -> f64;
}

#[derive(Debug)]
struct SystemStatusClock;

impl StatusClock for SystemStatusClock {
    fn now_unix_seconds(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |duration| duration.as_secs_f64())
    }
}

trait RelayEnrollment: Send + Sync {
    fn enroll<'a>(
        &'a self,
        relay_origin: &'a str,
        instance_id: &'a str,
        home_attestation: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, TransportError>> + Send + 'a>>;
}

#[derive(Debug)]
struct SplRelayEnrollment;

impl RelayEnrollment for SplRelayEnrollment {
    fn enroll<'a>(
        &'a self,
        relay_origin: &'a str,
        instance_id: &'a str,
        home_attestation: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, TransportError>> + Send + 'a>> {
        Box::pin(enroll_device(relay_origin, instance_id, home_attestation))
    }
}

fn map_bridge_start_error(error: BridgeStartError, port: u16) -> LinkServeError {
    match error {
        BridgeStartError::Capability(error) => {
            drop(error);
            LinkServeError::new(LinkServeErrorKind::BridgeCapability)
        }
        BridgeStartError::Bind(error) => LinkServeError::new(LinkServeErrorKind::Bind {
            port,
            addr_in_use: error.kind() == ErrorKind::AddrInUse,
        }),
    }
}

fn map_transport_error(error: TransportError) -> LinkServeTransportErrorKind {
    map_transport_error_ref(&error)
}

fn map_transport_error_ref(error: &TransportError) -> LinkServeTransportErrorKind {
    match error {
        TransportError::Io(_) => LinkServeTransportErrorKind::Io,
        TransportError::Tls(_) => LinkServeTransportErrorKind::Tls,
        TransportError::Crypto(_) => LinkServeTransportErrorKind::Crypto,
        TransportError::Mux(_) => LinkServeTransportErrorKind::Mux,
        TransportError::Http(_) => LinkServeTransportErrorKind::Http,
        TransportError::Json(_) => LinkServeTransportErrorKind::Json,
        TransportError::PairLink(_) => LinkServeTransportErrorKind::PairLink,
        TransportError::Pairing(_) => LinkServeTransportErrorKind::Pairing,
        TransportError::Rejected { status, body: _ } => {
            LinkServeTransportErrorKind::Rejected { status: *status }
        }
        TransportError::Relay(error) => LinkServeTransportErrorKind::Relay(map_relay_error(*error)),
        TransportError::RelayControlRejected { endpoint, status } => {
            LinkServeTransportErrorKind::RelayControlRejected {
                endpoint: map_relay_control_endpoint(*endpoint),
                status: *status,
            }
        }
        TransportError::NoEndpoint => LinkServeTransportErrorKind::NoEndpoint,
        TransportError::NotPaired => LinkServeTransportErrorKind::NotPaired,
        TransportError::LocalOffset => LinkServeTransportErrorKind::LocalOffset,
    }
}

fn map_relay_error(error: RelayError) -> LinkServeRelayErrorKind {
    match error {
        RelayError::HomeOffline => LinkServeRelayErrorKind::HomeOffline,
        RelayError::Unauthorized => LinkServeRelayErrorKind::Unauthorized,
        RelayError::Unpaid => LinkServeRelayErrorKind::Unpaid,
        RelayError::UnknownInstance => LinkServeRelayErrorKind::UnknownInstance,
        RelayError::PairWindowClosed => LinkServeRelayErrorKind::PairWindowClosed,
        RelayError::Overflow => LinkServeRelayErrorKind::Overflow,
        RelayError::Abnormal => LinkServeRelayErrorKind::Abnormal,
        RelayError::UpgradeRejected => LinkServeRelayErrorKind::UpgradeRejected,
        RelayError::Stalled => LinkServeRelayErrorKind::Stalled,
    }
}

fn map_relay_control_endpoint(endpoint: RelayControlEndpoint) -> LinkServeRelayControlEndpoint {
    match endpoint {
        RelayControlEndpoint::EnrollDevice => LinkServeRelayControlEndpoint::EnrollDevice,
        RelayControlEndpoint::TokenRefresh => LinkServeRelayControlEndpoint::TokenRefresh,
    }
}

fn serve_reason_code(kind: &LinkServeTransportErrorKind) -> &'static str {
    match kind {
        LinkServeTransportErrorKind::Io => "io",
        LinkServeTransportErrorKind::Tls => "tls",
        LinkServeTransportErrorKind::Crypto => "crypto",
        LinkServeTransportErrorKind::Mux => "mux",
        LinkServeTransportErrorKind::Http => "http",
        LinkServeTransportErrorKind::Json => "json",
        LinkServeTransportErrorKind::PairLink => "pair-link",
        LinkServeTransportErrorKind::Pairing => "pairing",
        LinkServeTransportErrorKind::Rejected { status: _ } => "rejected",
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::HomeOffline) => {
            "relay-home-offline"
        }
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Unauthorized) => {
            "relay-unauthorized"
        }
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Unpaid) => "relay-unpaid",
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::UnknownInstance) => {
            "relay-unknown-instance"
        }
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::PairWindowClosed) => {
            "relay-pair-window-closed"
        }
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Overflow) => "relay-overflow",
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Abnormal) => "relay-abnormal",
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::UpgradeRejected) => {
            "relay-upgrade-rejected"
        }
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Stalled) => "relay-stalled",
        LinkServeTransportErrorKind::RelayControlRejected {
            endpoint,
            status: _,
        } => match endpoint {
            LinkServeRelayControlEndpoint::EnrollDevice => "relay-control-enroll-device",
            LinkServeRelayControlEndpoint::TokenRefresh => "relay-control-token-refresh",
        },
        LinkServeTransportErrorKind::NoEndpoint => "no-endpoint",
        LinkServeTransportErrorKind::NotPaired => "not-paired",
        LinkServeTransportErrorKind::LocalOffset => "local-offset",
    }
}

fn serve_failure_detail(kind: &LinkServeTransportErrorKind) -> &'static str {
    match kind {
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::HomeOffline) => {
            "relay reports home offline"
        }
        LinkServeTransportErrorKind::Relay(LinkServeRelayErrorKind::Unauthorized)
        | LinkServeTransportErrorKind::RelayControlRejected { .. } => {
            "relay rejected link credentials"
        }
        LinkServeTransportErrorKind::NoEndpoint => "no journal endpoint is available",
        LinkServeTransportErrorKind::NotPaired => "link credentials are missing",
        _ => "link carrier failed",
    }
}

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use serde_json::json;
    use solstone_core_sol_client::seam::LinkServeEndpoint;
    use spl_core::bridge::RequestHead;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct EnrollmentCall {
        relay_origin: String,
        instance_id: String,
        home_attestation: String,
    }

    #[derive(Debug, Default)]
    struct FakeEnrollment {
        calls: Arc<Mutex<Vec<EnrollmentCall>>>,
    }

    impl FakeEnrollment {
        fn calls(&self) -> Vec<EnrollmentCall> {
            self.calls.lock().expect("enrollment calls lock").clone()
        }
    }

    impl RelayEnrollment for FakeEnrollment {
        fn enroll<'a>(
            &'a self,
            relay_origin: &'a str,
            instance_id: &'a str,
            home_attestation: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<String, TransportError>> + Send + 'a>> {
            let calls = self.calls.clone();
            let call = EnrollmentCall {
                relay_origin: relay_origin.to_string(),
                instance_id: instance_id.to_string(),
                home_attestation: home_attestation.to_string(),
            };
            Box::pin(async move {
                calls.lock().expect("enrollment calls lock").push(call);
                Ok("device-token".to_string())
            })
        }
    }

    #[derive(Debug)]
    struct FixedStatusClock(Mutex<f64>);

    impl FixedStatusClock {
        fn new(now: f64) -> Self {
            Self(Mutex::new(now))
        }

        fn set(&self, now: f64) {
            *self.0.lock().expect("clock lock") = now;
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

    fn serve_request(
        policy: LinkServeCarrierPolicy,
        relay_origin: Option<&str>,
    ) -> LinkServeRequest {
        let ca = ca_pem();
        LinkServeRequest {
            label: "laptop".to_string(),
            port: 5015,
            policy,
            relay_origin: relay_origin.map(str::to_string),
            bundle: LinkServeBundle {
                private_key_pem: "PRIVATE\n".to_string(),
                client_cert_pem: "CERT\n".to_string(),
                ca_chain_pem: vec![ca],
                home_attestation: "attestation.jwt".to_string(),
                instance_id: "home-instance".to_string(),
                home_label: "Home".to_string(),
                endpoints: vec![LinkServeEndpoint {
                    host: "192.168.1.10".to_string(),
                    port: 7657,
                }],
                local_endpoints: json!([{"ip": "192.168.1.10", "port": 7657}]),
            },
        }
    }

    fn bridge_status(listener_active: bool, carrier_live: bool) -> JournalBridgeStatus {
        JournalBridgeStatus {
            listener_active,
            contacted: false,
            carrier_live,
            active_requests: 0,
        }
    }

    fn request_head(target: &str) -> RequestHead {
        RequestHead {
            method: "GET".to_string(),
            target: target.to_string(),
            headers: vec![("host".to_string(), "127.0.0.1:5015".to_string())],
        }
    }

    #[test]
    fn direct_credentials_have_no_relay_fields_and_do_not_enroll() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let enrollment = Arc::new(FakeEnrollment::default());
        let request = serve_request(
            LinkServeCarrierPolicy::Direct,
            Some("https://poisoned.invalid"),
        );

        let credential = runtime
            .block_on(credential_from_request(&request, enrollment.clone()))
            .expect("direct credential");

        assert!(credential.relay_origin.is_none());
        assert!(credential.device_token.is_none());
        assert!(credential.device_token_expires_at.is_none());
        assert!(enrollment.calls().is_empty());
    }

    #[test]
    fn relay_credentials_enroll_at_serve_time_in_memory() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let enrollment = Arc::new(FakeEnrollment::default());
        let request = serve_request(
            LinkServeCarrierPolicy::RelayPermitted,
            Some("https://relay.example"),
        );

        let credential = runtime
            .block_on(credential_from_request(&request, enrollment.clone()))
            .expect("relay credential");

        assert_eq!(
            credential.relay_origin.as_deref(),
            Some("https://relay.example")
        );
        assert_eq!(credential.device_token.as_deref(), Some("device-token"));
        assert!(credential.device_token_expires_at.is_none());
        assert_eq!(
            enrollment.calls(),
            vec![EnrollmentCall {
                relay_origin: "https://relay.example".to_string(),
                instance_id: "home-instance".to_string(),
                home_attestation: "attestation.jwt".to_string(),
            }]
        );
    }

    #[test]
    fn relay_only_credentials_enroll_and_have_no_endpoints() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let enrollment = Arc::new(FakeEnrollment::default());
        let request = serve_request(
            LinkServeCarrierPolicy::RelayOnly,
            Some("https://relay.example"),
        );

        let credential = runtime
            .block_on(credential_from_request(&request, enrollment.clone()))
            .expect("relay-only credential");

        assert_eq!(
            credential.relay_origin,
            Some("https://relay.example".to_string())
        );
        assert_eq!(credential.device_token, Some("device-token".to_string()));
        assert!(credential.endpoints.is_empty());
        assert!(!request.bundle.endpoints.is_empty());
        assert_eq!(
            enrollment.calls(),
            vec![EnrollmentCall {
                relay_origin: "https://relay.example".to_string(),
                instance_id: "home-instance".to_string(),
                home_attestation: "attestation.jwt".to_string(),
            }]
        );
    }

    #[test]
    fn status_tracker_uses_one_shared_update_point_for_times_and_failures() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = StatusTracker::new(clock.clone());
        tracker.carrier_open_failed(&TransportError::NoEndpoint);
        let failed = tracker.snapshot(bridge_status(true, false));
        assert_eq!(failed.reconnect_count, 1);
        assert_eq!(
            failed.last_failure.as_ref().map(|failure| failure.at),
            Some(100.0)
        );

        clock.set(110.0);
        tracker.carrier_open_succeeded();
        clock.set(115.5);
        let connected = tracker.snapshot(bridge_status(true, true));
        assert_eq!(connected.last_connected_at, Some(110.0));
        assert_eq!(connected.connected_age_seconds, Some(5.5));
        assert_eq!(connected.reconnect_count, 1);
    }

    #[test]
    fn bridge_policy_status_is_local_and_attribution_hook_is_empty() {
        let tracker = Arc::new(StatusTracker::new(Arc::new(FixedStatusClock::new(10.0))));
        let policy = bridge_policy_for_port(5015, tracker);
        let status = bridge_status(true, false);
        assert_eq!(policy.port, 5015);
        assert!((policy.stream_response)(&request_head("/ordinary")));
        let local = (policy.local_response)(&request_head(STATUS_PATH), &status)
            .expect("status local response");
        assert_eq!(local.status, 200);
        assert_eq!(local.content_type, "application/json");
        let body: serde_json::Value =
            serde_json::from_slice(&local.body).expect("status json body");
        assert_eq!(
            body.as_object()
                .expect("status object")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                "active_requests",
                "connected_age_seconds",
                "health",
                "last_connected_at",
                "last_failure",
                "manager_alive",
                "next_retry_at",
                "reconnect_count",
                "state",
            ]
        );
        assert!((policy.local_response)(&request_head("/not-status"), &status).is_none());
        assert!((policy.attribution_headers)(&request_head(STATUS_PATH)).is_empty());
        assert_eq!(
            policy.max_request_body_bytes,
            solstone_core_ingest_contract::CONNECTION_BODY_LIMIT
        );
    }

    #[test]
    fn bridge_names_use_inert_reserved_header_sentinels() {
        let names = bridge_names();
        assert_eq!(
            names.observer_header_name,
            "x-solstone-link-serve-unused-observer"
        );
        assert_eq!(
            names.protocol_version_header_name,
            "x-solstone-link-serve-unused-protocol-version"
        );
    }

    #[test]
    fn solstone_adapter_adds_no_wildcard_bind_host_literal() {
        let source = include_str!("serve.rs");
        let wildcard_v4 = ["0", "0", "0", "0"].join(".");
        let wildcard_v6 = ":".repeat(2);
        let named_loopback = format!("{}{}", "local", "host");
        for host in [wildcard_v4, wildcard_v6, named_loopback] {
            assert!(!source.contains(&format!("{host:?}")));
        }
        let observer_mixed = ["X", "Solstone", "Observer"].join("-");
        let protocol_mixed = ["X", "Solstone", "Protocol", "Version"].join("-");
        let observer_lower = ["x", "solstone", "observer"].join("-");
        let protocol_lower = ["x", "solstone", "protocol", "version"].join("-");
        for header in [
            observer_mixed,
            protocol_mixed,
            observer_lower,
            protocol_lower,
        ] {
            assert!(
                !source.contains(&header),
                "serve.rs still contains reserved header literal {header}"
            );
        }
    }

    fn test_opener() -> SolstoneCarrierOpener {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("test key");
        let params = CertificateParams::new(Vec::<String>::new()).expect("test params");
        let cert = params.self_signed(&key).expect("test cert");
        let credential = Credential {
            client_key_pem: key.serialize_pem(),
            client_cert_pem: cert.pem(),
            ca_chain_pem: vec![cert.pem()],
            ca_fp_prefix: spl_core::ca::sha256(cert.der())[..16].to_vec(),
            instance_id: "home-instance".to_string(),
            home_label: "Home".to_string(),
            endpoints: vec![EndpointAddr {
                host: "127.0.0.1".to_string(),
                port: 1,
            }],
            home_attestation: None,
            local_endpoints: None,
            relay_origin: None,
            device_token: None,
            device_token_expires_at: None,
        };
        SolstoneCarrierOpener {
            client: Arc::new(TransportClient::new(credential, None).expect("test client")),
            tracker: Arc::new(StatusTracker::new(Arc::new(FixedStatusClock::new(0.0)))),
        }
    }

    #[test]
    fn proxy_headers_forwards_caller_headers_unchanged() {
        let opener = test_opener();
        let protocol = ["x", "solstone", "protocol", "version"].join("-");
        let incoming = [
            (protocol, "3".to_string()),
            ("x-custom".to_string(), "v".to_string()),
        ];
        let forwarded = opener.proxy_headers(&incoming).expect("proxy headers");
        assert_eq!(forwarded, incoming);
    }
}
