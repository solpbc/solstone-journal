// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! # Native Convey shell design
//!
//! ## D1: product shape — shell library and journal-host process
//!
//! This crate owns the HTTP shell routes, their frozen reference contract, and
//! `run_convey`, which binds loopback listeners and drives the accept loop.
//! `solstone-core` only parses arguments and resolves the journal path. Like
//! `solstone-core-convey-http`, this server-shaped crate is excluded from the
//! iOS canary: phones are clients, never hosts.
//!
//! ## D2: session state is distinct from transport admission
//!
//! `solstone-core-convey-http::gate::require_access` admits the closed,
//! accept-time transport identity. `session::classify_session` instead models
//! journal onboarding as unestablished, established, or corrupt. These two
//! gates deliberately share neither type nor implementation.
//!
//! ## D3: named default-deny exemptions
//!
//! The session gate has a small enumerable exemption inventory for root assets
//! and unmatched paths. Every registered route is gated by default; app-local
//! static files are deliberately not top-level-static exemptions.
//!
//! ## D4: fixed native registry inventory
//!
//! The app definitions and Lucide mapping are compiled into this crate as a
//! fixed native inventory. Configuration-driven ordering and stars are
//! intentionally deferred; chat-bar placeholder likewise uses its stable
//! default, not live state.
//!
//! ## D5: generated, dependency-free asset embedding
//!
//! `build.rs` emits auditable `include_bytes!` entries for the full top-level
//! static tree plus selected app workspace assets. This avoids a proc-macro asset crate
//! while retaining byte-identical source assets where appropriate; Thinking and Network use
//! intentionally shrunken crate-local copies.
//!
//! ## D6: every registry app is a native workspace
//!
//! Each app in the registry serves its own native routes. A bare `/app/{app}`
//! path for a registry app permanently redirects to `/app/{app}/`; any other
//! `/app/...` path no app route claims is the HTML 404 fallback.
//!
//! ## D7: host transport has one indispensable listener and one capability
//!
//! `serve` returns an error only when loopback cannot bind: local Convey is the
//! process's reason to exist. The paired-device door is independently useful
//! but optional, so it is always recorded as a [`DoorOutcome`] in the handle.
//! The `host` feature is defence in depth: `check-rust-ios` compiles this
//! crate for iOS, which is not a promise that an iOS app hosts Convey.
//!
//! ## D8: paired-device admission is connection-scoped and durable
//!
//! Each carrier receives a fresh verifier and identity cell, leaving room for
//! a second refusal-classification field. The door uses explicit mux limits
//! (documented beside their configuration): they constrain one carrier only,
//! never the uncapped population of carriers. Handshake admission fails closed;
//! after admission, only a definite `Present` removal closes a carrier, so a
//! transient unreadable ledger cannot discard otherwise unrecoverable material.
//! Server leaves are fresh with a 30-day residual lifetime; the pinned mTLS
//! client validates the CA fingerprint but not certificate validity.
//!
//! ## D9: Body store health and canonical entry paths
//!
//! The native Body store-health reader classifies a positive import claim
//! paired with an absent, unreadable, or empty aggregate as `Torn`; the
//! Python reference instead treats a missing aggregate as empty data and
//! answers 2xx. This diagnostic divergence is intentional during the
//! migration and is not owner-visible yet: the client is unchanged and the
//! five stub data routes still answer with a refusal.
//!
//! `/app/body` without its trailing slash reaches the generic app
//! catch-all, which permanently redirects to `/app/body/`. Every registry
//! app uses that same bare-path redirect; the slashed form is the native
//! shell entry point.

#[cfg(feature = "host")]
use std::path::Path as FsPath;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "host")]
use std::io;
#[cfg(feature = "host")]
use std::net::SocketAddr;
#[cfg(feature = "host")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(feature = "host")]
use std::time::Duration;

use axum::Extension;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Offset, Utc};
use serde_json::Value;
use solstone_core_journal_config::{owner_zone, zone_label};
#[cfg(feature = "host")]
use solstone_core_sol_link::DeviceDoorAuthorization;
#[cfg(feature = "host")]
use solstone_core_thinking::confidential::OperationRegistry;

#[cfg(feature = "host")]
mod agents_enable;
mod assets;
#[cfg(feature = "host")]
pub mod authorization_gate;
mod body;
mod clients;
mod device_migration;
#[cfg(test)]
mod devices_source_retired;
#[cfg(feature = "host")]
mod door;
mod entities;
mod form_body_guard;
#[cfg(feature = "host")]
mod link_health_cache;
#[cfg(feature = "host")]
mod local_network;
#[cfg(feature = "host")]
mod loopback_guard;
#[cfg(feature = "host")]
mod network;
#[cfg(feature = "host")]
mod network_status;
#[cfg(feature = "host")]
mod network_writes;
#[cfg(feature = "host")]
mod pair_window_manager;
#[cfg(feature = "host")]
mod paired_device;
pub mod registry;
mod relay_access;
mod relay_admission;
#[cfg(feature = "host")]
pub mod session;
pub mod session_gate;
mod setup_host;
pub mod source_link;
mod speakers;
mod speakers_attribution;
mod speakers_calendar;
mod speakers_cli_discovery;
mod speakers_cli_entities;
mod speakers_cli_maintenance;
mod speakers_cli_owner;
mod speakers_cli_reads;
mod speakers_discovery;
mod speakers_discovery_write;
mod speakers_dismissals;
mod speakers_known;
mod speakers_media;
mod speakers_npz;
mod speakers_owner;
#[cfg(feature = "test-hooks")]
pub mod speakers_owner_write;
#[cfg(not(feature = "test-hooks"))]
mod speakers_owner_write;
mod speakers_quality;
mod speakers_review;
mod speakers_source;
mod sse;
#[cfg(test)]
mod status_mark;
mod system;
#[cfg(feature = "host")]
mod thinking;
#[cfg(all(test, feature = "full-tests"))]
mod thinking_chatgpt_route_tests;
mod thinking_install;
#[cfg(feature = "host")]
mod thinking_sol_reads;
#[cfg(all(test, feature = "host"))]
mod thinking_sol_reads_contract;
#[cfg(feature = "host")]
mod weekly_heads_up;
#[cfg(feature = "host")]
mod windows_firewall;
#[cfg(feature = "host")]
pub use agents_enable::{SmeOperationsOverride, SmePoll, SmePollOutcome, SmeRuntimeOverride};
#[cfg(feature = "host")]
pub use network_writes::{
    HostLabelOverride, NetworkOperationsOverride, SplDisableFailureOverride, SplEnrollment,
    SplPoll, SplPollOutcome, SplRuntimeOverride,
};
#[cfg(feature = "host")]
pub use thinking::{
    AccessClientHandle, AccessSnapshot, ConfidentialAccess, ConfidentialPoll,
    ConfidentialRuntimeOverride, EndpointImageCheckOverride, PollOutcome,
};

use assets::lookup;
use registry::{ShellPayload, served_app, shell_payload, shell_payload_with_agents};

/// Journal filesystem root shared with converted app route handlers.
#[derive(Clone)]
pub(crate) struct JournalRoot(pub PathBuf);

/// Optional hosted-generation provenance carried to handlers that launch a
/// service-like helper. Unhosted Convey keeps the existing ordinary path.
#[derive(Clone)]
pub(crate) struct HostedLaunchContext(
    pub(crate) Option<Arc<solstone_core_system::lifecycle::HostedServiceParentRuntime>>,
);

#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct DiscoveryGenerationContext(pub solstone_core_system::process::ChildLaunchContext);

/// Reason the paired-device door was not made available at startup.
#[cfg(feature = "host")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoorWithheldReason {
    Unestablished,
    Corrupt,
    CommittedIdentityUnavailable,
}

/// Observable paired-device door startup outcome.
#[cfg(feature = "host")]
#[derive(Debug)]
pub enum DoorOutcome {
    Bound(SocketAddr),
    BindFailed { port: u16, source: io::Error },
    Withheld(DoorWithheldReason),
}

