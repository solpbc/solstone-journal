// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value};
use solstone_core_ingest_contract::CONNECTION_BODY_LIMIT;
use solstone_core_sol_client::link_credentials::{
    LinkCredentialStore, PairingIdentity, StoreLoadOutcome, StoreMutationError, StoreVersion,
    get_file_dev_ino, parse_relay_origin, same_relay_origin,
};
use solstone_core_sol_client::resident::ShutdownSignal;
use solstone_core_sol_client::seam::{
    LinkJournalMetadata, LinkServeBundle, LinkServeCarrierPolicy, LinkServeError,
    LinkServeErrorKind, LinkServeFailure, LinkServeRelayControlEndpoint, LinkServeRelayErrorKind,
    LinkServeRequest, LinkServeRunner, LinkServeRuntimeRecord, LinkServeSession,
    LinkServeStatusSnapshot, LinkServeTransportErrorKind,
};
use spl_core::bridge::{BridgeNames, RequestHeaderPolicy};
use spl_transport::client::{DialedCarrier, TokenPersistHook, TransportClient};
use spl_transport::credential::{Credential, EndpointAddr};
use spl_transport::journal_bridge::{
    self, BridgePolicy, BridgeStartError, CapabilityGate, CarrierOpener, JournalBridgeConfig,
    JournalBridgeHandle, JournalBridgeStatus, LocalResponse,
};
use spl_transport::relay_pairing::enroll_device;
use spl_transport::{RelayControlEndpoint, RelayError, TransportError, tls};

pub const STATUS_PATH: &str = "/_solstone/link/status";

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WireRelayAccessResponse {
    #[serde(rename = "ready")]
    Ready {
        protocol_version: u32,
        relay_origin: String,
        instance_id: String,
        device_token: String,
        expires_at: String,
    },
    #[serde(rename = "not_configured")]
    NotConfigured { protocol_version: u32 },
}

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

pub struct CurrentClientManager {
    inner: Mutex<CurrentClientState>,
    retired: AtomicBool,
    persist_uncertain: AtomicBool,
}

struct PendingAccess {
    version: StoreVersion,
    client: Option<Arc<TransportClient>>,
    incarnation: u64,
    clear: bool,
    renamed: bool,
}

struct CurrentClientState {
    client: Option<Arc<TransportClient>>,
    lan_client: Option<Arc<TransportClient>>,
    incarnation: u64,
    version: Option<StoreVersion>,
    pending: Option<PendingAccess>,
}

impl CurrentClientManager {
    pub fn new(initial_client: Option<Arc<TransportClient>>) -> Self {
        Self {
            inner: Mutex::new(CurrentClientState {
                client: initial_client,
                lan_client: None,
                incarnation: 0,
                version: None,
                pending: None,
            }),
            retired: AtomicBool::new(false),
            persist_uncertain: AtomicBool::new(false),
        }
    }

    pub fn set_initial(&self, initial_client: Arc<TransportClient>) {
        let mut state = self.inner.lock().expect("client manager lock");
        state.client = Some(initial_client);
    }

    pub fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
        let mut state = self.inner.lock().expect("client manager lock");
        state.client = None;
        state.pending = None;
        state.incarnation = state.incarnation.wrapping_add(1);
    }

    pub fn get(&self) -> (Option<Arc<TransportClient>>, u64) {
        let state = self.inner.lock().expect("client manager lock");
        (
            if self.retired.load(Ordering::SeqCst) {
                None
            } else {
                state.client.clone()
            },
            state.incarnation,
        )
    }

    pub fn swap(&self, new_client: Option<Arc<TransportClient>>) -> u64 {
        let mut state = self.inner.lock().expect("client manager lock");
        if self.retired.load(Ordering::SeqCst) {
            return state.incarnation;
        }
        state.pending = None;
        state.client = new_client;
        state.incarnation = state.incarnation.wrapping_add(1);
        state.incarnation
    }

    #[cfg(any(test, feature = "access-test-hooks"))]
    pub fn hook_for_test(
        self: &Arc<Self>,
        store: &LinkCredentialStore,
        identity: &PairingIdentity,
        origin: &str,
    ) -> TokenPersistHook {
        self.inner.lock().expect("client manager lock").version =
            Some(store.capture_version(identity).expect("test version"));
        make_token_persist_hook(
            Some(origin.to_owned()),
            store,
            identity,
            self.incarnation(),
            Arc::downgrade(self),
        )
        .expect("test hook")
    }

    #[cfg(any(test, feature = "access-test-hooks"))]
    pub fn persistence_uncertain_for_test(&self) -> bool {
        self.persist_uncertain.load(Ordering::SeqCst)
    }

    #[cfg(any(test, feature = "access-test-hooks"))]
    pub fn opener_for_test(
        self: &Arc<Self>,
        tracker: Arc<StatusTracker>,
    ) -> Arc<dyn CarrierOpener> {
        Arc::new(SolstoneCarrierOpener {
            client_manager: self.clone(),
            tracker,
        })
    }

    pub fn incarnation(&self) -> u64 {
        self.inner.lock().expect("client manager lock").incarnation
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|_| LinkServeError::new(LinkServeErrorKind::RuntimeUnavailable))?;

        let label = request
            .bundle_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&request.label);
        let store = LinkCredentialStore::new(request.bundle_dir.clone(), label);
        let identity = pairing_identity_from_bundle(&request.bundle)?;

        let mut initial_version = if request.bundle_dir.as_os_str().is_empty() {
            None
        } else {
            Some(
                store
                    .capture_version(&identity)
                    .map_err(|_| LinkServeError::new(LinkServeErrorKind::InvalidBundle))?,
            )
        };
        let store_outcome = if initial_version.is_some() {
            store.load_access()
        } else {
            request
                .bundle
                .relay_access
                .clone()
                .unwrap_or(StoreLoadOutcome::Absent)
        };
        if initial_version.is_some() && store.capture_version(&identity).ok() != initial_version {
            return Err(LinkServeError::new(LinkServeErrorKind::InvalidBundle));
        }

        let (credential, persist_hook_origin, _token_for_hook, initial_persist_uncertain) =
            match request.policy {
                LinkServeCarrierPolicy::Direct => {
                    let cred = credential_base(
                        &request,
                        endpoints_from_bundle(&request.bundle),
                        None,
                        None,
                        None,
                    )?;
                    (cred, None, None, false)
                }
                LinkServeCarrierPolicy::RelayPermitted => {
                    let configured_origin = request.relay_origin.clone();
                    match store_outcome {
                        StoreLoadOutcome::Ready(record)
                            if valid_saved_access(&record, &identity)
                                && configured_origin.as_ref().is_none_or(|orig| {
                                    same_relay_origin(
                                        record.relay_origin.as_deref().unwrap_or_default(),
                                        orig,
                                    )
                                }) =>
                        {
                            let origin = record
                                .relay_origin
                                .as_deref()
                                .and_then(|origin| parse_relay_origin(origin).ok());
                            let token = record.device_token.clone();
                            let exp = record.expires_at;
                            let cred = credential_base(
                                &request,
                                endpoints_from_bundle(&request.bundle),
                                origin.clone(),
                                token.clone(),
                                exp,
                            )?;
                            (cred, origin, token, false)
                        }
                        _ => {
                            let cred = credential_base(
                                &request,
                                endpoints_from_bundle(&request.bundle),
                                None,
                                None,
                                None,
                            )?;
                            (cred, None, None, false)
                        }
                    }
                }
                LinkServeCarrierPolicy::RelayOnly => {
                    let origin = request
                        .relay_origin
                        .clone()
                        .or_else(|| match &store_outcome {
                            StoreLoadOutcome::Ready(record) => record.relay_origin.clone(),
                            _ => None,
                        })
                        .unwrap_or_else(|| "https://link.solstone.app".to_string());
                    let origin = parse_relay_origin(&origin)
                        .map_err(|_| LinkServeError::new(LinkServeErrorKind::InvalidBundle))?;
                    match store_outcome {
                        StoreLoadOutcome::Ready(record) => {
                            if !valid_saved_access(&record, &identity)
                                || !same_relay_origin(
                                    record.relay_origin.as_deref().unwrap_or_default(),
                                    &origin,
                                )
                            {
                                return Err(LinkServeError::new(LinkServeErrorKind::Transport(
                                    LinkServeTransportErrorKind::NoEndpoint,
                                )));
                            }
                            let token = record.device_token.clone();
                            let exp = record.expires_at;
                            let cred = credential_base(
                                &request,
                                Vec::new(),
                                Some(origin.clone()),
                                token.clone(),
                                exp,
                            )?;
                            (cred, Some(origin), token, false)
                        }
                        StoreLoadOutcome::Disabled(_) => {
                            return Err(LinkServeError::new(LinkServeErrorKind::Transport(
                                LinkServeTransportErrorKind::NotPaired,
                            )));
                        }
                        StoreLoadOutcome::Unusable(_) => {
                            return Err(LinkServeError::new(LinkServeErrorKind::InvalidBundle));
                        }
                        StoreLoadOutcome::Absent => {
                            let token = runtime
                                .block_on(self.enrollment.enroll(
                                    &origin,
                                    &request.bundle.instance_id,
                                    &request.bundle.home_attestation,
                                ))
                                .map_err(|error| {
                                    LinkServeError::new(LinkServeErrorKind::Transport(
                                        map_transport_error(error),
                                    ))
                                })?;

                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64;
                            let exp = spl_core::relay_access::instance_claims(
                                &token,
                                &request.bundle.instance_id,
                                now,
                            )
                            .or_else(|| {
                                spl_core::relay_access::legacy_claims(
                                    &token,
                                    &request.bundle.instance_id,
                                    now,
                                )
                            })
                            .map(|c| c.exp)
                            .ok_or_else(|| {
                                LinkServeError::new(LinkServeErrorKind::InvalidBundle)
                            })?;
                            if let Some(version) = &initial_version {
                                let commit = store
                                    .publish_ready_if_current(&origin, &token, exp, version, || {
                                        true
                                    })
                                    .map_err(|_| {
                                        LinkServeError::new(LinkServeErrorKind::InvalidBundle)
                                    })?;
                                if !commit.durable {
                                    return Err(LinkServeError::new(
                                        LinkServeErrorKind::InvalidBundle,
                                    ));
                                }
                                initial_version = Some(commit.version);
                            }
                            let cred = credential_base(
                                &request,
                                Vec::new(),
                                Some(origin.clone()),
                                Some(token.clone()),
                                Some(exp),
                            )?;
                            (cred, Some(origin), Some(token), false)
                        }
                    }
                }
            };

        let client_manager = Arc::new(CurrentClientManager::new(None));

        client_manager
            .inner
            .lock()
            .expect("client manager lock")
            .version = initial_version;

        let token_persist_hook = make_token_persist_hook(
            persist_hook_origin,
            &store,
            &identity,
            0,
            Arc::downgrade(&client_manager),
        );

        let initial_client = match request.policy {
            LinkServeCarrierPolicy::RelayOnly => {
                TransportClient::new_relay_only(credential.clone(), token_persist_hook)
            }
            LinkServeCarrierPolicy::Direct | LinkServeCarrierPolicy::RelayPermitted => {
                TransportClient::new(credential.clone(), token_persist_hook)
            }
        }
        .map_err(|error| {
            LinkServeError::new(LinkServeErrorKind::Transport(map_transport_error(error)))
        })?;

        if request.policy != LinkServeCarrierPolicy::RelayOnly {
            let mut lan = credential.clone();
            lan.relay_origin = None;
            lan.device_token = None;
            lan.device_token_expires_at = None;
            client_manager
                .inner
                .lock()
                .expect("client manager lock")
                .lan_client = TransportClient::new(lan, None).ok().map(Arc::new);
        }
        client_manager.set_initial(Arc::new(initial_client));

        let ca_prefix = ca_fp_prefix(&request.bundle)?;
        let ca_fp_prefix_hex = ca_prefix
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let tracker = Arc::new(StatusTracker::with_metadata(
            self.clock.clone(),
            request.bundle_dir.clone(),
            request.bundle.instance_id.clone(),
            ca_fp_prefix_hex,
            request.bundle.paired_at.clone(),
            Some(runtime.handle().clone()),
            initial_persist_uncertain,
        ));

        let scheduler = Arc::new(OptionalJobScheduler {
            inner: Mutex::new(JobSchedulerState::default()),
            tracker: tracker.clone(),
            client_manager: client_manager.clone(),
            store: store.clone(),
            identity: identity.clone(),
            policy: request.policy,
            configured_relay_origin: request.relay_origin.clone(),
            bundle: request.bundle.clone(),
            ca_fp_prefix: ca_prefix,
        });

        tracker.set_scheduler(scheduler.clone());

        let opener = Arc::new(SolstoneCarrierOpener {
            client_manager: client_manager.clone(),
            tracker: tracker.clone(),
        });
        let policy = bridge_policy_for_port(request.port, tracker.clone());
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
        let port = handle.port();
        tracker.set_bound_port(port);

        let runtime_record = LinkServeRuntimeRecord { port };
        if !request.bundle_dir.as_os_str().is_empty()
            && let Ok(json_bytes) = serde_json::to_vec_pretty(&runtime_record)
        {
            atomic_write_file(&request.bundle_dir.join("serve_runtime.json"), &json_bytes);
        }

        Ok(Box::new(SplLinkServeSession {
            port,
            runtime,
            handle: Some(handle),
            bundle_dir: request.bundle_dir,
            scheduler,
        }))
    }
}

