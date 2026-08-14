// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read-only transcript day routes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, Utc};

mod attach;
mod calendar;
mod day;
mod shell;

#[cfg(test)]
mod corpus;

#[derive(Clone)]
pub struct Clock(Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>);

impl Clock {
    pub fn system() -> Self {
        Self(Arc::new(Utc::now))
    }

    pub fn fixed(now: DateTime<Utc>) -> Self {
        Self(Arc::new(move || now))
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}

pub fn router(journal_root: PathBuf, clock: Clock, shared_shell: fn() -> Response) -> Router {
    Router::new()
        .route("/app/transcripts/", get(shell::root))
        .route("/app/transcripts/workspace", get(shell::workspace))
        .route("/app/transcripts/{day}", get(shell::day))
        .route("/app/transcripts/api/index", get(calendar::index))
        .route("/app/transcripts/api/stats/{month}", get(calendar::stats))
        .route("/app/transcripts/api/ranges/{day}", get(day::ranges))
        .route("/app/transcripts/api/segments/{day}", get(day::segments))
        .route("/app/transcripts/api/day/{day}", get(day::day))
        .with_state(Arc::new(AppState {
            journal_root: Arc::new(journal_root),
            clock,
            shared_shell,
        }))
}

struct AppState {
    journal_root: Arc<PathBuf>,
    clock: Clock,
    shared_shell: fn() -> Response,
}

struct EmbeddedAsset {
    content_type: &'static str,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/transcripts_assets.rs"));

pub(crate) fn transcripts_copy_json() -> &'static str {
    TRANSCRIPTS_COPY_JSON
}

pub(crate) struct TranscriptError(String);

impl TranscriptError {
    pub(crate) fn health(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
    pub(crate) fn display(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
    pub(crate) fn response(self) -> Response {
        solstone_core_convey_http::envelope::error_envelope(
            "internal_error",
            "I couldn't complete that request.",
            self.0,
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .into_response()
    }
}

fn workspace_response() -> Response {
    use axum::body::Body;
    use axum::http::header;
    // Keep the generated browser-copy payload live alongside the workspace
    // asset. The current day-wave workspace has no copy interpolation point,
    // but later speaker controls consume this exact generated payload.
    let _copy = transcripts_copy_json();
    Response::builder()
        .header(header::CONTENT_TYPE, WORKSPACE.content_type)
        .body(Body::from(WORKSPACE.bytes))
        .expect("workspace response builds")
}