/// Inputs to the host listener lifecycle. The router is deliberately prebuilt.
#[cfg(feature = "host")]
pub struct ConveyServeOptions {
    pub journal_root: PathBuf,
    pub loopback_port: u16,
    pub door_port: u16,
    pub handshake_timeout: Duration,
    pub stream_stall_timeout: Duration,
    pub router: Router,
    pub carrier_loop_iterations: Arc<AtomicU64>,
    pub handshake_authorization_read_ticks: Arc<AtomicU64>,
}

/// Live host listener set. Call [`Self::shutdown`] in test and embedded lifecycles.
#[cfg(feature = "host")]
pub struct ConveyServeHandle {
    loopback_ipv4: SocketAddr,
    loopback_ipv6: SocketAddr,
    door_outcome: DoorOutcome,
    door: Arc<door::DoorLifecycle>,
    loopback_task: tokio::task::JoinHandle<()>,
    link_health_task: tokio::task::JoinHandle<()>,
    weekly_heads_up_task: tokio::task::JoinHandle<()>,
    local_network_task: tokio::task::JoinHandle<()>,
    service_token_renewal_task: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "host")]
impl ConveyServeHandle {
    pub fn loopback_ipv4_addr(&self) -> SocketAddr {
        self.loopback_ipv4
    }
    pub fn loopback_ipv6_addr(&self) -> SocketAddr {
        self.loopback_ipv6
    }
    pub fn door_outcome(&self) -> &DoorOutcome {
        &self.door_outcome
    }
    /// Latest door bind, including a door that opened after first-run finalize.
    pub fn live_door_addr(&self) -> Option<SocketAddr> {
        self.door.bound_addr()
    }
    pub fn shutdown(&self) {
        self.loopback_task.abort();
        self.door.shutdown();
        self.link_health_task.abort();
        self.weekly_heads_up_task.abort();
        self.local_network_task.abort();
        self.service_token_renewal_task.abort();
    }
    pub async fn stop_authorization_refresh(&mut self) {
        self.door.stop_authorization_refresh().await;
    }
    /// Testable lifecycle control for proving request-level confinement does
    /// not depend on the background pairing reaper.
    pub async fn stop_pairing_reaper(&mut self) {
        self.door.stop_pairing_reaper().await;
    }
    /// Test-visible equivalent of the cap-refusal log line.
    pub fn pairing_cap_refusals(&self) -> u64 {
        self.door.pairing_cap_refusals()
    }
    pub async fn await_forever(self) -> ! {
        std::future::pending().await
    }
}

/// Failure to establish the indispensable loopback listener.
#[cfg(feature = "host")]
#[derive(Debug)]
pub enum ConveyServeError {
    LoopbackBind { port: u16, source: io::Error },
    PairingWindowCleanup(solstone_core_sol_link::pairing::nonces::NonceStoreError),
    EmbeddedRoot(solstone_core_journal_io::JournalRootError),
    EmbeddedPayload(String),
}

#[cfg(feature = "host")]
impl std::fmt::Display for ConveyServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoopbackBind { port, source } => write!(
                formatter,
                "could not bind loopback port {port}: {source}. convey may already be running, including under another login. the default port is shared across logins"
            ),
            Self::EmbeddedPayload(error) => write!(formatter, "embedded payload: {error}"),
            Self::EmbeddedRoot(error) => write!(formatter, "embedded journal root: {error}"),
            Self::PairingWindowCleanup(error) => {
                write!(
                    formatter,
                    "could not retire stale relay pairing windows: {error}"
                )
            }
        }
    }
}

#[cfg(feature = "host")]
impl std::error::Error for ConveyServeError {}

/// App-owned serving lifecycle. There is no unauthenticated loopback listener:
/// local owner requests must arrive through the embedding app's private IPC.
#[cfg(feature = "host")]
pub struct EmbeddedConveyHandle {
    owner_router: Router,
    door: Arc<door::DoorLifecycle>,
    local_network_task: tokio::task::JoinHandle<()>,
    link_health_task: tokio::task::JoinHandle<()>,
    service_token_renewal_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    renewal_shutdown: tokio::sync::watch::Sender<bool>,
    backup_deps: solstone_core_backup_web::BackupWebDeps,
}

/// Clone-only stop signal; task and runtime ownership remains in the serving handle.
#[cfg(feature = "host")]
#[derive(Clone)]
pub struct EmbeddedConveyStop {
    door: Arc<door::DoorLifecycle>,
    local_network: Option<tokio::task::AbortHandle>,
    link_health: Option<tokio::task::AbortHandle>,
    renewal: tokio::sync::watch::Sender<bool>,
}

/// Publish cancellation before exposing the embedded listener. The callback
/// receives signals only; the constructor retains the actual serving owner.
#[cfg(feature = "host")]
pub struct EmbeddedStartup {
    pub cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    pub stop_ready: Arc<
        dyn Fn(EmbeddedConveyStop, solstone_core_backup_web::BackupRouteLifetime) + Send + Sync,
    >,
}

#[cfg(feature = "host")]
impl EmbeddedConveyStop {
    /// Close ingress and request support-task cancellation without joining.
    pub fn request_stop(&self) {
        if let Some(task) = &self.local_network {
            task.abort();
        }
        if let Some(task) = &self.link_health {
            task.abort();
        }
        self.renewal.send_replace(true);
        self.door.shutdown();
    }
}

#[cfg(feature = "host")]
impl EmbeddedConveyHandle {
    pub fn stop_handle(&self) -> EmbeddedConveyStop {
        EmbeddedConveyStop {
            door: Arc::clone(&self.door),
            local_network: Some(self.local_network_task.abort_handle()),
            link_health: Some(self.link_health_task.abort_handle()),
            renewal: self.renewal_shutdown.clone(),
        }
    }

    /// Share the exact operation ownership used by the embedded Home and backup routes.
    pub fn backup_dependencies(&self) -> solstone_core_backup_web::BackupWebDeps {
        self.backup_deps.clone()
    }

    /// Execute a request already admitted by the embedding app's owner boundary.
    /// Never expose this method through a network listener or public IPC endpoint.
    pub async fn request_owner(
        &self,
        mut request: axum::http::Request<axum::body::Body>,
    ) -> axum::response::Response {
        use solstone_core_convey_http::identity::AccessBasis;
        use tower::ServiceExt;
        request.extensions_mut().insert(AccessBasis::Localhost);
        self.owner_router.clone().oneshot(request).await.unwrap()
    }

    pub fn live_door_addr(&self) -> Option<SocketAddr> {
        self.door.bound_addr()
    }

    pub fn shutdown(&self) {
        self.stop_handle().request_stop();
    }

    /// Stop app-owned services and join any active token renewal before the
    /// embedding app releases its journal or runtime.
    pub async fn shutdown_and_wait(&self) {
        self.shutdown();
        join_embedded_renewal(&self.service_token_renewal_task).await;
    }
}

#[cfg(feature = "host")]
async fn join_embedded_renewal(slot: &tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>) {
    // Retain ownership across a cancelled waiter, and make concurrent waiters
    // observe completion rather than an empty slot while work is still active.
    let mut task = slot.lock().await;
    if let Some(task) = task.as_mut() {
        let _ = task.await;
    }
    task.take();
}

#[cfg(feature = "host")]
impl Drop for EmbeddedConveyHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Serve the existing authenticated device protocol inside an app process.
/// Setup, pairing, authorization, revocation and route semantics are shared
/// with desktop Convey; only the local owner transport is supplied by the app.
#[cfg(feature = "host")]
pub async fn bind_embedded_device_door(
    journal_root: PathBuf,
    address: std::net::SocketAddrV4,
) -> Result<EmbeddedConveyHandle, ConveyServeError> {
    bind_embedded_with_dependencies(journal_root, address, None, None, None).await
}

#[cfg(feature = "host")]
pub async fn bind_embedded_device_door_with_payload(
    journal_root: PathBuf,
    address: std::net::SocketAddrV4,
    payload_root: PathBuf,
) -> Result<EmbeddedConveyHandle, ConveyServeError> {
    bind_embedded_with_dependencies(journal_root, address, Some(payload_root), None, None).await
}