fn valid_saved_access(
    record: &solstone_core_sol_client::link_credentials::RelayAccessRecord,
    identity: &PairingIdentity,
) -> bool {
    let (Some(origin), Some(token), Some(exp)) = (
        &record.relay_origin,
        &record.device_token,
        record.expires_at,
    ) else {
        return false;
    };
    if parse_relay_origin(origin).is_err() || record.identity != *identity {
        return false;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    spl_core::relay_access::renewal_identity(token, now)
        .is_some_and(|(instance, _)| instance == identity.instance_id)
        && spl_core::relay_access::unverified_payload(token)
            .and_then(|v| v.get("exp").and_then(Value::as_i64))
            == Some(exp)
}

fn endpoints_from_bundle(bundle: &LinkServeBundle) -> Vec<EndpointAddr> {
    bundle
        .endpoints
        .iter()
        .map(|endpoint| EndpointAddr {
            host: endpoint.host.clone(),
            port: endpoint.port,
        })
        .collect()
}

fn credential_base(
    request: &LinkServeRequest,
    endpoints: Vec<EndpointAddr>,
    relay_origin: Option<String>,
    device_token: Option<String>,
    device_token_expires_at: Option<i64>,
) -> Result<Credential, LinkServeError> {
    Ok(Credential {
        client_key_pem: request.bundle.private_key_pem.clone(),
        client_cert_pem: request.bundle.client_cert_pem.clone(),
        ca_chain_pem: request.bundle.ca_chain_pem.clone(),
        ca_fp_prefix: ca_fp_prefix(&request.bundle)?,
        instance_id: request.bundle.instance_id.clone(),
        home_label: request.bundle.home_label.clone(),
        endpoints,
        home_attestation: Some(request.bundle.home_attestation.clone()),
        local_endpoints: Some(request.bundle.local_endpoints.clone()),
        relay_origin,
        device_token,
        device_token_expires_at,
    })
}

fn pairing_identity_from_bundle(
    bundle: &LinkServeBundle,
) -> Result<PairingIdentity, LinkServeError> {
    let cert_sha256 = format!(
        "sha256:{}",
        spl_core::ca::sha256_hex(bundle.client_cert_pem.as_bytes())
    );
    let ca_fingerprint = bundle_ca_fingerprint(bundle)?;
    Ok(PairingIdentity {
        cert_sha256,
        instance_id: bundle.instance_id.clone(),
        ca_fingerprint,
    })
}

fn bundle_ca_fingerprint(bundle: &LinkServeBundle) -> Result<String, LinkServeError> {
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
    Ok(format!(
        "sha256:{}",
        spl_core::ca::sha256_hex(first.as_ref())
    ))
}

fn make_token_persist_hook(
    origin: Option<String>,
    store: &LinkCredentialStore,
    identity: &PairingIdentity,
    incarnation: u64,
    client_manager_weak: std::sync::Weak<CurrentClientManager>,
) -> Option<TokenPersistHook> {
    let origin = origin?;
    let store_clone = store.clone();
    let identity_clone = identity.clone();
    Some(Arc::new(move |token: &str, exp: i64| {
        let Some(mgr) = client_manager_weak.upgrade() else {
            return;
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let mut state = mgr.inner.lock().expect("client manager lock");
        if mgr.retired.load(Ordering::SeqCst) || state.incarnation != incarnation {
            return;
        }
        let result = state
            .version
            .as_ref()
            .ok_or(StoreMutationError::StaleGeneration)
            .and_then(|version| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let claims = spl_core::relay_access::instance_claims(
                    token,
                    &identity_clone.instance_id,
                    now,
                )
                .or_else(|| {
                    spl_core::relay_access::legacy_claims(token, &identity_clone.instance_id, now)
                });
                if claims.is_none_or(|c| c.exp != exp) {
                    return Err(StoreMutationError::StaleGeneration);
                }
                store_clone.publish_ready_if_current(&origin, token, exp, version, || {
                    !mgr.retired.load(Ordering::SeqCst) && std::time::Instant::now() < deadline
                })
            });
        match result {
            Ok(commit) if commit.durable && !mgr.retired.load(Ordering::SeqCst) => {
                state.version = Some(commit.version);
                state.pending = None;
                mgr.persist_uncertain.store(false, Ordering::SeqCst);
            }
            result => {
                // The shared hook has no error return. Retire this incarnation so
                // the opener cannot admit the refreshed carrier after failed durability.
                if let Ok(commit) = result {
                    state.version = Some(commit.version);
                }
                mgr.persist_uncertain.store(true, Ordering::SeqCst);
                state.client = state.lan_client.clone();
                state.incarnation = state.incarnation.wrapping_add(1);
            }
        }
    }))
}

struct SplLinkServeSession {
    port: u16,
    runtime: tokio::runtime::Runtime,
    handle: Option<JournalBridgeHandle>,
    bundle_dir: PathBuf,
    scheduler: Arc<OptionalJobScheduler>,
}

impl LinkServeSession for SplLinkServeSession {
    fn bound_port(&self) -> u16 {
        self.port
    }

    fn serve(mut self: Box<Self>, shutdown: &dyn ShutdownSignal) -> Result<(), LinkServeError> {
        shutdown.wait();
        // 1. Retire epoch
        self.scheduler.retire();
        // 2. Delete runtime record
        if !self.bundle_dir.as_os_str().is_empty() {
            let _ = std::fs::remove_file(self.bundle_dir.join("serve_runtime.json"));
        }
        // 3. Shutdown and wait
        if let Some(handle) = self.handle.take() {
            self.runtime.block_on(handle.shutdown_and_wait());
        }
        Ok(())
    }
}

struct SolstoneCarrierOpener {
    client_manager: Arc<CurrentClientManager>,
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
            let (client_opt, incarnation) = self.client_manager.get();
            let result = match client_opt {
                Some(client) => client.dial_carrier().await,
                None => Err(TransportError::NotPaired),
            };
            let mut state = self
                .client_manager
                .inner
                .lock()
                .expect("client manager lock");
            if self.client_manager.retired.load(Ordering::SeqCst)
                || state.incarnation != incarnation
            {
                return Err(TransportError::NotPaired);
            }
            if matches!(
                &result,
                Err(TransportError::NotPaired
                    | TransportError::TlsAccessDenied
                    | TransportError::TlsCertificateUnknown)
            ) {
                state.client = None;
                state.pending = None;
                state.incarnation = state.incarnation.wrapping_add(1);
                self.tracker.operation_epoch.fetch_add(1, Ordering::SeqCst);
            }
            match &result {
                Ok(_) => self.tracker.carrier_open_succeeded(),
                Err(error) => self.tracker.carrier_open_failed(error),
            }
            result
        })
    }
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
        "ca_fp_prefix".to_string(),
        Value::String(snapshot.ca_fp_prefix.clone()),
    );
    root.insert(
        "connected_age_seconds".to_string(),
        option_f64(snapshot.connected_age_seconds),
    );
    root.insert("health".to_string(), Value::String(snapshot.health.clone()));
    root.insert(
        "instance_id".to_string(),
        Value::String(snapshot.instance_id.clone()),
    );
    root.insert(
        "journal_version".to_string(),
        snapshot
            .journal_version
            .as_ref()
            .map_or(Value::Null, |v| Value::String(v.clone())),
    );
    root.insert(
        "journal_version_fresh".to_string(),
        Value::Bool(snapshot.journal_version_fresh),
    );
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
        "paired_at".to_string(),
        Value::String(snapshot.paired_at.clone()),
    );
    root.insert(
        "persist_uncertain".to_string(),
        Value::Bool(snapshot.persist_uncertain),
    );
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
    bundle_dir: PathBuf,
    operation_epoch: std::sync::atomic::AtomicU64,
    retired: AtomicBool,
    expected_identity: Option<PairingIdentity>,
    expected_dev_ino: Option<(u64, u64)>,
    instance_id: String,
    ca_fp_prefix_hex: String,
    paired_at: String,
    runtime_handle: Option<tokio::runtime::Handle>,
    scheduler: Mutex<Option<Arc<OptionalJobScheduler>>>,
}

