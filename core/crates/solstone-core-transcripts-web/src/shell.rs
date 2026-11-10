// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::Arc;

use axum::Json;
use axum::body::HttpBody;
use axum::extract::{Path as RoutePath, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::json;

use crate::day::{invalid_day, prepare_day, valid_day};
use crate::{AppState, workspace_response};

/// The transcripts root opens at once; its first read picks the day.
pub(crate) async fn root(State(state): State<Arc<AppState>>) -> Response {
    page(&state)
}

/// The day an undated page opens on: the newest with segments, or else today
/// on the journal's clock.
pub(crate) async fn open_day(State(state): State<Arc<AppState>>) -> Response {
    let now = state.clock.now();
    let journal_root = state.journal_root.clone();
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::TranscriptsRoot,
        move || match open_day_from_journal(&journal_root, now) {
            Ok(day) => Json(json!({ "day": day })).into_response(),
            Err(error) => error.response(),
        },
    )
    .await
}

pub(crate) fn open_day_from_journal(
    journal_root: &std::path::Path,
    now: DateTime<Utc>,
) -> Result<String, crate::TranscriptError> {
    let mut days = solstone_core_journal_io::day_dirs(journal_root)
        .map_err(crate::TranscriptError::display)?
        .into_keys()
        .collect::<Vec<_>>();
    days.sort();
    for day in days.into_iter().rev() {
        if !prepare_day(journal_root, &day, now)?.segments.is_empty() {
            return Ok(day);
        }
    }
    Ok(now
        .with_timezone(&solstone_core_journal_config::owner_zone(journal_root))
        .format("%Y%m%d")
        .to_string())
}

fn page(state: &AppState) -> Response {
    let mut response = (state.shared_shell)();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline; filename=shell.html"),
    );
    if let Some(length) = response.body().size_hint().exact() {
        response.headers_mut().insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&length.to_string()).expect("body length header"),
        );
    }
    response
}

pub(crate) async fn day(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    if !valid_day(&day) {
        invalid_day()
    } else {
        page(&state)
    }
}
pub(crate) async fn workspace() -> Response {
    workspace_response()
}
