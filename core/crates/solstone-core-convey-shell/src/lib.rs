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
//! ## D6: converted workspaces, explicit named refusal for the rest
//!
//! Body, Entities, Health, Settings, Speakers, Stats, and Network are converted workspaces in this wave.
//! Home's shell, workspace, and static script are natively served while its API routes remain unconverted.
//! A known app that remains unconverted serves the embedded shell document at 501 for navigation paths,
//! including its bare path rather than following the converted-app redirect convention. Its nested fragment
//! paths receive a 501 `app_not_converted` JSON payload carrying its app name; unknown app paths remain the
//! legacy HTML 404 fallback.
//!
//! ## D7: host transport has one indispensable listener and one capability
//!
//! `serve` returns an error only when loopback cannot bind: local Convey is the
//! process's reason to exist. The paired-device door is independently useful
//! but optional, so it is always recorded as a [`DoorOutcome`] in the handle.
//! This crate is already excluded from `check-rust-ios` (Makefile:327 and
//! docs/PORTING.md:45,56); the `host` feature is defence in depth, not that
//! exclusion's replacement.
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
//! `/app/body` without its trailing slash reaches the generic converted-app
//! catch-all, which permanently redirects to `/app/body/`. Every converted
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
#[cfg(feature = "host")]
use solstone_core_sol_link::DeviceDoorAuthorization;
#[cfg(feature = "host")]
use solstone_core_thinking::confidential::OperationRegistry;

mod assets;
#[cfg(feature = "host")]
pub mod authorization_gate;
mod body;
mod clients;
#[cfg(feature = "host")]
mod door;
mod entities;
#[cfg(feature = "host")]
mod link_health_cache;
#[cfg(feature = "host")]
mod network;
#[cfg(feature = "host")]
mod network_status;
#[cfg(feature = "host")]
mod network_writes;
#[cfg(feature = "host")]
mod pair_window_manager;
pub mod refusal;
pub mod registry;
mod relay_admission;
#[cfg(feature = "host")]
mod restart;
pub mod session;
pub mod session_gate;
mod speakers;
mod speakers_analyze_client;
#[cfg(test)]
mod status_mark;
#[cfg(any(test, feature = "test-hooks"))]
pub use speakers_analyze_client::drive_discovery_cluster_helper;
mod speakers_attribution;
mod speakers_calendar;
mod speakers_cli_discovery;
mod speakers_cli_entities;
mod speakers_cli_maintenance;
mod speakers_cli_owner;
mod speakers_cli_reads;
mod speakers_discovery;
mod speakers_discovery_write;
mod speakers_known;
mod speakers_media;
mod speakers_npz;
mod speakers_owner;
mod speakers_owner_write;
mod speakers_quality;
mod speakers_review;
mod speakers_segment_catalog;
mod sse;
mod system;
#[cfg(feature = "host")]
mod thinking;
#[cfg(feature = "host")]
mod thinking_sol_reads;
#[cfg(all(test, feature = "host"))]
mod thinking_sol_reads_contract;
#[cfg(feature = "host")]
mod thinking_sol_writes;
#[cfg(all(test, feature = "host"))]
mod thinking_sol_writes_contract;
#[cfg(feature = "host")]
pub use network_writes::{
    NetworkOperationsOverride, SplDisableFailureOverride, SplEnrollment, SplPoll, SplPollOutcome,
    SplRuntimeOverride,
};
#[cfg(feature = "host")]
pub use thinking::{ConfidentialPoll, ConfidentialRuntimeOverride, PollOutcome};

use assets::lookup;
use refusal::AppNotConverted;
use registry::{ShellPayload, known_app, shell_payload};

#[cfg(feature = "host")]
pub use restart::{
    RestartConveyError, RestartConveyOptions, RestartConveyReport, RestartTransport,
    restart_convey, restart_convey_with_transport,
};

/// Journal filesystem root shared with converted app route handlers.
#[derive(Clone)]
pub(crate) struct JournalRoot(pub PathBuf);

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
}