#[derive(Debug, Default)]
struct StatusTrackerState {
    last_connected_at: Option<f64>,
    last_failure: Option<LinkServeFailure>,
    reconnect_count: u64,
    bound_port: Option<u16>,
    generation: u64,
    fetching_generation: Option<u64>,
    pending_fetch_generation: Option<u64>,
    cached_version: Option<String>,
    version_fresh: bool,
    persist_uncertain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    StaleGeneration,
    FetchFailed,
    UpdatedNoPersist,
    PairingMismatch,
    Persisted,
}

impl StatusTracker {
    #[allow(dead_code)]
    pub fn new(clock: Arc<dyn StatusClock>) -> Self {
        Self::with_metadata(
            clock,
            PathBuf::new(),
            String::new(),
            String::new(),
            String::new(),
            None,
            false,
        )
    }

    pub fn with_metadata(
        clock: Arc<dyn StatusClock>,
        bundle_dir: PathBuf,
        instance_id: String,
        ca_fp_prefix_hex: String,
        paired_at: String,
        runtime_handle: Option<tokio::runtime::Handle>,
        persist_uncertain: bool,
    ) -> Self {
        let (expected_dev_ino, expected_identity, cached_version) =
            if bundle_dir.as_os_str().is_empty() {
                (None, None, None)
            } else {
                let dev_ino = get_file_dev_ino(&bundle_dir).ok();
                let store = LinkCredentialStore::new(bundle_dir.clone(), "");
                let id = store.compute_identity().ok();
                let metadata_path = bundle_dir.join("journal_metadata.json");
                let version = std::fs::read_to_string(&metadata_path)
                    .ok()
                    .and_then(|content| serde_json::from_str::<LinkJournalMetadata>(&content).ok())
                    .filter(|meta| {
                        meta.instance_id == instance_id
                            && meta.ca_fp_prefix == ca_fp_prefix_hex
                            && meta.paired_at == paired_at
                            && is_valid_journal_version(&meta.journal_version)
                    })
                    .map(|meta| meta.journal_version);
                (dev_ino, id, version)
            };
        Self {
            inner: Mutex::new(StatusTrackerState {
                cached_version,
                version_fresh: false,
                persist_uncertain,
                ..StatusTrackerState::default()
            }),
            clock,
            bundle_dir,
            operation_epoch: std::sync::atomic::AtomicU64::new(0),
            retired: AtomicBool::new(false),
            expected_identity,
            expected_dev_ino,
            instance_id,
            ca_fp_prefix_hex,
            paired_at,
            runtime_handle,
            scheduler: Mutex::new(None),
        }
    }

    pub fn set_scheduler(&self, scheduler: Arc<OptionalJobScheduler>) {
        *self.scheduler.lock().expect("scheduler lock") = Some(scheduler);
    }

    pub fn set_persist_uncertain(&self, uncertain: bool) {
        let mut state = self.inner.lock().expect("status tracker lock");
        state.persist_uncertain = uncertain;
    }

    pub fn set_bound_port(self: &Arc<Self>, port: u16) {
        let dispatch_generation = {
            let mut state = self.inner.lock().expect("status tracker lock");
            state.bound_port = Some(port);
            let pending = state
                .pending_fetch_generation
                .take()
                .filter(|&pending| state.fetching_generation == Some(pending));
            pending.or_else(|| {
                if state.fetching_generation.is_none() {
                    let generation = state.generation;
                    state.fetching_generation = Some(generation);
                    Some(generation)
                } else {
                    None
                }
            })
        };
        if let Some(target_gen) = dispatch_generation {
            let scheduler = self.scheduler.lock().expect("scheduler lock").clone();
            if let (Some(sched), Some(handle)) = (scheduler, self.runtime_handle.as_ref()) {
                sched.trigger(handle, port, target_gen);
            }
        }
    }

    fn carrier_open_succeeded(self: &Arc<Self>) {
        let (port, new_generation, should_fetch) = {
            let mut state = self.inner.lock().expect("status tracker lock");
            state.last_connected_at = Some(self.clock.now_unix_seconds());
            state.generation = state.generation.saturating_add(1);
            state.version_fresh = false;
            let current_generation = state.generation;
            let should_fetch = if state.fetching_generation != Some(current_generation) {
                state.fetching_generation = Some(current_generation);
                true
            } else {
                false
            };
            if should_fetch && state.bound_port.is_none() {
                state.pending_fetch_generation = Some(current_generation);
            }
            (state.bound_port, current_generation, should_fetch)
        };
        if should_fetch && let Some(port) = port {
            let scheduler = self.scheduler.lock().expect("scheduler lock").clone();
            if let (Some(sched), Some(handle)) = (scheduler, self.runtime_handle.as_ref()) {
                sched.trigger(handle, port, new_generation);
            }
        }
    }

    fn pairing_is_current(&self) -> bool {
        if self.bundle_dir.as_os_str().is_empty() {
            return true;
        }
        if let Some(expected_dev_ino) = self.expected_dev_ino
            && get_file_dev_ino(&self.bundle_dir).ok() != Some(expected_dev_ino)
        {
            return false;
        }
        if let Some(expected_identity) = &self.expected_identity {
            let store = LinkCredentialStore::new(self.bundle_dir.clone(), "");
            if store.compute_identity().as_ref() != Ok(expected_identity) {
                return false;
            }
        }
        if !self.paired_at.is_empty() {
            let peer_match = std::fs::read(self.bundle_dir.join("peer.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|peer| {
                    peer.get("paired_at").and_then(Value::as_str) == Some(&self.paired_at)
                });
            if !peer_match {
                return false;
            }
        }
        true
    }

    fn carrier_open_failed(&self, error: &TransportError) {
        let mut state = self.inner.lock().expect("status tracker lock");
        state.reconnect_count = state.reconnect_count.saturating_add(1);
        state.generation = state.generation.saturating_add(1);
        state.last_failure = Some(failure_from_transport(error, self.clock.now_unix_seconds()));
        state.version_fresh = false;
    }

    #[cfg(any(test, feature = "access-test-hooks"))]
    pub fn carrier_events_for_test(&self) -> (u64, u64) {
        let state = self.inner.lock().expect("status tracker lock");
        (state.generation, state.reconnect_count)
    }

