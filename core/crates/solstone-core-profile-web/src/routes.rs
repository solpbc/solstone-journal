// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Axum transport for the native profile CLI contract.

use std::path::PathBuf;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::Deserialize;
use solstone_core_convey_http::envelope::error_envelope;

use crate::error::ProfileError;
use crate::pagination::parse_pagination;
use crate::profile;
use crate::types::ActiveCollection;

#[derive(Clone)]
pub(crate) struct RouteState {
    pub(crate) journal_root: PathBuf,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FullQuery {
    facets: Option<String>,
    include_mentions: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CadenceQuery {
    include_mentions: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ActiveQuery {
    window_days: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

pub(crate) async fn full(
    State(state): State<RouteState>,
    Path(name): Path<String>,
    Query(query): Query<FullQuery>,
) -> Response {
    match profile::full(
        &state.journal_root,
        &name,
        parse_facets(query.facets.as_deref()).as_deref(),
        truthy(query.include_mentions.as_deref()),
        Utc::now(),
    ) {
        Ok(Some(profile)) => Json(profile).into_response(),
        Ok(None) => entity_not_found(&name),
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn brief(State(state): State<RouteState>, Path(name): Path<String>) -> Response {
    match profile::brief(&state.journal_root, &name, Utc::now()) {
        Ok(Some(profile)) => Json(profile).into_response(),
        Ok(None) => entity_not_found(&name),
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn cadence(
    State(state): State<RouteState>,
    Path(name): Path<String>,
    Query(query): Query<CadenceQuery>,
) -> Response {
    match profile::cadence(
        &state.journal_root,
        &name,
        truthy(query.include_mentions.as_deref()),
        Utc::now(),
    ) {
        Ok(Some(cadence)) => Json(cadence).into_response(),
        Ok(None) => entity_not_found(&name),
        Err(error) => internal_error(error),
    }
}

pub(crate) async fn active(
    State(state): State<RouteState>,
    Query(query): Query<ActiveQuery>,
) -> Response {
    let window_days = match parse_window_days(query.window_days.as_deref()) {
        Ok(window_days) => window_days,
        Err(detail) => return invalid_request(detail),
    };
    let pagination = parse_pagination(query.limit.as_deref(), query.offset.as_deref());
    match profile::list_active(&state.journal_root, window_days, Utc::now()) {
        Ok(items) => {
            let total = items.len();
            let items = items
                .into_iter()
                .skip(pagination.offset)
                .take(pagination.limit)
                .collect();
            Json(ActiveCollection { items, total }).into_response()
        }
        Err(error) => internal_error(error),
    }
}

fn truthy(value: Option<&str>) -> bool {
    matches!(
        value
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn parse_facets(value: Option<&str>) -> Option<Vec<String>> {
    value
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|facets| !facets.is_empty())
}

fn parse_window_days(value: Option<&str>) -> Result<i64, &'static str> {
    let window_days = match value {
        None => 30,
        Some(value) => value
            .parse::<i64>()
            .map_err(|_| "window_days must be an integer")?,
    };
    if window_days <= 0 {
        return Err("window_days must be positive");
    }
    Ok(window_days)
}

fn entity_not_found(name: &str) -> Response {
    error_envelope(
        "entity_not_found",
        "that entity couldn't be found.",
        format!("no entity named '{name}'"),
        StatusCode::NOT_FOUND,
    )
    .into_response()
}

fn invalid_request(detail: &str) -> Response {
    error_envelope(
        "invalid_request_value",
        "one of those values couldn't be used.",
        detail,
        StatusCode::BAD_REQUEST,
    )
    .into_response()
}

fn internal_error(error: ProfileError) -> Response {
    log::error!("profile route failed: {error}");
    error_envelope(
        "profile_unavailable",
        "that profile couldn't be loaded.",
        "profile unavailable",
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .into_response()
}