#[cfg(feature = "host")]
impl std::fmt::Display for ConveyServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoopbackBind { port, source } => write!(
                formatter,
                "could not bind loopback port {port}: {source}. convey may already be running, including under another login. the default port is shared across logins"
            ),
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
    let link_health_task = tokio::spawn(link_health_cache::subscribe_relay_health(
        options.journal_root.clone(),
        link_health_cache.clone(),
    ));
    let door = Arc::new(door::DoorLifecycle::new(door::DoorStartOptions {
        journal_root: options.journal_root,
        port: options.door_port,
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
        .layer(Extension(link_health_cache))
        .layer(axum::middleware::from_fn_with_state(
            door.clone(),
            door::open_after_finalize,
        ));
    let loopback_task =
        tokio::spawn(async move { serve_loopback(listeners, loopback_router).await });
    let _ = door.ensure_started().await;
    let door_outcome = door.clone_outcome().expect("door start records an outcome");
    Ok(ConveyServeHandle {
        loopback_ipv4,
        loopback_ipv6,
        door_outcome,
        door,
        loopback_task,
        link_health_task,
    })
}

#[cfg(feature = "host")]
async fn serve_loopback(
    listeners: solstone_core_convey_http::listener::LoopbackListeners,
    router: Router,
) {
    use solstone_core_convey_http::identity::AccessBasis;
    use solstone_core_convey_http::serve::{serve_connection, tcp_builder};
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
pub fn run_convey(journal_root: PathBuf, port: u16) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("convey could not inspect current executable: {error}"))?;
    let executable_dir = executable
        .parent()
        .ok_or_else(|| format!("convey executable has no parent: {}", executable.display()))?;
    run_convey_from_executable_dir(journal_root, port, executable_dir)
}

/// Like [`run_convey`], but resolves packaged talent roots from `executable_dir`.
#[cfg(feature = "host")]
pub fn run_convey_from_executable_dir(
    journal_root: PathBuf,
    port: u16,
    executable_dir: &std::path::Path,
) -> Result<(), String> {
    let _roots = crate::thinking_sol_reads::TalentRoots::from_executable_dir(executable_dir)?;
    run_convey_bound(journal_root, port)
}

#[cfg(feature = "host")]
fn run_convey_bound(journal_root: PathBuf, port: u16) -> Result<(), String> {
    use solstone_core_journal_config::read_direct_door_port;
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use solstone_core_system::direct_door::{
        DirectDoorOutcome, DirectDoorPublishResult, peek_direct_door_generation,
        publish_direct_door,
    };
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
    let loopback_router = router(journal_root.clone());
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
                DoorWithheldReason::Unestablished => "journal setup is not complete",
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
    runtime.block_on(handle.await_forever())
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

pub fn router(journal_root: PathBuf) -> Router {
    solstone_core_convey_body::warm_trends(journal_root.clone());
    let shell = Arc::new(shell_payload());
    let route_journal_root = Arc::new(JournalRoot(journal_root.clone()));
    let operation_registry = Arc::new(OperationRegistry::default());
    let pair_windows = Arc::new(pair_window_manager::PairWindowManager::new(
        relay_admission::admission_registry_for(&journal_root),
    ));
    let mut routes = Router::new()
        .route("/", get(root))
        .route("/favicon.ico", get(favicon))
        .route("/static/{*path}", get(static_asset))
        .route("/api/shell", get(shell_api))
        .route("/api/system/status", get(system::status))
        .route("/sse/events", get(sse::events));
    for prefix in network::NETWORK_ROUTE_PREFIXES {
        routes = routes
            .merge(network::direct_routes(prefix, pair_windows.clone()))
            .merge(network::router(
                route_journal_root.clone(),
                prefix,
                operation_registry.clone(),
                pair_windows.clone(),
            ))
            .merge(clients::router(prefix));
    }
    let routes = routes
        .route("/app/devices", get(clients::redirect_app))
        .route("/app/devices/", get(clients::redirect_app))
        .route("/app/devices/workspace", get(clients::redirect_workspace))
        .merge(solstone_core_ingest::api_router(journal_root.clone()))
        .merge(solstone_core_push::api_router(journal_root.clone()))
        .merge(solstone_core_clients_web::router(journal_root.clone()))
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
            solstone_core_stats_web::Clock::local(),
        ))
        .merge(solstone_core_home_web::routes(
            journal_root.clone(),
            solstone_core_home_web::Clock::system(),
        ))
        .merge(solstone_core_backup_web::routes(journal_root.clone()))
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
        .route("/app/{app}", get(app_bare))
        .route("/app/{app}/", get(app_root))
        .route("/app/{app}/{*tail}", get(app_nested).fallback(not_found))
        .merge(solstone_core_records_web::api_router(journal_root.clone()))
        .merge(thinking::router(route_journal_root.clone()))
        .merge(solstone_core_sol_link::http::init_router(
            journal_root.clone(),
        ))
        .merge(solstone_core_facets_web::routes(
            journal_root.clone(),
            solstone_core_facets_web::Clock::local(),
        ))
        .merge(solstone_core_support_web::routes(journal_root.clone()))
        .layer(Extension(shell))
        .layer(Extension(route_journal_root));
    session_gate::apply_layer(routes, journal_root).fallback(not_found)
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

async fn shell_api(Extension(shell): Extension<Arc<ShellPayload>>) -> Response {
    Json((*shell).clone()).into_response()
}

async fn app_bare(Path(app): Path<String>) -> Response {
    if known_app(&app).is_some_and(|definition| definition.converted) {
        return Redirect::permanent(&format!("/app/{app}/")).into_response();
    }
    app_response(&app, AppRequestKind::Navigation)
}

async fn app_root(Path(app): Path<String>) -> Response {
    app_response(&app, AppRequestKind::Navigation)
}

async fn app_nested(Path((app, _tail)): Path<(String, String)>) -> Response {
    app_response(&app, AppRequestKind::Fragment)
}

enum AppRequestKind {
    Navigation,
    Fragment,
}

fn app_response(app: &str, request_kind: AppRequestKind) -> Response {
    match known_app(app) {
        Some(definition) if definition.converted => not_found_response(),
        // 🔴 NOT 2xx. The shell's loadBackground evaluates any response whose
        // `ok` is true, so a refusal served at 200 was parsed as JavaScript and
        // threw on every page load. 501 is the exact meaning -- the path is
        // recognized, the functionality is not implemented.
        Some(_) => match request_kind {
            AppRequestKind::Navigation => {
                asset_response_with_status("/static/shell.html", StatusCode::NOT_IMPLEMENTED)
            }
            AppRequestKind::Fragment => {
                (StatusCode::NOT_IMPLEMENTED, Json(AppNotConverted::new(app))).into_response()
            }
        },
        None => not_found_response(),
    }
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
    use crate::assets::lookup;
    use crate::registry::APP_REGISTRY;

    #[test]
    fn activities_remains_a_known_native_app() {
        assert!(crate::registry::known_app("activities").is_some());
    }

    #[tokio::test]
    async fn registry_conversion_flag_controls_fallback_refusals() {
        let journal = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        fs::create_dir_all(journal.path().join("config")).expect("config directory");
        fs::write(
            journal.path().join("config/journal.json"),
            br#"{"setup":{"completed_at":1700000000000}}"#,
        )
        .expect("journal config");
        let app = router(journal.path().to_path_buf());
        let shell = lookup("/static/shell.html").expect("embedded shell").bytes;

        for definition in APP_REGISTRY {
            let root_path = format!("/app/{}/", definition.name);
            let root = app
                .clone()
                .oneshot(
                    Request::get(&root_path)
                        .body(Body::empty())
                        .expect("root request"),
                )
                .await
                .expect("root responds");

            if definition.converted {
                let shell_refusal = root.status() == StatusCode::NOT_IMPLEMENTED
                    && root
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        == Some("text/html; charset=utf-8");
                assert!(
                    !shell_refusal,
                    "{} used the unconverted navigation fallback",
                    definition.name
                );
                continue;
            }

            assert_eq!(root.status(), StatusCode::NOT_IMPLEMENTED, "{root_path}");
            assert_eq!(
                root.headers().get(header::CONTENT_TYPE).unwrap(),
                "text/html; charset=utf-8",
                "{root_path}"
            );
            let root_body = to_bytes(root.into_body(), usize::MAX)
                .await
                .expect("root body");
            assert_eq!(root_body.as_ref(), shell, "{root_path}");

            let bare_path = format!("/app/{}", definition.name);
            let bare = app
                .clone()
                .oneshot(
                    Request::get(&bare_path)
                        .body(Body::empty())
                        .expect("bare request"),
                )
                .await
                .expect("bare responds");
            assert_eq!(bare.status(), StatusCode::NOT_IMPLEMENTED, "{bare_path}");
            assert_eq!(
                bare.headers().get(header::CONTENT_TYPE).unwrap(),
                "text/html; charset=utf-8",
                "{bare_path}"
            );
            let bare_body = to_bytes(bare.into_body(), usize::MAX)
                .await
                .expect("bare body");
            assert_eq!(bare_body.as_ref(), shell, "{bare_path}");

            let workspace_path = format!("/app/{}/workspace", definition.name);
            let workspace = app
                .clone()
                .oneshot(
                    Request::get(&workspace_path)
                        .body(Body::empty())
                        .expect("workspace request"),
                )
                .await
                .expect("workspace responds");
            assert_eq!(
                workspace.status(),
                StatusCode::NOT_IMPLEMENTED,
                "{workspace_path}"
            );
            assert_eq!(
                workspace.headers().get(header::CONTENT_TYPE).unwrap(),
                "application/json",
                "{workspace_path}"
            );
            let refusal: Value = serde_json::from_slice(
                &to_bytes(workspace.into_body(), usize::MAX)
                    .await
                    .expect("workspace body"),
            )
            .expect("workspace refusal parses");
            assert_eq!(
                refusal["reason_code"], "app_not_converted",
                "{workspace_path}"
            );
            assert_eq!(refusal["app"], definition.name, "{workspace_path}");
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

        assert!(
            APP_REGISTRY
                .iter()
                .any(|app| app.name == "transcripts" && app.converted)
        );
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
                // A path outside /app cannot match app_nested's unconverted-app refusal.
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