    #[cfg(any(test, feature = "host"))]
    pub fn bump_generation_for_test(&self) {
        self.operation_epoch.fetch_add(1, Ordering::SeqCst);
        let mut state = self.inner.lock().expect("status tracker lock");
        state.generation = state.generation.saturating_add(1);
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
            journal_version: state.cached_version.clone(),
            journal_version_fresh: state.version_fresh && bridge.carrier_live,
            instance_id: self.instance_id.clone(),
            ca_fp_prefix: self.ca_fp_prefix_hex.clone(),
            paired_at: self.paired_at.clone(),
            persist_uncertain: state.persist_uncertain
                || self
                    .scheduler
                    .lock()
                    .expect("scheduler lock")
                    .as_ref()
                    .is_some_and(|s| s.client_manager.persist_uncertain.load(Ordering::SeqCst)),
        }
    }

    pub fn apply_fetch_result(
        &self,
        target_generation: u64,
        parsed: Option<String>,
        journal_name: Option<String>,
    ) -> ApplyOutcome {
        if self.inner.lock().expect("status tracker lock").generation != target_generation {
            return ApplyOutcome::StaleGeneration;
        }
        let epoch = self.operation_epoch.load(Ordering::SeqCst);
        let version =
            self.metadata_store_version(std::time::Instant::now() + Duration::from_secs(15));
        self.apply_metadata_result(
            epoch,
            version.as_ref(),
            parsed,
            journal_name,
            true,
            std::time::Instant::now() + Duration::from_secs(15),
        )
    }

    fn metadata_store_version(&self, deadline: std::time::Instant) -> Option<StoreVersion> {
        let identity = self.expected_identity.as_ref()?;
        LinkCredentialStore::new(self.bundle_dir.clone(), "")
            .capture_version_if_current(identity, || {
                !self.retired.load(Ordering::SeqCst) && std::time::Instant::now() < deadline
            })
            .ok()
    }

    fn operation_is_current(&self, epoch: u64) -> bool {
        !self.retired.load(Ordering::SeqCst)
            && self.operation_epoch.load(Ordering::SeqCst) == epoch
            && self.pairing_is_current()
    }

    fn apply_metadata_result(
        &self,
        epoch: u64,
        version: Option<&StoreVersion>,
        parsed: Option<String>,
        journal_name: Option<String>,
        preserve_name: bool,
        deadline: std::time::Instant,
    ) -> ApplyOutcome {
        let mut state = self.inner.lock().expect("status tracker lock");
        if !self.operation_is_current(epoch) {
            return ApplyOutcome::PairingMismatch;
        }
        state.fetching_generation = None;
        let Some(journal_version) = parsed.filter(|v| is_valid_journal_version(v)) else {
            return ApplyOutcome::FetchFailed;
        };
        if self.bundle_dir.as_os_str().is_empty() {
            state.cached_version = Some(journal_version);
            state.version_fresh = true;
            return ApplyOutcome::UpdatedNoPersist;
        }
        let Some(version) = version else {
            return ApplyOutcome::PairingMismatch;
        };
        let journal_name = if preserve_name && journal_name.is_none() {
            std::fs::read(self.bundle_dir.join("journal_metadata.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<LinkJournalMetadata>(&bytes).ok())
                .filter(|meta| {
                    meta.instance_id == self.instance_id
                        && meta.ca_fp_prefix == self.ca_fp_prefix_hex
                        && meta.paired_at == self.paired_at
                })
                .and_then(|meta| meta.journal_name)
        } else {
            journal_name
        };
        let metadata = LinkJournalMetadata {
            instance_id: self.instance_id.clone(),
            ca_fp_prefix: self.ca_fp_prefix_hex.clone(),
            paired_at: self.paired_at.clone(),
            journal_version: journal_version.clone(),
            journal_name,
            observed_at: self.clock.now_unix_seconds(),
        };
        let result = LinkCredentialStore::new(self.bundle_dir.clone(), "")
            .write_journal_metadata_if_current(&metadata, version, || {
                !self.retired.load(Ordering::SeqCst)
                    && self.operation_epoch.load(Ordering::SeqCst) == epoch
                    && std::time::Instant::now() < deadline
            });
        match result {
            Ok(true) => {
                state.cached_version = Some(journal_version);
                state.version_fresh = true;
                ApplyOutcome::Persisted
            }
            Ok(false) | Err(StoreMutationError::PersistUncertain(_)) => {
                state.persist_uncertain = true;
                ApplyOutcome::FetchFailed
            }
            Err(
                StoreMutationError::IdentityMismatch
                | StoreMutationError::StaleGeneration
                | StoreMutationError::BundleNotFound
                | StoreMutationError::Retired,
            ) => ApplyOutcome::PairingMismatch,
            Err(_) => ApplyOutcome::FetchFailed,
        }
    }
}

pub struct OptionalJobScheduler {
    inner: Mutex<JobSchedulerState>,
    tracker: Arc<StatusTracker>,
    client_manager: Arc<CurrentClientManager>,
    store: LinkCredentialStore,
    identity: PairingIdentity,
    policy: LinkServeCarrierPolicy,
    configured_relay_origin: Option<String>,
    bundle: LinkServeBundle,
    ca_fp_prefix: Vec<u8>,
}

pub struct JobSchedulerTestParams {
    pub tracker: Arc<StatusTracker>,
    pub client_manager: Arc<CurrentClientManager>,
    pub store: LinkCredentialStore,
    pub identity: PairingIdentity,
    pub policy: LinkServeCarrierPolicy,
    pub configured_relay_origin: Option<String>,
    pub bundle: LinkServeBundle,
    pub ca_fp_prefix: Vec<u8>,
}

#[derive(Default)]
struct JobSchedulerState {
    running: bool,
    pending: bool,
    generation: u64,
    retired: bool,
}

impl OptionalJobScheduler {
    pub fn new_for_test(params: JobSchedulerTestParams) -> Self {
        Self {
            inner: Mutex::new(JobSchedulerState::default()),
            tracker: params.tracker,
            client_manager: params.client_manager,
            store: params.store,
            identity: params.identity,
            policy: params.policy,
            configured_relay_origin: params.configured_relay_origin,
            bundle: params.bundle,
            ca_fp_prefix: params.ca_fp_prefix,
        }
    }

    pub fn retire(&self) {
        self.tracker.retired.store(true, Ordering::SeqCst);
        self.client_manager.retire();
        let mut state = self.inner.lock().expect("scheduler lock");
        state.retired = true;
    }

    pub fn trigger(self: &Arc<Self>, handle: &tokio::runtime::Handle, port: u16, generation: u64) {
        {
            let mut state = self.inner.lock().expect("scheduler lock");
            if state.retired {
                return;
            }
            if state.running {
                state.pending = true;
                state.generation = generation;
                return;
            }
            state.running = true;
            state.pending = false;
            state.generation = generation;
        }
        let scheduler = Arc::clone(self);
        handle.spawn_blocking(move || scheduler.run_burst(port));
    }

    pub fn run_burst(self: Arc<Self>, port: u16) {
        for attempt in 0..2 {
            let gen_num = {
                let state = self.inner.lock().expect("scheduler lock");
                if state.retired {
                    return;
                }
                state.generation
            };

            std::thread::scope(|s| {
                s.spawn(|| self.run_access_job(port, gen_num));
                s.spawn(|| self.run_metadata_job(port, gen_num));
            });

            let mut state = self.inner.lock().expect("scheduler lock");
            if state.retired {
                return;
            }
            if attempt == 0 && state.pending {
                state.pending = false;
                // reconnect during the final bounded pass may be coalesced away.
                continue;
            }
            state.running = false;
            let mut tracker_state = self.tracker.inner.lock().expect("status tracker lock");
            tracker_state.fetching_generation = None;
            break;
        }
    }

    fn reconcile_pending(&self, deadline: std::time::Instant) {
        let mut state = self
            .client_manager
            .inner
            .lock()
            .expect("client manager lock");
        if self.client_manager.retired.load(Ordering::SeqCst) {
            return;
        }
        let Some(pending) = state.pending.take() else {
            return;
        };
        let current = || {
            !self.client_manager.retired.load(Ordering::SeqCst)
                && std::time::Instant::now() < deadline
        };
        let result = if pending.clear && !pending.renamed {
            self.store
                .publish_disabled_if_current(&pending.version, current)
        } else {
            self.store.reconcile_if_current(&pending.version, current)
        };
        if self.client_manager.retired.load(Ordering::SeqCst) {
            return;
        }
        match result {
            Ok(commit) if commit.durable => {
                state.version = Some(commit.version);
                if !pending.clear && self.policy != LinkServeCarrierPolicy::Direct {
                    state.client = pending.client;
                    state.incarnation = pending.incarnation;
                }
                self.client_manager
                    .persist_uncertain
                    .store(false, Ordering::SeqCst);
            }
            Ok(commit) => {
                state.pending = Some(PendingAccess {
                    version: commit.version,
                    renamed: true,
                    ..pending
                });
            }
            Err(
                StoreMutationError::StaleGeneration
                | StoreMutationError::IdentityMismatch
                | StoreMutationError::BundleNotFound,
            ) => {}
            Err(_) => state.pending = Some(pending),
        }
    }

    pub fn run_access_job(&self, port: u16, _target_generation: u64) {
        if self.store.bundle_dir().as_os_str().is_empty() {
            return;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        self.reconcile_pending(deadline);
        let operation_epoch = self.tracker.operation_epoch.load(Ordering::SeqCst);
        let incarnation = self.client_manager.incarnation();
        if self.client_manager.retired.load(Ordering::SeqCst) || !self.tracker.pairing_is_current()
        {
            return;
        }
        // Disk revision is captured before I/O and compared under the real writer lock.
        let Ok(version) = self.store.capture_version_if_current(&self.identity, || {
            !self.client_manager.retired.load(Ordering::SeqCst)
                && std::time::Instant::now() < deadline
        }) else {
            return;
        };
        let Some(agent) = description_agent(deadline) else {
            return;
        };
        let url = format!("http://127.0.0.1:{port}/app/network/api/relay/access");
        let Ok(resp) = agent
            .get(&url)
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .call()
        else {
            return;
        };
        if resp.status().as_u16() != 200 {
            return;
        }
        let Some(body) = bounded_response_body(resp) else {
            return;
        };
        let Ok(access_resp) = serde_json::from_slice::<WireRelayAccessResponse>(&body) else {
            return;
        };
        if std::time::Instant::now() >= deadline {
            return;
        }

        let (ready, candidate) = match access_resp {
            WireRelayAccessResponse::Ready {
                protocol_version,
                relay_origin,
                instance_id,
                device_token,
                expires_at,
            } => {
                if protocol_version != 2 || instance_id != self.identity.instance_id {
                    return;
                }
                let Ok(relay_origin) = parse_relay_origin(&relay_origin) else {
                    return;
                };
                if self
                    .configured_relay_origin
                    .as_ref()
                    .is_some_and(|configured| !same_relay_origin(&relay_origin, configured))
                {
                    return;
                }
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let Some(claims) = spl_core::relay_access::negotiated_claims(
                    2,
                    &device_token,
                    &expires_at,
                    &self.identity.instance_id,
                    now,
                ) else {
                    return;
                };
                let candidate = if self.policy == LinkServeCarrierPolicy::Direct {
                    None
                } else {
                    let hook = make_token_persist_hook(
                        Some(relay_origin.clone()),
                        &self.store,
                        &self.identity,
                        incarnation.wrapping_add(1),
                        Arc::downgrade(&self.client_manager),
                    );
                    let Ok(client) = self.make_client(
                        Some(relay_origin.clone()),
                        Some(device_token.clone()),
                        Some(claims.exp),
                        hook,
                    ) else {
                        return;
                    };
                    Some(Arc::new(client))
                };
                (Some((relay_origin, device_token, claims.exp)), candidate)
            }
            WireRelayAccessResponse::NotConfigured { protocol_version } => {
                if protocol_version != 2 {
                    return;
                }
                let candidate = if self.policy == LinkServeCarrierPolicy::RelayPermitted {
                    let Ok(client) = self.make_client(None, None, None, None) else {
                        return;
                    };
                    Some(Arc::new(client))
                } else {
                    None
                };
                (None, candidate)
            }
        };
        let mut state = self
            .client_manager
            .inner
            .lock()
            .expect("client manager lock");
        let current = || {
            !self.client_manager.retired.load(Ordering::SeqCst)
                && !self.tracker.retired.load(Ordering::SeqCst)
                && self.tracker.operation_epoch.load(Ordering::SeqCst) == operation_epoch
                && ready.as_ref().is_none_or(|(_, _, exp)| {
                    *exp > SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64
                })
                && std::time::Instant::now() < deadline
        };
        if state.incarnation != incarnation || !current() {
            return;
        }
        let result = match &ready {
            Some((origin, token, exp)) => self
                .store
                .publish_ready_if_current(origin, token, *exp, &version, current),
            None => self.store.publish_disabled_if_current(&version, current),
        };
        if self.client_manager.retired.load(Ordering::SeqCst) {
            return;
        }
        match result {
            Ok(commit) if commit.durable => {
                state.version = Some(commit.version);
                state.pending = None;
                if self.policy != LinkServeCarrierPolicy::Direct {
                    state.client = candidate;
                    state.incarnation = state.incarnation.wrapping_add(1);
                }
                self.client_manager
                    .persist_uncertain
                    .store(false, Ordering::SeqCst);
            }
            Ok(commit) => {
                let clear = ready.is_none();
                if clear && self.policy != LinkServeCarrierPolicy::Direct {
                    state.client = candidate.clone();
                    state.incarnation = state.incarnation.wrapping_add(1);
                }
                state.pending = Some(PendingAccess {
                    version: commit.version,
                    client: candidate,
                    incarnation: if clear || self.policy == LinkServeCarrierPolicy::Direct {
                        state.incarnation
                    } else {
                        state.incarnation.wrapping_add(1)
                    },
                    clear,
                    renamed: true,
                });
                self.client_manager
                    .persist_uncertain
                    .store(true, Ordering::SeqCst);
            }
            Err(
                StoreMutationError::StaleGeneration
                | StoreMutationError::IdentityMismatch
                | StoreMutationError::Retired
                | StoreMutationError::BundleNotFound,
            ) => {}
            Err(_) if ready.is_none() => {
                if self.policy != LinkServeCarrierPolicy::Direct {
                    state.client = candidate.clone();
                    state.incarnation = state.incarnation.wrapping_add(1);
                }
                state.pending = Some(PendingAccess {
                    version,
                    client: candidate,
                    incarnation: state.incarnation,
                    clear: true,
                    renamed: false,
                });
                self.client_manager
                    .persist_uncertain
                    .store(true, Ordering::SeqCst);
            }
            Err(_) => {}
        }
    }

    fn make_client(
        &self,
        relay_origin: Option<String>,
        device_token: Option<String>,
        device_token_expires_at: Option<i64>,
        hook: Option<TokenPersistHook>,
    ) -> Result<TransportClient, TransportError> {
        let credential = Credential {
            client_key_pem: self.bundle.private_key_pem.clone(),
            client_cert_pem: self.bundle.client_cert_pem.clone(),
            ca_chain_pem: self.bundle.ca_chain_pem.clone(),
            ca_fp_prefix: self.ca_fp_prefix.clone(),
            instance_id: self.bundle.instance_id.clone(),
            home_label: self.bundle.home_label.clone(),
            endpoints: if self.policy == LinkServeCarrierPolicy::RelayOnly {
                Vec::new()
            } else {
                endpoints_from_bundle(&self.bundle)
            },
            home_attestation: Some(self.bundle.home_attestation.clone()),
            local_endpoints: Some(self.bundle.local_endpoints.clone()),
            relay_origin,
            device_token,
            device_token_expires_at,
        };
        if self.policy == LinkServeCarrierPolicy::RelayOnly {
            TransportClient::new_relay_only(credential, hook)
        } else {
            TransportClient::new(credential, hook)
        }
    }

    pub fn run_metadata_job(&self, port: u16, target_generation: u64) {
        publish_device_description(self.tracker.clone(), port, target_generation);
    }
}

fn is_valid_journal_version(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

fn local_device_description() -> crate::client_description::ReportedDescription {
    let name = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
        });
    let name = crate::client_description::sanitize_string(name, 80).unwrap_or(None);
    let platform = Some(std::env::consts::OS.to_owned());
    let app_id = Some("solstone".to_owned());
    let app_version = Some(env!("CARGO_PKG_VERSION").to_owned());
    crate::client_description::ReportedDescription {
        name,
        platform,
        device_type: None,
        app_id,
        app_version,
    }
}

pub fn publish_device_description(tracker: Arc<StatusTracker>, port: u16, target_generation: u64) {
    publish_device_description_with(tracker, port, target_generation, local_device_description);
}

pub fn publish_device_description_with(
    tracker: Arc<StatusTracker>,
    port: u16,
    _target_generation: u64,
    sample: impl Fn() -> crate::client_description::ReportedDescription,
) {
    let epoch = tracker.operation_epoch.load(Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let version = tracker.metadata_store_version(deadline);
    if !tracker.bundle_dir.as_os_str().is_empty() && version.is_none() {
        return;
    }
    let self_url = format!("http://127.0.0.1:{port}/app/network/api/clients/self");
    for _ in 0..2 {
        if !tracker.operation_is_current(epoch) {
            return;
        }
        let Some(agent) = description_agent(deadline) else {
            return;
        };
        let Ok(get_resp) = agent
            .get(&self_url)
            .header("Cache-Control", "no-cache")
            .header("Pragma", "no-cache")
            .call()
        else {
            return;
        };
        if get_resp.status().as_u16() == 404 {
            // Older homes expose version only. Keep an existing name in that case.
            let Some(agent) = description_agent(deadline) else {
                return;
            };
            let url = format!("http://127.0.0.1:{port}/api/system/status");
            let Ok(response) = agent.get(&url).header("Cache-Control", "no-cache").call() else {
                return;
            };
            if response.status().as_u16() != 200 {
                return;
            }
            let Some(body) = bounded_response_body(response) else {
                return;
            };
            if std::time::Instant::now() >= deadline {
                return;
            }
            let parsed = serde_json::from_slice::<Value>(&body).ok().and_then(|v| {
                v.get("version")
                    .and_then(|v| v.get("current"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            tracker.apply_metadata_result(epoch, version.as_ref(), parsed, None, true, deadline);
            return;
        }
        if get_resp.status().as_u16() != 200 {
            return;
        }
        let Some(body) = bounded_response_body(get_resp) else {
            return;
        };
        if std::time::Instant::now() >= deadline {
            return;
        }
        let Some(desc_resp) = parse_description_response(&body) else {
            return;
        };
        if !matches!(
            tracker.apply_metadata_result(
                epoch,
                version.as_ref(),
                Some(desc_resp.journal.version),
                desc_resp.journal.name,
                false,
                deadline
            ),
            ApplyOutcome::Persisted | ApplyOutcome::UpdatedNoPersist
        ) {
            return;
        }
        let local = sanitize_local_description(sample());
        if desc_resp.reported == Some(local.clone()) {
            return;
        }
        let put_req = crate::client_description::PutSelfDescriptionRequest {
            protocol_version: 1,
            expected_revision: desc_resp.revision,
            reported: Some(local),
        };
        let Ok(put_body) = serde_json::to_vec(&put_req) else {
            return;
        };
        if !tracker.operation_is_current(epoch) {
            return;
        }
        let Some(agent) = description_agent(deadline) else {
            return;
        };
        let Ok(put_resp) = agent
            .put(&self_url)
            .header("Content-Type", "application/json")
            .send(&put_body[..])
        else {
            return;
        };
        if put_resp.status().as_u16() == 409 {
            continue;
        }
        if put_resp.status().as_u16() != 200 {
            return;
        }
        let Some(body) = bounded_response_body(put_resp) else {
            return;
        };
        if std::time::Instant::now() >= deadline {
            return;
        }
        let Some(response) = parse_description_response(&body) else {
            return;
        };
        tracker.apply_metadata_result(
            epoch,
            version.as_ref(),
            Some(response.journal.version),
            response.journal.name,
            false,
            deadline,
        );
        return;
    }
}

fn sanitize_local_description(
    raw: crate::client_description::ReportedDescription,
) -> crate::client_description::ReportedDescription {
    use crate::client_description::{ReportedDescription, sanitize_string};
    ReportedDescription {
        name: sanitize_string(raw.name, 80).unwrap_or(None),
        platform: sanitize_string(raw.platform, 64).unwrap_or(None),
        device_type: sanitize_string(raw.device_type, 64).unwrap_or(None),
        app_id: sanitize_string(raw.app_id, 64).unwrap_or(None),
        app_version: sanitize_string(raw.app_version, 64).unwrap_or(None),
    }
}

fn parse_description_response(
    body: &[u8],
) -> Option<crate::client_description::ClientDescriptionResponse> {
    let response: crate::client_description::ClientDescriptionResponse =
        serde_json::from_slice(body).ok()?;
    if response.protocol_version != 1 || !is_valid_journal_version(&response.journal.version) {
        return None;
    }
    if let Some(reported) = &response.reported
        && crate::client_description::sanitize_reported(reported.clone())
            .ok()
            .as_ref()
            != Some(reported)
    {
        return None;
    }
    Some(response)
}

fn bounded_response_body(response: ureq::http::Response<ureq::Body>) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut body = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(65537)
        .read_to_end(&mut body)
        .ok()?;
    (body.len() <= 65536).then_some(body)
}

fn description_agent(deadline: std::time::Instant) -> Option<ureq::Agent> {
    let remaining = deadline.checked_duration_since(std::time::Instant::now())?;
    Some(ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .timeout_global(Some(remaining))
            .build(),
    ))
}

fn atomic_write_file(path: &Path, bytes: &[u8]) {
    #[cfg(feature = "host")]
    {
        let _ = solstone_core_journal_io::atomic_replace(
            path,
            bytes,
            solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
        );
    }
    #[cfg(not(feature = "host"))]
    {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, path);
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
pub struct SystemStatusClock;

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
        TransportError::TlsAccessDenied => LinkServeTransportErrorKind::TlsAccessDenied,
        TransportError::TlsCertificateUnknown => LinkServeTransportErrorKind::TlsCertificateUnknown,
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
        RelayError::HomeListenConnection => LinkServeRelayErrorKind::Abnormal,
        RelayError::HomeRelayConfiguration | RelayError::HomeTunnelRejected(_) => {
            LinkServeRelayErrorKind::UpgradeRejected
        }
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
        LinkServeTransportErrorKind::TlsAccessDenied => "tls-access-denied",
        LinkServeTransportErrorKind::TlsCertificateUnknown => "tls-certificate-unknown",
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

#[cfg(test)]
mod tests {
    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use serde_json::json;
    use solstone_core_sol_client::link_credentials::{RelayAccessRecord, RelayAccessState};
    use solstone_core_sol_client::seam::LinkServeEndpoint;
    use spl_core::bridge::RequestHead;
    use std::fs;
    use std::sync::atomic::Ordering;

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
                calls
                    .lock()
                    .expect("enrollment calls lock")
                    .push(call.clone());
                Ok(test_instance_token(&call.instance_id))
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
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client key");
        let params =
            CertificateParams::new(vec!["client.test".to_string()]).expect("client params");
        let cert = params.self_signed(&key).expect("client cert");
        let ca = ca_pem();
        LinkServeRequest {
            label: "laptop".to_string(),
            port: 0,
            policy,
            relay_origin: relay_origin.map(str::to_string),
            bundle: LinkServeBundle {
                private_key_pem: key.serialize_pem(),
                client_cert_pem: cert.pem(),
                ca_chain_pem: vec![ca],
                home_attestation: "attestation.jwt".to_string(),
                instance_id: "home-instance".to_string(),
                home_label: "Home".to_string(),
                paired_at: "2026-07-26T00:00:00Z".to_string(),
                endpoints: vec![LinkServeEndpoint {
                    host: "192.168.1.10".to_string(),
                    port: 7657,
                }],
                local_endpoints: json!([{"ip": "192.168.1.10", "port": 7657}]),
                relay_access: None,
            },
            bundle_dir: PathBuf::new(),
        }
    }

    fn test_instance_token(instance: &str) -> String {
        use base64::Engine as _;
        let payload = json!({"iss":"issuer", "sub":format!("instance:{instance}"), "aud":"spl-relay", "scope":"session.dial", "ver":2,
            "instance_id":instance, "iat":1700000000i64, "exp":2500000000i64, "jti":"test"});
        format!(
            "e30.{}.sig",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }

    fn persist_test_request(request: &mut LinkServeRequest) {
        fs::write(
            request.bundle_dir.join("cert.pem"),
            &request.bundle.client_cert_pem,
        )
        .unwrap();
        fs::write(
            request.bundle_dir.join("chain.pem"),
            request.bundle.ca_chain_pem.join("\n"),
        )
        .unwrap();
        fs::write(
            request.bundle_dir.join("peer.json"),
            json!({"instance_id":request.bundle.instance_id, "paired_at":request.bundle.paired_at})
                .to_string(),
        )
        .unwrap();
        let identity = pairing_identity_from_bundle(&request.bundle).unwrap();
        if let Some(StoreLoadOutcome::Ready(record) | StoreLoadOutcome::Disabled(record)) =
            &mut request.bundle.relay_access
        {
            record.identity = identity;
            if record.state == RelayAccessState::Ready {
                record.device_token = Some(test_instance_token(&record.identity.instance_id));
                record.expires_at = Some(2500000000);
            }
            fs::write(
                request.bundle_dir.join("relay_access.json"),
                serde_json::to_vec(record).unwrap(),
            )
            .unwrap();
        }
    }

    fn bridge_status(listener_active: bool, carrier_live: bool) -> JournalBridgeStatus {
        JournalBridgeStatus {
            listener_active,
            contacted: false,
            carrier_live,
            active_requests: 0,
            terminal_reason: None,
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
        let enrollment = Arc::new(FakeEnrollment::default());
        let starter = ServeStarter {
            enrollment: enrollment.clone(),
            clock: Arc::new(SystemStatusClock),
        };
        let request = serve_request(
            LinkServeCarrierPolicy::Direct,
            Some("https://poisoned.invalid"),
        );

        let session = starter
            .start(request)
            .expect("direct starter should succeed");
        assert!(session.bound_port() > 0);
        assert!(enrollment.calls().is_empty());
    }

    #[test]
    fn relay_permitted_does_not_enroll_at_startup() {
        let enrollment = Arc::new(FakeEnrollment::default());
        let starter = ServeStarter {
            enrollment: enrollment.clone(),
            clock: Arc::new(SystemStatusClock),
        };
        let request = serve_request(
            LinkServeCarrierPolicy::RelayPermitted,
            Some("https://relay.example"),
        );

        let session = starter
            .start(request)
            .expect("relay permitted starter should succeed");
        assert!(session.bound_port() > 0);
        assert!(enrollment.calls().is_empty());
    }

    #[test]
    fn relay_only_credentials_enroll_when_absent_and_have_no_endpoints() {
        let enrollment = Arc::new(FakeEnrollment::default());
        let starter = ServeStarter {
            enrollment: enrollment.clone(),
            clock: Arc::new(SystemStatusClock),
        };
        let request = serve_request(
            LinkServeCarrierPolicy::RelayOnly,
            Some("https://relay.example"),
        );

        let session = starter
            .start(request)
            .expect("relay only starter should succeed");
        assert!(session.bound_port() > 0);
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
    fn relay_permitted_starter_uses_stored_token_when_present_and_origin_matches() {
        let temp_dir = TempDir::new("relay-permitted-stored");
        let mut request = serve_request(
            LinkServeCarrierPolicy::RelayPermitted,
            Some("https://relay.example"),
        );
        request.bundle_dir = temp_dir.path().to_path_buf();
        request.bundle.relay_access = Some(StoreLoadOutcome::Ready(RelayAccessRecord {
            state: RelayAccessState::Ready,
            relay_origin: Some("https://relay.example".to_string()),
            device_token: Some("stored-token-123".to_string()),
            expires_at: Some(9999999),
            access_generation: 1,
            identity: PairingIdentity {
                cert_sha256: "sha256:cert".to_string(),
                instance_id: "home-instance".to_string(),
                ca_fingerprint: "sha256:ca".to_string(),
            },
        }));

        persist_test_request(&mut request);
        let starter = ServeStarter::default();
        let session = match starter.start(request) {
            Ok(s) => s,
            Err(e) => panic!("starter should succeed, got: {:?}", e),
        };
        assert!(session.bound_port() > 0);
    }

    #[test]
    fn relay_only_starter_fails_when_store_disabled() {
        let temp_dir = TempDir::new("relay-only-disabled");
        let mut request = serve_request(
            LinkServeCarrierPolicy::RelayOnly,
            Some("https://relay.example"),
        );
        request.bundle_dir = temp_dir.path().to_path_buf();
        request.bundle.relay_access = Some(StoreLoadOutcome::Disabled(RelayAccessRecord {
            state: RelayAccessState::Disabled,
            relay_origin: None,
            device_token: None,
            expires_at: None,
            access_generation: 1,
            identity: PairingIdentity {
                cert_sha256: "sha256:cert".to_string(),
                instance_id: "home-instance".to_string(),
                ca_fingerprint: "sha256:ca".to_string(),
            },
        }));

        persist_test_request(&mut request);
        let starter = ServeStarter::default();
        let err = match starter.start(request) {
            Err(e) => e,
            Ok(_) => panic!("expected starter to fail"),
        };
        assert_eq!(
            err.kind,
            LinkServeErrorKind::Transport(LinkServeTransportErrorKind::NotPaired)
        );
    }

    #[test]
    fn relay_only_starter_fails_when_store_origin_mismatch() {
        let temp_dir = TempDir::new("relay-only-mismatch");
        let mut request = serve_request(
            LinkServeCarrierPolicy::RelayOnly,
            Some("https://relay.example"),
        );
        request.bundle_dir = temp_dir.path().to_path_buf();
        request.bundle.relay_access = Some(StoreLoadOutcome::Ready(RelayAccessRecord {
            state: RelayAccessState::Ready,
            relay_origin: Some("https://other-relay.example".to_string()),
            device_token: Some("stored-token-123".to_string()),
            expires_at: Some(9999999),
            access_generation: 1,
            identity: PairingIdentity {
                cert_sha256: "sha256:cert".to_string(),
                instance_id: "home-instance".to_string(),
                ca_fingerprint: "sha256:ca".to_string(),
            },
        }));

        persist_test_request(&mut request);
        let starter = ServeStarter::default();
        let err = match starter.start(request) {
            Err(e) => e,
            Ok(_) => panic!("expected starter to fail"),
        };
        assert_eq!(
            err.kind,
            LinkServeErrorKind::Transport(LinkServeTransportErrorKind::NoEndpoint)
        );
    }

    #[test]
    fn status_tracker_uses_one_shared_update_point_for_times_and_failures() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = Arc::new(StatusTracker::new(clock.clone()));
        tracker.carrier_open_failed(&TransportError::NoEndpoint);
        let failed = tracker.snapshot(bridge_status(true, false));
        assert_eq!(failed.reconnect_count, 1);
        assert!(!failed.journal_version_fresh);
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
        assert!(!connected.journal_version_fresh);
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = PathBuf::from("/var/tmp").join(format!(
                "solstone-link-test-{}-{}-{}",
                name,
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn status_tracker_seeds_cached_version_from_matching_metadata() {
        let temp_dir = TempDir::new("status-seeds-version");
        let metadata = LinkJournalMetadata {
            instance_id: "inst-123".to_string(),
            ca_fp_prefix: "abcd".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            journal_version: "2026.07.26".to_string(),
            journal_name: Some("My Journal".to_string()),
            observed_at: 1234.0,
        };
        let bytes = serde_json::to_vec(&metadata).expect("serialize");
        std::fs::write(temp_dir.path().join("journal_metadata.json"), bytes).expect("write");

        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = StatusTracker::with_metadata(
            clock.clone(),
            temp_dir.path().to_path_buf(),
            "inst-123".to_string(),
            "abcd".to_string(),
            "2026-07-26T00:00:00Z".to_string(),
            None,
            false,
        );
        let snap = tracker.snapshot(bridge_status(true, false));
        assert_eq!(snap.journal_version.as_deref(), Some("2026.07.26"));
        assert!(!snap.journal_version_fresh);
        assert_eq!(snap.instance_id, "inst-123");
        assert_eq!(snap.ca_fp_prefix, "abcd");

        let tracker_mismatch_inst = StatusTracker::with_metadata(
            clock.clone(),
            temp_dir.path().to_path_buf(),
            "other-inst".to_string(),
            "abcd".to_string(),
            "2026-07-26T00:00:00Z".to_string(),
            None,
            false,
        );
        let snap_mismatch_inst = tracker_mismatch_inst.snapshot(bridge_status(true, false));
        assert_eq!(snap_mismatch_inst.journal_version, None);
        assert!(!snap_mismatch_inst.journal_version_fresh);

        let tracker_mismatch_ca = StatusTracker::with_metadata(
            clock.clone(),
            temp_dir.path().to_path_buf(),
            "inst-123".to_string(),
            "mismatch-ca".to_string(),
            "2026-07-26T00:00:00Z".to_string(),
            None,
            false,
        );
        let snap_mismatch_ca = tracker_mismatch_ca.snapshot(bridge_status(true, false));
        assert_eq!(snap_mismatch_ca.journal_version, None);
        assert!(!snap_mismatch_ca.journal_version_fresh);

        let tracker_mismatch_paired_at = StatusTracker::with_metadata(
            clock.clone(),
            temp_dir.path().to_path_buf(),
            "inst-123".to_string(),
            "abcd".to_string(),
            "2026-08-01T00:00:00Z".to_string(),
            None,
            false,
        );
        let snap_mismatch_paired_at =
            tracker_mismatch_paired_at.snapshot(bridge_status(true, false));
        assert_eq!(snap_mismatch_paired_at.journal_version, None);
        assert!(!snap_mismatch_paired_at.journal_version_fresh);

        let invalid_meta = LinkJournalMetadata {
            instance_id: "inst-123".to_string(),
            ca_fp_prefix: "abcd".to_string(),
            paired_at: "2026-07-26T00:00:00Z".to_string(),
            journal_version: "invalid version\nwith\x1b[31m escape".to_string(),
            journal_name: None,
            observed_at: 1234.0,
        };
        std::fs::write(
            temp_dir.path().join("journal_metadata.json"),
            serde_json::to_vec(&invalid_meta).expect("serialize"),
        )
        .expect("write");
        let tracker_invalid = StatusTracker::with_metadata(
            clock,
            temp_dir.path().to_path_buf(),
            "inst-123".to_string(),
            "abcd".to_string(),
            "2026-07-26T00:00:00Z".to_string(),
            None,
            false,
        );
        let snap_invalid = tracker_invalid.snapshot(bridge_status(true, false));
        assert_eq!(snap_invalid.journal_version, None);
        assert!(!snap_invalid.journal_version_fresh);
    }

    #[test]
    fn apply_fetch_result_discards_stale_generation() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = StatusTracker::new(clock);
        {
            let mut state = tracker.inner.lock().expect("lock");
            state.generation = 3;
            state.cached_version = Some("2026.07.01".to_string());
            state.version_fresh = false;
        }

        let outcome = tracker.apply_fetch_result(2, Some("2026.07.26".to_string()), None);
        assert_eq!(outcome, ApplyOutcome::StaleGeneration);

        let snap = tracker.snapshot(bridge_status(true, true));
        assert_eq!(snap.journal_version.as_deref(), Some("2026.07.01"));
        assert!(!snap.journal_version_fresh);
    }

    #[test]
    fn apply_fetch_result_pairing_revision_fence() {
        let temp_dir = TempDir::new("status-pairing-fence");
        fs::write(temp_dir.path().join("cert.pem"), "test-client").unwrap();
        fs::write(
            temp_dir.path().join("chain.pem"),
            "-----BEGIN CERTIFICATE-----\nY2E=\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("peer.json"),
            json!({"instance_id":"inst-123","paired_at":"2026-07-26T00:00:00Z"}).to_string(),
        )
        .unwrap();
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = StatusTracker::with_metadata(
            clock,
            temp_dir.path().to_path_buf(),
            "inst-123".to_string(),
            "abcd".to_string(),
            "2026-07-26T00:00:00Z".to_string(),
            None,
            false,
        );
        {
            let mut state = tracker.inner.lock().expect("lock");
            state.generation = 1;
        }

        std::fs::write(
            temp_dir.path().join("peer.json"),
            json!({ "instance_id":"inst-123", "paired_at": "different-paired-at" }).to_string(),
        )
        .expect("write peer.json");
        let outcome_mismatch = tracker.apply_fetch_result(1, Some("2026.07.26".to_string()), None);
        assert_eq!(outcome_mismatch, ApplyOutcome::PairingMismatch);
        assert!(!temp_dir.path().join("journal_metadata.json").exists());
        let snap_mismatch = tracker.snapshot(bridge_status(true, true));
        assert_eq!(snap_mismatch.journal_version, None);
        assert!(!snap_mismatch.journal_version_fresh);

        std::fs::write(
            temp_dir.path().join("peer.json"),
            json!({ "instance_id":"inst-123", "paired_at": "2026-07-26T00:00:00Z" }).to_string(),
        )
        .expect("write peer.json");
        let outcome_match = tracker.apply_fetch_result(
            1,
            Some("2026.07.26".to_string()),
            Some("Test Journal".to_string()),
        );
        assert_eq!(outcome_match, ApplyOutcome::Persisted);
        assert!(temp_dir.path().join("journal_metadata.json").exists());
        let meta: LinkJournalMetadata = serde_json::from_slice(
            &std::fs::read(temp_dir.path().join("journal_metadata.json")).expect("read"),
        )
        .expect("deserialize");
        assert_eq!(meta.journal_version, "2026.07.26");
        assert_eq!(meta.journal_name.as_deref(), Some("Test Journal"));
        assert_eq!(meta.instance_id, "inst-123");
        assert_eq!(meta.ca_fp_prefix, "abcd");
        assert_eq!(meta.paired_at, "2026-07-26T00:00:00Z");
        let snap_match = tracker.snapshot(bridge_status(true, true));
        assert_eq!(snap_match.journal_version.as_deref(), Some("2026.07.26"));
        assert!(snap_match.journal_version_fresh);
    }

    #[test]
    fn publish_device_description_handles_dead_port_without_panic() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = Arc::new(StatusTracker::new(clock));
        publish_device_description(tracker.clone(), 1, 0);
        let state = tracker.inner.lock().expect("lock");
        assert_eq!(state.cached_version, None);
    }

    #[test]
    fn snapshot_journal_version_freshness_requires_carrier_live() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = StatusTracker::new(clock);
        {
            let mut state = tracker.inner.lock().expect("lock");
            state.cached_version = Some("2026.07.26".to_string());
            state.version_fresh = true;
        }

        let disconnected = tracker.snapshot(bridge_status(true, false));
        assert_eq!(disconnected.journal_version.as_deref(), Some("2026.07.26"));
        assert!(!disconnected.journal_version_fresh);

        let connected = tracker.snapshot(bridge_status(true, true));
        assert_eq!(connected.journal_version.as_deref(), Some("2026.07.26"));
        assert!(connected.journal_version_fresh);
    }

    #[test]
    fn bound_port_race_pending_fetch_dispatches_when_port_set() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = Arc::new(StatusTracker::new(clock));

        tracker.carrier_open_succeeded();
        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.generation, 1);
            assert_eq!(state.fetching_generation, Some(1));
            assert_eq!(state.pending_fetch_generation, Some(1));
            assert_eq!(state.bound_port, None);
        }

        tracker.set_bound_port(5015);
        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.bound_port, Some(5015));
            assert_eq!(state.pending_fetch_generation, None);
        }
    }

    #[test]
    fn set_bound_port_kicks_a_startup_fetch_with_no_prior_carrier_event() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = Arc::new(StatusTracker::new(clock));
        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.generation, 0);
            assert_eq!(state.fetching_generation, None);
        }

        tracker.set_bound_port(5015);
        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.bound_port, Some(5015));
            assert_eq!(
                state.fetching_generation,
                Some(0),
                "set_bound_port must claim a fetch at the current generation even without a prior carrier_open_succeeded"
            );
            assert_eq!(state.pending_fetch_generation, None);
        }

        tracker.set_bound_port(5015);
        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.fetching_generation, Some(0));
        }
    }

    #[tokio::test]
    async fn set_bound_port_dispatches_startup_fetch_with_no_prior_carrier_event() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let handle = tokio::runtime::Handle::current();
        let tracker = Arc::new(StatusTracker::with_metadata(
            clock,
            PathBuf::new(),
            "inst-123".to_string(),
            "abcd".to_string(),
            "2026-07-26T00:00:00Z".to_string(),
            Some(handle),
            false,
        ));

        tracker.set_bound_port(5015);
        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.fetching_generation, Some(0));
        }
    }

    #[tokio::test]
    async fn set_bound_port_dispatches_pending_fetch_from_non_runtime_thread() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let handle = tokio::runtime::Handle::current();
        let tracker = Arc::new(StatusTracker::with_metadata(
            clock,
            PathBuf::new(),
            "inst-123".to_string(),
            "abcd".to_string(),
            "2026-07-26T00:00:00Z".to_string(),
            Some(handle),
            false,
        ));

        tracker.carrier_open_succeeded();
        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.generation, 1);
            assert_eq!(state.fetching_generation, Some(1));
            assert_eq!(state.pending_fetch_generation, Some(1));
            assert_eq!(state.bound_port, None);
        }

        let tracker_clone = tracker.clone();
        let thread_handle = std::thread::spawn(move || {
            assert!(
                tokio::runtime::Handle::try_current().is_err(),
                "test thread must not have an ambient tokio runtime context"
            );
            tracker_clone.set_bound_port(5015);
        });
        thread_handle.join().expect("join thread");

        {
            let state = tracker.inner.lock().expect("lock");
            assert_eq!(state.bound_port, Some(5015));
            assert_eq!(state.pending_fetch_generation, None);
        }
    }

    #[test]
    fn carrier_open_failed_bumps_generation() {
        let clock = Arc::new(FixedStatusClock::new(100.0));
        let tracker = StatusTracker::new(clock);
        {
            let mut state = tracker.inner.lock().expect("lock");
            state.generation = 5;
        }

        tracker.carrier_open_failed(&TransportError::NoEndpoint);
        let state = tracker.inner.lock().expect("lock");
        assert_eq!(state.generation, 6);
        assert_eq!(state.reconnect_count, 1);
        assert!(!state.version_fresh);
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
                "ca_fp_prefix",
                "connected_age_seconds",
                "health",
                "instance_id",
                "journal_version",
                "journal_version_fresh",
                "last_connected_at",
                "last_failure",
                "manager_alive",
                "next_retry_at",
                "paired_at",
                "persist_uncertain",
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
        let client = Arc::new(TransportClient::new(credential, None).expect("test client"));
        SolstoneCarrierOpener {
            client_manager: Arc::new(CurrentClientManager::new(Some(client))),
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

    #[test]
    fn shutdown_fences_metadata_before_waiting_for_client_mutation() {
        let request = serve_request(LinkServeCarrierPolicy::Direct, None);
        let tracker = Arc::new(StatusTracker::new(Arc::new(SystemStatusClock)));
        let manager = Arc::new(CurrentClientManager::new(None));
        let scheduler = Arc::new(OptionalJobScheduler::new_for_test(JobSchedulerTestParams {
            tracker: tracker.clone(),
            client_manager: manager.clone(),
            store: LinkCredentialStore::new(PathBuf::new(), ""),
            identity: pairing_identity_from_bundle(&request.bundle).unwrap(),
            policy: LinkServeCarrierPolicy::Direct,
            configured_relay_origin: None,
            ca_fp_prefix: ca_fp_prefix(&request.bundle).unwrap(),
            bundle: request.bundle,
        }));
        let owner = manager.inner.lock().unwrap();
        let retiring = std::thread::spawn(move || scheduler.retire());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !tracker.retired.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(tracker.retired.load(Ordering::SeqCst));
        assert!(!tracker.operation_is_current(0));
        drop(owner);
        retiring.join().unwrap();
    }

    #[test]
    fn token_persist_hook_of_retired_incarnation_cannot_overwrite_successor() {
        let temp = TempDir::new("token-persist-retired");
        let bundle_dir = temp.path().join("laptop");
        fs::create_dir_all(&bundle_dir).expect("bundle dir");
        fs::write(bundle_dir.join("cert.pem"), "TEST CERT\n").expect("cert");
        fs::write(bundle_dir.join("chain.pem"), "TEST CHAIN\n").expect("chain");
        fs::write(
            bundle_dir.join("peer.json"),
            serde_json::json!({
                "instance_id": "home-1",
                "home_label": "Home",
                "paired_at": "2026-07-26T00:00:00Z"
            })
            .to_string(),
        )
        .expect("peer");

        let store = LinkCredentialStore::new(bundle_dir.clone(), "laptop");
        let identity = PairingIdentity {
            cert_sha256: "sha256:1111".to_string(),
            instance_id: "home-1".to_string(),
            ca_fingerprint: "sha256:2222".to_string(),
        };

        // Manager starts at incarnation 0
        let client_manager = Arc::new(CurrentClientManager::new(None));
        assert_eq!(client_manager.incarnation(), 0);
        let hook = make_token_persist_hook(
            Some("https://link.solstone.app".to_string()),
            &store,
            &identity,
            0,
            Arc::downgrade(&client_manager),
        )
        .expect("hook");

        // Advance incarnation to 1
        client_manager.swap(None);
        assert_eq!(client_manager.incarnation(), 1);

        // Calling hook from incarnation 0 should no-op
        hook("stale_token", 999999);
        assert_eq!(store.load_access(), StoreLoadOutcome::Absent);

        // If manager is dropped entirely, hook should also no-op
        drop(client_manager);
        hook("stale_token_after_drop", 999999);
        assert_eq!(store.load_access(), StoreLoadOutcome::Absent);
    }
}