/// Compose app-owned routes using the same optional agent owner surface as the
/// desktop host. The device authorization gate still admits every network call.
#[cfg(feature = "host")]
pub async fn bind_embedded_device_door_with_routes(
    journal_root: PathBuf,
    address: std::net::SocketAddrV4,
    payload_root: Option<PathBuf>,
    agents_routes: Router,
) -> Result<EmbeddedConveyHandle, ConveyServeError> {
    bind_embedded_with_dependencies(
        journal_root,
        address,
        payload_root,
        Some(agents_routes),
        None,
    )
    .await
}

#[cfg(feature = "host")]
pub async fn bind_embedded_device_door_with_routes_and_startup(
    journal_root: PathBuf,
    address: std::net::SocketAddrV4,
    payload_root: Option<PathBuf>,
    agents_routes: Router,
    startup: EmbeddedStartup,
) -> Result<EmbeddedConveyHandle, ConveyServeError> {
    bind_embedded_with_dependencies(
        journal_root,
        address,
        payload_root,
        Some(agents_routes),
        Some(startup),
    )
    .await
}

/// Compose iOS backup routes and the door using the app's actual retained root.
#[cfg(all(feature = "host", target_os = "ios"))]
pub async fn bind_embedded_device_door_with_root(
    root: Arc<solstone_core_journal_io::JournalRoot>,
    address: std::net::SocketAddrV4,
    payload_root: Option<PathBuf>,
    agents_routes: Router,
    startup: EmbeddedStartup,
) -> Result<EmbeddedConveyHandle, ConveyServeError> {
    root.revalidate_canonical_binding()
        .map_err(ConveyServeError::EmbeddedRoot)?;
    let journal_root = root.canonical_path().to_path_buf();
    let roots = payload_root
        .map(|payload| thinking_sol_reads::TalentRoots::from_payload_root(&payload))
        .transpose()
        .map_err(ConveyServeError::EmbeddedPayload)?;
    let backup_deps = solstone_core_backup_web::BackupWebDeps::with_ios_root(root.clone())
        .map_err(ConveyServeError::EmbeddedRoot)?;
    let app = router_with_roots(
        journal_root.clone(),
        None,
        Some(agents_routes),
        roots,
        Some(backup_deps.clone()),
    );
    root.revalidate_canonical_binding()
        .map_err(ConveyServeError::EmbeddedRoot)?;
    bind_embedded_router(
        journal_root,
        address,
        app,
        backup_deps,
        Some(root),
        Some(startup),
    )
    .await
}

#[cfg(feature = "host")]
async fn bind_embedded_with_dependencies(
    journal_root: PathBuf,
    address: std::net::SocketAddrV4,
    payload_root: Option<PathBuf>,
    agents_routes: Option<Router>,
    startup: Option<EmbeddedStartup>,
) -> Result<EmbeddedConveyHandle, ConveyServeError> {
    let roots = payload_root
        .map(|payload| thinking_sol_reads::TalentRoots::from_payload_root(&payload))
        .transpose()
        .map_err(ConveyServeError::EmbeddedPayload)?;
    let backup_deps = solstone_core_backup_web::BackupWebDeps::new(journal_root.clone());
    let app = router_with_roots(
        journal_root.clone(),
        None,
        agents_routes,
        roots,
        Some(backup_deps.clone()),
    );
    bind_embedded_router(journal_root, address, app, backup_deps, None, startup).await
}

#[cfg(feature = "host")]
async fn bind_embedded_router(
    journal_root: PathBuf,
    address: std::net::SocketAddrV4,
    app: Router,
    backup_deps: solstone_core_backup_web::BackupWebDeps,
    _retained_root: Option<Arc<solstone_core_journal_io::JournalRoot>>,
    startup: Option<EmbeddedStartup>,
) -> Result<EmbeddedConveyHandle, ConveyServeError> {
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use tokio::sync::watch;

    let relay_admissions = relay_admission::admission_registry_for(&journal_root);
    relay_admissions.clear();
    pair_window_manager::cleanup_relay_windows_on_startup(
        &journal_root,
        pair_window_manager::unix_seconds(),
    )
    .map_err(ConveyServeError::PairingWindowCleanup)?;
    let (sender, receiver) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    #[cfg(not(target_os = "ios"))]
    let binding = Arc::new(
        solstone_core_journal_io::JournalRoot::open(&journal_root)
            .map_err(ConveyServeError::EmbeddedRoot)?,
    );
    #[cfg(target_os = "ios")]
    let binding = match _retained_root {
        Some(binding) => {
            binding
                .revalidate_canonical_binding()
                .map_err(ConveyServeError::EmbeddedRoot)?;
            binding
        }
        None => Arc::new(
            solstone_core_journal_io::JournalRoot::open(&journal_root)
                .map_err(ConveyServeError::EmbeddedRoot)?,
        ),
    };
    let cache: link_health_cache::RelayHealthCacheStore = Arc::new(std::sync::Mutex::new(None));
    let (renewal_shutdown, renewal_stop) = tokio::sync::watch::channel(false);
    let cancelled = startup
        .as_ref()
        .map(|startup| Arc::clone(&startup.cancelled))
        .unwrap_or_else(|| Arc::new(|| false));
    let admission = EmbeddedAdmission {
        stopped: renewal_stop.clone(),
        cancelled,
    };
    let router = app
        .layer(Extension(cache.clone()))
        .layer(axum::middleware::from_fn_with_state(
            admission.clone(),
            embedded_admission_gate,
        ));
    let door_router = authorization_gate::authorized_router_with_router(
        router.clone(),
        journal_root.clone(),
        receiver,
    );
    let service_root = journal_root.clone();
    let door = Arc::new(door::DoorLifecycle::new(door::DoorStartOptions {
        journal_root,
        port: address.port(),
        network_ipv4: Some(*address.ip()),
        handshake_timeout: Duration::from_secs(10),
        stream_stall_timeout: Duration::from_secs(60),
        router: door_router
            .into_inner()
            .layer(axum::middleware::from_fn_with_state(
                binding.clone(),
                embedded_root_gate,
            )),
        carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
        handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
        authorization_sender: sender,
        relay_admissions,
    }));
    let owner_router = router
        .merge(local_network::routes(door.clone()))
        .layer(axum::middleware::from_fn_with_state(
            door.clone(),
            door::open_after_finalize,
        ))
        .layer(axum::middleware::from_fn_with_state(
            binding,
            embedded_root_gate,
        ))
        .layer(axum::middleware::from_fn_with_state(
            admission,
            embedded_admission_gate,
        ));
    if let Some(startup) = &startup {
        (startup.stop_ready)(
            EmbeddedConveyStop {
                door: Arc::clone(&door),
                local_network: None,
                link_health: None,
                renewal: renewal_shutdown.clone(),
            },
            backup_deps.lifetime(),
        );
    }
    let _ = door.ensure_started().await;
    let local_network_task = tokio::spawn(local_network::watch(door.clone()));
    let (link_health_task, service_token_renewal_task) =
        start_relay_health_tasks(service_root, cache, Some(renewal_stop));
    let handle = EmbeddedConveyHandle {
        owner_router,
        door,
        local_network_task,
        link_health_task,
        service_token_renewal_task: tokio::sync::Mutex::new(Some(service_token_renewal_task)),
        renewal_shutdown,
        backup_deps,
    };
    if *handle.renewal_shutdown.borrow()
        || startup
            .as_ref()
            .is_some_and(|startup| (startup.cancelled)())
    {
        handle.shutdown();
    }
    if let Some(startup) = startup {
        (startup.stop_ready)(handle.stop_handle(), handle.backup_deps.lifetime());
    }
    Ok(handle)
}

#[cfg(feature = "host")]
#[derive(Clone)]
struct EmbeddedAdmission {
    stopped: tokio::sync::watch::Receiver<bool>,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
}

