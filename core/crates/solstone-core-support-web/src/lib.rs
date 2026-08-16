// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read and page routes for the Support Convey surface.

use std::path::PathBuf;

use axum::{
    Router,
    extract::{Path, Query},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use chrono::Local;
use serde_json::{Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_support_portal::{
    PortalClient, PortalOperationError, collect_all, is_enabled, native_platform,
    portal_url_from_settings,
};

const SHELL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../solstone/convey/static/shell.html"
));
const WORKSPACE: &[u8] = include_bytes!("../assets/workspace.html");
const BACKGROUND: &[u8] = include_bytes!("../assets/background.html");
const SUPPORT_JS: &[u8] = include_bytes!("../assets/static/support.js");

/// Build the complete W2b Support route surface for one journal.
pub fn routes(journal_root: PathBuf) -> Router {
    let layer_root = journal_root.clone();
    let config_root = journal_root.clone();
    let tickets_root = journal_root.clone();
    let closed_root = journal_root.clone();
    let ticket_root = journal_root.clone();
    let articles_root = journal_root.clone();
    let article_root = journal_root.clone();
    let announcements_root = journal_root.clone();
    let diagnostics_root = journal_root.clone();
    let badge_root = journal_root;
    Router::new()
        .route(
            "/app/support",
            get(|| async { Redirect::permanent("/app/support/") }),
        )
        .route("/app/support/", get(shell))
        .route("/app/support/workspace", get(workspace))
        .route("/app/support/background", get(background))
        .route("/app/support/static/support.js", get(support_js))
        .route(
            "/app/support/api/config",
            get(move || config(config_root.clone())),
        )
        .route(
            "/app/support/api/tickets",
            get(move |query| tickets(tickets_root.clone(), query)),
        )
        .route(
            "/app/support/api/tickets/closed",
            get(move |query| closed(closed_root.clone(), query)),
        )
        .route(
            "/app/support/api/tickets/{id}",
            get(move |id| ticket(ticket_root.clone(), id)),
        )
        .route(
            "/app/support/api/articles",
            get(move |query| articles(articles_root.clone(), query)),
        )
        .route(
            "/app/support/api/articles/{slug}",
            get(move |slug| article(article_root.clone(), slug)),
        )
        .route(
            "/app/support/api/announcements",
            get(move || announcements(announcements_root.clone())),
        )
        .route(
            "/app/support/api/diagnostics",
            get(move || diagnostics(diagnostics_root.clone())),
        )
        .route(
            "/app/support/api/badge-count",
            get(move || badge_count(badge_root.clone())),
        )
        .layer(middleware::from_fn(move |request, next| {
            drain_before_and_after(layer_root.clone(), request, next)
        }))
}

async fn shell() -> Response {
    bytes(SHELL, "text/html; charset=utf-8")
}
async fn workspace() -> Response {
    bytes(WORKSPACE, "text/html; charset=utf-8")
}
async fn background() -> Response {
    bytes(BACKGROUND, "text/html; charset=utf-8")
}
async fn support_js() -> Response {
    bytes(SUPPORT_JS, "application/javascript")
}

fn bytes(value: &'static [u8], content_type: &'static str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(value))
        .expect("embedded support asset response")
}

async fn drain_before_and_after(
    root: PathBuf,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    drain(&root);
    let response = next.run(request).await;
    drain(&root);
    response
}

fn drain(root: &std::path::Path) {
    if let Ok(mut client) = PortalClient::from_journal_settings(root, None, false) {
        let _ = client.drain_pending_acknowledgements();
    }
}

async fn config(root: PathBuf) -> Response {
    json_response(
        json!({"enabled": is_enabled(&root), "portal_url": portal_url_from_settings(&root)}),
    )
}

async fn diagnostics(root: PathBuf) -> Response {
    json_response(Value::Object(collect_all(
        &root,
        Local::now(),
        native_platform(),
    )))
}

async fn tickets(root: PathBuf, Query(query): Query<TicketQuery>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    let result = with_client(&root, |client| {
        client.list_tickets(
            query.status.as_deref(),
            query.product.as_deref(),
            query.severity.as_deref(),
        )
    });
    portal_response(result)
}

async fn closed(root: PathBuf, Query(query): Query<ClosedQuery>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| {
        client.list_closed_history(query.cursor.as_deref())
    }))
}

async fn ticket(root: PathBuf, Path(id): Path<String>) -> Response {
    let Ok(id) = id.parse::<i64>() else {
        return http_not_found();
    };
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| client.get_ticket(id)))
}

async fn articles(root: PathBuf, Query(query): Query<ArticleQuery>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| {
        client.search_articles(query.q.as_deref())
    }))
}

async fn article(root: PathBuf, Path(slug): Path<String>) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| client.get_article(&slug)))
}

async fn announcements(root: PathBuf) -> Response {
    if let Some(response) = disabled(&root, false) {
        return response;
    }
    portal_response(with_client(&root, |client| client.list_announcements()))
}

async fn badge_count(root: PathBuf) -> Response {
    if let Some(response) = disabled(&root, true) {
        return response;
    }
    match with_client(&root, |client| {
        client.list_tickets(Some("open"), None, None)
    }) {
        Ok(Value::Array(tickets)) => json_response(
            json!({"count": tickets.iter().filter(|ticket| ticket.get("updated_at").and_then(Value::as_str) > ticket.get("created_at").and_then(Value::as_str)).count()}),
        ),
        Ok(_) => portal_failed("support portal ticket list is not an array"),
        Err(error) => portal_failed(&error.to_string()),
    }
}

fn with_client<T>(
    root: &std::path::Path,
    operation: impl FnOnce(&mut PortalClient) -> Result<T, PortalOperationError>,
) -> Result<T, PortalOperationError> {
    let mut client = PortalClient::from_journal_settings(root, None, false)
        .map_err(PortalOperationError::Portal)?;
    operation(&mut client)
}

fn portal_response(result: Result<Value, PortalOperationError>) -> Response {
    match result {
        Ok(value) => json_response(value),
        Err(error) => portal_failed(&error.to_string()),
    }
}
fn json_response(value: Value) -> Response {
    axum::Json(value).into_response()
}
fn disabled(root: &std::path::Path, badge: bool) -> Option<Response> {
    (!is_enabled(root)).then(|| {
        error_envelope(
            "feature_unavailable",
            "I couldn't use that feature because it isn't enabled.",
            if badge {
                "Support is not enabled"
            } else {
                "Support is disabled"
            },
            StatusCode::FORBIDDEN,
        )
        .into_response()
    })
}
fn portal_failed(detail: &str) -> Response {
    error_envelope(
        "support_portal_failed",
        "I couldn't reach support right now.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}
fn http_not_found() -> Response {
    error_envelope(
        "http_error",
        "I couldn't complete that request.",
        "Not Found",
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

#[derive(serde::Deserialize)]
struct TicketQuery {
    status: Option<String>,
    product: Option<String>,
    severity: Option<String>,
}
#[derive(serde::Deserialize)]
struct ClosedQuery {
    cursor: Option<String>,
}
#[derive(serde::Deserialize)]
struct ArticleQuery {
    q: Option<String>,
}

#[cfg(test)]
mod corpus;
