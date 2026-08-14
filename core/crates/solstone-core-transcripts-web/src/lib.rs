// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read-only transcript day routes.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde_json::json;

mod assemble;
mod attach;
mod calendar;
mod day;
mod segment;
mod segment_media;
mod segment_speakers;
mod serve_file;
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
        .route("/app/transcripts/api/read/{day}", get(assemble::api_read))
        .route(
            "/app/transcripts/api/segment/{day}/{stream}/{segment_key}/reprocess",
            post(unconverted_transcripts),
        )
        .route(
            "/app/transcripts/api/segment/{day}/{stream}/{segment_key}",
            get(segment::segment_content).delete(unconverted_transcripts),
        )
        .route(
            "/app/transcripts/api/serve_file/{day}/{*rel_path}",
            get(serve_file::serve_file),
        )
        .with_state(Arc::new(AppState {
            journal_root: Arc::new(journal_root),
            clock,
            shared_shell,
        }))
}

async fn unconverted_transcripts() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "error": "This app isn't available yet.",
            "reason_code": "app_not_converted",
            "detail": "The transcripts app has not been ported to the native shell.",
            "app": "transcripts",
        })),
    )
        .into_response()
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
    Response::builder()
        .header(header::CONTENT_TYPE, WORKSPACE.content_type)
        .body(Body::from(WORKSPACE.bytes))
        .expect("workspace response builds")
}

pub(crate) fn legacy_error_response(
    reason_code: impl Into<String>,
    message: impl Into<String>,
    detail: impl Into<String>,
    status: StatusCode,
) -> Response {
    let (status, Json(envelope)) =
        solstone_core_convey_http::envelope::error_envelope(reason_code, message, detail, status);
    let mut body = serde_json::to_vec(&envelope).expect("error envelope serializes");
    // Flask's jsonify response includes this terminal newline.
    body.push(b'\n');
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("legacy error response builds")
}