#[cfg(feature = "host")]
async fn embedded_admission_gate(
    axum::extract::State(admission): axum::extract::State<EmbeddedAdmission>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if *admission.stopped.borrow() || (admission.cancelled)() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error":"server_stopping"})),
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(feature = "host")]
async fn embedded_root_gate(
    axum::extract::State(root): axum::extract::State<Arc<solstone_core_journal_io::JournalRoot>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if root.revalidate_canonical_binding().is_err() {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error":"journal_root_changed"})),
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(feature = "host")]
fn start_relay_health_tasks(
    journal_root: PathBuf,
    cache: link_health_cache::RelayHealthCacheStore,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>) {
    let link_health_task = tokio::spawn(link_health_cache::subscribe_relay_health(
        journal_root.clone(),
        cache.clone(),
    ));
    let renewal_health_cache = cache.clone();
    let renewal_config = solstone_core_spl::RenewalConfig::production(
        journal_root.clone(),
        Arc::new(move || {
            let cache = renewal_health_cache
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
                .unwrap_or(0);
            link_health_cache::service_token_health_from_cache(cache.as_ref(), now_ms)
        }),
    );
    let service_token_renewal_task = match shutdown {
        Some(shutdown) => tokio::spawn(solstone_core_spl::run_embedded_service_token_renewal(
            renewal_config,
            shutdown,
        )),
        None => solstone_core_spl::spawn_service_token_renewal(renewal_config),
    };
    (link_health_task, service_token_renewal_task)
}

/// Bind loopback and, when available, the paired-device door.
///
/// The loopback port is machine-wide and shared across logins. A second copy,
/// including one started under another login, must fail this bind rather than
/// isolate per user.
#[cfg(feature = "host")]
pub async fn serve(options: ConveyServeOptions) -> Result<ConveyServeHandle, ConveyServeError> {
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use tokio::sync::watch;

    let (authorization_sender, _) = watch::channel(DeviceDoorAuthorization::from(
        AuthorizedClientsRead::Missing,
    ));
    let door_router = authorization_gate::DoorRouter::unconfined(options.router.clone());
    bind_with_authorization(options, door_router, authorization_sender).await
}

#[cfg(feature = "host")]
pub async fn bind_with_authorization(
    options: ConveyServeOptions,
    door_router: authorization_gate::DoorRouter,
    authorization_sender: tokio::sync::watch::Sender<DeviceDoorAuthorization>,
) -> Result<ConveyServeHandle, ConveyServeError> {
    use solstone_core_convey_http::listener::bind_loopback;

    let relay_admissions = relay_admission::admission_registry_for(&options.journal_root);
    relay_admissions.clear();
    pair_window_manager::cleanup_relay_windows_on_startup(
        &options.journal_root,
        pair_window_manager::unix_seconds(),
    )
    .map_err(ConveyServeError::PairingWindowCleanup)?;

    let listeners = bind_loopback(options.loopback_port)
        .await
        .map_err(|source| ConveyServeError::LoopbackBind {
            port: options.loopback_port,
            source,
        })?;
    let loopback_ipv4 = listeners
        .ipv4_addr()
        .map_err(|source| ConveyServeError::LoopbackBind {
            port: options.loopback_port,
            source,
        })?;
    let loopback_ipv6 = listeners
        .ipv6_addr()
        .map_err(|source| ConveyServeError::LoopbackBind {
            port: options.loopback_port,
            source,
        })?;
    let link_health_cache = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (link_health_task, service_token_renewal_task) = start_relay_health_tasks(
        options.journal_root.clone(),
        link_health_cache.clone(),
        None,
    );
    let push_portal = std::env::var("SERVICES_PORTAL_URL")
        .unwrap_or_else(|_| agents_enable::DEFAULT_PORTAL_URL.to_owned())
        .trim_end_matches('/')
        .to_owned();
    let weekly_heads_up_task = tokio::spawn(weekly_heads_up::subscribe_weekly_heads_up(
        options.journal_root.clone(),
        push_portal,
    ));
    let door = Arc::new(door::DoorLifecycle::new(door::DoorStartOptions {
        journal_root: options.journal_root,
        port: options.door_port,
        network_ipv4: None,
        handshake_timeout: options.handshake_timeout,
        stream_stall_timeout: options.stream_stall_timeout,
        router: door_router
            .into_inner()
            .layer(Extension(link_health_cache.clone())),
        carrier_loop_iterations: options.carrier_loop_iterations,
        handshake_authorization_read_ticks: options.handshake_authorization_read_ticks,
        authorization_sender,
        relay_admissions,
    }));
    let loopback_router = options
        .router
        .clone()
        .merge(local_network::routes(door.clone()))
        .layer(Extension(link_health_cache))
        .layer(axum::middleware::from_fn_with_state(
            door.clone(),
            door::open_after_finalize,
        ));
    let loopback_task =
        tokio::spawn(async move { serve_loopback(listeners, loopback_router).await });
    let _ = door.ensure_started().await;
    let local_network_task = tokio::spawn(local_network::watch(door.clone()));
    let door_outcome = door.clone_outcome().expect("door start records an outcome");
    Ok(ConveyServeHandle {
        loopback_ipv4,
        loopback_ipv6,
        door_outcome,
        door,
        loopback_task,
        link_health_task,
        weekly_heads_up_task,
        local_network_task,
        service_token_renewal_task,
    })
}

#[cfg(feature = "host")]
async fn serve_loopback(
    listeners: solstone_core_convey_http::listener::LoopbackListeners,
    router: Router,
) {
    use solstone_core_convey_http::identity::AccessBasis;
    use solstone_core_convey_http::serve::{serve_connection, tcp_builder};
    // Every connection here is `Localhost`, the owner. The guard is layered on
    // this listener alone; the door serves the shared routes without it.
    let router = loopback_guard::apply_layer(router);
    loop {
        let Ok((stream, _)) = listeners.accept().await else {
            continue;
        };
        let router = router.clone();
        tokio::spawn(async move {
            let builder = tcp_builder();
            if let Err(error) =
                serve_connection(stream, router, AccessBasis::Localhost, &builder).await
            {
                log::debug!("convey loopback connection failed: {error}");
            }
        });
    }
}

/// Run the production Convey server until its process is terminated; port zero is unsupported.
#[cfg(feature = "host")]
pub fn run_convey(
    journal_root: PathBuf,
    port: u16,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<(), String> {
    run_convey_with_hosted_parent(
        journal_root,
        port,
        None,
        #[cfg(windows)]
        generation,
    )
}

/// Run Convey with an optional birth-admitted hosted parent lifetime.
#[cfg(feature = "host")]
pub fn run_convey_with_hosted_parent(
    journal_root: PathBuf,
    port: u16,
    hosted_parent: Option<Arc<solstone_core_system::lifecycle::HostedServiceParentRuntime>>,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<(), String> {
    run_convey_with_hosted_parent_and_routes(
        journal_root,
        port,
        hosted_parent,
        None,
        #[cfg(windows)]
        generation,
    )
}

/// Run Convey with an optional, compile-time-gated local owner surface.
#[cfg(feature = "host")]
pub fn run_convey_with_hosted_parent_and_routes(
    journal_root: PathBuf,
    port: u16,
    hosted_parent: Option<Arc<solstone_core_system::lifecycle::HostedServiceParentRuntime>>,
    agents_routes: Option<Router>,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("convey could not inspect current executable: {error}"))?;
    let executable_dir = executable
        .parent()
        .ok_or_else(|| format!("convey executable has no parent: {}", executable.display()))?;
    let _roots = crate::thinking_sol_reads::TalentRoots::from_executable_dir(executable_dir)?;
    run_convey_bound(
        journal_root,
        port,
        hosted_parent,
        agents_routes,
        #[cfg(windows)]
        generation,
    )
}

/// Like [`run_convey`], but resolves packaged talent roots from `executable_dir`.
#[cfg(feature = "host")]
pub fn run_convey_from_executable_dir(
    journal_root: PathBuf,
    port: u16,
    executable_dir: &std::path::Path,
    #[cfg(windows)] generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<(), String> {
    let _roots = crate::thinking_sol_reads::TalentRoots::from_executable_dir(executable_dir)?;
    run_convey_bound(
        journal_root,
        port,
        None,
        None,
        #[cfg(windows)]
        generation,
    )
}

#[cfg(feature = "host")]
fn run_convey_bound(
    journal_root: PathBuf,
    port: u16,
    hosted_parent: Option<Arc<solstone_core_system::lifecycle::HostedServiceParentRuntime>>,
    agents_routes: Option<Router>,
    #[cfg(windows)] discovery_generation: &solstone_core_system::process::ChildLaunchContext,
) -> Result<(), String> {
    use solstone_core_journal_config::read_direct_door_port;
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use solstone_core_system::direct_door::{
        DirectDoorOutcome, DirectDoorPublishResult, peek_direct_door_generation,
        publish_direct_door, withhold_direct_door,
    };
    use solstone_core_system::lifecycle::{HostedServiceShutdownEvidence, ParentLossReason};
    use tokio::sync::watch;

    if port == 0 {
        return Err("convey --port 0 is not supported; choose a concrete loopback port".to_owned());
    }
    let direct_port = read_direct_door_port(&journal_root).map_err(|error| error.to_string())?;
    let generation = peek_direct_door_generation(&journal_root)
        .map_err(|error| format!("convey: failed to read direct-door generation: {error}"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("convey could not create its Tokio runtime: {error}"))?;
    let (authorization_sender, authorization_receiver) = watch::channel(
        DeviceDoorAuthorization::from(AuthorizedClientsRead::Missing),
    );
    let loopback_router =
        router_with_hosted_parent(journal_root.clone(), hosted_parent.clone(), agents_routes);
    // Location search documents follow the segments they come from; see
    // `solstone-core-location::projection`.
    solstone_core_map_web::start_maintenance(journal_root.clone());
    #[cfg(windows)]
    let loopback_router = loopback_router.layer(Extension(DiscoveryGenerationContext(
        discovery_generation.clone(),
    )));
    #[cfg(windows)]
    let loopback_router = loopback_router.layer(Extension(
        solstone_core_transcripts_web::SenseLaunchContext {
            generation: discovery_generation.clone(),
            hosted_parent: hosted_parent.clone(),
        },
    ));
    let door_router = authorization_gate::authorized_router_with_router(
        loopback_router.clone(),
        journal_root.clone(),
        authorization_receiver,
    );
    let handle = runtime
        .block_on(bind_with_authorization(
            ConveyServeOptions {
                journal_root: journal_root.clone(),
                loopback_port: port,
                door_port: direct_port,
                // spl_transport::connection::HANDSHAKE_TIMEOUT is the shipped symmetric 10 s budget.
                handshake_timeout: Duration::from_secs(10),
                stream_stall_timeout: Duration::from_secs(60),
                router: loopback_router,
                carrier_loop_iterations: Arc::new(AtomicU64::new(0)),
                handshake_authorization_read_ticks: Arc::new(AtomicU64::new(0)),
            },
            door_router,
            authorization_sender,
        ))
        .map_err(|error| error.to_string())?;
    // The door outcome is the ONLY signal an operator gets that linked devices can
    // reach this journal, and until now `serve` computed it and `run_convey` dropped
    // it on the floor -- so a door that refused was indistinguishable from one that
    // never existed. That is the exact failure this door was built to end, and the
    // library-level tests could not see it because they read `door_outcome()` directly.
    match handle.door_outcome() {
        DoorOutcome::Bound(address) => {
            eprintln!("convey: paired-device door listening on {address}");
        }
        DoorOutcome::BindFailed { port, source } => {
            eprintln!(
                "convey: paired-device door could NOT bind port {port}: {source} -- linked devices cannot reach this journal"
            );
        }
        DoorOutcome::Withheld(reason) => {
            let detail = match reason {
                DoorWithheldReason::Unestablished => "solstone journal setup is not complete",
                DoorWithheldReason::Corrupt => "journal config is corrupt",
                DoorWithheldReason::CommittedIdentityUnavailable => {
                    "the committed link identity under journal/link could not be loaded"
                }
            };
            eprintln!(
                "convey: paired-device door withheld: {detail} -- linked devices cannot reach this journal"
            );
        }
    }
    let publish_outcome = match handle.door_outcome() {
        DoorOutcome::Bound(address) => DirectDoorOutcome::Bound {
            port: address.port(),
        },
        DoorOutcome::BindFailed { port, .. } => DirectDoorOutcome::BindFailed { port: *port },
        DoorOutcome::Withheld(_) => DirectDoorOutcome::Withheld { port: direct_port },
    };
    match publish_direct_door(&journal_root, generation, publish_outcome) {
        Ok(DirectDoorPublishResult::Published) => {}
        Ok(DirectDoorPublishResult::RejectedStale) => {
            return Err("convey: direct-door publish rejected as stale".to_owned());
        }
        Err(error) => {
            return Err(format!(
                "convey: failed to publish direct-door record: {error}"
            ));
        }
    }
    write_port_file(&journal_root, handle.loopback_ipv4_addr().port())?;
    if let Some(parent) = hosted_parent {
        runtime.block_on(async {
            tokio::select! {
                reason = parent.await_parent_loss() => reason,
                _ = wait_for_retire_expected_request(&parent) => ParentLossReason::ExitedOrReused,
                _ = wait_for_hosted_termination_signal() => ParentLossReason::ExitedOrReused,
            }
        });
        // Stop archive children (restic) before the process goes, rather than
        // leave them running after it; a few seconds bounds the wait.
        let _ = solstone_core_backup_web::stop_archive_work(Duration::from_secs(5));
        // `ConveyServeHandle::shutdown` synchronously aborts every listener
        // task and has no fallible result; Convey has no separate runner once
        // those tasks have been aborted.
        handle.shutdown();
        let artifact_cleanup = withhold_direct_door(&journal_root, generation, direct_port)
            .map_err(|error| format!("convey: failed to withhold direct-door record: {error}"))
            .and_then(|result| match result {
                DirectDoorPublishResult::Published => remove_port_file(&journal_root),
                DirectDoorPublishResult::RejectedStale => {
                    Err("convey: direct-door withdrawal rejected as stale".to_owned())
                }
            });
        parent
            .finish_parent_loss(HostedServiceShutdownEvidence {
                listener_stopped: true,
                service_runner_stopped: true,
                operational_artifacts_cleaned: artifact_cleanup.is_ok(),
            })
            .map_err(|error| format!("convey: parent-loss coordination failed: {error}"))?;
        artifact_cleanup?;
        return Ok(());
    }
    runtime.block_on(handle.await_forever())
}

#[cfg(feature = "host")]
async fn wait_for_retire_expected_request(
    parent: &Arc<solstone_core_system::lifecycle::HostedServiceParentRuntime>,
) {
    loop {
        if parent.retire_expected_requested() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(feature = "host")]
async fn wait_for_hosted_termination_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            let _ = signal.recv().await;
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(feature = "host")]
fn write_port_file(journal_root: &FsPath, port: u16) -> Result<(), String> {
    let health = journal_root.join("health");
    std::fs::create_dir_all(&health)
        .map_err(|error| format!("convey could not create health directory: {error}"))?;
    static NEXT_PORT_TEMP: AtomicU64 = AtomicU64::new(0);
    let target = health.join("convey.port");
    let temporary = health.join(format!(
        ".convey.port.{}.{}.tmp",
        std::process::id(),
        NEXT_PORT_TEMP.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::write(&temporary, port.to_string())
        .and_then(|()| std::fs::rename(&temporary, &target))
        .map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("convey could not write its port file: {error}")
        })
}

#[cfg(feature = "host")]
fn remove_port_file(journal_root: &FsPath) -> Result<(), String> {
    let path = journal_root.join("health/convey.port");
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("convey could not remove port file: {error}")),
    }
}

pub fn router(journal_root: PathBuf) -> Router {
    router_with_hosted_parent(journal_root, None, None)
}

fn router_with_hosted_parent(
    journal_root: PathBuf,
    hosted_parent: Option<Arc<solstone_core_system::lifecycle::HostedServiceParentRuntime>>,
    agents_routes: Option<Router>,
) -> Router {
    router_with_roots(journal_root, hosted_parent, agents_routes, None, None)
}

/// Compose the production routes with an explicitly packaged app payload.
pub fn router_with_payload_root(
    journal_root: PathBuf,
    payload_root: &std::path::Path,
) -> Result<Router, String> {
    let roots = thinking_sol_reads::TalentRoots::from_payload_root(payload_root)?;
    Ok(router_with_roots(
        journal_root,
        None,
        None,
        Some(roots),
        None,
    ))
}

fn router_with_roots(
    journal_root: PathBuf,
    hosted_parent: Option<Arc<solstone_core_system::lifecycle::HostedServiceParentRuntime>>,
    agents_routes: Option<Router>,
    payload_roots: Option<thinking_sol_reads::TalentRoots>,
    backup_deps: Option<solstone_core_backup_web::BackupWebDeps>,
) -> Router {
    solstone_core_convey_body::warm_trends(journal_root.clone());
    let include_agents = agents_routes.is_some();
    let shell = Arc::new(if include_agents {
        shell_payload_with_agents(true)
    } else {
        shell_payload()
    });
    let route_journal_root = Arc::new(JournalRoot(journal_root.clone()));
    let operation_registry = Arc::new(OperationRegistry::default());
    let relay_admissions = relay_admission::admission_registry_for(&journal_root);
    let relay_access = Arc::new(solstone_core_spl::relay_access::RelayAccessCache::new());
    let pair_windows = Arc::new(pair_window_manager::PairWindowManager::new(
        relay_admissions,
        relay_access.clone(),
    ));
    let backup_deps = backup_deps
        .unwrap_or_else(|| solstone_core_backup_web::BackupWebDeps::new(journal_root.clone()));
    let mut routes = Router::new()
        .route("/", get(root))
        .route("/favicon.ico", get(favicon))
        .route("/static/{*path}", get(static_asset))
        .route("/api/shell", get(shell_api))
        .route("/api/streams/labels", get(streams_labels_api))
        .route(
            "/api/system/about",
            get(|| async {
                axum::Json(solstone_core_about::host_about(env!("CARGO_PKG_VERSION")))
            }),
        )
        .route(
            "/api/system/status",
            get({
                let journal_root = journal_root.clone();
                move || system::status(journal_root.clone())
            }),
        )
        .route(
            "/sse/events",
            get({
                let journal_root = journal_root.clone();
                move || sse::events(journal_root.clone())
            }),
        );
    for prefix in network::NETWORK_ROUTE_PREFIXES {
        routes = routes
            .merge(network::direct_routes(prefix, pair_windows.clone()))
            .merge(network::router(
                route_journal_root.clone(),
                prefix,
                operation_registry.clone(),
                pair_windows.clone(),
            ))
            .merge(clients::router(prefix, pair_windows.clone()));
    }
    let routes = routes
        .route("/app/devices", get(clients::redirect_app))
        .route("/app/devices/", get(clients::redirect_app))
        .route("/app/devices/workspace", get(clients::redirect_workspace))
        .merge(if include_agents {
            Router::new()
                .route("/app/agents/", get(agents_shell))
                .route("/app/agents/workspace", get(agents_workspace))
        } else {
            Router::new()
        })
        .merge(agents_enable::router(
            "/app/agents",
            operation_registry.clone(),
        ))
        .merge(agents_routes.unwrap_or_default())
        .merge(solstone_core_ingest::api_router(journal_root.clone()))
        .merge({
            let push_portal = std::env::var("SERVICES_PORTAL_URL")
                .unwrap_or_else(|_| agents_enable::DEFAULT_PORTAL_URL.to_owned())
                .trim_end_matches('/')
                .to_owned();
            solstone_core_push::api_router(journal_root.clone(), push_portal)
        })
        .route("/app/speakers/", get(speakers::shell))
        .route("/app/speakers/{day}", get(speakers::shell_for_day))
        .route("/app/speakers/workspace", get(speakers::workspace))
        .route(
            "/app/speakers/static/who_is_this.js",
            get(speakers::who_is_this),
        )
        .route("/app/speakers/api/state", get(speakers::state))
        .route("/app/speakers/api/index", get(speakers_calendar::index))
        .route("/app/speakers/api/grid", get(speakers_calendar::grid))
        .route("/app/speakers/api/quality", get(speakers_quality::quality))
        .route(
            "/app/speakers/api/owner/status",
            get(speakers_owner::status),
        )
        .route(
            "/app/speakers/api/discovery/cache",
            get(speakers_discovery::cache),
        )
        .route(
            "/app/speakers/api/discovery/cluster/{cluster_id}/presence",
            get(speakers_discovery::presence),
        )
        .route(
            "/app/speakers/api/discovery/resolve-statement",
            get(speakers_discovery::resolve_statement),
        )
        .route(
            "/app/speakers/api/people/search",
            get(speakers_media::people_search),
        )
        .route(
            "/app/speakers/api/serve_audio/{day}/{*rel_path}",
            get(speakers_media::serve_audio),
        )
        .route(
            "/app/speakers/api/speakers/known",
            get(speakers_known::known),
        )
        .route(
            "/app/speakers/api/speakers/{day}/{stream}/{segment_key}",
            get(speakers_review::segment_speakers),
        )
        .route(
            "/app/speakers/api/review/{day}/{stream}/{segment_key}/{source}",
            get(speakers_review::review),
        )
        .route(
            "/app/speakers/api/stats/{month}",
            get(speakers_calendar::stats),
        )
        .route(
            "/app/speakers/api/segments/{day}",
            get(speakers_calendar::segments),
        )
        .route(
            "/app/speakers/api/segments-cli/{day}",
            get(speakers_cli_reads::segments),
        )
        .route(
            "/app/speakers/api/review-cli/{day}/{stream}/{segment_key}/{source}",
            get(speakers_cli_reads::review),
        )
        .route("/app/speakers/api/status", get(speakers_cli_reads::status))
        .route(
            "/app/speakers/api/suggest",
            get(speakers_cli_reads::suggest),
        )
        .route(
            "/app/speakers/api/name-variants/keep-separate",
            get(speakers_cli_reads::keep_separate),
        )
        .route(
            "/app/speakers/api/discovery/dismissals",
            get(speakers_cli_reads::dismissals),
        )
        .route(
            "/app/speakers/api/owner/tag-cli",
            post(speakers_cli_owner::tag),
        )
        .route(
            "/app/speakers/api/owner/confirm-cli",
            post(speakers_cli_owner::confirm),
        )
        .route(
            "/app/speakers/api/owner/reject-cli",
            post(speakers_cli_owner::reject),
        )
        .route(
            "/app/speakers/api/discovery/identify-cli",
            post(speakers_cli_discovery::identify),
        )
        .route(
            "/app/speakers/api/discovery/identify/operations",
            get(speakers_cli_discovery::operations),
        )
        .route(
            "/app/speakers/api/discovery/identify/operations/{operation_id}",
            get(speakers_cli_discovery::operation),
        )
        .route(
            "/app/speakers/api/bootstrap",
            post(speakers_cli_maintenance::bootstrap),
        )
        .route(
            "/app/speakers/api/resolve-names",
            post(speakers_cli_maintenance::resolve_names),
        )
        .route(
            "/app/speakers/api/seed-from-imports",
            post(speakers_cli_maintenance::seed_from_imports),
        )
        .route(
            "/app/speakers/api/backfill",
            post(speakers_cli_maintenance::backfill),
        )
        .route(
            "/app/speakers/api/backfill/operations/{operation_id}",
            get(speakers_cli_maintenance::backfill_status),
        )
        .route(
            "/app/speakers/api/backfill/operations/{operation_id}/resume",
            post(speakers_cli_maintenance::backfill_resume),
        )
        .route(
            "/app/speakers/api/repair",
            post(speakers_cli_maintenance::repair),
        )
        .route(
            "/app/speakers/api/repair/operations/{operation_id}",
            get(speakers_cli_maintenance::repair_status),
        )
        .route(
            "/app/speakers/api/repair/operations/{operation_id}/resume",
            post(speakers_cli_maintenance::repair_resume),
        )
        .route(
            "/app/speakers/api/backfill-last-seen",
            post(speakers_cli_maintenance::backfill_last_seen),
        )
        .route(
            "/app/speakers/api/wipe",
            post(speakers_cli_maintenance::wipe),
        )
        .route(
            "/app/speakers/api/attribute-segment",
            post(speakers_cli_maintenance::attribute),
        )
        .route(
            "/app/speakers/api/assign-attribution",
            post(speakers_attribution::assign),
        )
        .route(
            "/app/speakers/api/confirm-attribution",
            post(speakers_attribution::confirm),
        )
        .route(
            "/app/speakers/api/correct-attribution",
            post(speakers_attribution::correct),
        )
        .route(
            "/app/speakers/api/propagate-correction",
            post(speakers_attribution::propagate),
        )
        .route(
            "/app/speakers/api/discovery/identify",
            post(speakers_discovery_write::identify),
        )
        .route(
            "/app/speakers/api/voice/identify",
            post(speakers_discovery_write::identify_voice),
        )
        .route(
            "/app/speakers/api/discovery/identify/undo",
            post(speakers_discovery_write::undo),
        )
        .route(
            "/app/speakers/api/discovery/dismiss",
            post(speakers_discovery_write::dismiss),
        )
        .route(
            "/app/speakers/api/discovery/scan",
            post(speakers_discovery_write::scan),
        )
        .route(
            "/app/speakers/api/owner/detect",
            post(speakers_owner_write::detect),
        )
        .route(
            "/app/speakers/api/owner/build-from-tags",
            post(speakers_owner_write::build_from_tags),
        )
        .route(
            "/app/speakers/api/owner/rebuild",
            post(speakers_owner_write::rebuild),
        )
        .route(
            "/app/speakers/api/owner/confirm",
            post(speakers_owner_write::confirm),
        )
        .route(
            "/app/speakers/api/owner/reject",
            post(speakers_owner_write::reject),
        )
        .route(
            "/app/speakers/api/owner/set-aside",
            post(speakers_owner_write::set_aside),
        )
        .route(
            "/app/speakers/api/owner/undo",
            post(speakers_owner_write::undo),
        )
        .route(
            "/app/speakers/api/owner/classify",
            post(speakers_owner_write::classify),
        )
        .route(
            "/app/speakers/api/owner/ready",
            post(speakers_owner_write::ready),
        )
        .route(
            "/app/speakers/api/merge-names",
            post(speakers_cli_entities::merge_names),
        )
        .route(
            "/app/speakers/api/link-import",
            post(speakers_cli_entities::link_import),
        )
        .route("/app/entities/", get(entities::shell))
        .route("/app/entities/workspace", get(entities::workspace))
        .merge(solstone_core_entities::api_router(journal_root.clone()))
        .merge(solstone_core_settings_web::routes(journal_root.clone()))
        .merge(solstone_core_health_web::routes(journal_root.clone()))
        .merge(solstone_core_profile_web::routes(journal_root.clone()))
        .merge(solstone_core_stats_web::routes(
            journal_root.clone(),
            solstone_core_stats_web::Clock::real(),
        ))
        .merge(solstone_core_home_web::routes_with_backup_deps(
            backup_deps.clone(),
            solstone_core_home_web::Clock::system(),
            source_link::reference_is_moment,
        ))
        .merge(solstone_core_backup_web::routes_with_deps(backup_deps))
        .route("/app/body/", get(body::shell))
        .route("/app/body/trends", get(body::trends))
        .route("/app/body/{day}", get(body::shell_for_day))
        .route("/app/body/workspace", get(body::workspace))
        .route("/app/body/background", get(body::background))
        .merge(solstone_core_convey_body::api_router(journal_root.clone()))
        .merge(solstone_core_import_web::routes(journal_root.clone()))
        .merge(solstone_core_transcripts_web::router(
            journal_root.clone(),
            solstone_core_transcripts_web::Clock::system(),
            || asset_response("/static/shell.html"),
        ))
        .merge(solstone_core_map_web::router(journal_root.clone(), || {
            asset_response("/static/shell.html")
        }))
        .route(
            "/app/{app}",
            get(move |app: Path<String>| app_bare(app, include_agents)),
        )
        .route("/app/{app}/", get(not_found))
        .route("/app/{app}/{*tail}", get(not_found).fallback(not_found))
        .merge(solstone_core_records_web::api_router(journal_root.clone()))
        .merge(match payload_roots {
            Some(roots) => thinking::router_with_roots(route_journal_root.clone(), roots),
            None => thinking::router(route_journal_root.clone()),
        })
        .merge(solstone_core_sol_link::http::init_router_with_host(
            journal_root.clone(),
            Arc::new(setup_host::ConveySetupHost),
        ))
        .merge(solstone_core_facets_web::routes(
            journal_root.clone(),
            solstone_core_facets_web::Clock::owner(journal_root.clone()),
        ))
        .merge(solstone_core_support_web::routes(journal_root.clone()))
        .merge(source_link::source_link_router(
            journal_root.clone(),
            Arc::new(source_link::FilesystemReads),
        ))
        .layer(Extension(shell))
        .layer(Extension(route_journal_root))
        .layer(Extension(HostedLaunchContext(hosted_parent)));
    let routes = session_gate::apply_layer_with_agents(routes, journal_root, include_agents);
    form_body_guard::apply_layer(routes).fallback(not_found)
}

pub(crate) fn asset_response(path: &str) -> Response {
    asset_response_with_status(path, StatusCode::OK)
}

pub(crate) fn asset_response_with_status(path: &str, status: StatusCode) -> Response {
    let Some(asset) = lookup(path) else {
        return not_found_response();
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, asset.content_type)
        .body(Body::from(asset.bytes))
        .expect("embedded asset response builds")
}

pub(crate) fn not_found_response() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(
            "<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n",
        ))
        .expect("not found response builds")
}

