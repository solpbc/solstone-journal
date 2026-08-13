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
//! static tree plus the Body, speakers, devices, and entities workspace assets. This avoids a proc-macro asset crate
//! while retaining byte-identical source assets in the binary.
//!
//! ## D6: converted workspaces, explicit named refusal for the rest
//!
//! Body, Speakers, Devices, and entities are converted workspaces in this wave. Every other known
//! app receives a 501 `app_not_converted` JSON payload carrying its app name;
//! unknown app paths remain the legacy HTML 404 fallback.
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
//! W1b's second refusal-classification field. The door uses explicit mux limits
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
//! catch-all and answers the shared HTML 404 — the same pre-existing
//! behavior every converted app has today; `/app/body/` with the trailing
//! slash is the explicit native shell entry point.

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
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
#[cfg(feature = "host")]
use solstone_core_sol_link::DeviceDoorAuthorization;

mod assets;
#[cfg(feature = "host")]
pub mod authorization_gate;
mod body;
mod devices;
#[cfg(feature = "host")]
mod door;
mod entities;
#[cfg(feature = "host")]
mod network;
pub mod refusal;
pub mod registry;
#[cfg(feature = "host")]
mod restart;
pub mod session;
pub mod session_gate;
mod speakers;
mod speakers_analyze_client;
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
mod sse;
mod system;
#[cfg(feature = "host")]
mod thinking;

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
    loopback_task: tokio::task::JoinHandle<()>,
    refresh_task: Option<tokio::task::JoinHandle<()>>,
    accept_task: Option<tokio::task::JoinHandle<()>>,
    pairing_reaper_task: Option<tokio::task::JoinHandle<()>>,
    pairing_cap_refusals: Option<Arc<AtomicU64>>,
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
    pub fn shutdown(&self) {
        self.loopback_task.abort();
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
    pub async fn stop_authorization_refresh(&mut self) {
        let Some(task) = self.refresh_task.take() else {
            return;
        };
        task.abort();
        match task.await {
            Ok(()) | Err(_) => {}
        }
    }
    /// Testable lifecycle control for proving request-level confinement does
    /// not depend on the background pairing reaper.
    pub async fn stop_pairing_reaper(&mut self) {
        let Some(task) = self.pairing_reaper_task.take() else {
            return;
        };
        task.abort();
        match task.await {
            Ok(()) | Err(_) => {}
        }
    }
    /// Test-visible equivalent of the cap-refusal log line.
    pub fn pairing_cap_refusals(&self) -> u64 {
        self.pairing_cap_refusals
            .as_ref()
            .map_or(0, |counter| counter.load(Ordering::Acquire))
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
}

#[cfg(feature = "host")]
impl std::fmt::Display for ConveyServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoopbackBind { port, source } => write!(
                formatter,
                "convey may already be running; could not bind loopback port {port}: {source}"
            ),
        }
    }
}

#[cfg(feature = "host")]
impl std::error::Error for ConveyServeError {}

/// Bind loopback and, when available, the paired-device door.
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
    let loopback_router = options.router.clone();
    let loopback_task =
        tokio::spawn(async move { serve_loopback(listeners, loopback_router).await });
    let door_start = door::start(door::DoorStartOptions {
        journal_root: options.journal_root,
        port: options.door_port,
        handshake_timeout: options.handshake_timeout,
        stream_stall_timeout: options.stream_stall_timeout,
        router: door_router.into_inner(),
        carrier_loop_iterations: options.carrier_loop_iterations,
        handshake_authorization_read_ticks: options.handshake_authorization_read_ticks,
        authorization_sender,
    })
    .await;
    Ok(ConveyServeHandle {
        loopback_ipv4,
        loopback_ipv6,
        door_outcome: door_start.outcome,
        loopback_task,
        refresh_task: door_start.refresh_task,
        accept_task: door_start.accept_task,
        pairing_reaper_task: door_start.pairing_reaper_task,
        pairing_cap_refusals: door_start.pairing_cap_refusals,
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
    use solstone_core_sol_link::ledger::AuthorizedClientsRead;
    use tokio::sync::watch;

    if port == 0 {
        return Err("convey --port 0 is not supported; choose a concrete loopback port".to_owned());
    }
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
                door_port: 7657,
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
    let routes = Router::new()
        .route("/", get(root))
        .route("/favicon.ico", get(favicon))
        .route("/static/{*path}", get(static_asset))
        .route("/api/shell", get(shell_api))
        .route("/api/system/status", get(system::status))
        .route("/sse/events", get(sse::events))
        .route("/app/network/pair-start", post(network::pair_start))
        .route(
            "/app/network/api/pair/nonce-status",
            get(network::nonce_status),
        )
        .route(spl_core::PAIR_PATH, post(network::pair))
        .route("/app/network/api/devices", get(network::devices))
        .route("/app/devices/", get(devices::shell))
        .route("/app/devices/workspace", get(devices::workspace))
        .route("/app/devices/api/list", get(devices::list))
        .route(
            "/app/devices/api/{key_prefix}",
            axum::routing::delete(devices::delete),
        )
        .route("/app/devices/api/{key_prefix}/key", get(devices::key))
        .route("/app/devices/api/create", post(devices::create_retired))
        .route(
            "/app/observer/callosum",
            get(devices::observer_wire_refusal),
        )
        .route(
            "/app/observer/ingest/{*tail}",
            get(devices::observer_wire_refusal),
        )
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
        .route("/app/body/", get(body::shell))
        .route("/app/body/trends", get(body::trends))
        .route("/app/body/{day}", get(body::shell_for_day))
        .route("/app/body/workspace", get(body::workspace))
        .route("/app/body/background", get(body::background))
        .merge(solstone_core_convey_body::api_router(journal_root.clone()))
        .route("/app/{app}", get(app_root))
        .route("/app/{app}/", get(app_root))
        .route("/app/{app}/{*tail}", get(app_nested))
        .merge(thinking::router(route_journal_root.clone()))
        .merge(network::router(route_journal_root.clone()))
        .layer(Extension(shell))
        .layer(Extension(route_journal_root));
    session_gate::apply_layer(routes, journal_root).fallback(not_found)
}

pub(crate) fn asset_response(path: &str) -> Response {
    let Some(asset) = lookup(path) else {
        return not_found_response();
    };
    Response::builder()
        .status(StatusCode::OK)
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

async fn app_root(Path(app): Path<String>) -> Response {
    app_response(&app)
}

async fn app_nested(Path((app, _tail)): Path<(String, String)>) -> Response {
    app_response(&app)
}

fn app_response(app: &str) -> Response {
    match known_app(app) {
        Some(definition) if definition.converted => not_found_response(),
        // 🔴 NOT 2xx. The shell's loadBackground evaluates any response whose
        // `ok` is true, so a refusal served at 200 was parsed as JavaScript and
        // threw on every page load. 501 is the exact meaning -- the path is
        // recognized, the functionality is not implemented -- and it routes
        // every client into the failure branch it already has.
        Some(_) => (StatusCode::NOT_IMPLEMENTED, Json(AppNotConverted::new(app))).into_response(),
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::router;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn router_invokes_trends_warm_once() {
        let path = std::env::temp_dir().join(format!(
            "solstone-convey-shell-trends-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        let before = solstone_core_convey_body::trends_warm_invocations();
        let _router = router(path.clone());
        assert_eq!(
            solstone_core_convey_body::trends_warm_invocations() - before,
            1
        );
        let _ = fs::remove_dir_all(path);
    }
}
