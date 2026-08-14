// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(test)]
use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::HttpBody;
use axum::extract::{Path as RoutePath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use chrono::{DateTime, Utc};

use crate::day::{invalid_day, prepare_day, valid_day};
use crate::{AppState, workspace_response};

pub(crate) async fn root(State(state): State<Arc<AppState>>) -> Response {
    let now = state.clock.now();
    let target = match redirect_target_from_journal(&state.journal_root, now) {
        Ok(target) => target,
        Err(error) => return error.response(),
    };
    let location = format!("/app/transcripts/{target}");
    // Flask's redirect body is part of the frozen Convey corpus contract.
    let body = format!(
        "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n<h1>Redirecting...</h1>\n<p>You should be redirected automatically to the target URL: <a href=\"{location}\">{location}</a>. If not, click the link.\n"
    );
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CONTENT_LENGTH, body.len())
        .body(axum::body::Body::from(body))
        .expect("redirect builds")
}

fn redirect_target_from_journal(
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
    Ok(now.format("%Y%m%d").to_string())
}
pub(crate) async fn day(
    State(state): State<Arc<AppState>>,
    RoutePath(day): RoutePath<String>,
) -> Response {
    if !valid_day(&day) {
        invalid_day()
    } else {
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
}
pub(crate) async fn workspace() -> Response {
    workspace_response()
}
#[cfg(test)]
pub(crate) fn redirect_target(day_counts: &BTreeMap<String, usize>, now: DateTime<Utc>) -> String {
    day_counts
        .iter()
        .rev()
        .find(|(_, count)| **count > 0)
        .map(|(day, _)| day.clone())
        .unwrap_or_else(|| now.format("%Y%m%d").to_string())
}