async fn root() -> Response {
    let location = "/app/home/";
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(session_gate::redirect_body(location)))
        .expect("root redirect builds")
}

async fn favicon() -> Response {
    asset_response("/favicon.ico")
}

async fn static_asset(Path(path): Path<String>) -> Response {
    asset_response(&format!("/static/{path}"))
}

async fn agents_shell() -> Response {
    asset_response("/static/shell.html")
}

async fn agents_workspace() -> Response {
    asset_response("/app/agents/workspace")
}

async fn shell_api(
    Extension(shell): Extension<Arc<ShellPayload>>,
    Extension(journal): Extension<Arc<JournalRoot>>,
) -> Response {
    let mut payload = match serde_json::to_value(&*shell) {
        Ok(Value::Object(map)) => map,
        _ => return (StatusCode::INTERNAL_SERVER_ERROR, "shell payload error").into_response(),
    };
    let zone = owner_zone(&journal.0);
    let now = Utc::now().with_timezone(&zone);
    let label = zone_label(Some(zone), now.offset().fix().local_minus_utc());
    // `today` is for clients with no zone table of their own, like the
    // `solstone` CLI: the journal's day key, which this computer's may not be.
    payload.insert(
        "clock".to_string(),
        serde_json::json!({
            "tz": zone.name(),
            "label": label,
            "today": now.format("%Y%m%d").to_string(),
        }),
    );
    Json(Value::Object(payload)).into_response()
}

