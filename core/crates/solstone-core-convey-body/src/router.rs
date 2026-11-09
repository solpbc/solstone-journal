// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as RoutePath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use solstone_core_convey_http::envelope::error_envelope;

use crate::listing::body_days;
use crate::status::{index_payload, stats_payload};

/// Build the mergeable read-only Body API route surface.
pub fn api_router(journal_root: impl AsRef<Path>) -> Router {
    Router::new()
        .route("/app/body/api/index", get(index_route))
        .route("/app/body/api/stats/{month}", get(stats_route))
        .route("/app/body/api/day/{day}", get(crate::day::day_route))
        .route("/app/body/api/status", get(crate::status::status_route))
        .route("/app/body/api/recent", get(crate::status::recent_route))
        .route("/app/body/api/window", get(crate::window::window_route))
        .with_state(Arc::new(journal_root.as_ref().to_path_buf()))
}

async fn index_route(State(root): State<Arc<PathBuf>>) -> Response {
    match body_days(&root) {
        Ok(days) => Json(index_payload(&days)).into_response(),
        Err(error) => unreadable_response(error),
    }
}

async fn stats_route(
    State(root): State<Arc<PathBuf>>,
    RoutePath(month): RoutePath<String>,
) -> Response {
    let days = match body_days(&root) {
        Ok(days) => days,
        Err(error) => return unreadable_response(error),
    };
    if !is_month(&month) {
        return error_envelope(
            "invalid_request_value",
            "one of those values couldn't be used.",
            "Invalid month",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }
    Json(stats_payload(&root, &days, &month)).into_response()
}

/// A journal whose body data can't be listed answers once with this.
pub(crate) fn unreadable_response(detail: String) -> Response {
    error_envelope(
        "body_unreadable",
        "the body app couldn't read your journal.",
        detail,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .into_response()
}

fn is_month(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}
