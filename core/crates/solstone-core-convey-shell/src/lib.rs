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
//! ## D4: registry fidelity over dynamic configuration
//!
//! The 23 app definitions and Lucide mapping are compiled into this crate in
//! corpus order. Configuration-driven ordering, stars, and agent renaming are
//! intentionally deferred because the frozen reference fixture exercises none
//! of them; chat-bar placeholder likewise uses its stable default, not live state.
//!
//! ## D5: generated, dependency-free asset embedding
//!
//! `build.rs` emits auditable `include_bytes!` entries for the full top-level
//! static tree and the speakers assets. This avoids a proc-macro asset crate
//! while retaining byte-identical source assets in the binary.
//!
//! ## D6: one converted app, explicit named refusal for the rest
//!
//! Speakers is the only converted workspace in this wave. Every other known
//! app receives a 200 `app_not_converted` JSON payload carrying its app name;
//! unknown app paths remain the legacy HTML 404 fallback.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

mod assets;
pub mod refusal;
pub mod registry;
pub mod session;
pub mod session_gate;
mod speakers;
mod sse;
mod system;

use assets::lookup;
use refusal::AppNotConverted;
use registry::{ShellPayload, known_app, shell_payload};

/// Run the loopback Convey server until its process is terminated; port zero is unsupported.
pub fn run_convey(journal_root: PathBuf, port: u16) -> Result<(), String> {
    if port == 0 {
        return Err("convey --port 0 is not supported; choose a concrete loopback port".to_owned());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| format!("convey could not create its Tokio runtime: {error}"))?;
    runtime.block_on(serve_loop(journal_root, port))
}

async fn serve_loop(journal_root: PathBuf, port: u16) -> Result<(), String> {
    use solstone_core_convey_http::identity::AccessBasis;
    use solstone_core_convey_http::listener::bind_loopback;
    use solstone_core_convey_http::serve::{serve_connection, tcp_builder};

    let listeners = bind_loopback(port).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::AddrInUse {
            format!("convey could not bind port {port}: convey may already be running")
        } else {
            format!("convey could not bind port {port}: {error}")
        }
    })?;
    let bound_port = listeners
        .ipv4_addr()
        .map_err(|error| format!("convey could not determine its bound port: {error}"))?
        .port();
    write_port_file(&journal_root, bound_port)?;
    let app = router(journal_root);

    loop {
        let (stream, _) = listeners
            .accept()
            .await
            .map_err(|error| format!("convey could not accept a connection: {error}"))?;
        let app = app.clone();
        tokio::spawn(async move {
            let builder = tcp_builder();
            if let Err(error) =
                serve_connection(stream, app, AccessBasis::Localhost, &builder).await
            {
                eprintln!("convey connection failed: {error}");
            }
        });
    }
}

fn write_port_file(journal_root: &FsPath, port: u16) -> Result<(), String> {
    let health = journal_root.join("health");
    std::fs::create_dir_all(&health)
        .map_err(|error| format!("convey could not create health directory: {error}"))?;
    std::fs::write(health.join("convey.port"), port.to_string())
        .map_err(|error| format!("convey could not write its port file: {error}"))
}

pub fn router(journal_root: PathBuf) -> Router {
    let shell = Arc::new(shell_payload());
    let routes = Router::new()
        .route("/", get(root))
        .route("/favicon.ico", get(favicon))
        .route("/static/{*path}", get(static_asset))
        .route("/api/shell", get(shell_api))
        .route("/api/system/status", get(system::status))
        .route("/sse/events", get(sse::events))
        .route("/app/speakers/", get(speakers::shell))
        .route("/app/speakers/{day}", get(speakers::shell_for_day))
        .route("/app/speakers/workspace", get(speakers::workspace))
        .route(
            "/app/speakers/static/who_is_this.js",
            get(speakers::who_is_this),
        )
        .route("/app/speakers/api/state", get(speakers::state))
        .route("/app/{app}", get(app_root))
        .route("/app/{app}/", get(app_root))
        .route("/app/{app}/{*tail}", get(app_nested))
        .layer(Extension(shell));
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
        Some(_) => Json(AppNotConverted::new(app)).into_response(),
        None => not_found_response(),
    }
}

async fn not_found() -> Response {
    not_found_response()
}