async fn streams_labels_api(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let table = solstone_core_home::provenance::load_stream_labels(&journal.0);
    match table.status {
        solstone_core_home::provenance::LabelTableStatus::Failed => {
            Json(serde_json::json!({ "status": "failed" })).into_response()
        }
        status => {
            let status_str = match status {
                solstone_core_home::provenance::LabelTableStatus::Complete => "complete",
                solstone_core_home::provenance::LabelTableStatus::Partial => "partial",
                solstone_core_home::provenance::LabelTableStatus::Failed => unreachable!(),
            };
            let labels = solstone_core_home::provenance::rendered_stream_labels(&table);
            let says = solstone_core_home::provenance::rendered_stream_says(&table);
            Json(serde_json::json!({
                "status": status_str,
                "labels": labels,
                "says": says,
            }))
            .into_response()
        }
    }
}

async fn app_bare(Path(app): Path<String>, include_agents: bool) -> Response {
    if served_app(&app, include_agents).is_some() {
        return Redirect::permanent(&format!("/app/{app}/")).into_response();
    }
    not_found_response()
}

async fn not_found() -> Response {
    not_found_response()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::router;
    use crate::registry::APP_REGISTRY;

    #[cfg(feature = "host")]
    #[tokio::test]
    async fn renewal_drain_retains_active_work_across_concurrent_and_cancelled_waiters() {
        use std::{future::Future, task::Poll};
        let (release, blocked) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = blocked.await;
        });
        let slot = tokio::sync::Mutex::new(Some(task));
        let mut first = Box::pin(super::join_embedded_renewal(&slot));
        let mut second = Box::pin(super::join_embedded_renewal(&slot));
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        drop(first);
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        release.send(()).unwrap();
        second.await;
        assert!(slot.lock().await.is_none());
    }

    #[tokio::test]
    async fn stream_labels_serve_words_with_what_they_say_and_fail_whole() {
        let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        fs::create_dir_all(journal.path().join("config")).expect("config directory");
        fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .expect("journal config");
        fs::create_dir_all(journal.path().join("streams")).expect("streams directory");
        fs::write(
            journal.path().join("streams/s1.json"),
            br#"{"name":"s1","kind":"observer","host":null,"platform":null,"created_at":1,"last_day":null,"last_segment":null,"seq":1,"cid":"sha256:1","source":"audio"}"#,
        )
        .expect("stream record");
        let read = |app: axum::Router| async move {
            let response = app
                .oneshot(
                    Request::get("/api/streams/labels")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("router responds");
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            serde_json::from_slice::<Value>(&body).expect("json")
        };
        let app = super::router_with_hosted_parent(journal.path().to_path_buf(), None, None);
        let body = read(app.clone()).await;
        assert_eq!(body["status"], "complete");
        assert!(body["labels"]["s1"].is_string());
        assert_eq!(body["says"]["s1"]["what"], "audio");
        assert_eq!(body["says"]["s1"]["imported"], false);
        fs::write(journal.path().join("streams/broken.json"), b"{").expect("broken record");
        let body = read(app).await;
        assert_eq!(body["status"], "failed");
        assert!(body.get("labels").is_none() && body.get("says").is_none());
    }

    #[test]
    fn activities_remains_a_known_native_app() {
        assert!(crate::registry::known_app("activities").is_some());
    }

    #[tokio::test]
    async fn every_served_app_redirects_its_bare_path_including_the_mounted_agents_app() {
        let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        fs::create_dir_all(journal.path().join("config")).expect("config directory");
        fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .expect("journal config");
        for include_agents in [false, true] {
            let app = super::router_with_hosted_parent(
                journal.path().to_path_buf(),
                None,
                include_agents.then(axum::Router::new),
            );
            for definition in crate::registry::served_apps(include_agents) {
                let path = format!("/app/{}", definition.name);
                let response = app
                    .clone()
                    .oneshot(Request::get(&path).body(Body::empty()).expect("request"))
                    .await
                    .expect("router responds");
                assert_eq!(
                    response.status(),
                    StatusCode::PERMANENT_REDIRECT,
                    "{path} (agents mounted: {include_agents})"
                );
                assert_eq!(
                    response
                        .headers()
                        .get(header::LOCATION)
                        .and_then(|value| value.to_str().ok()),
                    Some(format!("{path}/").as_str()),
                    "{path}"
                );
            }
            let agents = app
                .clone()
                .oneshot(
                    Request::get("/app/agents")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("router responds");
            let expected = if include_agents {
                StatusCode::PERMANENT_REDIRECT
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(
                agents.status(),
                expected,
                "agents mounted: {include_agents}"
            );
        }
    }

    #[tokio::test]
    async fn no_registry_app_is_served_a_not_implemented_response() {
        let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        fs::create_dir_all(journal.path().join("config")).expect("config directory");
        fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .expect("journal config");
        let app = router(journal.path().to_path_buf());

        for definition in APP_REGISTRY {
            for path in [
                format!("/app/{}/", definition.name),
                format!("/app/{}/workspace", definition.name),
            ] {
                let response = app
                    .clone()
                    .oneshot(Request::get(&path).body(Body::empty()).expect("request"))
                    .await
                    .expect("router responds");
                assert_ne!(response.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
            }
        }
    }

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn router_invokes_trends_warm_once() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        let probe = solstone_core_convey_body::TrendsWarmProbe::new();
        let app = router(root.path().to_path_buf());
        assert_eq!(probe.count(), 1);
        let response = app
            .oneshot(
                Request::get("/favicon.ico")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router responds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(probe.count(), 1);
    }

    #[tokio::test]
    async fn converted_transcripts_and_search_routes_are_real_and_unserved_paths_are_404() {
        let root = std::env::temp_dir().join(format!(
            "solstone-convey-shell-transcripts-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .unwrap();
        let analyzed = root.join("chronicle/20260731/field/090000_300");
        fs::create_dir_all(&analyzed).unwrap();
        fs::write(analyzed.join("audio.flac"), b"raw").unwrap();
        fs::write(analyzed.join("audio.jsonl"), b"{\"start\":\"00:00:01\"}\n").unwrap();
        let deleted = root.join("chronicle/20260731/field/090001_300");
        fs::create_dir_all(deleted.join("talents")).unwrap();
        fs::write(deleted.join("audio.flac"), b"raw").unwrap();
        fs::write(deleted.join("audio.jsonl"), b"{}\n").unwrap();
        fs::write(deleted.join("stream.json"), b"{}").unwrap();
        fs::write(deleted.join("talents/sense.json"), b"{}").unwrap();

        assert!(APP_REGISTRY.iter().any(|app| app.name == "transcripts"));
        let app = router(root.clone());
        let reprocess = app
            .clone()
            .oneshot(
                Request::post("/app/transcripts/api/segment/20260731/field/090000_300/reprocess")
                    .body(Body::from(r#"{"modality":"audio"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reprocess.status(), StatusCode::BAD_REQUEST);
        let reprocess_body: Value =
            serde_json::from_slice(&to_bytes(reprocess.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(reprocess_body["reason_code"], "invalid_operation_for_state");

        let deleted = app
            .clone()
            .oneshot(
                Request::delete("/app/transcripts/api/segment/20260731/field/090001_300")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        let deleted: Value =
            serde_json::from_slice(&to_bytes(deleted.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let pending = deleted["pending"].as_str().unwrap();
        let cancelled = app
            .clone()
            .oneshot(
                Request::post(format!("/app/transcripts/api/cancel-delete/{pending}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::OK);

        for (path, expected) in [
            ("/app/transcripts/", StatusCode::FOUND),
            ("/app/transcripts/workspace", StatusCode::OK),
            ("/app/transcripts/20260731", StatusCode::OK),
            ("/app/transcripts/api/index", StatusCode::OK),
            ("/app/transcripts/api/stats/202607", StatusCode::OK),
            ("/app/transcripts/api/ranges/20260731", StatusCode::OK),
            ("/app/transcripts/api/segments/20260731", StatusCode::OK),
            ("/app/transcripts/api/day/20260731", StatusCode::OK),
            ("/app/transcripts/api/read/20260731", StatusCode::OK),
            (
                "/app/transcripts/api/segment/20260731/field/090000_300",
                StatusCode::OK,
            ),
            (
                "/app/transcripts/api/serve_file/20260731/field/090000_300/audio.flac",
                StatusCode::OK,
            ),
            ("/app/search/", StatusCode::OK),
            ("/app/search", StatusCode::PERMANENT_REDIRECT),
            ("/app/health", StatusCode::PERMANENT_REDIRECT),
            ("/app/health/", StatusCode::OK),
            ("/app/search/workspace", StatusCode::OK),
            ("/app/search/api/agents?day=20260731", StatusCode::OK),
            (
                "/app/search/api/read?path=config/journal.json",
                StatusCode::OK,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "{path}");
        }
        let search = app
            .clone()
            .oneshot(
                Request::get("/app/search/api/search?q=native-route-registration-probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(search.status(), StatusCode::NOT_FOUND);

        for path in ["/app/chat/", "/app/chat/api/state"] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let missing = app
            .oneshot(
                Request::get("/native-route-registration-probe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            missing.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            to_bytes(missing.into_body(), usize::MAX).await.unwrap().as_ref(),
            b"<!doctype html>\n<html lang=en>\n<title>404 Not Found</title>\n<h1>Not Found</h1>\n<p>The requested URL was not found on the server. If you entered the URL manually please check your spelling and try again.</p>\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
